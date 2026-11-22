//! The closed-loop simulator and the reference days.
//!
//! A hidden "true" feeder produces the measurements the FMS is allowed to see — a metered
//! subset with noise, an RTU at the Ortsnetzstation where there is one — and profiles for the
//! rest. Simulated Steuerboxen apply the FMS's commands, acknowledge them, lose some, and fall
//! back to their failsafe on a lost channel. Behind each one a household guard applies the limit
//! the way an EMS such as hems does, and counts every limit below its own computed floor. Each
//! day runs twice — with the FMS and without — on the same seed.

// The simulator walks parallel vectors built together from one feeder (households, boxes,
// draws); every index comes from enumerating one of them. It never decodes untrusted input, so
// the indexing lint the protocol core lives under is relaxed here and only here.
#![allow(clippy::indexing_slicing)]

mod feeder;

pub use feeder::{Feeder, Household, Shape};

use crate::command::{Cause, MessageId};
use crate::engine::{Alarm, Engine, Input, Output, SetupError};
use crate::evidence::{Log, PublicationRow, publication};
use crate::grid::{AssetRef, Topology};
use crate::model::{Connection, ConnectionId, DeviceKind};
use crate::rules::{RuleSet, Selection};
use crate::state::{Measurement, MeteringSetup, Profile, Reading, required_metering_share};
use crate::time::berlin;
use crate::units::Kw;
use jiff::civil::{Date, date};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A small deterministic generator (SplitMix64) — the simulator needs no global state.
#[derive(Debug, Clone)]
pub struct Prng(u64);

impl Prng {
    /// Seeded.
    #[must_use]
    pub fn new(seed: u64) -> Prng {
        Prng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    /// Next 64 bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in [−1, 1).
    pub fn sym(&mut self) -> f64 {
        self.uniform() * 2.0 - 1.0
    }
}

/// A reference day.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Day {
    /// Command-line name.
    pub name: &'static str,
    /// One line.
    pub title: &'static str,
    /// What it shows.
    pub story: &'static str,
    /// The date (sets weekday, sun and the rule set's validity).
    pub date: Date,
    /// Outdoor temperature, °C.
    pub temperature: f64,
    /// The feeder.
    pub shape: Shape,
    /// Share of households with a smart meter gateway sending TAF 10 values.
    pub metered: f64,
    /// What the Ortsnetzstation measures.
    pub ons: MeteringSetup,
    /// Run under the präventiv rule set (planning data only).
    pub praeventiv: bool,
    /// A branch whose CLS channels are down between two hours of the day.
    pub outage: Option<(usize, f64, f64)>,
    /// The transformer RTU is spoofed in the evening and one meter lies.
    pub lying: bool,
    /// Share of commands and acknowledgements lost on the way.
    pub loss: f64,
}

const WINTER: Shape =
    Shape { transformer_kva: 400.0, branches: [8, 8, 7, 9], per_node: 3, heat_pumps: 0.36, wallboxes: 0.28, wallboxes_last_branch: 0.28, pv: 0.3 };

/// Every reference day.
#[must_use]
pub fn days() -> Vec<Day> {
    vec![
        Day {
            name: "cold-snap",
            title: "January cold snap: heat pumps overload the transformer",
            story: "−12 °C. From late afternoon the heat pumps and their heating elements run flat out and the 400 kVA transformer goes above its rating. 30 % of the households have a smart meter gateway and the Ortsnetzstation measures the transformer.",
            date: date(2026, 1, 15),
            temperature: -12.0,
            shape: WINTER,
            metered: 0.3,
            ons: MeteringSetup::Transformer,
            praeventiv: false,
            outage: None,
            lying: false,
            loss: 0.02,
        },
        Day {
            name: "wallbox-evening",
            title: "October evening: a new estate plugs in its cars",
            story: "8 °C. On the last branch four households in five have a wallbox, and between 17:00 and 20:00 most of them start charging at 11 kW. The cable heats up and the voltage at the far end sags.",
            date: date(2026, 10, 14),
            temperature: 8.0,
            shape: Shape { heat_pumps: 0.3, wallboxes: 0.2, wallboxes_last_branch: 0.85, ..WINTER },
            metered: 0.3,
            ons: MeteringSetup::Feeders,
            praeventiv: false,
            outage: None,
            lying: false,
            loss: 0.02,
        },
        Day {
            name: "pv-noon",
            title: "Midsummer noon: plenty of PV, no reason to act",
            story: "24 °C and clear sky. Storage charges from the roof and the feeder runs in feed-in. Nothing in § 14a applies; the right number of commands is zero.",
            date: date(2026, 6, 21),
            temperature: 24.0,
            shape: Shape { heat_pumps: 0.1, wallboxes: 0.12, wallboxes_last_branch: 0.12, pv: 0.7, ..WINTER },
            metered: 0.3,
            ons: MeteringSetup::Transformer,
            praeventiv: false,
            outage: None,
            lying: false,
            loss: 0.02,
        },
        Day {
            name: "channel-outage",
            title: "Cold snap with a lost CLS channel on branch 2",
            story: "The cold snap again, but the gateways on branch 2 lose their channel from 17:30 to 19:00. Their Steuerboxen fall back to the failsafe value and the FMS plans with it instead of with what it commanded.",
            date: date(2026, 1, 15),
            temperature: -12.0,
            shape: WINTER,
            metered: 0.3,
            ons: MeteringSetup::Transformer,
            praeventiv: false,
            outage: Some((1, 17.5, 19.0)),
            lying: false,
            loss: 0.02,
        },
        Day {
            name: "lying-sensor",
            title: "A mild evening and a sensor that lies",
            story: "6 °C, no real congestion. From 17:00 to 19:00 the transformer reading is spoofed 80 % high and one meter reports 120 kW. The FMS refuses both as bad data and sends nothing.",
            date: date(2026, 2, 10),
            temperature: 6.0,
            shape: Shape { wallboxes: 0.1, wallboxes_last_branch: 0.1, ..WINTER },
            metered: 0.3,
            ons: MeteringSetup::Transformer,
            praeventiv: false,
            outage: None,
            lying: true,
            loss: 0.02,
        },
        Day {
            name: "beyond-14a",
            title: "Deep frost on a dense estate: more than § 14a can give",
            story: "−16 °C on a feeder where six households in ten heat electrically. Even with every SteuVE held at its Mindestleistung the transformer stays above its rating. The FMS goes as deep as the law allows, raises a shortfall with the kilowatts missing, and the publication proposes Netzausbau.",
            date: date(2026, 1, 21),
            temperature: -16.0,
            shape: Shape { heat_pumps: 0.6, wallboxes: 0.35, ..WINTER },
            metered: 0.3,
            ons: MeteringSetup::Transformer,
            praeventiv: false,
            outage: None,
            lying: false,
            loss: 0.02,
        },
        Day {
            name: "blind-feeder",
            title: "Cold snap on a feeder without measurement: präventive Steuerung",
            story: "The cold snap on a feeder with no gateway and no RTU. Netzorientierte Steuerung is impossible here, so the DSO runs the transitional präventive Steuerung on planning data, at most two hours per household and day. It takes the edge off the evening, and the report shows its price: when the budget runs out the deferred heat comes back — the reason the regime ends in 2028.",
            date: date(2026, 1, 15),
            temperature: -12.0,
            shape: WINTER,
            metered: 0.0,
            ons: MeteringSetup::None,
            praeventiv: true,
            outage: None,
            lying: false,
            loss: 0.02,
        },
    ]
}

/// A reference day by name.
#[must_use]
pub fn day(name: &str) -> Option<Day> {
    days().into_iter().find(|d| d.name == name)
}

/// How the physical feeder fared over the day.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    /// Minutes with any asset above its rating or any node below the voltage limit.
    pub congested_minutes: f64,
    /// Highest loading of any asset, per cent of rating.
    pub peak_loading_percent: f64,
    /// Which asset.
    pub peak_asset: String,
    /// Lowest voltage, p.u.
    pub min_voltage: f64,
    /// EV energy still owed at midnight, kWh.
    pub ev_unserved_kwh: f64,
}

/// One point of the day's chart (every 15 minutes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Point {
    /// Local time `HH:MM`.
    pub time: String,
    /// Transformer loading without the FMS, per cent.
    pub without: f64,
    /// With the FMS, per cent.
    pub with: f64,
    /// What the FMS believed, per cent (None while it could not determine the state).
    pub believed: Option<f64>,
    /// Households with a reduction in effect.
    pub reduced: usize,
}

/// The day's report.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    /// The day.
    pub day: String,
    /// Its title.
    pub title: String,
    /// Its story.
    pub story: String,
    /// Date.
    pub date: Date,
    /// Seed.
    pub seed: u64,
    /// Households.
    pub households: usize,
    /// SteuVE behind them.
    pub steuve: usize,
    /// Share with a gateway.
    pub metered_share: f64,
    /// What the Ortsnetzstation measures.
    pub ons: MeteringSetup,
    /// FNN Hinweis 2e: the gateway share a radial feeder with this measurement needs.
    pub required_share: f64,
    /// Rule set.
    pub rule_set: String,
    /// Without the FMS.
    pub without: Outcome,
    /// With the FMS.
    pub with: Outcome,
    /// Commands by cause.
    pub commands: BTreeMap<String, usize>,
    /// Releases.
    pub releases: usize,
    /// Retries.
    pub retries: usize,
    /// Measurements refused.
    pub rejected: usize,
    /// Alarms by kind.
    pub alarms: BTreeMap<String, usize>,
    /// Commands below the floor, checked independently from the log. Must be zero.
    pub floor_violations: usize,
    /// Limits below the household guard's own floor. Must be zero.
    pub below_minimum: usize,
    /// SteuVE energy curtailed, kWh.
    pub curtailed_kwh: f64,
    /// Most any one household lost, kWh.
    pub worst_household_kwh: f64,
    /// Households reduced at least once.
    pub households_reduced: usize,
    /// Mean absolute error of the believed transformer loading, kW.
    pub estimator_mae_kw: f64,
    /// The publication row this day alone would produce.
    pub publication: Vec<PublicationRow>,
    /// The same three figures from what the simulated Steuerboxen actually applied — the ground
    /// truth the publication is checked against.
    pub truth: TruthRow,
    /// Share of cycles in which the true transformer loading lay within the FMS's 95 % interval.
    pub coverage_95: f64,
    /// Records in the evidence log.
    pub records: usize,
    /// Its head hash.
    pub head: String,
    /// The chart.
    pub series: Vec<Point>,
}

/// Hours, peak SteuVE and intensity of the day, from the reductions the Steuerboxen applied.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TruthRow {
    /// Hours with any reduction in effect.
    pub hours: f64,
    /// Peak number of SteuVE reduced at once.
    pub steuve: usize,
    /// Eingriffsintensität over the month, per cent.
    pub intensity_percent: f64,
}

/// A finished run: the report, the evidence log and the master data it refers to.
#[derive(Debug, Clone)]
pub struct Run {
    /// The report.
    pub report: Report,
    /// The evidence log.
    pub log: Log,
    /// The connections (for publication and documentation).
    pub connections: Vec<Connection>,
}

const STEP_MIN: i64 = 5;
const STEPS: usize = 24 * 60 / STEP_MIN as usize;

fn bump(h: f64, centre: f64, width: f64) -> f64 {
    (-((h - centre) / width).powi(2)).exp()
}

fn base_kw(h: f64) -> f64 {
    0.25 + 0.55 * bump(h, 7.5, 1.5) + 0.25 * bump(h, 12.5, 2.0) + 0.9 * bump(h, 19.0, 2.5)
}

/// Electrical draw of a heat pump as a share of its registered Netzanschlussleistung (which
/// includes the Heizstab): about a third at −12 °C through the day, three quarters at the
/// evening peak when the Heizstab joins in.
fn heat_share(t: f64, h: f64) -> f64 {
    let need = ((15.0 - t) / 27.0).clamp(0.0, 1.0);
    (need * (0.33 + 0.15 * bump(h, 7.0, 1.5) + 0.4 * bump(h, 18.5, 2.0)) + 0.03).min(1.0)
}

fn sun(day: Date, h: f64) -> f64 {
    let doy = f64::from(day.day_of_year());
    let season = 0.5 - 0.5 * ((doy + 10.0) / 365.0 * std::f64::consts::TAU).cos();
    let (rise, set) = (8.0 - 3.0 * season, 16.5 + 4.5 * season);
    if h <= rise || h >= set {
        return 0.0;
    }
    (0.25 + 0.55 * season) * (std::f64::consts::PI * (h - rise) / (set - rise)).sin()
}

#[derive(Clone)]
struct Box_ {
    limit: Option<(Kw, Timestamp, MessageId)>,
    channel_up: bool,
    failsafe_since: Option<Timestamp>,
}

struct Physics {
    ev_left: Vec<f64>,
    hp_debt: Vec<f64>,
}

#[derive(Default)]
struct Draw {
    net: f64,
    steuve_demand: f64,
    steuve_actual: f64,
}

/// Run a reference day.
///
/// # Errors
/// If the generated feeder does not fit together (a bug).
pub fn run(day: &Day, seed: u64) -> Result<Run, SetupError> {
    run_with(day, seed, Selection::Netzbereich)
}

/// Run a reference day with a choice of who is drawn on (Ziff. 4.3).
///
/// # Errors
/// If the generated feeder does not fit together (a bug).
pub fn run_with(day: &Day, seed: u64, selection: Selection) -> Result<Run, SetupError> {
    let feeder = Feeder::generate(seed, day.shape, day.metered);
    let base = simulate(day, &feeder, seed, false, selection)?;
    let mut with = simulate(day, &feeder, seed, true, selection)?;
    let mut report = with.report.take().unwrap_or_default();
    report.without = base.outcome;
    report.series = with.series.iter().zip(base.series.iter()).map(|(w, b)| Point { without: b.without, ..w.clone() }).collect();
    Ok(Run { report, log: with.log, connections: feeder.connections })
}

struct Sim {
    outcome: Outcome,
    series: Vec<Point>,
    log: Log,
    report: Option<Report>,
}

#[allow(clippy::too_many_lines)]
fn simulate(day: &Day, feeder: &Feeder, seed: u64, fms: bool, selection: Selection) -> Result<Sim, SetupError> {
    let mut rules = if day.praeventiv { RuleSet::bk6_22_300_praeventiv(date(2026, 1, 1)) } else { RuleSet::bk6_22_300() };
    rules.selection = selection;
    let topo = Topology::new(feeder.grid.clone()).map_err(|_| SetupError::UnknownNode(ConnectionId::from("?"), "?".into()))?;
    let mut engine = Engine::new(topo.clone(), feeder.connections.clone(), rules.clone())?;
    let conns: BTreeMap<ConnectionId, Connection> = feeder.connections.iter().map(|c| (c.id.clone(), c.clone())).collect();
    let n = feeder.households.len();
    let start = day.date.to_zoned(berlin()).map(|z| z.timestamp()).unwrap_or(Timestamp::UNIX_EPOCH);
    let step = SignedDuration::from_mins(STEP_MIN);
    let dt_h = STEP_MIN as f64 / 60.0;

    let mut rng = Prng::new(seed.wrapping_mul(31).wrapping_add(7));
    let mut boxes = vec![Box_ { limit: None, channel_up: true, failsafe_since: None }; n];
    let mut phys = Physics { ev_left: feeder.households.iter().map(|h| h.ev_need).collect(), hp_debt: vec![0.0; n] };
    let mut log = Log::new();
    let mut acks: Vec<(ConnectionId, MessageId)> = Vec::new();
    // Warm start: the minute before midnight looks like the forecast, so the first cycle has data.
    let mut last_net: Vec<f64> = feeder
        .households
        .iter()
        .zip(&feeder.connections)
        .map(|(hh, c)| {
            let (hp, ev, bat, pv) = expected(day, hh, c, 0.0);
            base_kw(0.0) * hh.base_scale + hp + ev + bat - pv
        })
        .collect();
    let warm = {
        let mut nl: BTreeMap<crate::model::NodeId, f64> = BTreeMap::new();
        for (c, v) in feeder.connections.iter().zip(&last_net) {
            *nl.entry(c.node.clone()).or_default() += v;
        }
        topo.flows(&nl)
    };
    let mut last_trafo = warm.transformer;
    let mut last_lines: Vec<f64> = warm.line;
    let mut outcome = Outcome { min_voltage: 1.0, ..Outcome::default() };
    let mut series = Vec::new();
    let mut curtailed = vec![0.0; n];
    let mut below_minimum = 0;
    let mut est_err = (0.0, 0usize);
    let mut covered = (0usize, 0usize);
    let mut truth_row = TruthRow::default();
    let mut truth_cut_days = 0.0;
    let installed_area: f64 = feeder.connections.iter().map(|c| c.steuve_nominal(day.date).0).sum();
    let idx: BTreeMap<&ConnectionId, usize> = feeder.households.iter().enumerate().map(|(i, h)| (&h.id, i)).collect();

    for s in 0..STEPS {
        let now = start.saturating_add(step * i32::try_from(s).unwrap_or(0)).unwrap_or(start);
        let h = s as f64 * dt_h;

        // 1. Channels: the GWA notices a lost channel within the 120 s heartbeat timeout.
        let mut inputs: Vec<Input> = Vec::new();
        for (i, hh) in feeder.households.iter().enumerate() {
            let down = day.outage.is_some_and(|(b, from, to)| hh.branch == b && h >= from && h < to);
            if down && boxes[i].channel_up {
                boxes[i].channel_up = false;
                boxes[i].failsafe_since = Some(now);
                boxes[i].limit = None;
                inputs.push(Input::ChannelLost(hh.id.clone()));
            } else if !down && !boxes[i].channel_up {
                boxes[i].channel_up = true;
                boxes[i].failsafe_since = None;
                inputs.push(Input::ChannelRestored(hh.id.clone()));
            }
        }
        for (c, m) in acks.drain(..) {
            inputs.push(Input::Ack { connection: c, msg: m });
        }

        // 2. What the FMS may see: last step's values, a minute old.
        {
            let at = now.saturating_sub(SignedDuration::from_mins(1)).unwrap_or(now);
            let mut ms = Vec::new();
            let mut first_lie = day.lying && (17.0..19.0).contains(&h);
            for (i, hh) in feeder.households.iter().enumerate() {
                if hh.metered {
                    let mut v = last_net[i] * (1.0 + 0.01 * rng.sym());
                    if first_lie {
                        v = 120.0;
                        first_lie = false;
                    }
                    ms.push(Measurement {
                        source: format!("smgw:{}", hh.id),
                        at,
                        reading: Reading::Connection { connection: hh.id.clone(), total: Kw(v), steuve: None },
                    });
                }
            }
            let spoof = if day.lying && (17.0..19.0).contains(&h) { 1.8 } else { 1.0 };
            match day.ons {
                MeteringSetup::Transformer => {
                    let truth = last_trafo;
                    ms.push(Measurement {
                        source: "rtu:ONS/T1".into(),
                        at,
                        reading: Reading::Asset { asset: "T1".into(), total: Kw(truth * spoof * (1.0 + 0.005 * rng.sym())) },
                    });
                }
                MeteringSetup::Feeders => {
                    for b in 0..day.shape.branches.len() {
                        let line = format!("L{}-01", b + 1);
                        let idx = feeder.grid.lines.iter().position(|l| l.id == line);
                        let truth = idx.and_then(|i| last_lines.get(i)).copied().unwrap_or(0.0);
                        ms.push(Measurement {
                            source: format!("rtu:ONS/{line}"),
                            at,
                            reading: Reading::Asset { asset: line, total: Kw(truth * spoof * (1.0 + 0.005 * rng.sym())) },
                        });
                    }
                }
                MeteringSetup::None => {}
            }
            inputs.push(Input::Measurements(ms));
        }

        // 3. Planning data: what an SLP-style forecast would say about each household.
        let profiles: Vec<Profile> = feeder
            .households
            .iter()
            .zip(&feeder.connections)
            .map(|(hh, c)| {
                let (hp, ev, bat, pv) = expected(day, hh, c, h);
                let base = base_kw(h) * hh.base_scale;
                let total = base + hp + ev + bat - pv;
                // An honest spread: a wallbox is either off or at 11 kW, whatever the mean says.
                let ev_nom = nominal_of(c, DeviceKind::ChargingPoint);
                let q = if ev_nom > 0.0 { (ev / ev_nom).clamp(0.0, 1.0) } else { 0.0 };
                let sigma = ((0.3 * (base + hp)).powi(2) + (0.2 * pv).powi(2) + ev_nom * ev_nom * q * (1.0 - q) + 0.16).sqrt();
                Profile { connection: c.id.clone(), total: Kw(total), sigma: Kw(sigma), steuve: Kw(hp + ev + bat) }
            })
            .collect();
        inputs.push(Input::Profiles(profiles));
        inputs.push(Input::Tick);

        // 4. The FMS decides; the Steuerboxen receive.
        if fms {
            for input in inputs {
                let outs = engine.step(input, now);
                for o in &outs {
                    match o {
                        Output::Command(c) => {
                            let Some(&i) = idx.get(&c.connection) else { continue };
                            if boxes[i].channel_up && rng.uniform() >= day.loss {
                                boxes[i].limit = Some((c.limit, c.valid_until, c.msg));
                                if rng.uniform() >= day.loss {
                                    acks.push((c.connection.clone(), c.msg));
                                }
                            }
                        }
                        Output::Release(r) => {
                            if let Some(&i) = idx.get(&r.connection)
                                && boxes[i].channel_up
                            {
                                boxes[i].limit = None;
                            }
                        }
                        _ => {}
                    }
                }
                log.extend(now, outs);
            }
        }

        // 5. The physical step.
        let mut draws = Vec::with_capacity(n);
        for (i, (hh, c)) in feeder.households.iter().zip(&feeder.connections).enumerate() {
            let b = &mut boxes[i];
            if b.limit.is_some_and(|(_, until, _)| until <= now) {
                b.limit = None;
            }
            let limit = if b.channel_up {
                b.limit.map(|(l, _, _)| l)
            } else {
                b.failsafe_since.filter(|since| now.duration_since(*since) < c.failsafe_for).and(c.failsafe)
            };
            if let (Some(l), Some(f)) = (limit, guard_floor(c))
                && l.0 + 1e-6 < f
            {
                below_minimum += 1;
            }
            let d = household(day, hh, c, h, &mut phys, i, limit, &mut rng, dt_h);
            curtailed[i] += (d.steuve_demand - d.steuve_actual).max(0.0) * dt_h;
            draws.push(d);
        }

        // 6. The truth: the loss-corrected branch-flow model (within 0,1 % of an AC power flow).
        let mut node_loads: BTreeMap<crate::model::NodeId, f64> = BTreeMap::new();
        for (c, d) in feeder.connections.iter().zip(&draws) {
            *node_loads.entry(c.node.clone()).or_default() += d.net;
        }
        let truth = topo.flows(&node_loads);
        let mut over = false;
        for a in topo.assets() {
            let flow = match a {
                AssetRef::Transformer => truth.transformer,
                AssetRef::Line(i) => truth.line.get(i).copied().unwrap_or(0.0),
            };
            let pct = flow.abs() / topo.rating(a).0 * 100.0;
            if pct > outcome.peak_loading_percent {
                outcome.peak_loading_percent = pct;
                outcome.peak_asset = topo.asset_id(a).to_owned();
            }
            over |= flow > topo.rating(a).0;
        }
        for v in truth.voltage.values() {
            outcome.min_voltage = outcome.min_voltage.min(*v);
            over |= *v < rules.v_min.0;
        }
        if over {
            outcome.congested_minutes += STEP_MIN as f64;
        }
        let trafo = truth.transformer;
        let believed = engine.view().transformer.filter(|_| fms);
        // The FMS estimated the state its measurements describe: the previous step's truth.
        if let Some(b) = believed {
            est_err.0 += (b.mean - last_trafo).abs();
            est_err.1 += 1;
            covered.1 += 1;
            if (b.mean - last_trafo).abs() <= 1.96 * b.sd + 1e-9 {
                covered.0 += 1;
            }
        }
        // Ground truth of the publication: what the Steuerboxen applied, not what the log says.
        let mut units = 0usize;
        for (b, c) in boxes.iter().zip(&feeder.connections) {
            if let (true, Some((l, _, _))) = (b.channel_up, b.limit) {
                units += c.steuve(day.date).len();
                if installed_area > 0.0 {
                    truth_cut_days += (c.steuve_nominal(day.date).0 - l.0).max(0.0) / installed_area * dt_h / 24.0;
                }
            }
        }
        if units > 0 {
            truth_row.hours += dt_h;
        }
        truth_row.steuve = truth_row.steuve.max(units);
        if s % 3 == 0 {
            let r = topo.rating(AssetRef::Transformer).0;
            series.push(Point {
                time: format!("{:02}:{:02}", s * 5 / 60, s * 5 % 60),
                without: (trafo / r * 1000.0).round() / 10.0,
                with: (trafo / r * 1000.0).round() / 10.0,
                believed: believed.map(|b| (b.mean / r * 1000.0).round() / 10.0),
                reduced: boxes.iter().filter(|b| b.limit.is_some() || !b.channel_up).count(),
            });
        }
        last_net = draws.iter().map(|d| d.net).collect();
        last_trafo = truth.transformer;
        last_lines = truth.line;
    }
    outcome.ev_unserved_kwh = round1(phys.ev_left.iter().sum());
    outcome.congested_minutes = outcome.congested_minutes.round();
    outcome.peak_loading_percent = round1(outcome.peak_loading_percent);
    outcome.min_voltage = (outcome.min_voltage * 1000.0).round() / 1000.0;

    let report = fms.then(|| {
        let mut commands: BTreeMap<String, usize> = BTreeMap::new();
        let mut alarms: BTreeMap<String, usize> = BTreeMap::new();
        let (mut releases, mut retries, mut rejected, mut floor_violations) = (0, 0, 0, 0);
        let mut reduced = std::collections::BTreeSet::new();
        for r in log.records() {
            match &r.event {
                Output::Command(c) if c.attempt > 1 => retries += 1,
                Output::Command(c) => {
                    let cause = match c.reason.cause {
                        Cause::Relieve => "relieve",
                        Cause::Renew => "renew",
                        Cause::Plan => "plan",
                        Cause::Restore => "restore",
                        Cause::Operator => "operator",
                    };
                    *commands.entry(cause.into()).or_default() += 1;
                    reduced.insert(c.connection.clone());
                    // Independent check: recompute the floor from the master data.
                    if let Some(f) = conns.get(&c.connection).and_then(|x| rules.floor(x, day.date))
                        && c.limit.0 + 1e-6 < f.total.round_w().0
                    {
                        floor_violations += 1;
                    }
                }
                Output::Release(_) => releases += 1,
                Output::Rejected { .. } => rejected += 1,
                Output::Alarm(a) => *alarms.entry(alarm_name(a).into()).or_default() += 1,
                _ => {}
            }
        }
        let pubrows = publication(&log, &feeder.connections, day.date, &BTreeMap::new()).rows;
        Report {
            day: day.name.into(),
            title: day.title.into(),
            story: day.story.into(),
            date: day.date,
            seed,
            households: n,
            steuve: feeder.connections.iter().map(|c| c.steuve(day.date).len()).sum(),
            metered_share: day.metered,
            ons: day.ons,
            required_share: required_metering_share(day.ons),
            rule_set: rules.name.clone(),
            without: Outcome::default(),
            with: outcome.clone(),
            commands,
            releases,
            retries,
            rejected,
            alarms,
            floor_violations,
            below_minimum,
            curtailed_kwh: round1(curtailed.iter().sum()),
            worst_household_kwh: round1(curtailed.iter().copied().fold(0.0, f64::max)),
            households_reduced: reduced.len(),
            estimator_mae_kw: if est_err.1 > 0 { round1(est_err.0 / est_err.1 as f64) } else { 0.0 },
            publication: pubrows,
            truth: TruthRow {
                hours: (truth_row.hours * 100.0).round() / 100.0,
                steuve: truth_row.steuve,
                intensity_percent: (truth_cut_days / f64::from(day.date.days_in_month()) * 100.0 * 100.0).round() / 100.0,
            },
            coverage_95: if covered.1 > 0 { (covered.0 as f64 / covered.1 as f64 * 1000.0).round() / 1000.0 } else { 0.0 },
            records: log.records().len(),
            head: log.head().to_owned(),
            series: Vec::new(),
        }
    });
    Ok(Sim { outcome, series, log, report })
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

fn alarm_name(a: &Alarm) -> &'static str {
    match a {
        Alarm::Shortfall { .. } => "shortfall",
        Alarm::ShortfallCleared { .. } => "shortfall-cleared",
        Alarm::StaleData => "stale-data",
        Alarm::DataRestored => "data-restored",
        Alarm::Deferred { .. } => "deferred",
        Alarm::RuleSetInactive { .. } => "rule-set-inactive",
        Alarm::FeedIn { .. } => "feed-in",
        Alarm::Refused { .. } => "refused",
        Alarm::Inconsistent { .. } => "inconsistent",
    }
}

/// The household EMS's own idea of what it is owed — written here from Ziff. 4.5 independently of
/// `RuleSet::floor`, so that "never below the minimum" is not the engine agreeing with itself.
/// The simulator's devices are all commissioned in 2025, take part and are not exempt.
fn guard_floor(c: &Connection) -> Option<f64> {
    const GZF: [f64; 10] = [1.0, 1.0, 0.8, 0.75, 0.7, 0.65, 0.6, 0.55, 0.5, 0.45];
    let sum = |k: DeviceKind| -> f64 { c.devices.iter().filter(|d| d.kind == k).map(|d| d.nominal.0).sum() };
    let (hp, cool) = (sum(DeviceKind::HeatPump), sum(DeviceKind::Cooling));
    let mut units: Vec<(bool, f64)> = Vec::new();
    for t in [hp, cool] {
        if t > 4.2 {
            units.push((true, t));
        }
    }
    for d in c.devices.iter().filter(|d| matches!(d.kind, DeviceKind::ChargingPoint | DeviceKind::Storage) && d.nominal.0 > 4.2) {
        units.push((false, d.nominal.0));
    }
    if units.is_empty() {
        return None;
    }
    let raw = match c.mode {
        crate::model::ControlMode::Direct => units.iter().map(|(thermal, p)| if *thermal && *p > 11.0 { 0.4 * p } else { 4.2 }).sum(),
        crate::model::ControlMode::Ems => {
            let n = units.len();
            let base = if hp > 11.0 || cool > 11.0 { (0.4 * hp).max(0.4 * cool) } else { 4.2 };
            base + (n as f64 - 1.0) * GZF.get(n.min(9)).copied().unwrap_or(0.45) * 4.2
        }
    };
    Some((raw * 1000.0_f64).round() / 1000.0)
}

fn nominal_of(c: &Connection, kind: DeviceKind) -> f64 {
    c.devices.iter().filter(|d| d.kind == kind).map(|d| d.nominal.0).sum()
}

/// What a forecast expects of a household — no knowledge of its car.
fn expected(day: &Day, hh: &Household, c: &Connection, h: f64) -> (f64, f64, f64, f64) {
    let hp = nominal_of(c, DeviceKind::HeatPump) * heat_share(day.temperature, h);
    let ev = nominal_of(c, DeviceKind::ChargingPoint) * 0.4 * bump(h, 19.5, 2.8);
    let pv = hh.pv_kwp * sun(day.date, h);
    let bat = nominal_of(c, DeviceKind::Storage).min((pv - base_kw(h) * hh.base_scale).max(0.0));
    (hp, ev, bat, pv)
}

#[allow(clippy::too_many_arguments)]
fn household(day: &Day, hh: &Household, c: &Connection, h: f64, phys: &mut Physics, i: usize, limit: Option<Kw>, rng: &mut Prng, dt_h: f64) -> Draw {
    let base = base_kw(h) * hh.base_scale * (1.0 + 0.12 * rng.sym());
    let hp_nom = nominal_of(c, DeviceKind::HeatPump);
    let debt = phys.hp_debt.get(i).copied().unwrap_or(0.0);
    let mut hp = (hp_nom * heat_share(day.temperature, h) * (1.0 + 0.08 * rng.sym())).min(hp_nom);
    let catch_up = (debt / dt_h).min(hp_nom - hp).max(0.0);
    hp += catch_up;
    let ev_nom = nominal_of(c, DeviceKind::ChargingPoint);
    let left = phys.ev_left.get(i).copied().unwrap_or(0.0);
    let ev = if ev_nom > 0.0 && h >= hh.ev_arrival && left > 0.0 { ev_nom.min(left / dt_h) } else { 0.0 };
    let pv = hh.pv_kwp * sun(day.date, h) * (1.0 + 0.05 * rng.sym());
    let bat = nominal_of(c, DeviceKind::Storage).min((pv - base).max(0.0));
    let demand = hp + ev + bat;

    // The household guard: the limit binds the SteuVE together, shared in proportion.
    let actual = limit.map_or(demand, |l| demand.min(l.0.max(0.0)));
    let k = if demand > 0.0 { actual / demand } else { 1.0 };
    let (hp_a, ev_a) = (hp * k, ev * k);
    if let Some(d) = phys.hp_debt.get_mut(i) {
        // Half of the heat not delivered is made up later; the rest is comfort lost.
        *d = (*d - catch_up * dt_h + (hp - hp_a) * dt_h * 0.5).max(0.0);
    }
    if let Some(l) = phys.ev_left.get_mut(i) {
        *l = (*l - ev_a * dt_h).max(0.0);
    }
    Draw { net: base + actual - pv, steuve_demand: demand, steuve_actual: actual }
}
