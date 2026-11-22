//! The estimator against a power flow it did not write: pandapower's AC Newton–Raphson.
//!
//! Every connection metered, so the estimator's flows and voltages are its linearised branch-flow
//! model alone; pandapower solves the full AC equations on the same feeder and loads. The
//! difference is the linearisation and the losses it neglects.
//!
//! Skips when no Python with pandapower is found (`$FMS_PYTHON`, then `.venv/bin/python`);
//! `FMS_REQUIRE_PANDAPOWER=1` turns the skip into a failure, which is what CI sets.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use fms::grid::{AssetRef, Topology};
use fms::model::ConnectionId;
use fms::rules::RuleSet;
use fms::sim::{Feeder, Shape};
use fms::state::{self, Measurement, Reading};
use fms::units::Kw;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn python() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidates = [std::env::var_os("FMS_PYTHON").map(PathBuf::from), Some(root.join(".venv/bin/python")), Some(PathBuf::from("python3"))];
    candidates
        .into_iter()
        .flatten()
        .find(|p| Command::new(p).args(["-c", "import pandapower"]).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success()))
}

#[test]
fn linearised_flows_and_voltages_agree_with_pandapower() {
    let Some(py) = python() else {
        assert!(std::env::var_os("FMS_REQUIRE_PANDAPOWER").is_none(), "FMS_REQUIRE_PANDAPOWER is set but no Python with pandapower was found");
        eprintln!("skipped: no Python with pandapower (set FMS_PYTHON)");
        return;
    };
    let shape = Shape { transformer_kva: 400.0, branches: [8, 8, 7, 12], per_node: 3, heat_pumps: 0.5, wallboxes: 0.4, wallboxes_last_branch: 0.9, pv: 0.0 };
    for seed in 1..=3u64 {
        let f = Feeder::generate(seed, shape, 1.0);
        let topo = Topology::new(f.grid.clone()).unwrap();
        let on = jiff::civil::date(2026, 1, 15);
        // An evening near the transformer rating: base load plus a third of the installed SteuVE power.
        let loads: BTreeMap<ConnectionId, f64> = f.connections.iter().map(|c| (c.id.clone(), 1.0 + 0.3 * c.steuve_nominal(on).0)).collect();

        let mut node_kw: BTreeMap<String, f64> = BTreeMap::new();
        for c in &f.connections {
            *node_kw.entry(c.node.0.clone()).or_default() += loads[&c.id];
        }
        let request = serde_json::json!({ "v_nom_kv": f.grid.v_nom_kv, "transformer": f.grid.transformer, "lines": f.grid.lines, "loads": node_kw });
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/oracle/pandapower_flow.py");
        let mut child = Command::new(&py).arg(&script).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(request.to_string().as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "pandapower failed");
        let pp: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();

        let conns: BTreeMap<_, _> = f.connections.iter().map(|c| (c.id.clone(), c.clone())).collect();
        let at: jiff::Timestamp = "2026-01-15T17:00:00Z".parse().unwrap();
        let meters: BTreeMap<_, _> = conns
            .keys()
            .map(|id| {
                (
                    id.clone(),
                    Measurement {
                        source: format!("smgw:{id}"),
                        at,
                        reading: Reading::Connection { connection: id.clone(), total: Kw(loads[id]), steuve: None },
                    },
                )
            })
            .collect();
        let rules = RuleSet::bk6_22_300();
        let inputs =
            state::Inputs { topology: &topo, connections: &conns, meters: &meters, assets: &BTreeMap::new(), profiles: &BTreeMap::new(), rules: &rules };
        let (snap, rejected) = state::estimate(&inputs, at);
        assert!(rejected.is_empty());

        let mut worst_flow: f64 = 0.0;
        for (i, line) in f.grid.lines.iter().enumerate() {
            let ours = snap.flow(&topo, &conns, AssetRef::Line(i)).mean;
            let theirs = pp["lines"][&line.id].as_f64().unwrap();
            worst_flow = worst_flow.max((ours - theirs).abs() / line.rating.0);
        }
        let mut worst_v: f64 = 0.0;
        for n in topo.nodes() {
            let ours = snap.voltage(&topo, &conns, n).mean;
            let theirs = pp["voltages"][&n.0].as_f64().unwrap();
            worst_v = worst_v.max((ours - theirs).abs());
        }
        let total = snap.flow(&topo, &conns, AssetRef::Transformer).mean;
        eprintln!(
            "seed {seed}: worst flow error {:.2} % of rating, worst voltage error {:.4} p.u.; losses {:.1} kW (pandapower {:.1} kW) on {total:.0} kW",
            worst_flow * 100.0,
            worst_v,
            total - loads.values().sum::<f64>(),
            pp["losses"].as_f64().unwrap()
        );
        assert!(worst_flow < 0.01, "seed {seed}: flow differs by {:.2} % of a rating", worst_flow * 100.0);
        assert!(worst_v < 0.005, "seed {seed}: voltage differs by {worst_v:.4} p.u.");
        // The topology's own flow model — the simulator's truth — agrees as well.
        let mut node: BTreeMap<fms::model::NodeId, f64> = BTreeMap::new();
        for c in &f.connections {
            *node.entry(c.node.clone()).or_default() += loads[&c.id];
        }
        let flows = topo.flows(&node);
        assert!((flows.transformer - pp["transformer"].as_f64().unwrap()).abs() < 0.005 * f.grid.transformer.rating.0);
    }
}
