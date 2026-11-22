//! The `fms` binary, end to end: what a DSO or a reviewer actually types.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::process::{Command, Output};

fn fms(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_fms")).args(args).env("NO_COLOR", "1").output().unwrap()
}

fn text(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("fms-cli-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_day_its_log_its_verification_and_its_publication() {
    let dir = scratch("day");
    let o = fms(&["sim", "cold-snap", "--out", dir.to_str().unwrap()]);
    assert!(o.status.success(), "{}", text(&o));
    let out = text(&o);
    assert!(out.contains("below the floor 0 ✓"), "{out}");
    assert!(out.contains("congested"), "{out}");

    let log = dir.join("evidence.jsonl");
    let o = fms(&["verify", log.to_str().unwrap()]);
    assert!(o.status.success(), "{}", text(&o));
    let report: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("report.json")).unwrap()).unwrap();
    let head = report["head"].as_str().unwrap();
    assert!(fms(&["verify", log.to_str().unwrap(), "--expect-head", head]).status.success());

    // Cut the tail: the chain still verifies, the anchor does not.
    let lines: Vec<String> = std::fs::read_to_string(&log).unwrap().lines().map(str::to_owned).collect();
    let cut = dir.join("cut.jsonl");
    std::fs::write(&cut, lines[..lines.len() - 2].join("\n")).unwrap();
    assert!(fms(&["verify", cut.to_str().unwrap()]).status.success());
    let o = fms(&["verify", cut.to_str().unwrap(), "--expect-head", head]);
    assert!(!o.status.success());
    assert!(text(&o).contains("not in this log"), "{}", text(&o));

    let o = fms(&["publish", log.to_str().unwrap(), "--connections", dir.join("connections.json").to_str().unwrap(), "--month", "2026-01"]);
    assert!(o.status.success(), "{}", text(&o));
    let csv = String::from_utf8_lossy(&o.stdout);
    assert!(csv.starts_with("NeBe-ID;PLZ;Art der Steuerung;"), "{csv}");
    assert!(csv.contains("NB-34117-0042;\"34117;34119\";netzorientiert;"), "{csv}");

    let first = report["series"].as_array().unwrap().len();
    assert_eq!(first, 96, "a chart point every 15 minutes");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn floors_are_explained_with_their_clause() {
    let o = fms(&["floor", "heat-pump:9", "wallbox:11", "cooling:3"]);
    assert!(o.status.success());
    let t = text(&o);
    assert!(t.contains("7.56 kW"), "{t}");
    assert!(t.contains("Ziff. 4.5.2"), "{t}");
    assert!(t.contains("not a SteuVE"), "{t}");
    assert!(text(&fms(&["floor", "--mode", "direct", "heat-pump:15"])).contains("6.00 kW"));
    assert!(!fms(&["floor", "sauna:9"]).status.success());
}

#[test]
fn sr_ids_are_checked_and_completed() {
    assert_eq!(String::from_utf8_lossy(&fms(&["sr-id", "C113735592"]).stdout).trim(), "C1137355923");
    assert!(fms(&["sr-id", "C1137355923"]).status.success());
    assert!(!fms(&["sr-id", "C1137355924"]).status.success());
}

#[test]
fn unknown_days_and_oversized_inputs_are_errors_not_panics() {
    let o = fms(&["sim", "no-such-day"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(text(&o).contains("fms days"));
    // A sparse 2 GiB file costs nothing on disk and must be refused before it is read.
    let dir = scratch("big");
    let big = dir.join("big.jsonl");
    std::fs::File::create(&big).unwrap().set_len(2 << 30).unwrap();
    let o = fms(&["verify", big.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(2));
    assert!(text(&o).contains("1 GiB"), "{}", text(&o));
    std::fs::remove_dir_all(dir).ok();
}
