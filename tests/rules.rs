//! The § 14a rules as tests, one per clause they come from.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use fms::model::{Connection, ControlMode, Device, DeviceKind, SrId};
use fms::rules::RuleSet;
use fms::units::Kw;
use jiff::SignedDuration;
use jiff::civil::{Date, date};

const ON: Date = date(2026, 1, 15);

fn dev(id: &str, kind: DeviceKind, kw: f64) -> Device {
    Device { id: id.into(), kind, nominal: Kw(kw), commissioned: date(2025, 1, 1), opted_in: false, exempt: false }
}

fn conn(mode: ControlMode, devices: Vec<Device>) -> Connection {
    Connection {
        id: "H1".into(),
        resource: None,
        node: "N1".into(),
        mode,
        area: "NB-1".into(),
        postcode: "34117".into(),
        devices,
        relay_only: false,
        failsafe: None,
        failsafe_for: SignedDuration::from_hours(2),
        fuse: Kw(44.0),
    }
}

fn floor(c: &Connection) -> f64 {
    RuleSet::bk6_22_300().floor(c, ON).map(|f| f.total.round_w().0).unwrap_or(-1.0)
}

#[test]
fn steuve_threshold_is_strictly_above_4_2_kw() {
    // Ziff. 2.4.1: Netzanschlussleistung > 4,2 kW.
    assert!(conn(ControlMode::Ems, vec![dev("hp", DeviceKind::HeatPump, 4.2)]).steuve(ON).is_empty());
    assert_eq!(conn(ControlMode::Ems, vec![dev("hp", DeviceKind::HeatPump, 4.3)]).steuve(ON).len(), 1);
}

#[test]
fn heat_pumps_and_cooling_are_grouped_per_connection() {
    // Ziff. 2.4.2: several heat pumps (or cooling units) count as one SteuVE if their sum is above 4,2 kW.
    let c = conn(ControlMode::Ems, vec![dev("hp1", DeviceKind::HeatPump, 3.0), dev("hp2", DeviceKind::HeatPump, 3.0), dev("k1", DeviceKind::Cooling, 2.0)]);
    let s = c.steuve(ON);
    assert_eq!(s.len(), 1, "two heat pumps are one SteuVE; 2 kW of cooling is none");
    assert_eq!(s[0].id, "hp1+hp2");
    assert_eq!(s[0].nominal, Kw(6.0));
}

#[test]
fn only_new_or_opted_in_and_not_exempt_devices_take_part() {
    // Ziff. 3.1.b: commissioned after 31.12.2023; older ones by choice; aa/bb exemptions.
    let mut old = dev("wb", DeviceKind::ChargingPoint, 11.0);
    old.commissioned = date(2023, 12, 31);
    assert!(conn(ControlMode::Ems, vec![old.clone()]).steuve(ON).is_empty());
    old.opted_in = true;
    assert_eq!(conn(ControlMode::Ems, vec![old]).steuve(ON).len(), 1);
    let mut exempt = dev("hp", DeviceKind::HeatPump, 9.0);
    exempt.exempt = true;
    assert!(conn(ControlMode::Ems, vec![exempt]).steuve(ON).is_empty());
}

#[test]
fn direct_floor_is_4_2_kw_or_40_percent_above_11_kw() {
    // Ziff. 4.5.1.
    assert_eq!(floor(&conn(ControlMode::Direct, vec![dev("wb", DeviceKind::ChargingPoint, 11.0)])), 4.2);
    assert_eq!(floor(&conn(ControlMode::Direct, vec![dev("hp", DeviceKind::HeatPump, 15.0)])), 6.0);
    assert_eq!(floor(&conn(ControlMode::Direct, vec![dev("hp", DeviceKind::HeatPump, 15.0), dev("wb", DeviceKind::ChargingPoint, 11.0)])), 10.2);
}

#[test]
fn ems_floor_follows_the_gzf_table() {
    // Ziff. 4.5.2: P_min = 4,2 + (n − 1) · GZF(n) · 4,2.
    let gzf = [(1, 1.0), (2, 0.8), (3, 0.75), (4, 0.7), (5, 0.65), (6, 0.6), (7, 0.55), (8, 0.5), (9, 0.45), (10, 0.45)];
    for (n, g) in gzf {
        let devices = (0..n).map(|i| dev(&format!("wb{i}"), DeviceKind::ChargingPoint, 11.0)).collect();
        let expected = ((4.2 + f64::from(n - 1) * g * 4.2) * 1000.0).round() / 1000.0;
        assert_eq!(floor(&conn(ControlMode::Ems, devices)), expected, "n = {n}");
    }
}

#[test]
fn ems_floor_with_a_large_heat_pump() {
    // Ziff. 4.5.2 Satz 3: max(0,4·ΣP_WP; 0,4·ΣP_Klima) + (n − 1)·GZF·4,2.
    let c = conn(ControlMode::Ems, vec![dev("hp", DeviceKind::HeatPump, 15.0), dev("wb", DeviceKind::ChargingPoint, 11.0)]);
    assert_eq!(floor(&c), 6.0 + 0.8 * 4.2);
    let c = conn(ControlMode::Ems, vec![dev("hp", DeviceKind::HeatPump, 9.0), dev("wb", DeviceKind::ChargingPoint, 11.0)]);
    assert_eq!(floor(&c), 7.56);
}

#[test]
fn no_limit_below_the_floor_for_any_request() {
    let rules = RuleSet::bk6_22_300();
    let mut x: u64 = 7;
    for _ in 0..2000 {
        x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let n = 1 + (x >> 60) as usize % 5;
        let kinds = [DeviceKind::ChargingPoint, DeviceKind::HeatPump, DeviceKind::Cooling, DeviceKind::Storage];
        let devices = (0..n).map(|i| dev(&format!("d{i}"), kinds[(x >> (i * 3)) as usize % 4], 1.0 + ((x >> (i * 5 + 8)) % 200) as f64 / 10.0)).collect();
        let mode = if x & 1 == 0 { ControlMode::Ems } else { ControlMode::Direct };
        let mut c = conn(mode, devices);
        c.relay_only = x & 2 == 0;
        let requested = Kw(((x >> 20) % 300) as f64 / 10.0 - 5.0);
        if let (Some(f), Some(l)) = (rules.floor(&c, ON), rules.limit(&c, requested, ON)) {
            assert!(l.value() >= f.total.round_w(), "{l:?} below {f:?}");
            assert!(f.total <= c.steuve_nominal(ON) + Kw(1e-9), "a floor above the installed power: {f:?}");
        }
    }
}

#[test]
fn a_relay_has_one_reduced_state_its_floor() {
    let mut c = conn(ControlMode::Ems, vec![dev("hp", DeviceKind::HeatPump, 9.0), dev("wb", DeviceKind::ChargingPoint, 11.0)]);
    c.relay_only = true;
    let l = RuleSet::bk6_22_300().limit(&c, Kw(15.0), ON).unwrap();
    assert_eq!(l.value(), Kw(7.56));
}

#[test]
fn praeventiv_lasts_24_months_and_never_past_2028() {
    // Ziff. 10.5 a: at most 24 months from first use, and not after 31.12.2028.
    let r = RuleSet::bk6_22_300_praeventiv(date(2026, 3, 1));
    assert!(r.valid_on(date(2028, 2, 29)));
    assert!(!r.valid_on(date(2028, 3, 1)));
    let late = RuleSet::bk6_22_300_praeventiv(date(2028, 1, 1));
    assert!(late.valid_on(date(2028, 12, 31)));
    assert!(!late.valid_on(date(2029, 1, 1)));
    assert_eq!(r.daily_budget, Some(SignedDuration::from_hours(2)));
}

#[test]
fn sr_id_check_digit_matches_the_bdew_example() {
    // BDEW Bildungsvorschrift Ressourcen-ID: A113735592 → 5.
    assert_eq!(SrId::check_digit("A113735592"), Some(5));
    let body = "C113735592";
    let d = SrId::check_digit(body).unwrap();
    assert!(format!("{body}{d}").parse::<SrId>().is_ok());
    assert!(format!("{body}{}", (d + 1) % 10).parse::<SrId>().is_err());
    assert!("A1137355925".parse::<SrId>().is_err(), "an SR-ID starts with C");
}

#[test]
fn an_unlawful_rule_set_is_refused() {
    assert!(RuleSet::bk6_22_300().validate().is_ok());
    assert!(RuleSet::bk6_22_300_praeventiv(date(2026, 1, 1)).validate().is_ok());
    let low = RuleSet { min_per_device: Kw(0.0), ..RuleSet::bk6_22_300() };
    assert!(low.validate().unwrap_err().iter().any(|e| e.contains("4.5.1")));
    let gzf = RuleSet { gzf: vec![(1, 1.0), (2, 0.1)], ..RuleSet::bk6_22_300() };
    assert!(gzf.validate().is_err());
    let early = RuleSet { trigger: 0.5, ..RuleSet::bk6_22_300() };
    assert!(early.validate().is_err());
    let long = RuleSet { daily_budget: Some(SignedDuration::from_hours(3)), ..RuleSet::bk6_22_300_praeventiv(date(2026, 1, 1)) };
    assert!(long.validate().unwrap_err().iter().any(|e| e.contains("10.5 c")));
    let late = RuleSet { valid_until: Some(date(2029, 6, 30)), ..RuleSet::bk6_22_300_praeventiv(date(2028, 1, 1)) };
    assert!(late.validate().is_err());
    let blind = RuleSet { require_determined: false, ..RuleSet::bk6_22_300() };
    assert!(blind.validate().is_err(), "netzorientiert without a determined state");
}
