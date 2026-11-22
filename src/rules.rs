//! Rule sets: the dated, cited parameters the engine executes, and the Mindestleistung.
//!
//! A rule set is data. A new Festlegung edition, a DSO's own parameters or the end of a
//! transitional regime is a new value, never a branch in the engine.

use crate::model::{Connection, ControlMode, DeviceKind, SteuVe, THRESHOLD};
use crate::units::{Kw, PerUnit};
use jiff::civil::{Date, date};
use jiff::{SignedDuration, ToSpan};
use serde::{Deserialize, Serialize};

/// Under which regime a reduction is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Regime {
    /// Ziff. 4: on a determined grid state, only as far and as long as necessary.
    Netzorientiert,
    /// Ziff. 10.5: on planning data, transitional, at most two hours a day.
    Praeventiv,
}

impl Regime {
    /// The word the publication uses.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Regime::Netzorientiert => "netzorientiert",
            Regime::Praeventiv => "präventiv",
        }
    }
}

/// Which SteuVE take part in relieving a congestion (Ziff. 4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Selection {
    /// Every SteuVE of the Netzbereich, as Ziff. 4.3 Satz 2 presumes they all relieve equally.
    Netzbereich,
    /// Only the SteuVE electrically behind the congested asset — a DSO's narrower reading.
    Downstream,
}

/// The parameters of one regime, valid over an interval, with the source they come from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleSet {
    /// A stable name recorded with every command, e.g. `bk6-22-300/netzorientiert`.
    pub name: String,
    /// The documents and clauses the numbers come from.
    pub source: String,
    /// The regime this set authorises.
    pub regime: Regime,
    /// First day on which it authorises commands.
    pub valid_from: Date,
    /// Last day on which it authorises commands, if it ends.
    pub valid_until: Option<Date>,

    /// Ziff. 4.5.1: the minimum per SteuVE (4,2 kW).
    pub min_per_device: Kw,
    /// Ziff. 4.5.1: above this, a heat pump or cooling is owed a share instead (11 kW).
    pub large_thermal: Kw,
    /// Ziff. 4.5.1: the share a large heat pump or cooling is owed (0,4).
    pub large_thermal_share: f64,
    /// Ziff. 4.5.2: Gleichzeitigkeitsfaktor by number of SteuVE; the last entry holds beyond.
    pub gzf: Vec<(u32, f64)>,

    /// Who is drawn on (Ziff. 4.3).
    pub selection: Selection,
    /// FNN Hinweis 2c: act from this share of the permissible loading or voltage drop (≥ 0,8).
    pub trigger: f64,
    /// Reductions aim at `trigger · rating · (1 − margin)`.
    pub margin: f64,
    /// Lower voltage limit (−8 % of nominal, the LV share of EN 50160).
    pub v_min: PerUnit,
    /// One-sided z-score: act only if the estimate exceeds the trigger at this confidence.
    pub confidence_z: f64,
    /// FNN Hinweis 2e: the state is determined only if its 99th-percentile error is within
    /// this share of the rating for current (10 %) …
    pub accuracy_current: f64,
    /// … and within this many p.u. for voltage (2 %).
    pub accuracy_voltage: f64,
    /// Netzorientiert demands a determined state; präventiv rests on planning data.
    pub require_determined: bool,
    /// Measurements older than this are not used (a result is valid for at most 5 minutes).
    pub max_data_age: SignedDuration,

    /// Ziff. 4.3: return is stepwise — each cycle restores this share of the remaining reduction.
    pub release_step: f64,
    /// FNN Hinweis 2c: the return proceeds in slots of this length.
    pub release_interval: SignedDuration,
    /// Released when the unreduced flow would stay below `trigger · rating · (1 − hysteresis)`.
    pub hysteresis: f64,
    /// The share of a feeder's controllable resources whose state may change in one cycle.
    pub max_switch_share: f64,
    /// Every command carries this end time and is renewed while needed (FNN Hinweis 2c).
    pub command_duration: SignedDuration,
    /// A command not acknowledged within this time is retried.
    pub ack_timeout: SignedDuration,
    /// Attempts before a resource is `Unknown`.
    pub max_attempts: u32,
    /// Präventiv only: reduction time allowed per connection and day.
    pub daily_budget: Option<SignedDuration>,
}

impl RuleSet {
    /// BK6-22-300, netzorientierte Steuerung, in force since 1 January 2024.
    #[must_use]
    pub fn bk6_22_300() -> RuleSet {
        RuleSet {
            name: "bk6-22-300/netzorientiert".into(),
            source: "BNetzA BK6-22-300 Anlage 1 Ziff. 2.4, 3.1, 4.1–4.6, 7, 8.4; VDE FNN Hinweise 2c, 2e (2025)".into(),
            regime: Regime::Netzorientiert,
            valid_from: date(2024, 1, 1),
            valid_until: None,
            min_per_device: THRESHOLD,
            large_thermal: Kw(11.0),
            large_thermal_share: 0.4,
            gzf: vec![(1, 1.0), (2, 0.8), (3, 0.75), (4, 0.7), (5, 0.65), (6, 0.6), (7, 0.55), (8, 0.5), (9, 0.45)],
            selection: Selection::Netzbereich,
            trigger: 0.9,
            margin: 0.05,
            v_min: PerUnit(0.92),
            confidence_z: 1.645,
            accuracy_current: 0.10,
            accuracy_voltage: 0.02,
            require_determined: true,
            max_data_age: SignedDuration::from_mins(5),
            release_step: 0.5,
            release_interval: SignedDuration::from_mins(15),
            hysteresis: 0.10,
            max_switch_share: 0.5,
            command_duration: SignedDuration::from_mins(60),
            ack_timeout: SignedDuration::from_mins(1),
            max_attempts: 3,
            daily_budget: None,
        }
    }

    /// BK6-22-300 Ziff. 10.5, präventive Steuerung on planning data: at most two hours per
    /// connection and day, for at most 24 months from `first_use` in the Netzbereich, and never
    /// after 31 December 2028.
    #[must_use]
    pub fn bk6_22_300_praeventiv(first_use: Date) -> RuleSet {
        let window_end = first_use.checked_add(24.months()).ok().and_then(|d| d.yesterday().ok()).unwrap_or(first_use);
        RuleSet {
            name: "bk6-22-300/praeventiv".into(),
            source: "BNetzA BK6-22-300 Anlage 1 Ziff. 10.5 a–c".into(),
            regime: Regime::Praeventiv,
            valid_from: first_use,
            valid_until: Some(window_end.min(date(2028, 12, 31))),
            confidence_z: 0.0,
            require_determined: false,
            max_data_age: SignedDuration::from_mins(30),
            daily_budget: Some(SignedDuration::from_hours(2)),
            ..RuleSet::bk6_22_300()
        }
    }

    /// Check every parameter against what the Festlegung and the FNN Hinweise allow. A rule set is
    /// configuration, and configuration can be edited: a floor of 0 kW or a präventiv window past
    /// 2028 must not become an engine that obeys it.
    ///
    /// # Errors
    /// Every violated bound, each with the clause it comes from.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut e = Vec::new();
        let legal = RuleSet::bk6_22_300();
        if self.min_per_device < legal.min_per_device {
            e.push(format!("min_per_device {} is below the 4,2 kW of Ziff. 4.5.1", self.min_per_device));
        }
        if self.large_thermal_share < legal.large_thermal_share || self.large_thermal > legal.large_thermal {
            e.push("large heat pumps and cooling are owed at least 0,4 × their power above 11 kW (Ziff. 4.5.1)".into());
        }
        for n in 1..=12 {
            if self.gzf(n) < legal.gzf(n) {
                e.push(format!("GZF for {n} SteuVE is {} — below the {} of Ziff. 4.5.2", self.gzf(n), legal.gzf(n)));
            }
        }
        if !(0.8..=1.0).contains(&self.trigger) {
            e.push(format!("trigger {} is outside 0,8–1,0 (FNN Hinweis 2c)", self.trigger));
        }
        for (name, v) in
            [("margin", self.margin), ("hysteresis", self.hysteresis), ("release_step", self.release_step), ("max_switch_share", self.max_switch_share)]
        {
            if !(0.0..=1.0).contains(&v) || v.is_nan() {
                e.push(format!("{name} {v} is not a share between 0 and 1"));
            }
        }
        if self.max_switch_share <= 0.0 {
            e.push("max_switch_share 0 would forbid every command".into());
        }
        if self.confidence_z < 0.0 || self.accuracy_current <= 0.0 || self.accuracy_voltage <= 0.0 {
            e.push("confidence and accuracy must be positive".into());
        }
        if self.regime == Regime::Netzorientiert && !self.require_determined {
            e.push("netzorientierte Steuerung requires a determined grid state (Ziff. 4.1, 2.6)".into());
        }
        if self.max_data_age > SignedDuration::from_mins(5) && self.regime == Regime::Netzorientiert {
            e.push("a Netzzustandsermittlung is valid for at most 5 minutes (Ziff. 4.2, FNN 2g)".into());
        }
        if self.command_duration <= SignedDuration::ZERO {
            e.push("every command needs an end time (FNN Hinweis 2c)".into());
        }
        if self.regime == Regime::Praeventiv {
            if self.daily_budget.is_none_or(|b| b > SignedDuration::from_hours(2)) {
                e.push("präventive Steuerung is limited to two hours a day (Ziff. 10.5 c)".into());
            }
            if self.valid_until.is_none_or(|u| u > date(2028, 12, 31)) {
                e.push("präventive Steuerung ends on 31.12.2028 at the latest (Ziff. 10.5)".into());
            }
            if self.valid_until.is_some_and(|u| self.valid_from.checked_add(24.months()).is_ok_and(|lim| u >= lim)) {
                e.push("präventive Steuerung lasts at most 24 months per Netzbereich (Ziff. 10.5 a)".into());
            }
        }
        if e.is_empty() { Ok(()) } else { Err(e) }
    }

    /// Whether the set authorises commands on `on`.
    #[must_use]
    pub fn valid_on(&self, on: Date) -> bool {
        on >= self.valid_from && self.valid_until.is_none_or(|u| on <= u)
    }

    /// The Gleichzeitigkeitsfaktor for `n` SteuVE.
    #[must_use]
    pub fn gzf(&self, n: u32) -> f64 {
        self.gzf.iter().rev().find(|(k, _)| *k <= n).map_or(1.0, |(_, f)| *f)
    }

    /// What one SteuVE is owed in Direktansteuerung (Ziff. 4.5.1).
    #[must_use]
    pub fn device_floor(&self, d: &SteuVe) -> Kw {
        if d.kind.is_thermal() && d.nominal > self.large_thermal { d.nominal * self.large_thermal_share } else { self.min_per_device.min(d.nominal) }
    }

    /// The Mindestleistung of a connection on `on`, with the explanation of how it was computed.
    ///
    /// `None` when the connection has no SteuVE.
    #[must_use]
    pub fn floor(&self, c: &Connection, on: Date) -> Option<Floor> {
        let steuve = c.steuve(on);
        if steuve.is_empty() {
            return None;
        }
        let n = u32::try_from(steuve.len()).unwrap_or(u32::MAX);
        match c.mode {
            ControlMode::Direct => {
                let per: Vec<(String, Kw)> = steuve.iter().map(|d| (d.id.clone(), self.device_floor(d))).collect();
                let total = per.iter().map(|(_, k)| *k).sum();
                let parts: Vec<String> = per.iter().map(|(id, k)| format!("{id} {:.2}", k.0)).collect();
                Some(Floor {
                    total,
                    per_device: per,
                    explanation: format!("Direktansteuerung: {} = {:.2} kW [Ziff. 4.5.1]", parts.join(" + "), Kw::round_w(total).0),
                })
            }
            ControlMode::Ems => {
                let sum = |kind: DeviceKind| -> Kw { steuve.iter().filter(|d| d.kind == kind).map(|d| d.nominal).sum() };
                let (hp, cool) = (sum(DeviceKind::HeatPump), sum(DeviceKind::Cooling));
                let large = hp > self.large_thermal || cool > self.large_thermal;
                let base = if large { (hp * self.large_thermal_share).max(cool * self.large_thermal_share) } else { self.min_per_device };
                let g = self.gzf(n);
                let total = base + self.min_per_device * (f64::from(n - 1) * g);
                let base_txt = if large { format!("max(0,4 · {:.1}; 0,4 · {:.1})", hp.0, cool.0) } else { format!("{:.1}", base.0) };
                Some(Floor {
                    total,
                    per_device: Vec::new(),
                    explanation: format!("EMS: {base_txt} + ({n} − 1) · {g} · {:.1} = {:.2} kW [Ziff. 4.5.2]", self.min_per_device.0, total.round_w().0),
                })
            }
        }
    }

    /// A limit for `c`, raised to the floor if `requested` is below it.
    ///
    /// This is the only way to make a [`Limit`], which is what makes "no command below the
    /// floor" a property of the type rather than of a check somebody might forget. A relay-only
    /// connection has one reduced state: its floor.
    #[must_use]
    pub fn limit(&self, c: &Connection, requested: Kw, on: Date) -> Option<Limit> {
        let floor = self.floor(c, on)?.total.round_w();
        let value = if c.relay_only { floor } else { requested.round_w().max(floor) };
        Some(Limit { value, floor, raised: requested.round_w() < floor })
    }
}

/// A connection's Mindestleistung and how it came about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Floor {
    /// The guaranteed minimum of the SteuVE consumption.
    pub total: Kw,
    /// In Direktansteuerung, each SteuVE's own minimum.
    pub per_device: Vec<(String, Kw)>,
    /// The computation, with the clause it follows.
    pub explanation: String,
}

/// A commanded limit on the SteuVE consumption of one connection, never below its floor.
///
/// Serialize-only on purpose: a `Limit` cannot be read back from untrusted bytes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Limit {
    value: Kw,
    floor: Kw,
    raised: bool,
}

impl Limit {
    /// The commanded value.
    #[must_use]
    pub fn value(&self) -> Kw {
        self.value
    }

    /// The floor it was checked against.
    #[must_use]
    pub fn floor(&self) -> Kw {
        self.floor
    }

    /// Whether the requested value was below the floor and had to be raised.
    #[must_use]
    pub fn raised(&self) -> bool {
        self.raised
    }
}
