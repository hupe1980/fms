//! The reference days, over many seeds: the properties the simulator exists to check.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use fms::sim;

const SEEDS: std::ops::RangeInclusive<u64> = 1..=4;

#[test]
fn no_day_ever_goes_below_a_floor() {
    for seed in SEEDS {
        for d in sim::days() {
            let r = sim::run(&d, seed).unwrap().report;
            assert_eq!(r.floor_violations, 0, "{} seed {seed}", d.name);
            assert_eq!(r.below_minimum, 0, "{} seed {seed}: the household guard saw a limit below its own floor", d.name);
        }
    }
}

#[test]
fn relievable_days_are_relieved() {
    for seed in SEEDS {
        for name in ["cold-snap", "wallbox-evening", "channel-outage"] {
            let r = sim::run(&sim::day(name).unwrap(), seed).unwrap().report;
            if seed == 1 {
                assert!(r.without.congested_minutes > 0.0, "{name} seed 1 should be congested without the FMS");
            }
            assert!(
                r.with.congested_minutes <= (r.without.congested_minutes * 0.1).max(15.0),
                "{name} seed {seed}: {} min congested with the FMS, {} without",
                r.with.congested_minutes,
                r.without.congested_minutes
            );
        }
    }
}

#[test]
fn quiet_days_and_lying_sensors_cause_nothing() {
    for seed in SEEDS {
        for name in ["pv-noon", "lying-sensor"] {
            let r = sim::run(&sim::day(name).unwrap(), seed).unwrap().report;
            assert_eq!(r.commands.values().sum::<usize>(), 0, "{name} seed {seed}: {:?}", r.commands);
        }
        let r = sim::run(&sim::day("lying-sensor").unwrap(), seed).unwrap().report;
        assert!(r.rejected >= 2, "the spoofed RTU and the lying meter are refused");
    }
}

#[test]
fn beyond_14a_reports_a_shortfall_rather_than_cutting_deeper() {
    let r = sim::run(&sim::day("beyond-14a").unwrap(), 1).unwrap().report;
    assert!(r.alarms.get("shortfall").copied().unwrap_or(0) > 0);
    assert!(r.with.peak_loading_percent < r.without.peak_loading_percent);
}

#[test]
fn praeventiv_stays_within_two_hours_a_day() {
    let run = sim::run(&sim::day("blind-feeder").unwrap(), 1).unwrap();
    let end = run.log.records().last().unwrap().at;
    let mut per: std::collections::BTreeMap<_, f64> = std::collections::BTreeMap::new();
    for iv in fms::evidence::interventions(&run.log, end) {
        assert_eq!(iv.regime, fms::rules::Regime::Praeventiv);
        *per.entry(iv.connection).or_default() += iv.duration().as_secs_f64() / 3600.0;
    }
    assert!(!per.is_empty());
    // A blind feeder is reported as one: FNN 2e asks 70 % of connections with nothing at the ONS.
    assert_eq!(run.report.metered_share, 0.0);
    assert_eq!(run.report.required_share, 0.7);
    // The budget is spent in whole control cycles; one cycle of slack.
    assert!(per.values().all(|h| *h <= 2.0 + 5.0 / 60.0 + 1e-9), "{per:?}");
}

#[test]
fn a_seed_is_a_replay() {
    let d = sim::day("channel-outage").unwrap();
    assert_eq!(sim::run(&d, 9).unwrap().log.head(), sim::run(&d, 9).unwrap().log.head());
    assert_ne!(sim::run(&d, 9).unwrap().log.head(), sim::run(&d, 10).unwrap().log.head());
}

#[test]
fn the_publication_matches_what_the_steuerboxen_applied() {
    // The publication is computed from the log; the truth from what the simulated Steuerboxen
    // actually held. They must agree — this is the check the log cannot give itself. Tolerance:
    // two five-minute cycles (the log ends at its last record, the truth at the end of the day).
    for seed in 1..=2 {
        for d in sim::days() {
            let r = sim::run(&d, seed).unwrap().report;
            match r.publication.first() {
                None => assert_eq!(r.truth.hours, 0.0, "{} seed {seed}", d.name),
                Some(p) => {
                    assert!((p.hours - r.truth.hours).abs() <= 0.17, "{} seed {seed}: hours {} vs {}", d.name, p.hours, r.truth.hours);
                    assert!(p.steuve.abs_diff(r.truth.steuve) <= 3, "{} seed {seed}: SteuVE {} vs {}", d.name, p.steuve, r.truth.steuve);
                    assert!(
                        (p.intensity_percent - r.truth.intensity_percent).abs() <= 0.02,
                        "{} seed {seed}: intensity {} vs {}",
                        d.name,
                        p.intensity_percent,
                        r.truth.intensity_percent
                    );
                }
            }
        }
    }
}

#[test]
fn the_stated_uncertainty_covers_the_truth() {
    for seed in SEEDS {
        for name in ["cold-snap", "wallbox-evening", "channel-outage", "lying-sensor"] {
            let r = sim::run(&sim::day(name).unwrap(), seed).unwrap().report;
            assert!(r.coverage_95 >= 0.95, "{name} seed {seed}: the 95 % interval covered the truth in {} of cycles", r.coverage_95);
        }
    }
}

#[test]
fn ten_percent_message_loss_does_not_change_the_outcome() {
    let mut d = sim::day("cold-snap").unwrap();
    d.loss = 0.10;
    for seed in SEEDS {
        let r = sim::run(&d, seed).unwrap().report;
        assert!(r.with.congested_minutes <= (r.without.congested_minutes * 0.1).max(15.0), "seed {seed}: {} min with 10 % loss", r.with.congested_minutes);
        assert!(r.retries > 0, "losses were retried");
        assert_eq!(r.floor_violations + r.below_minimum, 0);
    }
}

#[test]
#[cfg_attr(debug_assertions, ignore = "timing is only meaningful in release")]
fn a_day_runs_in_well_under_two_seconds() {
    let start = std::time::Instant::now();
    sim::run(&sim::day("cold-snap").unwrap(), 1).unwrap();
    assert!(start.elapsed() < std::time::Duration::from_secs(2), "{:?}", start.elapsed());
}
