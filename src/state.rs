//! Netzzustandsermittlung: measurements and profiles in, a grid state with its uncertainty out.
//!
//! Every connection's load starts as a Gaussian from its profile. Each fresh measurement — a
//! smart meter gateway's TAF 10 value, the transformer or an LV feeder measured at the
//! Ortsnetzstation — is a linear observation of those loads, and is folded in by the exact
//! Gaussian update (a Kalman step, which is weighted least squares). Every flow and voltage is
//! linear in the loads, so its mean and standard deviation follow exactly from the posterior.
//!
//! An Ortsnetzstation measurement whose normalised innovation against everything else exceeds
//! [`BAD_DATA_SIGMA`] is refused as bad data before it is used: one lying sensor cannot cause a
//! reduction. Smart-meter values are checked for plausibility against the connection's fuse.

use crate::grid::{AssetRef, Topology};
use crate::model::{Connection, ConnectionId};
use crate::rules::RuleSet;
use crate::units::Kw;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A transformer measurement further than this many standard deviations from the rest is refused.
pub const BAD_DATA_SIGMA: f64 = 5.0;

/// What a meter reported.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Reading {
    /// A connection's net consumption from its smart meter gateway.
    Connection {
        /// The connection.
        connection: ConnectionId,
        /// Net active power (feed-in negative).
        total: Kw,
        /// The SteuVE share, where the gateway meters it separately.
        #[serde(default)]
        steuve: Option<Kw>,
    },
    /// An asset measured at the Ortsnetzstation: the transformer, or an LV feeder's first section.
    Asset {
        /// The asset id in the topology (`T1`, `L1-01`).
        asset: String,
        /// Active power through it, towards the loads.
        total: Kw,
    },
}

/// One measurement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measurement {
    /// The meter or RTU it came from — recorded in every reason that uses it.
    pub source: String,
    /// When it was measured.
    pub at: Timestamp,
    /// The value.
    pub reading: Reading,
}

/// A forecast for one connection: its expected unconstrained demand and the spread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    /// The connection.
    pub connection: ConnectionId,
    /// Expected net consumption.
    pub total: Kw,
    /// Standard deviation of that expectation.
    pub sigma: Kw,
    /// Expected SteuVE consumption, unconstrained.
    pub steuve: Kw,
}

/// How an estimate was determined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Quality {
    /// From profiles alone — planning data.
    Pseudo,
    /// Profiles conditioned on a measurement upstream.
    Reconciled,
    /// Every input measured.
    Measured,
}

/// A value with its uncertainty.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Estimate {
    /// Mean.
    pub mean: f64,
    /// Standard deviation.
    pub sd: f64,
    /// How it was determined.
    pub quality: Quality,
}

/// Why a measurement was not used.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Rejection {
    /// The connection is not in the master data.
    UnknownConnection,
    /// The asset is not in the topology.
    UnknownAsset,
    /// Outside what the connection's fuse allows.
    Implausible,
    /// A SteuVE share larger than the total or negative.
    InconsistentShare,
    /// Time stamp in the future.
    FromTheFuture,
    /// Contradicts the rest of the grid state by more than the bad-data bound.
    BadData {
        /// Normalised residual.
        sigma: f64,
    },
}

/// One connection's load in the snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct Load {
    /// Net consumption.
    pub total: Estimate,
    /// Current SteuVE consumption (point estimate).
    pub steuve: Kw,
    /// Expected unconstrained SteuVE consumption (from the profile).
    pub steuve_demand: Kw,
    /// The smart-meter measurement used, if any.
    pub source: Option<(String, Timestamp)>,
}

/// The grid state at one instant: the posterior over all connection loads.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// When it was computed.
    pub at: Timestamp,
    /// Per connection.
    pub loads: BTreeMap<ConnectionId, Load>,
    /// Asset measurements used: source, time, asset.
    pub assets: Vec<(String, Timestamp, String)>,
    ids: Vec<ConnectionId>,
    mean: Vec<f64>,
    /// Row-major covariance, `n × n`.
    cov: Vec<f64>,
    /// Connections covered by a used asset measurement.
    covered: Vec<bool>,
    /// Per line: the I²R losses of that line and every line beyond it, at the mean flows (kW).
    losses_below: Vec<f64>,
}

/// Accuracy of a smart meter or RTU value: 1 % plus 50 W.
#[must_use]
pub fn meter_sigma(value: Kw) -> f64 {
    0.01 * value.0.abs() + 0.05
}

impl Snapshot {
    fn weights(&self, w: impl Fn(&ConnectionId) -> f64) -> Vec<f64> {
        self.ids.iter().map(w).collect()
    }

    fn mean_var(&self, w: &[f64]) -> (f64, f64) {
        let n = self.ids.len();
        let mut mean = 0.0;
        let mut var = 0.0;
        for (i, wi) in w.iter().enumerate() {
            if *wi == 0.0 {
                continue;
            }
            mean += wi * self.mean.get(i).copied().unwrap_or(0.0);
            let row = self.cov.get(i * n..(i + 1) * n).unwrap_or(&[]);
            var += wi * row.iter().zip(w).map(|(c, wj)| c * wj).sum::<f64>();
        }
        (mean, var.max(0.0))
    }

    /// Mean and standard deviation of `Σ w(c)·P(c)` over the connections.
    #[must_use]
    pub fn functional(&self, w: impl Fn(&ConnectionId) -> f64) -> (f64, f64) {
        let (m, v) = self.mean_var(&self.weights(w));
        (m, v.sqrt())
    }

    /// The flow through a thermal asset.
    #[must_use]
    pub fn flow(&self, topo: &Topology, conns: &BTreeMap<ConnectionId, Connection>, a: AssetRef) -> Estimate {
        let involved = |id: &ConnectionId| conns.get(id).is_some_and(|c| topo.downstream(a, &c.node));
        let (mean, sd) = self.functional(|id| f64::from(u8::from(involved(id))));
        let losses = match a {
            AssetRef::Transformer => topo.transformer_losses(&self.losses_below),
            AssetRef::Line(i) => self.losses_below.get(i).copied().unwrap_or(0.0),
        };
        Estimate { mean: mean + losses, sd, quality: self.quality_of(involved) }
    }

    /// The voltage at a node.
    #[must_use]
    pub fn voltage(&self, topo: &Topology, conns: &BTreeMap<ConnectionId, Connection>, node: &crate::model::NodeId) -> Estimate {
        let s = |id: &ConnectionId| conns.get(id).map_or(0.0, |c| topo.voltage_sensitivity(node, &c.node));
        let (dv, sd) = self.functional(s);
        // The loss flow along the path drops voltage too.
        let dv_loss = topo.loss_voltage_drop(node, &self.losses_below);
        Estimate { mean: 1.0 + dv - dv_loss, sd, quality: self.quality_of(|id| s(id) != 0.0) }
    }

    fn quality_of(&self, involved: impl Fn(&ConnectionId) -> bool) -> Quality {
        let mut q = Quality::Measured;
        for (i, id) in self.ids.iter().enumerate() {
            if !involved(id) || self.loads.get(id).is_some_and(|l| l.source.is_some()) {
                continue;
            }
            q = q.min(if self.covered.get(i).copied().unwrap_or(false) { Quality::Reconciled } else { Quality::Pseudo });
        }
        q
    }

    /// Fold in one linear observation `y = w·P + ε`, `ε ~ N(0, r)`. With `test`, `Err` with the
    /// normalised innovation if it fails the bad-data test.
    fn observe(&mut self, w: &[f64], y: f64, r: f64, test: bool) -> Result<(), f64> {
        let n = self.ids.len();
        let (pred, var) = self.mean_var(w);
        let s = var + r;
        let innovation = y - pred;
        let z = innovation / s.sqrt();
        if test && z.abs() > BAD_DATA_SIGMA {
            return Err(z);
        }
        // k = Σw / s;  μ += k·innovation;  Σ −= k (Σw)ᵀ
        let sw: Vec<f64> = (0..n).map(|i| self.cov.get(i * n..(i + 1) * n).unwrap_or(&[]).iter().zip(w).map(|(c, wj)| c * wj).sum()).collect();
        for (i, swi) in sw.iter().enumerate() {
            if let Some(m) = self.mean.get_mut(i) {
                *m += swi / s * innovation;
            }
            for (j, swj) in sw.iter().enumerate() {
                if let Some(c) = self.cov.get_mut(i * n + j) {
                    *c -= swi * swj / s;
                }
            }
        }
        Ok(())
    }
}

/// Check one measurement against the master data.
///
/// # Errors
/// The [`Rejection`] reason.
pub fn plausible(m: &Measurement, conns: &BTreeMap<ConnectionId, Connection>, now: Timestamp) -> Result<(), Rejection> {
    if m.at > now {
        return Err(Rejection::FromTheFuture);
    }
    match &m.reading {
        Reading::Connection { connection, total, steuve } => {
            let c = conns.get(connection).ok_or(Rejection::UnknownConnection)?;
            if !total.0.is_finite() || total.0.abs() > c.fuse.0 * 1.1 {
                return Err(Rejection::Implausible);
            }
            if let Some(s) = steuve
                && (!s.0.is_finite() || s.0 < -0.05 || s.0 > total.0.max(0.0) + 0.05 + meter_sigma(*total) * 3.0)
            {
                return Err(Rejection::InconsistentShare);
            }
            Ok(())
        }
        Reading::Asset { total, .. } => {
            if total.0.is_finite() {
                Ok(())
            } else {
                Err(Rejection::Implausible)
            }
        }
    }
}

/// Inputs to one estimation.
pub struct Inputs<'a> {
    /// The topology.
    pub topology: &'a Topology,
    /// Master data.
    pub connections: &'a BTreeMap<ConnectionId, Connection>,
    /// Latest plausible measurement per connection.
    pub meters: &'a BTreeMap<ConnectionId, Measurement>,
    /// Latest plausible measurement per asset.
    pub assets: &'a BTreeMap<String, Measurement>,
    /// Current profiles.
    pub profiles: &'a BTreeMap<ConnectionId, Profile>,
    /// Parameters.
    pub rules: &'a RuleSet,
}

/// Estimate the state at `now`. Returns the snapshot and the measurements refused as bad data.
#[must_use]
pub fn estimate(i: &Inputs<'_>, now: Timestamp) -> (Snapshot, Vec<(String, Rejection)>) {
    let fresh = |m: &&Measurement| now.duration_since(m.at) <= i.rules.max_data_age;
    let on = crate::time::local_date(now);
    let ids: Vec<ConnectionId> = i.connections.keys().cloned().collect();
    let n = ids.len();
    let mut snap = Snapshot {
        at: now,
        loads: BTreeMap::new(),
        assets: Vec::new(),
        ids: ids.clone(),
        mean: vec![0.0; n],
        cov: vec![0.0; n * n],
        covered: vec![false; n],
        losses_below: Vec::new(),
    };
    let mut rejected = Vec::new();

    // Prior: the profiles. Without one, nothing is known but the fuse — an uninformative prior,
    // so a meter value is taken as it is rather than pulled towards zero.
    for (k, id) in ids.iter().enumerate() {
        let fuse = i.connections.get(id).map_or(50.0, |c| c.fuse.0);
        let (mean, sd) = i.profiles.get(id).map_or((0.0, fuse), |p| (p.total.0, p.sigma.0.max(0.05)));
        if let Some(m) = snap.mean.get_mut(k) {
            *m = mean;
        }
        if let Some(c) = snap.cov.get_mut(k * n + k) {
            *c = sd * sd;
        }
    }
    let prior = snap.mean.clone();

    // Smart meters first, then the Ortsnetzstation — so the bad-data test on an asset
    // measurement is made against everything else that is known.
    let mut metered = vec![None; n];
    for (k, id) in ids.iter().enumerate() {
        let Some(m) = i.meters.get(id).filter(fresh) else { continue };
        let Reading::Connection { total, .. } = &m.reading else { continue };
        let mut w = vec![0.0; n];
        if let Some(x) = w.get_mut(k) {
            *x = 1.0;
        }
        // A meter is the only direct observation of its connection: there is nothing redundant to
        // test it against but a forecast, so only the plausibility checks apply to it.
        match snap.observe(&w, total.0, meter_sigma(*total).powi(2), false) {
            Ok(()) => {
                if let Some(x) = metered.get_mut(k) {
                    *x = Some(m);
                }
            }
            Err(z) => rejected.push((m.source.clone(), Rejection::BadData { sigma: (z * 10.0).round() / 10.0 })),
        }
    }
    // A measurement at the sending end of an asset includes the losses beyond it; the loads are
    // what is left once those losses — predicted from the state so far — are taken off.
    let pre_losses = losses(i.topology, i.connections, &snap);
    for m in i.assets.values().filter(fresh) {
        let Reading::Asset { asset, total } = &m.reading else { continue };
        let Some(a) = i.topology.assets().find(|a| i.topology.asset_id(*a) == asset) else {
            rejected.push((m.source.clone(), Rejection::UnknownAsset));
            continue;
        };
        let w: Vec<f64> = ids.iter().map(|id| f64::from(u8::from(i.connections.get(id).is_some_and(|c| i.topology.downstream(a, &c.node))))).collect();
        let loss = match a {
            AssetRef::Transformer => i.topology.transformer_losses(&pre_losses),
            AssetRef::Line(k) => pre_losses.get(k).copied().unwrap_or(0.0),
        };
        match snap.observe(&w, total.0 - loss, meter_sigma(*total).powi(2), true) {
            Ok(()) => {
                for (k, wk) in w.iter().enumerate() {
                    if *wk > 0.0
                        && let Some(c) = snap.covered.get_mut(k)
                    {
                        *c = true;
                    }
                }
                snap.assets.push((m.source.clone(), m.at, asset.clone()));
            }
            Err(z) => rejected.push((m.source.clone(), Rejection::BadData { sigma: (z * 10.0).round() / 10.0 })),
        }
    }

    for (k, id) in ids.iter().enumerate() {
        let Some(c) = i.connections.get(id) else { continue };
        let profile = i.profiles.get(id);
        let nominal = c.steuve_nominal(on);
        let demand = profile.map_or(Kw::ZERO, |p| p.steuve.min(nominal));
        let mean = snap.mean.get(k).copied().unwrap_or(0.0);
        let sd = snap.cov.get(k * n + k).copied().unwrap_or(0.0).max(0.0).sqrt();
        let load = match metered.get(k).copied().flatten() {
            Some(m) => {
                let (total, steuve) = match &m.reading {
                    Reading::Connection { total, steuve, .. } => (*total, *steuve),
                    Reading::Asset { total, .. } => (*total, None),
                };
                // Without a separately metered share, the SteuVE are assumed to draw what the
                // profile expects of them (or their nominal power without a profile), capped by the total.
                let cap = if profile.is_some() { demand } else { nominal };
                let s = steuve.unwrap_or_else(|| total.max(Kw::ZERO).min(cap));
                Load {
                    total: Estimate { mean, sd, quality: Quality::Measured },
                    steuve: s,
                    steuve_demand: demand.max(s),
                    source: Some((m.source.clone(), m.at)),
                }
            }
            None => {
                let before = prior.get(k).copied().unwrap_or(0.0);
                let share = if before > 0.0 { (demand.0 / before).clamp(0.0, 1.0) } else { 0.0 };
                // The SteuVE share moves in proportion with the connection, within [0, nominal].
                let s = Kw(demand.0 + (mean - before) * share).max(Kw::ZERO).min(nominal);
                let quality = if snap.covered.get(k).copied().unwrap_or(false) { Quality::Reconciled } else { Quality::Pseudo };
                Load { total: Estimate { mean, sd, quality }, steuve: s, steuve_demand: demand.max(s), source: None }
            }
        };
        snap.loads.insert(id.clone(), load);
    }
    snap.losses_below = losses(i.topology, i.connections, &snap);
    (snap, rejected)
}

/// The second-order loss correction at the posterior mean flows.
fn losses(topo: &Topology, conns: &BTreeMap<ConnectionId, Connection>, snap: &Snapshot) -> Vec<f64> {
    let spec = topo.spec();
    let loads: Vec<f64> = (0..spec.lines.len())
        .map(|i| snap.functional(|id| f64::from(u8::from(conns.get(id).is_some_and(|c| topo.downstream(AssetRef::Line(i), &c.node))))).0)
        .collect();
    let vm: Vec<f64> =
        spec.lines.iter().map(|l| 1.0 + snap.functional(|id| conns.get(id).map_or(0.0, |c| topo.voltage_sensitivity(&l.to, &c.node))).0).collect();
    topo.losses_below(&loads, &vm)
}

/// A limit the estimate says is violated, with the confidence the rule set demands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Congestion {
    /// The asset.
    pub asset: String,
    /// Thermal or voltage.
    pub kind: CongestionKind,
    /// The limit (kW, or p.u. for voltage).
    pub limit: f64,
    /// The estimate that violates it.
    pub estimate: Estimate,
}

/// What kind of limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CongestionKind {
    /// Loading above rating.
    Thermal,
    /// Voltage below the band.
    Voltage,
}

/// What the Ortsnetzstation measures (VDE FNN Hinweis 2e, Table 1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MeteringSetup {
    /// Every LV feeder outgoing from the busbar.
    Feeders,
    /// The transformer total.
    Transformer,
    /// Nothing.
    #[default]
    None,
}

/// FNN Hinweis 2e, Table 1, radial grids: the share of connections whose gateways must deliver
/// TAF 10 values for the grid state to count as determined.
#[must_use]
pub fn required_metering_share(setup: MeteringSetup) -> f64 {
    match setup {
        MeteringSetup::Feeders => 0.15,
        MeteringSetup::Transformer => 0.30,
        MeteringSetup::None => 0.70,
    }
}
