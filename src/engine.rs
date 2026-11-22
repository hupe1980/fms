//! The decision core: `step(input, now) → outputs`.
//!
//! The engine owns no socket, thread, database or clock. Measurements, profiles,
//! acknowledgements and operator instructions go in; commands, releases, alarms and state
//! changes come out, and every one of them belongs in the evidence log.
//!
//! One control cycle runs per [`Input::Tick`]:
//!
//! 1. dispatch bookkeeping — retries, timeouts, expiry;
//! 2. the grid state ([`crate::state::estimate`]) and the congestions it is confident about;
//! 3. one controller per congested asset solves a depth `α`: every SteuVE it draws on may take
//!    `floor + (1 − α) · (installed − floor)`; relay-only resources switch whole;
//! 4. `α` deepens at once and returns stepwise, one slot at a time, with hysteresis;
//! 5. at most a rule-set share of a feeder's resources changes state per cycle.

use crate::command::{Cause, Command, InputRef, MessageId, Reason, Release};
use crate::dispatch::{Delivery, State, Tracker, Transition};
use crate::grid::{AssetRef, Topology};
use crate::model::{Connection, ConnectionId, ControlMode, NodeId};
use crate::rules::{Regime, RuleSet};
use crate::state::{self, Congestion, CongestionKind, Estimate, Measurement, Profile, Quality, Reading, Rejection, Snapshot};
use crate::time::local_date;
use crate::units::Kw;
use jiff::civil::Date;
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// What goes in.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// Meter and RTU values.
    Measurements(Vec<Measurement>),
    /// Forecasts of unconstrained demand.
    Profiles(Vec<Profile>),
    /// The Steuerbox acknowledged a command.
    Ack {
        /// Resource.
        connection: ConnectionId,
        /// The command.
        msg: MessageId,
    },
    /// The Steuerbox refused a command.
    Refused {
        /// Resource.
        connection: ConnectionId,
        /// The command.
        msg: MessageId,
        /// Its reason.
        why: String,
    },
    /// The GWA reports the CLS channel down.
    ChannelLost(ConnectionId),
    /// The channel is back.
    ChannelRestored(ConnectionId),
    /// An operator's instruction: a limit (floored like any other) or `None` to release.
    Operator {
        /// Who — a named credential.
        author: String,
        /// Resource.
        connection: ConnectionId,
        /// The requested limit.
        limit: Option<Kw>,
        /// Why.
        note: String,
    },
    /// Run a control cycle.
    Tick,
}

/// Something the operator must know.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "alarm", rename_all = "kebab-case")]
pub enum Alarm {
    /// Every candidate is at its floor and the asset is still congested.
    Shortfall {
        /// Asset.
        asset: String,
        /// Relief that could not be found.
        missing: Kw,
    },
    /// The congestion ended or relief became sufficient again.
    ShortfallCleared {
        /// Asset.
        asset: String,
    },
    /// No fresh measurement: the grid state cannot be determined, no new reductions.
    StaleData,
    /// Fresh data again.
    DataRestored,
    /// State changes held back by the mass-switching limit.
    Deferred {
        /// How many.
        count: usize,
    },
    /// The rule set does not authorise commands today.
    RuleSetInactive {
        /// Its name.
        rule_set: String,
    },
    /// A congestion the floor-respecting reductions could act on is in feed-in — out of § 14a scope.
    FeedIn {
        /// Asset.
        asset: String,
    },
    /// The Steuerbox refused a command.
    Refused {
        /// Resource.
        connection: ConnectionId,
        /// Its reason.
        why: String,
    },
    /// An Ortsnetzstation measurement keeps contradicting everything else: a faulty sensor, a
    /// spoofed value — or meters that under-report. Someone should look.
    Inconsistent {
        /// The measurement's source.
        source: String,
        /// Consecutive cycles it was refused.
        cycles: u32,
    },
}

/// What comes out. Every variant is an evidence record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Output {
    /// Send this limit.
    Command(Command),
    /// Send this release.
    Release(Release),
    /// Tell the operator.
    Alarm(Alarm),
    /// A measurement was not used.
    Rejected {
        /// Its source.
        source: String,
        /// Why.
        reason: Rejection,
    },
    /// A resource changed state.
    Transition(Transition),
    /// An acknowledgement was not applied.
    IgnoredAck {
        /// Resource.
        connection: ConnectionId,
        /// The message.
        msg: MessageId,
        /// Why.
        why: String,
    },
}

/// Why the engine could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupError {
    /// Two connections share an id.
    DuplicateConnection(ConnectionId),
    /// A connection hangs at a node the topology does not have.
    UnknownNode(ConnectionId, NodeId),
    /// The rule set breaks a legal bound.
    RuleSet(Vec<String>),
}

impl std::fmt::Display for SetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SetupError::DuplicateConnection(c) => write!(f, "connection {c} appears twice"),
            SetupError::UnknownNode(c, n) => write!(f, "connection {c} hangs at node {n}, which the topology does not have"),
            SetupError::RuleSet(e) => write!(f, "the rule set is not lawful: {}", e.join("; ")),
        }
    }
}

impl std::error::Error for SetupError {}

/// A summary of the last cycle, for displays.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CycleView {
    /// The transformer's estimated loading.
    pub transformer: Option<Estimate>,
    /// Congestions acted on.
    pub congestions: Vec<Congestion>,
    /// Resources with a reduction in effect or in flight.
    pub reduced: usize,
}

/// The engine for one radial feeder.
#[derive(Debug)]
pub struct Engine {
    topo: Topology,
    conns: BTreeMap<ConnectionId, Connection>,
    rules: RuleSet,
    meters: BTreeMap<ConnectionId, Measurement>,
    assets: BTreeMap<String, Measurement>,
    profiles: BTreeMap<ConnectionId, Profile>,
    tracker: Tracker,
    operator_held: BTreeSet<ConnectionId>,
    budget: BTreeMap<ConnectionId, (Date, SignedDuration)>,
    last_tick: Option<Timestamp>,
    next_msg: u64,
    stale: bool,
    inactive: bool,
    shortfall: BTreeSet<String>,
    /// Consecutive bad-data refusals per source.
    bad_data: BTreeMap<String, u32>,
    /// Per controller: its depth `α` and when it last stepped back.
    controllers: BTreeMap<String, (f64, Timestamp)>,
    voltage_node: Option<NodeId>,
    view: CycleView,
}

impl Engine {
    /// Build an engine over a checked topology, its connections and a rule set.
    ///
    /// # Errors
    /// [`SetupError`] if the rule set breaks a legal bound or the master data does not fit the topology.
    pub fn new(topo: Topology, connections: Vec<Connection>, rules: RuleSet) -> Result<Engine, SetupError> {
        rules.validate().map_err(SetupError::RuleSet)?;
        let mut conns = BTreeMap::new();
        let mut tracker = Tracker::default();
        for c in connections {
            if !topo.contains(&c.node) {
                return Err(SetupError::UnknownNode(c.id.clone(), c.node.clone()));
            }
            tracker.register(c.id.clone(), c.failsafe, c.failsafe_for);
            if let Some(dup) = conns.insert(c.id.clone(), c) {
                return Err(SetupError::DuplicateConnection(dup.id));
            }
        }
        Ok(Engine {
            topo,
            conns,
            rules,
            meters: BTreeMap::new(),
            assets: BTreeMap::new(),
            profiles: BTreeMap::new(),
            tracker,
            operator_held: BTreeSet::new(),
            budget: BTreeMap::new(),
            last_tick: None,
            next_msg: 1,
            stale: false,
            inactive: false,
            shortfall: BTreeSet::new(),
            bad_data: BTreeMap::new(),
            controllers: BTreeMap::new(),
            voltage_node: None,
            view: CycleView::default(),
        })
    }

    /// The rule set in force.
    #[must_use]
    pub fn rules(&self) -> &RuleSet {
        &self.rules
    }

    /// The topology.
    #[must_use]
    pub fn topology(&self) -> &Topology {
        &self.topo
    }

    /// The master data.
    #[must_use]
    pub fn connections(&self) -> &BTreeMap<ConnectionId, Connection> {
        &self.conns
    }

    /// The dispatch state of a resource.
    #[must_use]
    pub fn state_of(&self, c: &ConnectionId) -> State {
        self.tracker.state(c)
    }

    /// The limit planning assumes at a resource.
    #[must_use]
    pub fn effective_limit(&self, c: &ConnectionId) -> Option<Kw> {
        self.tracker.effective(c)
    }

    /// What the last cycle saw.
    #[must_use]
    pub fn view(&self) -> &CycleView {
        &self.view
    }

    fn delivery(&self) -> Delivery {
        Delivery { ack_timeout: self.rules.ack_timeout, max_attempts: self.rules.max_attempts }
    }

    fn msg(&mut self) -> MessageId {
        let m = MessageId(self.next_msg);
        self.next_msg += 1;
        m
    }

    /// Feed one input; collect what it caused.
    pub fn step(&mut self, input: Input, now: Timestamp) -> Vec<Output> {
        let mut out = Vec::new();
        match input {
            Input::Measurements(ms) => {
                for m in ms {
                    if let Err(reason) = state::plausible(&m, &self.conns, now) {
                        out.push(Output::Rejected { source: m.source.clone(), reason });
                        continue;
                    }
                    match &m.reading {
                        Reading::Connection { connection, .. } => {
                            if self.meters.get(connection).is_none_or(|old| old.at <= m.at) {
                                self.meters.insert(connection.clone(), m);
                            }
                        }
                        Reading::Asset { asset, .. } => {
                            if !self.topo.assets().any(|a| self.topo.asset_id(a) == asset) {
                                out.push(Output::Rejected { source: m.source.clone(), reason: Rejection::UnknownAsset });
                                continue;
                            }
                            if self.assets.get(asset).is_none_or(|old| old.at <= m.at) {
                                self.assets.insert(asset.clone(), m);
                            }
                        }
                    }
                }
            }
            Input::Profiles(ps) => {
                for p in ps {
                    if self.conns.contains_key(&p.connection) {
                        self.profiles.insert(p.connection.clone(), p);
                    }
                }
            }
            Input::Ack { connection, msg } => match self.tracker.ack(&connection, msg) {
                Ok(t) => out.extend(t.map(Output::Transition)),
                Err(why) => out.push(Output::IgnoredAck { connection, msg, why }),
            },
            Input::Refused { connection, msg, why } => {
                if self.tracker.current(&connection).is_some_and(|c| c.msg == msg) {
                    out.extend(self.tracker.release(&connection, msg).map(Output::Transition));
                    out.push(Output::Alarm(Alarm::Refused { connection, why }));
                } else {
                    out.push(Output::IgnoredAck { connection, msg, why: format!("refusal of unknown message: {why}") });
                }
            }
            Input::ChannelLost(c) => out.extend(self.tracker.channel_lost(&c, now).map(Output::Transition)),
            Input::ChannelRestored(c) => out.extend(self.tracker.channel_restored(&c).map(Output::Transition)),
            Input::Operator { author, connection, limit, note } => self.operator(author, connection, limit, note, now, &mut out),
            Input::Tick => self.cycle(now, &mut out),
        }
        out
    }

    fn operator(&mut self, author: String, connection: ConnectionId, limit: Option<Kw>, note: String, now: Timestamp, out: &mut Vec<Output>) {
        let on = local_date(now);
        let Some(c) = self.conns.get(&connection) else { return };
        match limit {
            None => {
                self.operator_held.remove(&connection);
                if self.tracker.current(&connection).is_some() {
                    let msg = self.msg();
                    let projected = Estimate { mean: 0.0, sd: 0.0, quality: Quality::Pseudo };
                    out.push(Output::Release(Release { msg, connection: connection.clone(), asset: format!("operator:{author}"), projected }));
                    out.extend(self.tracker.release(&connection, msg).map(Output::Transition));
                }
            }
            Some(requested) => {
                if !self.rules.valid_on(on) {
                    out.push(Output::Alarm(Alarm::RuleSetInactive { rule_set: self.rules.name.clone() }));
                    return;
                }
                let Some(lim) = self.rules.limit(c, requested, on) else { return };
                let reason = Reason {
                    cause: Cause::Operator,
                    asset: format!("operator:{author}"),
                    kind: CongestionKind::Thermal,
                    limit: requested.0,
                    estimate: Estimate { mean: 0.0, sd: 0.0, quality: Quality::Pseudo },
                    needed: Kw::ZERO,
                    inputs: Vec::new(),
                    rule_set: self.rules.name.clone(),
                    author: Some(author),
                    note: Some(note),
                };
                let cmd = self.command(c.clone(), lim.value(), lim.floor(), lim.raised(), reason, now);
                self.operator_held.insert(connection);
                self.emit(cmd, now, out);
            }
        }
    }

    fn command(&mut self, c: Connection, limit: Kw, floor: Kw, at_floor: bool, reason: Reason, now: Timestamp) -> Command {
        let on = local_date(now);
        let per_device = if c.mode == ControlMode::Direct {
            // Above-floor headroom split in proportion to each device's own headroom.
            let steuve = c.steuve(on);
            let heads: Vec<(String, Kw, Kw)> =
                steuve.iter().map(|d| (d.id.clone(), self.rules.device_floor(d), d.nominal - self.rules.device_floor(d))).collect();
            let total_head: Kw = heads.iter().map(|h| h.2).sum();
            let spare = (limit - floor).max(Kw::ZERO);
            heads.into_iter().map(|(id, f, h)| (id, (f + if total_head.0 > 0.0 { spare * (h.0 / total_head.0) } else { Kw::ZERO }).round_w())).collect()
        } else {
            Vec::new()
        };
        Command {
            msg: self.msg(),
            connection: c.id.clone(),
            limit,
            floor,
            at_floor,
            per_device,
            valid_until: now.saturating_add(self.rules.command_duration).unwrap_or(now),
            regime: self.rules.regime,
            attempt: 1,
            reason,
        }
    }

    fn emit(&mut self, cmd: Command, now: Timestamp, out: &mut Vec<Output>) {
        let d = self.delivery();
        let t = self.tracker.send(&cmd, now, d);
        out.push(Output::Command(cmd));
        out.extend(t.map(Output::Transition));
    }

    fn cycle(&mut self, now: Timestamp, out: &mut Vec<Output>) {
        let on = local_date(now);
        let d = self.delivery();
        let tick = self.tracker.tick(now, d);
        // A rule set past its validity authorises nothing — not even a retry of an older command.
        if self.rules.valid_on(on) {
            out.extend(tick.resend.into_iter().map(Output::Command));
        }
        out.extend(tick.transitions.into_iter().map(Output::Transition));

        // Präventiv budget: time each resource spent reduced today.
        if let Some(prev) = self.last_tick {
            let dt = now.duration_since(prev);
            for id in self.conns.keys() {
                if self.tracker.effective(id).is_some() && self.tracker.current(id).is_some_and(|c| c.regime == Regime::Praeventiv) {
                    let e = self.budget.entry(id.clone()).or_insert((on, SignedDuration::ZERO));
                    if e.0 != on {
                        *e = (on, SignedDuration::ZERO);
                    }
                    e.1 = e.1.saturating_add(dt);
                }
            }
        }
        self.last_tick = Some(now);

        if !self.rules.valid_on(on) {
            if !self.inactive {
                out.push(Output::Alarm(Alarm::RuleSetInactive { rule_set: self.rules.name.clone() }));
            }
            self.inactive = true;
            return;
        }
        self.inactive = false;

        let inputs = state::Inputs {
            topology: &self.topo,
            connections: &self.conns,
            meters: &self.meters,
            assets: &self.assets,
            profiles: &self.profiles,
            rules: &self.rules,
        };
        let (snap, rejected) = state::estimate(&inputs, now);
        let mut refused_now = BTreeSet::new();
        for (source, reason) in rejected {
            // A refused value is dropped, so it is refused once, not on every cycle it stays fresh.
            self.meters.retain(|_, m| m.source != source);
            self.assets.retain(|_, m| m.source != source);
            if matches!(reason, Rejection::BadData { .. }) {
                let n = self.bad_data.entry(source.clone()).or_insert(0);
                *n += 1;
                if *n == 2 {
                    out.push(Output::Alarm(Alarm::Inconsistent { source: source.clone(), cycles: *n }));
                }
                refused_now.insert(source.clone());
            }
            out.push(Output::Rejected { source, reason });
        }
        self.bad_data.retain(|s, _| refused_now.contains(s));

        let determined = !snap.assets.is_empty() || snap.loads.values().any(|l| l.source.is_some());
        let needs_data = self.rules.require_determined;
        if needs_data && !determined {
            if !self.stale {
                out.push(Output::Alarm(Alarm::StaleData));
            }
            self.stale = true;
            self.view = CycleView { transformer: None, congestions: Vec::new(), reduced: self.reduced() };
            return;
        }
        if self.stale {
            out.push(Output::Alarm(Alarm::DataRestored));
        }
        self.stale = false;

        let (plan, congestions) = self.control(&snap, now, out);
        self.apply(&snap, &plan, now, out);
        self.view = CycleView { transformer: Some(snap.flow(&self.topo, &self.conns, AssetRef::Transformer)), congestions, reduced: self.reduced() };
    }

    fn reduced(&self) -> usize {
        self.conns.keys().filter(|c| self.tracker.effective(c).is_some()).count()
    }

    /// The voltage at which the trigger share of the permissible drop is used up.
    fn v_trigger(&self) -> f64 {
        1.0 - self.rules.trigger * (1.0 - self.rules.v_min.0)
    }

    fn inputs_for(&self, snap: &Snapshot, involved: impl Fn(&NodeId) -> bool) -> Vec<InputRef> {
        let mut v: Vec<InputRef> = snap
            .loads
            .iter()
            .filter(|(id, _)| self.conns.get(*id).is_some_and(|c| involved(&c.node)))
            .filter_map(|(_, l)| l.source.as_ref().map(|(s, at)| InputRef { source: s.clone(), at: *at }))
            .collect();
        // An Ortsnetzstation measurement is an input only where it covers an involved connection.
        for (s, at, asset) in &snap.assets {
            let covers = self
                .topo
                .assets()
                .find(|a| self.topo.asset_id(*a) == asset)
                .is_some_and(|a| self.conns.values().any(|c| involved(&c.node) && self.topo.downstream(a, &c.node)));
            if covers {
                v.push(InputRef { source: s.clone(), at: *at });
            }
        }
        v
    }

    fn budget_left(&self, c: &ConnectionId, on: Date) -> bool {
        match self.rules.daily_budget {
            None => true,
            Some(b) => self.budget.get(c).is_none_or(|(d, used)| *d != on || *used < b),
        }
    }

    /// What a reduced resource would draw back if released.
    fn restore(&self, snap: &Snapshot, c: &ConnectionId) -> f64 {
        match (snap.loads.get(c), self.tracker.effective(c)) {
            (Some(l), Some(_)) => (l.steuve_demand - l.steuve).0.max(0.0),
            _ => 0.0,
        }
    }

    /// The members of a controller: those whose reduction relieves it (`weight > 0`), and under
    /// [`Selection::Netzbereich`] every other SteuVE of the same Netzbereiche too — with weight
    /// zero, so they share the depth without being counted as relief.
    fn members(&self, snap: &Snapshot, on: Date, weight: impl Fn(&Connection) -> f64) -> Vec<Member> {
        let areas: BTreeSet<&str> = self.conns.values().filter(|c| weight(c) > 0.0).map(|c| c.area.as_str()).collect();
        self.conns
            .values()
            .filter_map(|c| {
                let w = weight(c);
                let in_scope = w > 0.0 || (self.rules.selection == crate::rules::Selection::Netzbereich && areas.contains(c.area.as_str()));
                if in_scope { self.member(snap, c, w, on) } else { None }
            })
            .collect()
    }

    fn member(&self, snap: &Snapshot, c: &Connection, weight: f64, on: Date) -> Option<Member> {
        if weight < 0.0 || self.operator_held.contains(&c.id) || self.tracker.state(&c.id) == State::Failsafe || !self.budget_left(&c.id, on) {
            return None;
        }
        let floor = self.rules.floor(c, on)?.total;
        let load = snap.loads.get(&c.id)?;
        // Präventiv: the depth tapers over the last half hour of the daily budget, so the end of
        // the budget is a step-down rather than a load step.
        let cap = match self.rules.daily_budget {
            None => 1.0,
            Some(b) => {
                let used = self.budget.get(&c.id).filter(|(d, _)| *d == on).map_or(SignedDuration::ZERO, |(_, u)| *u);
                (b.saturating_sub(used).as_secs_f64() / 1800.0).clamp(0.0, 1.0)
            }
        };
        Some(Member {
            id: c.id.clone(),
            weight,
            demand: load.steuve_demand.max(load.steuve).0,
            installed: c.steuve_nominal(on).0,
            floor: floor.0,
            relay: c.relay_only,
            cap,
        })
    }

    /// One controller per congested asset (and one for voltage). Each holds a depth `α`: every
    /// SteuVE behind it may draw `floor + (1 − α) · (installed − floor)`. The same `α` for all of
    /// them is the non-discriminatory share of Ziff. 4.3; it is solved each cycle against the
    /// projected *unconstrained* flow, and it falls back stepwise.
    fn control(&mut self, snap: &Snapshot, now: Timestamp, out: &mut Vec<Output>) -> (BTreeMap<ConnectionId, (f64, Reason)>, Vec<Congestion>) {
        const P99: f64 = 2.326;
        let on = local_date(now);
        let r = self.rules.clone();
        let z = r.confidence_z;

        let mut acc = Acc::default();
        for a in self.topo.assets() {
            let e = snap.flow(&self.topo, &self.conns, a);
            let rating = self.topo.rating(a).0;
            let limit = r.trigger * rating;
            let determined = !r.require_determined || P99 * e.sd <= r.accuracy_current * rating;
            let congested = determined && e.mean - z * e.sd > limit;
            if determined && -e.mean - z * e.sd > rating {
                out.push(Output::Alarm(Alarm::FeedIn { asset: self.topo.asset_id(a).to_owned() }));
            }
            let downstream = |c: &Connection| self.topo.downstream(a, &c.node);
            let restore: f64 = self.conns.values().filter(|c| downstream(c)).map(|c| self.restore(snap, &c.id)).sum();
            let members = self.members(snap, on, |c| f64::from(u8::from(downstream(c))));
            let inputs = if congested || self.controllers.contains_key(self.topo.asset_id(a)) {
                self.inputs_for(snap, |n| self.topo.downstream(a, n))
            } else {
                Vec::new()
            };
            let spec = Spec {
                key: self.topo.asset_id(a).to_owned(),
                kind: CongestionKind::Thermal,
                limit,
                estimate: e,
                unconstrained: e.mean + restore,
                target: limit * (1.0 - r.margin),
                determined,
                congested,
                members,
                inputs,
            };
            self.run_controller(&mut acc, spec, now, out);
        }

        // Voltage: one controller, at the worst node — or where it was already acting.
        let v_trigger = self.v_trigger();
        let mut worst: Option<(NodeId, Estimate)> = self.voltage_node.clone().map(|n| {
            let e = snap.voltage(&self.topo, &self.conns, &n);
            (n, e)
        });
        for n in self.topo.nodes() {
            let e = snap.voltage(&self.topo, &self.conns, n);
            if worst.as_ref().is_none_or(|(_, w)| e.mean < w.mean) && e.mean + z * e.sd < v_trigger {
                worst = Some((n.clone(), e));
            }
        }
        self.voltage_node = None;
        if let Some((n, e)) = worst {
            let determined = !r.require_determined || P99 * e.sd <= r.accuracy_voltage;
            let congested = determined && e.mean + z * e.sd < v_trigger;
            let sens = |c: &Connection| -self.topo.voltage_sensitivity(&n, &c.node);
            let restore: f64 = self.conns.values().map(|c| sens(c) * self.restore(snap, &c.id)).sum();
            let members = self.members(snap, on, |c| sens(c).max(0.0));
            let inputs = self.inputs_for(snap, |m| self.topo.voltage_sensitivity(&n, m) != 0.0);
            let spec = Spec {
                key: "voltage".into(),
                kind: CongestionKind::Voltage,
                limit: v_trigger,
                estimate: e,
                unconstrained: e.mean - restore,
                target: v_trigger + 0.005,
                determined,
                congested,
                members,
                inputs,
            };
            if self.run_controller(&mut acc, spec, now, out) {
                self.voltage_node = Some(n);
            }
        }

        for a in self.shortfall.difference(&acc.short) {
            out.push(Output::Alarm(Alarm::ShortfallCleared { asset: a.clone() }));
        }
        self.shortfall = acc.short;
        self.controllers = acc.next;
        (acc.plan, acc.congestions)
    }

    /// Advance one controller; `true` if it stays active.
    fn run_controller(&self, acc: &mut Acc, s: Spec, now: Timestamp, out: &mut Vec<Output>) -> bool {
        let r = &self.rules;
        let (prev, last_back) = self.controllers.get(&s.key).copied().unwrap_or((0.0, now));
        if !s.congested && prev == 0.0 {
            return false;
        }
        let needed = match s.kind {
            CongestionKind::Thermal => s.unconstrained - s.target,
            CongestionKind::Voltage => s.target - s.unconstrained,
        };
        let (star, _) = solve(&s.members, needed);
        // A shortfall is about the hard limit — the rating, the voltage band — not the trigger:
        // the floors may fail to reach the target and still hold the asset.
        let hard = match s.kind {
            CongestionKind::Thermal => s.unconstrained - s.limit / r.trigger.max(f64::EPSILON),
            CongestionKind::Voltage => r.v_min.0 - s.unconstrained,
        };
        let (_, missing) = solve(&s.members, hard);
        // Stepping back aims below the target by the hysteresis, so it does not invite the next step down.
        let margin = match s.kind {
            CongestionKind::Thermal => r.hysteresis * s.limit,
            CongestionKind::Voltage => 0.01,
        };
        let (star_back, _) = solve(&s.members, needed + margin);
        let slot_over = now.duration_since(last_back) >= r.release_interval;
        // Deeper at once; back only stepwise, one slot at a time (Ziff. 4.3, FNN 2c);
        // hold while the state is not determined.
        let (mut alpha, mut stepped) = (prev, last_back);
        if !s.determined {
        } else if star >= prev {
            alpha = star;
        } else if slot_over && star_back < prev {
            alpha = star_back.max(prev * (1.0 - r.release_step));
            stepped = now;
        }
        let release_ok = match s.kind {
            CongestionKind::Thermal => s.unconstrained + r.confidence_z * s.estimate.sd < s.limit * (1.0 - r.hysteresis),
            CongestionKind::Voltage => s.unconstrained - r.confidence_z * s.estimate.sd > s.limit + 0.01,
        };
        if alpha < 0.03 {
            alpha = if release_ok { 0.0 } else { 0.03 };
        }
        if missing > 0.0 && alpha >= 1.0 {
            acc.short.insert(s.key.clone());
            if !self.shortfall.contains(&s.key) {
                let max_w = s.members.iter().map(|m| m.weight).fold(0.0, f64::max);
                let kw = if max_w > 0.0 { missing / max_w } else { missing };
                out.push(Output::Alarm(Alarm::Shortfall { asset: s.key.clone(), missing: Kw(kw).round_w() }));
            }
        }
        if alpha <= 0.0 {
            return false;
        }
        acc.next.insert(s.key.clone(), (alpha, stepped));
        let cause = if r.regime == Regime::Praeventiv { Cause::Plan } else { Cause::Relieve };
        let reason = Reason {
            cause,
            asset: s.key.clone(),
            kind: s.kind,
            limit: s.limit,
            estimate: s.estimate,
            needed: Kw(needed.max(0.0)).round_w(),
            inputs: s.inputs,
            rule_set: r.name.clone(),
            author: None,
            note: None,
        };
        for m in &s.members {
            let a = alpha.min(m.cap);
            if a > 0.0 && acc.plan.get(&m.id).is_none_or(|(x, _)| a > *x) {
                acc.plan.insert(m.id.clone(), (a, reason.clone()));
            }
        }
        acc.congestions.push(Congestion { asset: s.key, kind: s.kind, limit: s.limit, estimate: s.estimate });
        true
    }

    fn apply(&mut self, snap: &Snapshot, plan: &BTreeMap<ConnectionId, (f64, Reason)>, now: Timestamp, out: &mut Vec<Output>) {
        let on = local_date(now);
        let controllable = self.conns.values().filter(|c| !c.steuve(on).is_empty()).count();
        let mut cap = ((controllable as f64) * self.rules.max_switch_share).ceil().max(1.0) as usize;

        enum Change {
            Command(Box<(Connection, crate::rules::Limit, Reason)>),
            Release(ConnectionId, String),
        }
        // Priority: deeper first (grid safety), then restores, then releases.
        let mut changes: Vec<(u8, f64, Change)> = Vec::new();
        let mut renewals = Vec::new();
        for c in self.conns.values() {
            if self.operator_held.contains(&c.id) {
                continue;
            }
            let cur = self.tracker.current(&c.id).cloned();
            match (plan.get(&c.id), cur) {
                (Some((alpha, reason)), cur) => {
                    let Some(floor) = self.rules.floor(c, on) else { continue };
                    if c.relay_only && *alpha < 0.5 {
                        if let Some(cur) = cur {
                            changes.push((2, 0.0, Change::Release(c.id.clone(), cur.reason.asset.clone())));
                        }
                        continue;
                    }
                    let installed = c.steuve_nominal(on);
                    let requested = floor.total + (installed - floor.total) * (1.0 - alpha);
                    let Some(lim) = self.rules.limit(c, requested, on) else { continue };
                    match cur {
                        None => changes.push((0, installed.0 - lim.value().0, Change::Command(Box::new((c.clone(), lim, reason.clone()))))),
                        Some(cur) => {
                            let delta = lim.value().0 - cur.limit.0;
                            if delta.abs() > (0.5f64).max(cur.limit.0 * 0.05) {
                                let mut reason = reason.clone();
                                let prio = if delta < 0.0 {
                                    0
                                } else {
                                    reason.cause = Cause::Restore;
                                    1
                                };
                                changes.push((prio, -delta, Change::Command(Box::new((c.clone(), lim, reason)))));
                            } else if cur.valid_until.duration_since(now) < self.rules.command_duration / 4 {
                                let reason = Reason { cause: Cause::Renew, ..reason.clone() };
                                // A renewal goes through the floor computation like any other command.
                                if let Some(lim) = self.rules.limit(c, cur.limit, on) {
                                    renewals.push((c.clone(), lim.value(), lim.floor(), lim.raised(), reason));
                                }
                            }
                        }
                    }
                }
                // A präventiv budget that has run out is a legal limit: its release goes first and is never deferred.
                (None, Some(cur)) if !self.budget_left(&c.id, on) => changes.push((0, f64::INFINITY, Change::Release(c.id.clone(), cur.reason.asset.clone()))),
                (None, Some(cur)) => changes.push((2, 0.0, Change::Release(c.id.clone(), cur.reason.asset.clone()))),
                (None, None) => {}
            }
        }
        changes.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)));
        let mut deferred = 0;
        for (_, prio, ch) in changes {
            let forced = prio.is_infinite();
            if cap == 0 && !forced {
                deferred += 1;
                continue;
            }
            cap = cap.saturating_sub(1);
            match ch {
                Change::Command(b) => {
                    let (c, lim, reason) = *b;
                    let cmd = self.command(c, lim.value(), lim.floor(), lim.raised(), reason, now);
                    self.emit(cmd, now, out);
                }
                Change::Release(id, asset) => {
                    let msg = self.msg();
                    let e = snap.loads.get(&id).map_or(Estimate { mean: 0.0, sd: 0.0, quality: Quality::Pseudo }, |l| l.total);
                    out.push(Output::Release(Release { msg, connection: id.clone(), asset, projected: e }));
                    out.extend(self.tracker.release(&id, msg).map(Output::Transition));
                }
            }
        }
        for (c, limit, floor, at_floor, reason) in renewals {
            let cmd = self.command(c, limit, floor, at_floor, reason, now);
            self.emit(cmd, now, out);
        }
        if deferred > 0 {
            out.push(Output::Alarm(Alarm::Deferred { count: deferred }));
        }
    }
}

/// What one cycle's controllers decided.
#[derive(Default)]
struct Acc {
    plan: BTreeMap<ConnectionId, (f64, Reason)>,
    congestions: Vec<Congestion>,
    short: BTreeSet<String>,
    next: BTreeMap<String, (f64, Timestamp)>,
}

/// One controller's inputs this cycle.
struct Spec {
    key: String,
    kind: CongestionKind,
    limit: f64,
    estimate: Estimate,
    unconstrained: f64,
    target: f64,
    determined: bool,
    congested: bool,
    members: Vec<Member>,
    inputs: Vec<InputRef>,
}

/// A SteuVE connection behind a controller.
struct Member {
    id: ConnectionId,
    /// Relief per kW (1 for thermal; p.u. per kW for voltage).
    weight: f64,
    /// Unconstrained SteuVE demand.
    demand: f64,
    installed: f64,
    floor: f64,
    relay: bool,
    /// The deepest `α` this member may take (below 1 only for a präventiv budget running out).
    cap: f64,
}

impl Member {
    fn allowed(&self, alpha: f64) -> f64 {
        let alpha = alpha.min(self.cap);
        if self.relay { if alpha >= 0.5 { self.floor } else { f64::INFINITY } } else { self.floor + (1.0 - alpha) * (self.installed - self.floor).max(0.0) }
    }
}

/// The smallest `α` whose relief covers `needed`, and what is missing at `α = 1`.
fn solve(members: &[Member], needed: f64) -> (f64, f64) {
    if needed <= 0.0 {
        return (0.0, 0.0);
    }
    let relief = |alpha: f64| -> f64 { members.iter().map(|m| m.weight * (m.demand - m.allowed(alpha)).max(0.0)).sum() };
    let full = relief(1.0);
    if full < needed {
        return (1.0, needed - full);
    }
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if relief(mid) >= needed { hi = mid } else { lo = mid }
    }
    (hi, 0.0)
}
