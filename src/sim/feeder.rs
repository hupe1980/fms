//! A deterministic, SimBench-like low-voltage feeder with households behind it.

use super::Prng;
use crate::grid::{GridSpec, Line, Transformer};
use crate::model::{Connection, ConnectionId, ControlMode, Device, DeviceKind, NodeId};
use crate::units::Kw;
use jiff::SignedDuration;
use jiff::civil::date;
use serde::{Deserialize, Serialize};

/// What the feeder looks like.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    /// Transformer rating, kVA.
    pub transformer_kva: f64,
    /// Nodes per branch, one entry per branch.
    pub branches: [usize; 4],
    /// Households per node.
    pub per_node: usize,
    /// Share of households with a heat pump.
    pub heat_pumps: f64,
    /// Share of households with a wallbox.
    pub wallboxes: f64,
    /// Share of households with a wallbox on the last branch (the new estate).
    pub wallboxes_last_branch: f64,
    /// Share with PV (and, of those, half with storage).
    pub pv: f64,
}

/// One household's physical equipment, beyond the master data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Household {
    /// The connection.
    pub id: ConnectionId,
    /// Branch index (0..4).
    pub branch: usize,
    /// Scales the base load.
    pub base_scale: f64,
    /// PV peak power, kWp.
    pub pv_kwp: f64,
    /// EV arrival, hours after midnight.
    pub ev_arrival: f64,
    /// EV energy needed, kWh.
    pub ev_need: f64,
    /// Smart meter gateway reporting TAF 10 values.
    pub metered: bool,
}

/// The grid, the master data, and the households.
#[derive(Debug, Clone)]
pub struct Feeder {
    /// Topology.
    pub grid: GridSpec,
    /// Master data.
    pub connections: Vec<Connection>,
    /// Physics.
    pub households: Vec<Household>,
}

/// Rating of NAYY 4×150 SE at 0,4 kV (≈ 275 A).
const CABLE_150_KW: f64 = 190.0;
/// Rating of NAYY 4×95 SE (≈ 215 A).
const CABLE_95_KW: f64 = 149.0;

impl Feeder {
    /// Generate a feeder from a seed. `metered` is the share of households with a smart meter gateway.
    #[must_use]
    pub fn generate(seed: u64, shape: Shape, metered: f64) -> Feeder {
        let mut rng = Prng::new(seed);
        let root = NodeId::from("ONS");
        let grid_lines = {
            let mut lines = Vec::new();
            for (b, &n) in shape.branches.iter().enumerate() {
                let mut from = root.clone();
                for k in 0..n {
                    let to = NodeId(format!("B{}N{:02}", b + 1, k + 1));
                    let thick = k < n / 2;
                    let (rating, r_per_km) = if thick { (CABLE_150_KW, 0.206) } else { (CABLE_95_KW, 0.320) };
                    let len_km = 0.03 + rng.uniform() * 0.02;
                    lines.push(Line {
                        id: format!("L{}-{:02}", b + 1, k + 1),
                        from: from.clone(),
                        to: to.clone(),
                        rating: Kw(rating),
                        r_ohm: r_per_km * len_km,
                    });
                    from = to;
                }
            }
            lines
        };
        let grid = GridSpec { v_nom_kv: 0.4, transformer: Transformer { id: "T1".into(), node: root, rating: Kw(shape.transformer_kva) }, lines: grid_lines };

        let mut connections = Vec::new();
        let mut households = Vec::new();
        let commissioned = date(2025, 4, 1);
        for (b, &n) in shape.branches.iter().enumerate() {
            for k in 0..n {
                for h in 0..shape.per_node {
                    let id = ConnectionId(format!("H{}{:02}{}", b + 1, k + 1, (b'a' + h as u8) as char));
                    let mut devices = Vec::new();
                    if rng.uniform() < shape.heat_pumps {
                        // Compressor plus Heizstab, as the Netzanschlussleistung the DSO registers.
                        let nominal = 5.0 + (rng.uniform() * 4.0).round();
                        devices.push(Device {
                            id: "hp".into(),
                            kind: DeviceKind::HeatPump,
                            nominal: Kw(nominal),
                            commissioned,
                            opted_in: false,
                            exempt: false,
                        });
                    }
                    let ev_share = if b == shape.branches.len() - 1 { shape.wallboxes_last_branch } else { shape.wallboxes };
                    let has_ev = rng.uniform() < ev_share;
                    if has_ev {
                        devices.push(Device {
                            id: "wb".into(),
                            kind: DeviceKind::ChargingPoint,
                            nominal: Kw(11.0),
                            commissioned,
                            opted_in: false,
                            exempt: false,
                        });
                    }
                    let pv = rng.uniform() < shape.pv;
                    if pv && rng.uniform() < 0.5 {
                        devices.push(Device { id: "bat".into(), kind: DeviceKind::Storage, nominal: Kw(5.0), commissioned, opted_in: false, exempt: false });
                    }
                    let roll = rng.uniform();
                    let mode = if roll < 0.7 { ControlMode::Ems } else { ControlMode::Direct };
                    let relay_only = roll > 0.95;
                    connections.push(Connection {
                        id: id.clone(),
                        resource: None,
                        node: NodeId(format!("B{}N{:02}", b + 1, k + 1)),
                        mode,
                        area: "NB-34117-0042".into(),
                        postcode: if b < 2 { "34117".into() } else { "34119".into() },
                        devices,
                        relay_only,
                        failsafe: None,
                        failsafe_for: SignedDuration::from_hours(2),
                        fuse: Kw(44.0),
                    });
                    households.push(Household {
                        id,
                        branch: b,
                        base_scale: 0.6 + rng.uniform() * 0.9,
                        pv_kwp: if pv { 5.0 + (rng.uniform() * 6.0).round() } else { 0.0 },
                        ev_arrival: 16.5 + rng.uniform() * 3.5,
                        ev_need: if has_ev { 10.0 + rng.uniform() * 22.0 } else { 0.0 },
                        metered: rng.uniform() < metered,
                    });
                }
            }
        }
        // The Steuerbox hands the EMS the Mindestleistung as its failsafe value.
        let rules = crate::rules::RuleSet::bk6_22_300();
        for c in &mut connections {
            c.failsafe = rules.floor(c, commissioned).map(|f| f.total.round_w());
        }
        Feeder { grid, connections, households }
    }
}
