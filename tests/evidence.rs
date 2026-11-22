//! The evidence log: tamper evidence, documentation, and the BDEW publication.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use fms::evidence::{Log, Measure, documentation, interventions, publication, publication_csv};
use fms::sim;
use jiff::civil::date;
use std::collections::BTreeMap;

fn cold_snap() -> sim::Run {
    sim::run(&sim::day("cold-snap").unwrap(), 1).unwrap()
}

#[test]
fn a_clean_log_verifies_and_round_trips() {
    let run = cold_snap();
    let text = run.log.to_jsonl();
    let back = Log::from_jsonl(&text).unwrap();
    assert_eq!(back.head(), run.log.head());
    assert_eq!(back.records().len(), run.log.records().len());
}

#[test]
fn every_kind_of_tampering_is_located() {
    let text = cold_snap().log.to_jsonl();
    let lines: Vec<&str> = text.lines().collect();
    let target = lines.iter().position(|l| l.contains("\"type\":\"command\"")).unwrap();

    // Changed: a commanded limit edited after the fact.
    let mut changed = lines.clone();
    let edited = changed[target].replacen("\"limit\":", "\"limit\":1", 1);
    changed[target] = &edited;
    let err = Log::from_jsonl(&changed.join("\n")).unwrap_err();
    assert_eq!(err.line, target + 1);
    assert!(err.why.contains("hash"), "{err}");

    // Removed.
    let mut removed = lines.clone();
    removed.remove(target);
    assert_eq!(Log::from_jsonl(&removed.join("\n")).unwrap_err().line, target + 1);

    // Inserted: a copy of a record slipped in after it.
    let mut inserted = lines.clone();
    inserted.insert(target + 1, lines[target]);
    assert_eq!(Log::from_jsonl(&inserted.join("\n")).unwrap_err().line, target + 2);

    // Reordered.
    let mut swapped = lines.clone();
    swapped.swap(target, target + 1);
    assert_eq!(Log::from_jsonl(&swapped.join("\n")).unwrap_err().line, target + 1);
}

#[test]
fn the_publication_follows_the_bdew_format() {
    let run = cold_snap();
    let p = publication(&run.log, &run.connections, date(2026, 1, 1), &BTreeMap::new());
    assert_eq!(p.rows.len(), 1);
    let r = &p.rows[0];
    assert_eq!(r.area, "NB-34117-0042");
    assert_eq!(r.postcodes, vec!["34117", "34119"]);
    assert!(r.hours > 0.5 && r.hours <= 24.0, "{r:?}");
    assert!(r.intensity_percent > 0.0 && r.intensity_percent < 100.0);
    assert!(r.steuve > 0 && r.steuve <= run.report.steuve);
    assert_eq!(r.measure, Measure::KeineMassnahme, "a fully relieved month needs no measure");
    assert!(!p.warnings.is_empty(), "an assumed measure is flagged for the DSO to confirm");

    let csv = publication_csv(&p);
    assert!(csv.lines().all(|l| !l.contains("H1") && !l.contains("smgw")), "no connection or meter id in the publication");
    let mut lines = csv.lines();
    assert_eq!(csv.lines().next().unwrap().split(';').count(), 8, "the eight BDEW fields");
    assert!(lines.next().unwrap().starts_with("NeBe-ID;PLZ;Art der Steuerung"));
    let row = lines.next().unwrap();
    assert!(row.starts_with("NB-34117-0042;\"34117;34119\";netzorientiert;"), "{row}");
    assert!(row.ends_with(";Keine Maßnahme;-"), "{row}");

    // Nothing in another month.
    assert!(publication(&run.log, &run.connections, date(2026, 2, 1), &BTreeMap::new()).rows.is_empty());
}

#[test]
fn intensity_is_the_monthly_mean_of_the_bdew_daily_value() {
    // The BDEW example: 11 kW held at 4,2 kW for 2 h in a Netzbereich of that one SteuVE gives
    // (11 − 4,2)/11 · 2/24 = 5,15 % for the day; January's value is that over 31 days.
    let run = cold_snap();
    let mut by_area: BTreeMap<String, f64> = BTreeMap::new();
    let on = date(2026, 1, 15);
    for c in &run.connections {
        *by_area.entry(c.area.clone()).or_default() += c.steuve_nominal(on).0;
    }
    let end = run.log.records().last().unwrap().at;
    let mut expected = 0.0;
    for iv in interventions(&run.log, end) {
        let c = run.connections.iter().find(|c| c.id == iv.connection).unwrap();
        let installed = c.steuve_nominal(on).0;
        expected += (installed - iv.limit.0).max(0.0) / by_area[&c.area] * iv.duration().as_secs_f64() / 86_400.0;
    }
    let expected = (expected / 31.0 * 100.0 * 100.0).round() / 100.0;
    let p = publication(&run.log, &run.connections, on, &BTreeMap::new());
    assert!((p.rows[0].intensity_percent - expected).abs() < 0.011, "{} vs {expected}", p.rows[0].intensity_percent);
    assert!(((11.0 - 4.2) / 11.0 * 2.0 / 24.0 * 100.0 - 5.15_f64).abs() < 0.01);
}

#[test]
fn a_shortfall_month_proposes_netzausbau() {
    let run = sim::run(&sim::day("beyond-14a").unwrap(), 1).unwrap();
    let p = publication(&run.log, &run.connections, date(2026, 1, 1), &BTreeMap::new());
    assert_eq!(p.rows[0].measure, Measure::Netzausbau);
    assert!(p.warnings.iter().any(|w| w.contains("shortfall")));
}

#[test]
fn a_customer_sees_only_their_own_reductions() {
    let run = cold_snap();
    let end = run.log.records().last().unwrap().at;
    let one = interventions(&run.log, end)[0].connection.clone();
    let doc = documentation(&run.log, &one, end);
    assert!(!doc.is_empty());
    assert!(doc.iter().all(|i| i.connection == one));
    assert!(doc.iter().all(|i| i.limit >= i.floor));
    // Ziff. 7.2: with 2 % loss almost every reduction has its acknowledgement as proof.
    let all = interventions(&run.log, end);
    let proven = all.iter().filter(|i| i.confirmed.is_some()).count();
    assert!(proven * 10 >= all.len() * 9, "{proven} of {} acknowledged", all.len());
}

#[test]
fn garbage_is_an_error_not_a_panic() {
    for junk in ["{", "null", "{\"seq\":1}", "\u{0}\u{1}", "[1,2,3]", &"x".repeat(100_000)] {
        assert!(Log::from_jsonl(junk).is_err(), "{junk:.20}");
    }
    assert!(Log::from_jsonl("").unwrap().records().is_empty());
}

#[test]
fn a_truncated_tail_is_caught_only_by_an_anchored_head() {
    let run = cold_snap();
    let full = run.log.to_jsonl();
    let anchored = run.log.head().to_owned();
    let lines: Vec<&str> = full.lines().collect();
    let cut = Log::from_jsonl(&lines[..lines.len() - 3].join("\n")).unwrap();
    // The chain alone cannot see it …
    assert_eq!(cut.records().len(), lines.len() - 3);
    // … the anchor can.
    assert_ne!(cut.head(), anchored);
    assert!(!cut.records().iter().any(|r| r.hash == anchored));
}
