//! The decision core: allocation, floors, release, dispatch, alarms.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use fms::command::Cause;
use fms::dispatch::State;
use fms::engine::{Alarm, Engine, Input, Output};
use fms::grid::{GridSpec, Line, Topology, Transformer};
use fms::model::{Connection, ControlMode, Device, DeviceKind};
use fms::rules::RuleSet;
use fms::state::{Measurement, Profile, Reading};
use fms::units::Kw;
use jiff::civil::date;
use jiff::{SignedDuration, Timestamp};

fn t(min: i64) -> Timestamp {
    "2026-01-15T17:00:00Z".parse::<Timestamp>().unwrap().saturating_add(SignedDuration::from_mins(min)).unwrap()
}

/// A 100 kVA transformer, one line, `n` EMS households with an 11 kW wallbox each.
fn setup(n: usize, rules: RuleSet) -> (Engine, Vec<Connection>) {
    let grid = GridSpec {
        v_nom_kv: 0.4,
        transformer: Transformer { id: "T1".into(), node: "ONS".into(), rating: Kw(100.0) },
        lines: vec![Line { id: "L1".into(), from: "ONS".into(), to: "N1".into(), rating: Kw(1000.0), r_ohm: 0.0001 }],
    };
    let conns: Vec<Connection> = (0..n)
        .map(|i| Connection {
            id: format!("H{i:02}").as_str().into(),
            resource: None,
            node: "N1".into(),
            mode: ControlMode::Ems,
            area: "NB-1".into(),
            postcode: "34117".into(),
            devices: vec![Device {
                id: "wb".into(),
                kind: DeviceKind::ChargingPoint,
                nominal: Kw(11.0),
                commissioned: date(2025, 1, 1),
                opted_in: false,
                exempt: false,
            }],
            relay_only: false,
            failsafe: Some(Kw(4.2)),
            failsafe_for: SignedDuration::from_hours(2),
            fuse: Kw(44.0),
        })
        .collect();
    (Engine::new(Topology::new(grid).unwrap(), conns.clone(), rules).unwrap(), conns)
}

/// Every household metered drawing `kw`, all of it the wallbox, plus the transformer.
fn measure(e: &mut Engine, conns: &[Connection], kw: f64, at: Timestamp) -> Vec<Output> {
    measure_at(e, conns, kw, at, at)
}

/// The same, measured at `at` and processed at `now`.
fn measure_at(e: &mut Engine, conns: &[Connection], kw: f64, at: Timestamp, now: Timestamp) -> Vec<Output> {
    let mut ms: Vec<Measurement> = conns
        .iter()
        .map(|c| Measurement {
            source: format!("smgw:{}", c.id),
            at,
            reading: Reading::Connection { connection: c.id.clone(), total: Kw(kw), steuve: Some(Kw(kw)) },
        })
        .collect();
    ms.push(Measurement { source: "rtu:T1".into(), at, reading: Reading::Asset { asset: "T1".into(), total: Kw(kw * conns.len() as f64) } });
    let mut out = e.step(Input::Measurements(ms), now);
    let ps = conns.iter().map(|c| Profile { connection: c.id.clone(), total: Kw(11.0), sigma: Kw(1.0), steuve: Kw(11.0) }).collect();
    out.extend(e.step(Input::Profiles(ps), now));
    out.extend(e.step(Input::Tick, now));
    out
}

fn commands(out: &[Output]) -> Vec<&fms::command::Command> {
    out.iter().filter_map(|o| if let Output::Command(c) = o { Some(c) } else { None }).collect()
}

#[test]
fn a_congestion_is_shared_equally_and_never_below_the_floor() {
    let (mut e, conns) = setup(12, RuleSet { max_switch_share: 1.0, ..RuleSet::bk6_22_300() });
    // 12 × 11 kW = 132 kW on a 100 kVA transformer.
    let out = measure(&mut e, &conns, 11.0, t(0));
    let cmds = commands(&out);
    assert_eq!(cmds.len(), 12, "every SteuVE behind the asset takes part");
    let first = cmds[0].limit;
    assert!(cmds.iter().all(|c| c.limit == first), "one depth for all: {:?}", cmds.iter().map(|c| c.limit).collect::<Vec<_>>());
    assert!(cmds.iter().all(|c| c.limit >= c.floor && c.floor == Kw(4.2)));
    // Target: 0,9 · 100 · 0,95 = 85,5 kW → 85,5 / 12 ≈ 7,1 kW each.
    assert!((first.0 * 12.0 - 85.5).abs() < 1.0, "limit {first}");
    let r = &cmds[0].reason;
    assert_eq!(r.cause, Cause::Relieve);
    assert_eq!(r.asset, "T1");
    assert!(r.inputs.iter().any(|i| i.source == "rtu:T1"), "the reason names the measurements");
}

#[test]
fn a_congestion_the_floors_cannot_hold_raises_a_shortfall() {
    let (mut e, conns) = setup(30, RuleSet { max_switch_share: 1.0, ..RuleSet::bk6_22_300() });
    // 30 × 11 = 330 kW; even at 4,2 kW each it is 126 kW — 26 kW above the 100 kVA rating.
    let out = measure(&mut e, &conns, 11.0, t(0));
    assert!(commands(&out).iter().all(|c| c.limit == Kw(4.2)));
    assert!(out.iter().any(|o| matches!(o, Output::Alarm(Alarm::Shortfall { asset, missing }) if asset == "T1" && (missing.0 - 26.0).abs() < 1.0)));
}

#[test]
fn the_return_is_stepwise_then_a_release() {
    let (mut e, conns) = setup(12, RuleSet { max_switch_share: 1.0, ..RuleSet::bk6_22_300() });
    let first = commands(&measure(&mut e, &conns, 11.0, t(0)))[0].limit;
    // Demand falls away: the households now draw 3 kW and the profiles say so.
    let mut limits = vec![first];
    let mut released = false;
    for k in 1..=24 {
        let at = t(5 * k);
        let mut ms: Vec<Measurement> = conns
            .iter()
            .map(|c| Measurement {
                source: format!("smgw:{}", c.id),
                at,
                reading: Reading::Connection { connection: c.id.clone(), total: Kw(3.0), steuve: Some(Kw(3.0)) },
            })
            .collect();
        ms.push(Measurement { source: "rtu:T1".into(), at, reading: Reading::Asset { asset: "T1".into(), total: Kw(36.0) } });
        e.step(Input::Measurements(ms), at);
        e.step(Input::Profiles(conns.iter().map(|c| Profile { connection: c.id.clone(), total: Kw(3.0), sigma: Kw(0.5), steuve: Kw(3.0) }).collect()), at);
        let out = e.step(Input::Tick, at);
        if let Some(c) = commands(&out).into_iter().find(|c| c.attempt == 1) {
            assert_eq!(c.reason.cause, Cause::Restore);
            limits.push(c.limit);
        }
        if out.iter().any(|o| matches!(o, Output::Release(_))) {
            released = true;
            break;
        }
    }
    assert!(limits.len() >= 2, "at least one restore step before the release: {limits:?}");
    assert!(limits.windows(2).all(|w| w[1] > w[0]), "each step restores more: {limits:?}");
    assert!(released, "eventually released");
}

#[test]
fn stale_data_stops_new_reductions_and_says_so() {
    let (mut e, conns) = setup(12, RuleSet::bk6_22_300());
    e.step(Input::Profiles(conns.iter().map(|c| Profile { connection: c.id.clone(), total: Kw(11.0), sigma: Kw(1.0), steuve: Kw(11.0) }).collect()), t(0));
    let out = e.step(Input::Tick, t(0));
    assert!(commands(&out).is_empty(), "no measurement, no netzorientierte reduction");
    assert!(out.iter().any(|o| matches!(o, Output::Alarm(Alarm::StaleData))));
}

#[test]
fn an_implausible_transformer_reading_is_refused_and_does_nothing() {
    let (mut e, conns) = setup(12, RuleSet::bk6_22_300());
    let at = t(0);
    let mut ms: Vec<Measurement> = conns
        .iter()
        .map(|c| Measurement {
            source: format!("smgw:{}", c.id),
            at,
            reading: Reading::Connection { connection: c.id.clone(), total: Kw(3.0), steuve: Some(Kw(3.0)) },
        })
        .collect();
    ms.push(Measurement { source: "rtu:T1".into(), at, reading: Reading::Asset { asset: "T1".into(), total: Kw(400.0) } });
    e.step(Input::Measurements(ms), at);
    e.step(Input::Profiles(conns.iter().map(|c| Profile { connection: c.id.clone(), total: Kw(3.0), sigma: Kw(0.5), steuve: Kw(3.0) }).collect()), at);
    let out = e.step(Input::Tick, at);
    assert!(out.iter().any(|o| matches!(o, Output::Rejected { source, .. } if source == "rtu:T1")));
    assert!(commands(&out).is_empty());
    // The same contradiction next cycle is no longer a glitch: someone should look.
    let at = t(5);
    let mut ms: Vec<Measurement> = conns
        .iter()
        .map(|c| Measurement {
            source: format!("smgw:{}", c.id),
            at,
            reading: Reading::Connection { connection: c.id.clone(), total: Kw(3.0), steuve: Some(Kw(3.0)) },
        })
        .collect();
    ms.push(Measurement { source: "rtu:T1".into(), at, reading: Reading::Asset { asset: "T1".into(), total: Kw(400.0) } });
    e.step(Input::Measurements(ms), at);
    let out = e.step(Input::Tick, at);
    assert!(out.iter().any(|o| matches!(o, Output::Alarm(Alarm::Inconsistent { source, cycles: 2 }) if source == "rtu:T1")));
}

#[test]
fn the_mass_switching_limit_defers_and_reports() {
    let (mut e, conns) = setup(12, RuleSet { max_switch_share: 0.25, ..RuleSet::bk6_22_300() });
    let out = measure(&mut e, &conns, 11.0, t(0));
    assert_eq!(commands(&out).len(), 3);
    assert!(out.iter().any(|o| matches!(o, Output::Alarm(Alarm::Deferred { count: 9 }))));
}

#[test]
fn acknowledgements_retries_and_unknown() {
    let (mut e, conns) = setup(12, RuleSet { max_switch_share: 1.0, ..RuleSet::bk6_22_300() });
    let cmds: Vec<_> = commands(&measure(&mut e, &conns, 11.0, t(0))).into_iter().cloned().collect();
    let (a, b) = (&cmds[0], &cmds[1]);
    e.step(Input::Ack { connection: a.connection.clone(), msg: a.msg }, t(0));
    assert_eq!(e.state_of(&a.connection), State::Confirmed);
    // A stale or foreign acknowledgement is never applied.
    let out = e.step(Input::Ack { connection: b.connection.clone(), msg: a.msg }, t(0));
    assert!(matches!(out[0], Output::IgnoredAck { .. }));
    assert_eq!(e.state_of(&b.connection), State::Commanded);
    // b is never acknowledged: retried, then unknown — and planned as unreduced.
    let mut retries = 0;
    for k in 1..=12 {
        let out = e.step(Input::Tick, t(k));
        retries += commands(&out).iter().filter(|c| c.connection == b.connection && c.attempt > 1).count();
    }
    assert_eq!(retries, 2);
    assert_eq!(e.state_of(&b.connection), State::Unknown);
    assert_eq!(e.effective_limit(&b.connection), None);
}

#[test]
fn a_lost_channel_plans_with_the_failsafe_until_it_lapses() {
    let (mut e, conns) = setup(12, RuleSet::bk6_22_300());
    let c = conns[0].id.clone();
    e.step(Input::ChannelLost(c.clone()), t(0));
    assert_eq!(e.state_of(&c), State::Failsafe);
    assert_eq!(e.effective_limit(&c), Some(Kw(4.2)));
    e.step(Input::Tick, t(125));
    assert_eq!(e.effective_limit(&c), None, "after its 2 h the EMS is no longer bound");
    e.step(Input::ChannelRestored(c.clone()), t(130));
    assert_eq!(e.state_of(&c), State::Idle);
}

#[test]
fn an_operator_limit_is_floored_and_attributed() {
    let (mut e, conns) = setup(2, RuleSet::bk6_22_300());
    let out = e.step(Input::Operator { author: "ops:alice".into(), connection: conns[0].id.clone(), limit: Some(Kw(1.0)), note: "test".into() }, t(0));
    let c = commands(&out)[0];
    assert_eq!(c.limit, Kw(4.2));
    assert!(c.at_floor);
    assert_eq!(c.reason.cause, Cause::Operator);
    assert_eq!(c.reason.author.as_deref(), Some("ops:alice"));
}

#[test]
fn a_rule_set_outside_its_validity_commands_nothing() {
    let (mut e, conns) = setup(12, RuleSet::bk6_22_300_praeventiv(date(2020, 1, 1)));
    let out = measure(&mut e, &conns, 11.0, t(0));
    assert!(commands(&out).is_empty());
    assert!(out.iter().any(|o| matches!(o, Output::Alarm(Alarm::RuleSetInactive { .. }))));
}

#[test]
fn old_and_future_measurements_are_not_used() {
    let (mut e, conns) = setup(12, RuleSet::bk6_22_300());
    // Ten minutes old under a five-minute rule set: as good as none.
    let out = measure_at(&mut e, &conns, 11.0, t(-10), t(0));
    assert!(commands(&out).is_empty(), "a stale congestion caused a command");
    assert!(out.iter().any(|o| matches!(o, Output::Alarm(Alarm::StaleData))));
    // The same values fresh do cause one — so it was the age that stopped it.
    assert!(!commands(&measure(&mut e, &conns, 11.0, t(1))).is_empty());
    // From the future: refused outright.
    let m =
        Measurement { source: "smgw:H00".into(), at: t(10), reading: Reading::Connection { connection: conns[0].id.clone(), total: Kw(3.0), steuve: None } };
    let out = e.step(Input::Measurements(vec![m]), t(0));
    assert!(matches!(&out[0], Output::Rejected { reason: fms::state::Rejection::FromTheFuture, .. }));
}

#[test]
fn unknown_connections_are_refused() {
    let (mut e, _) = setup(2, RuleSet::bk6_22_300());
    let m = Measurement { source: "smgw:X".into(), at: t(0), reading: Reading::Connection { connection: "X".into(), total: Kw(3.0), steuve: None } };
    let out = e.step(Input::Measurements(vec![m]), t(0));
    assert!(matches!(&out[0], Output::Rejected { reason: fms::state::Rejection::UnknownConnection, .. }));
}

#[test]
fn every_command_ends_by_itself() {
    let (mut e, conns) = setup(12, RuleSet { max_switch_share: 1.0, ..RuleSet::bk6_22_300() });
    let out = measure(&mut e, &conns, 11.0, t(0));
    let duration = RuleSet::bk6_22_300().command_duration;
    assert!(commands(&out).iter().all(|c| c.valid_until == t(0).saturating_add(duration).unwrap()));
}

#[test]
fn an_unknown_asset_is_refused_at_the_door() {
    let (mut e, _) = setup(2, RuleSet::bk6_22_300());
    let m = Measurement { source: "rtu:X".into(), at: t(0), reading: Reading::Asset { asset: "T9".into(), total: Kw(1.0) } };
    let out = e.step(Input::Measurements(vec![m]), t(0));
    assert!(matches!(&out[0], Output::Rejected { reason: fms::state::Rejection::UnknownAsset, .. }));
}

#[test]
fn an_undetermined_estimate_does_not_reduce_a_determined_one_does() {
    // No ONS measurement, one meter in twelve: the transformer's σ fails FNN 2e although the mean
    // is far above the trigger. With narrow profiles the same mean is determined and acted on.
    for (sigma, expect) in [(6.0, false), (0.3, true)] {
        let (mut e, conns) = setup(12, RuleSet { max_switch_share: 1.0, ..RuleSet::bk6_22_300() });
        let at = t(0);
        let m = Measurement {
            source: "smgw:H00".into(),
            at,
            reading: Reading::Connection { connection: conns[0].id.clone(), total: Kw(11.0), steuve: Some(Kw(11.0)) },
        };
        e.step(Input::Measurements(vec![m]), at);
        e.step(Input::Profiles(conns.iter().map(|c| Profile { connection: c.id.clone(), total: Kw(11.0), sigma: Kw(sigma), steuve: Kw(11.0) }).collect()), at);
        let out = e.step(Input::Tick, at);
        assert_eq!(!commands(&out).is_empty(), expect, "profile σ {sigma} kW");
    }
}

#[test]
fn an_expired_rule_set_lets_no_operator_command_through() {
    let (mut e, conns) = setup(2, RuleSet::bk6_22_300_praeventiv(date(2020, 1, 1)));
    let out = e.step(Input::Operator { author: "ops:alice".into(), connection: conns[0].id.clone(), limit: Some(Kw(5.0)), note: "test".into() }, t(0));
    assert!(commands(&out).is_empty());
    assert!(out.iter().any(|o| matches!(o, Output::Alarm(Alarm::RuleSetInactive { .. }))));
}

#[test]
fn a_command_during_a_lost_channel_is_recorded_as_held() {
    let (mut e, conns) = setup(12, RuleSet { max_switch_share: 1.0, ..RuleSet::bk6_22_300() });
    e.step(Input::ChannelLost(conns[0].id.clone()), t(0));
    let out = e.step(Input::Operator { author: "ops:alice".into(), connection: conns[0].id.clone(), limit: Some(Kw(5.0)), note: "test".into() }, t(0));
    assert!(out.iter().any(|o| matches!(o, Output::Transition(tr) if tr.why.contains("held"))));
}

/// A tiny deterministic generator for the property tests.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
    fn f(&mut self, max: f64) -> f64 {
        (self.next() % 10_000) as f64 / 10_000.0 * max
    }
}

fn random_inputs(seed: u64, conns: &[Connection]) -> Vec<(Input, Timestamp)> {
    let mut g = Lcg(seed);
    let mut v = Vec::new();
    for k in 0..300i64 {
        let now = t(k);
        let c = &conns[(g.next() as usize) % conns.len()];
        let input = match g.next() % 9 {
            0 | 1 => {
                let mut ms = Vec::new();
                for c in conns {
                    if g.next().is_multiple_of(3) {
                        continue;
                    }
                    let kw = g.f(60.0) - 5.0;
                    let age = (g.next() % 8) as i64;
                    let steuve = if g.next().is_multiple_of(2) { Some(Kw(g.f(kw.max(0.0)))) } else { None };
                    ms.push(Measurement {
                        source: format!("smgw:{}", c.id),
                        at: t(k - age),
                        reading: Reading::Connection { connection: c.id.clone(), total: Kw(kw), steuve },
                    });
                }
                ms.push(Measurement { source: "rtu:T1".into(), at: now, reading: Reading::Asset { asset: "T1".into(), total: Kw(g.f(400.0)) } });
                Input::Measurements(ms)
            }
            2 => {
                let mut ps = Vec::new();
                for c in conns {
                    ps.push(Profile { connection: c.id.clone(), total: Kw(g.f(20.0)), sigma: Kw(g.f(8.0)), steuve: Kw(g.f(15.0)) });
                }
                Input::Profiles(ps)
            }
            3 => Input::Ack { connection: c.id.clone(), msg: fms::command::MessageId(g.next() % 400) },
            4 => Input::ChannelLost(c.id.clone()),
            5 => Input::ChannelRestored(c.id.clone()),
            6 => {
                let limit = if !g.next().is_multiple_of(3) { Some(Kw(g.f(30.0) - 5.0)) } else { None };
                Input::Operator { author: "ops:fuzz".into(), connection: c.id.clone(), limit, note: String::new() }
            }
            7 => Input::Refused { connection: c.id.clone(), msg: fms::command::MessageId(g.next() % 400), why: "fuzz".into() },
            _ => Input::Tick,
        };
        v.push((input, now));
        v.push((Input::Tick, now));
    }
    v
}

#[test]
fn arbitrary_input_never_panics_and_never_goes_below_a_floor() {
    for seed in 0..40 {
        let (mut e, conns) = setup(12, RuleSet { max_switch_share: 1.0, ..RuleSet::bk6_22_300() });
        for (input, now) in random_inputs(seed, &conns) {
            for o in e.step(input, now) {
                if let Output::Command(c) = o {
                    assert!(c.limit >= c.floor && c.floor >= Kw(4.2), "seed {seed}: {c:?}");
                    assert!(c.valid_until > now);
                }
            }
        }
    }
}

#[test]
fn the_same_inputs_give_the_same_outputs() {
    // Sans-IO means replayable: two engines fed the same sequence agree record for record.
    for seed in 0..10 {
        let (mut a, conns) = setup(12, RuleSet::bk6_22_300());
        let (mut b, _) = setup(12, RuleSet::bk6_22_300());
        for (input, now) in random_inputs(seed, &conns) {
            assert_eq!(a.step(input.clone(), now), b.step(input, now), "seed {seed}");
        }
    }
}
