//! The radial low-voltage topology: one transformer, line sections, nodes.
//!
//! Flows are the sums of the loads downstream; voltages follow the linearised branch-flow
//! model (LinDistFlow) at unity power factor: `ΔV = Σ R·P / V²` along the path to the
//! transformer. Both are linear in the loads, which is what lets the estimator carry its
//! uncertainty through them exactly and the allocator know each load's sensitivity.

use crate::model::NodeId;
use crate::units::Kw;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// The transformer of the Ortsnetzstation, the root of the feeder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transformer {
    /// Asset identifier.
    pub id: String,
    /// The busbar node it feeds.
    pub node: NodeId,
    /// Rated power (kVA, treated as kW at unity power factor).
    pub rating: Kw,
}

/// A line section between two nodes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Line {
    /// Asset identifier.
    pub id: String,
    /// The node nearer the transformer.
    pub from: NodeId,
    /// The node further away.
    pub to: NodeId,
    /// Thermal rating as power at nominal voltage.
    pub rating: Kw,
    /// Resistance in ohm.
    pub r_ohm: f64,
}

/// A topology as imported (for example from CIM), before it is checked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GridSpec {
    /// Nominal line-to-line voltage in kV (0.4 in German LV).
    pub v_nom_kv: f64,
    /// The transformer.
    pub transformer: Transformer,
    /// The line sections.
    pub lines: Vec<Line>,
}

/// Why a topology was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopologyError {
    /// A node is fed by two lines — the grid is not radial.
    NotRadial(NodeId),
    /// A line starts at a node no path from the transformer reaches.
    Unreachable(String),
    /// A non-positive rating or a negative resistance.
    BadParameter(String),
}

impl fmt::Display for TopologyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TopologyError::NotRadial(n) => write!(f, "node {n} is fed by more than one line; only radial feeders are supported"),
            TopologyError::Unreachable(l) => write!(f, "line {l} is not connected to the transformer"),
            TopologyError::BadParameter(a) => write!(f, "asset {a} has a non-positive rating or a negative resistance"),
        }
    }
}

impl std::error::Error for TopologyError {}

/// An asset with a thermal limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AssetRef {
    /// The transformer.
    Transformer,
    /// The line with this index.
    Line(usize),
}

/// A checked radial topology.
#[derive(Debug, Clone)]
pub struct Topology {
    spec: GridSpec,
    /// For each node, the indices of the lines from the transformer to it, root first.
    paths: BTreeMap<NodeId, Vec<usize>>,
}

impl Topology {
    /// Check that `spec` is a tree rooted at the transformer.
    ///
    /// # Errors
    /// [`TopologyError`] if it is meshed, disconnected or has impossible parameters.
    pub fn new(spec: GridSpec) -> Result<Topology, TopologyError> {
        if spec.transformer.rating.0 <= 0.0 {
            return Err(TopologyError::BadParameter(spec.transformer.id.clone()));
        }
        let mut fed: BTreeSet<&NodeId> = BTreeSet::new();
        for l in &spec.lines {
            if l.rating.0 <= 0.0 || l.r_ohm < 0.0 {
                return Err(TopologyError::BadParameter(l.id.clone()));
            }
            if !fed.insert(&l.to) || l.to == spec.transformer.node {
                return Err(TopologyError::NotRadial(l.to.clone()));
            }
        }
        let mut paths: BTreeMap<NodeId, Vec<usize>> = BTreeMap::new();
        paths.insert(spec.transformer.node.clone(), Vec::new());
        // Breadth-first from the root; a line whose `from` is never reached is disconnected.
        let mut frontier = vec![spec.transformer.node.clone()];
        while let Some(n) = frontier.pop() {
            let base = paths.get(&n).cloned().unwrap_or_default();
            for (i, l) in spec.lines.iter().enumerate() {
                if l.from == n && !paths.contains_key(&l.to) {
                    let mut p = base.clone();
                    p.push(i);
                    paths.insert(l.to.clone(), p);
                    frontier.push(l.to.clone());
                }
            }
        }
        if let Some(l) = spec.lines.iter().find(|l| !paths.contains_key(&l.to)) {
            return Err(TopologyError::Unreachable(l.id.clone()));
        }
        Ok(Topology { spec, paths })
    }

    /// The specification it was built from.
    #[must_use]
    pub fn spec(&self) -> &GridSpec {
        &self.spec
    }

    /// Whether `node` exists.
    #[must_use]
    pub fn contains(&self, node: &NodeId) -> bool {
        self.paths.contains_key(node)
    }

    /// Every asset with a thermal limit, transformer first.
    pub fn assets(&self) -> impl Iterator<Item = AssetRef> + '_ {
        std::iter::once(AssetRef::Transformer).chain((0..self.spec.lines.len()).map(AssetRef::Line))
    }

    /// The nodes, in a stable order.
    pub fn nodes(&self) -> impl Iterator<Item = &NodeId> {
        self.paths.keys()
    }

    /// The asset's identifier.
    #[must_use]
    pub fn asset_id(&self, a: AssetRef) -> &str {
        match a {
            AssetRef::Transformer => &self.spec.transformer.id,
            AssetRef::Line(i) => self.spec.lines.get(i).map_or("?", |l| l.id.as_str()),
        }
    }

    /// The asset's thermal rating.
    #[must_use]
    pub fn rating(&self, a: AssetRef) -> Kw {
        match a {
            AssetRef::Transformer => self.spec.transformer.rating,
            AssetRef::Line(i) => self.spec.lines.get(i).map_or(Kw::ZERO, |l| l.rating),
        }
    }

    /// Whether a load at `node` flows through asset `a` (its sensitivity, 0 or 1).
    #[must_use]
    pub fn downstream(&self, a: AssetRef, node: &NodeId) -> bool {
        match a {
            AssetRef::Transformer => self.contains(node),
            AssetRef::Line(i) => self.paths.get(node).is_some_and(|p| p.contains(&i)),
        }
    }

    /// The voltage sensitivity `∂V_at/∂P_load` in p.u. per kW (negative: load lowers voltage).
    #[must_use]
    pub fn voltage_sensitivity(&self, at: &NodeId, load: &NodeId) -> f64 {
        let (Some(pa), Some(pl)) = (self.paths.get(at), self.paths.get(load)) else { return 0.0 };
        let v2 = self.spec.v_nom_kv * self.spec.v_nom_kv * 1000.0;
        let r: f64 = pa.iter().zip(pl.iter()).take_while(|(a, b)| a == b).filter_map(|(i, _)| self.spec.lines.get(*i)).map(|l| l.r_ohm).sum();
        -r / v2
    }

    /// Depth of a node (number of lines to the transformer).
    #[must_use]
    pub fn depth(&self, node: &NodeId) -> usize {
        self.paths.get(node).map_or(0, Vec::len)
    }

    /// For each line, the I²R losses of that line and every line beyond it, given each line's
    /// downstream load and the per-unit voltage at its far end. A few fixed-point steps: the flow
    /// through a section carries the losses beyond it, and current grows as voltage sags.
    #[must_use]
    pub fn losses_below(&self, line_loads: &[f64], vm: &[f64]) -> Vec<f64> {
        let lines = &self.spec.lines;
        let v2 = self.spec.v_nom_kv.powi(2) * 1000.0;
        let below: Vec<Vec<usize>> =
            (0..lines.len()).map(|i| lines.iter().enumerate().filter(|(_, l)| self.downstream(AssetRef::Line(i), &l.to)).map(|(k, _)| k).collect()).collect();
        let mut own = vec![0.0; lines.len()];
        let mut acc = vec![0.0; lines.len()];
        for _ in 0..4 {
            own = lines
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    let beyond = acc.get(i).copied().unwrap_or(0.0) - own.get(i).copied().unwrap_or(0.0);
                    let f = line_loads.get(i).copied().unwrap_or(0.0) + beyond;
                    let v = vm.get(i).copied().unwrap_or(1.0).max(0.5);
                    l.r_ohm * f * f / (v2 * v * v)
                })
                .collect();
            acc = below.iter().map(|b| b.iter().map(|k| own.get(*k).copied().unwrap_or(0.0)).sum()).collect();
        }
        acc
    }

    /// The losses carried by the transformer: everything below the lines leaving its busbar.
    #[must_use]
    pub fn transformer_losses(&self, losses_below: &[f64]) -> f64 {
        self.spec.lines.iter().enumerate().filter(|(_, l)| l.from == self.spec.transformer.node).map(|(i, _)| losses_below.get(i).copied().unwrap_or(0.0)).sum()
    }

    /// The voltage drop along the path to `node` caused by the loss flow.
    #[must_use]
    pub fn loss_voltage_drop(&self, node: &NodeId, losses_below: &[f64]) -> f64 {
        let v2 = self.spec.v_nom_kv.powi(2) * 1000.0;
        self.paths
            .get(node)
            .map_or(0.0, |p| p.iter().map(|i| self.spec.lines.get(*i).map_or(0.0, |l| l.r_ohm) * losses_below.get(*i).copied().unwrap_or(0.0) / v2).sum())
    }

    /// Flows and voltages for known loads per node: the linear branch-flow model with its
    /// second-order loss correction. Within a fraction of a per cent of an AC power flow on LV
    /// feeders (checked against pandapower).
    #[must_use]
    pub fn flows(&self, node_loads: &BTreeMap<NodeId, f64>) -> Flows {
        let load = |pred: &dyn Fn(&NodeId) -> bool| -> f64 { node_loads.iter().filter(|(n, _)| pred(n)).map(|(_, p)| p).sum() };
        let lines = &self.spec.lines;
        let line_loads: Vec<f64> = (0..lines.len()).map(|i| load(&|n| self.downstream(AssetRef::Line(i), n))).collect();
        let lin_v = |n: &NodeId| 1.0 + node_loads.iter().map(|(m, p)| self.voltage_sensitivity(n, m) * p).sum::<f64>();
        let vm: Vec<f64> = lines.iter().map(|l| lin_v(&l.to)).collect();
        let below = self.losses_below(&line_loads, &vm);
        let line: Vec<f64> = line_loads.iter().zip(&below).map(|(l, b)| l + b).collect();
        let transformer = node_loads.values().sum::<f64>() + self.transformer_losses(&below);
        let voltage = self.paths.keys().map(|n| (n.clone(), lin_v(n) - self.loss_voltage_drop(n, &below))).collect();
        Flows { line, transformer, voltage }
    }
}

/// Flows and voltages of a feeder.
#[derive(Debug, Clone, PartialEq)]
pub struct Flows {
    /// Sending-end flow per line, in the order of the specification.
    pub line: Vec<f64>,
    /// Through the transformer.
    pub transformer: f64,
    /// Per node, p.u.
    pub voltage: BTreeMap<NodeId, f64>,
}
