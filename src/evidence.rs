//! The evidence log: every output, hash-chained, and the reports that are queries over it.
//!
//! Each record carries the SHA-256 of its predecessor, so a changed, removed, inserted or
//! reordered line is detected by anyone holding the log — the BNetzA, an auditor, a customer.
//! The documentation of a connection and the monthly VNBdigital publication are computed from
//! the log and the master data alone; no counter exists that could disagree with it.

use crate::command::Cause;
use crate::engine::Output;
use crate::model::{Connection, ConnectionId};
use crate::rules::Regime;
use crate::time::{berlin, local_date};
use crate::units::Kw;
use jiff::civil::Date;
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// The `prev` of the first record.
pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// One line of the log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// Position, from 1.
    pub seq: u64,
    /// When the engine produced it.
    pub at: Timestamp,
    /// Hash of the previous record.
    pub prev: String,
    /// Hash of this record (over `seq`, `at`, `prev` and `event`).
    pub hash: String,
    /// What happened.
    pub event: Output,
}

#[derive(Serialize)]
struct Unsigned<'a> {
    seq: u64,
    at: Timestamp,
    prev: &'a str,
    event: &'a Output,
}

fn hex(bytes: &[u8]) -> String {
    use fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn digest(seq: u64, at: Timestamp, prev: &str, event: &Output) -> String {
    let body = serde_json::to_vec(&Unsigned { seq, at, prev, event }).unwrap_or_default();
    hex(&Sha256::digest(body))
}

/// An append-only, hash-chained log.
#[derive(Debug, Clone, Default)]
pub struct Log {
    records: Vec<Record>,
}

impl Log {
    /// An empty log.
    #[must_use]
    pub fn new() -> Log {
        Log::default()
    }

    /// Append one output; returns its hash.
    pub fn append(&mut self, at: Timestamp, event: Output) -> String {
        let seq = self.records.len() as u64 + 1;
        let prev = self.head().to_owned();
        let hash = digest(seq, at, &prev, &event);
        self.records.push(Record { seq, at, prev, hash: hash.clone(), event });
        hash
    }

    /// Append every output of one step.
    pub fn extend(&mut self, at: Timestamp, events: impl IntoIterator<Item = Output>) {
        for e in events {
            self.append(at, e);
        }
    }

    /// The hash of the last record — what to anchor outside the operator's reach.
    #[must_use]
    pub fn head(&self) -> &str {
        self.records.last().map_or(GENESIS, |r| r.hash.as_str())
    }

    /// The records.
    #[must_use]
    pub fn records(&self) -> &[Record] {
        &self.records
    }

    /// The log as JSON Lines.
    #[must_use]
    pub fn to_jsonl(&self) -> String {
        let mut s = String::new();
        for r in &self.records {
            s.push_str(&serde_json::to_string(r).unwrap_or_default());
            s.push('\n');
        }
        s
    }

    /// Read and verify a JSON Lines log.
    ///
    /// # Errors
    /// [`Broken`] at the first line that does not parse or does not chain.
    pub fn from_jsonl(text: &str) -> Result<Log, Broken> {
        let mut records = Vec::new();
        let mut prev = GENESIS.to_owned();
        for (i, line) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
            let n = i + 1;
            let r: Record = serde_json::from_str(line).map_err(|e| Broken { line: n, seq: None, why: format!("not a record: {e}") })?;
            let expected_seq = records.len() as u64 + 1;
            if r.seq != expected_seq {
                return Err(Broken {
                    line: n,
                    seq: Some(r.seq),
                    why: format!("sequence {} where {expected_seq} was expected — a record was removed, inserted or moved", r.seq),
                });
            }
            if r.prev != prev {
                return Err(Broken { line: n, seq: Some(r.seq), why: "does not chain to the record before it".into() });
            }
            if digest(r.seq, r.at, &r.prev, &r.event) != r.hash {
                return Err(Broken { line: n, seq: Some(r.seq), why: "content does not match its hash — the record was changed".into() });
            }
            prev.clone_from(&r.hash);
            records.push(r);
        }
        Ok(Log { records })
    }
}

/// Where and why a log failed verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Broken {
    /// Line number, from 1.
    pub line: usize,
    /// The sequence number on that line, if it parsed.
    pub seq: Option<u64>,
    /// What is wrong.
    pub why: String,
}

impl fmt::Display for Broken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.seq {
            Some(s) => write!(f, "line {} (record {s}): {}", self.line, self.why),
            None => write!(f, "line {}: {}", self.line, self.why),
        }
    }
}

impl std::error::Error for Broken {}

/// One reduction of one connection, reconstructed from the log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Intervention {
    /// The resource.
    pub connection: ConnectionId,
    /// When the command was issued.
    pub start: Timestamp,
    /// When it was released, superseded or expired.
    pub end: Timestamp,
    /// The commanded limit.
    pub limit: Kw,
    /// The floor it respected.
    pub floor: Kw,
    /// Regime.
    pub regime: Regime,
    /// Why.
    pub cause: Cause,
    /// The asset concerned.
    pub asset: String,
    /// Number of measurements behind it.
    pub inputs: usize,
    /// The full reason of the command that started it: the estimate, the limit at stake, the
    /// relief needed and the measurements — what Ziff. 7.1 asks the DSO to document.
    pub reason: crate::command::Reason,
    /// When the Steuerbox's acknowledgement of the first command arrived — the proof that it was
    /// carried out (Ziff. 7.2). `None` if it never did.
    pub confirmed: Option<Timestamp>,
}

impl Intervention {
    /// Its length.
    #[must_use]
    pub fn duration(&self) -> SignedDuration {
        self.end.duration_since(self.start)
    }
}

/// Every reduction in the log, per connection in time order. `until` closes reductions still
/// running at the end of the log.
#[must_use]
pub fn interventions(log: &Log, until: Timestamp) -> Vec<Intervention> {
    let mut open: BTreeMap<ConnectionId, (Intervention, Timestamp)> = BTreeMap::new();
    let mut msgs: BTreeMap<ConnectionId, crate::command::MessageId> = BTreeMap::new();
    let mut done = Vec::new();
    let close = |iv: Intervention, at: Timestamp, valid: Timestamp, done: &mut Vec<Intervention>| {
        let end = at.min(valid);
        if end > iv.start {
            done.push(Intervention { end, ..iv });
        }
    };
    for r in log.records() {
        match &r.event {
            Output::Command(c) if c.attempt == 1 => {
                if let Some((iv, valid)) = open.remove(&c.connection) {
                    close(iv, r.at, valid, &mut done);
                }
                let iv = Intervention {
                    connection: c.connection.clone(),
                    start: r.at,
                    end: r.at,
                    limit: c.limit,
                    floor: c.floor,
                    regime: c.regime,
                    cause: c.reason.cause,
                    asset: c.reason.asset.clone(),
                    inputs: c.reason.inputs.len(),
                    reason: c.reason.clone(),
                    confirmed: None,
                };
                msgs.insert(c.connection.clone(), c.msg);
                open.insert(c.connection.clone(), (iv, c.valid_until));
            }
            Output::Release(rel) => {
                if let Some((iv, valid)) = open.remove(&rel.connection) {
                    close(iv, r.at, valid, &mut done);
                }
            }
            Output::Transition(t) if t.to == crate::dispatch::State::Confirmed => {
                if let Some((iv, _)) = open.get_mut(&t.connection)
                    && iv.confirmed.is_none()
                    && t.msg.is_some()
                    && t.msg == msgs.get(&t.connection).copied()
                {
                    iv.confirmed = Some(r.at);
                }
            }
            Output::Transition(t) if t.to == crate::dispatch::State::Idle => {
                if let Some((iv, valid)) = open.remove(&t.connection) {
                    close(iv, r.at, valid, &mut done);
                }
            }
            _ => {}
        }
    }
    for (_, (iv, valid)) in open {
        close(iv, until, valid, &mut done);
    }
    // A renewal continues the reduction it renews: merge back-to-back intervals with equal limit.
    done.sort_by(|a, b| a.connection.cmp(&b.connection).then(a.start.cmp(&b.start)));
    let mut merged: Vec<Intervention> = Vec::new();
    for iv in done {
        if let Some(last) = merged.last_mut()
            && last.connection == iv.connection
            && last.end == iv.start
            && (last.limit.0 - iv.limit.0).abs() < 1e-9
            && last.regime == iv.regime
        {
            last.end = iv.end;
            last.confirmed = last.confirmed.or(iv.confirmed);
            continue;
        }
        merged.push(iv);
    }
    merged
}

/// What the DSO does to reduce control in an area (BDEW format field 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Measure {
    /// Netzdigitalisierung — präventiv only.
    Netzdigitalisierung,
    /// Netzausbau — either regime.
    Netzausbau,
    /// Netzoptimierung — netzorientiert only.
    Netzoptimierung,
    /// Keine Maßnahme — netzorientiert only.
    KeineMassnahme,
}

impl Measure {
    /// The text the format uses.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Measure::Netzdigitalisierung => "Netzdigitalisierung",
            Measure::Netzausbau => "Netzausbau",
            Measure::Netzoptimierung => "Netzoptimierung",
            Measure::KeineMassnahme => "Keine Maßnahme",
        }
    }

    /// Whether the format allows this measure for the regime.
    #[must_use]
    pub fn allowed(self, regime: Regime) -> bool {
        match self {
            Measure::Netzausbau => true,
            Measure::Netzdigitalisierung => regime == Regime::Praeventiv,
            Measure::Netzoptimierung | Measure::KeineMassnahme => regime == Regime::Netzorientiert,
        }
    }
}

/// The DSO's plan for an area: the measure and its expected completion (`H1 2027`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AreaPlan {
    /// Field 7.
    pub measure: Measure,
    /// Field 8: `H1 JJJJ`, `H2 JJJJ`, or `None` for "-".
    pub completion: Option<String>,
}

/// One row of the monthly publication on VNBdigital (BDEW format v1.0, 30.01.2025).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PublicationRow {
    /// Field 1: Netzbereich (NeBe-ID).
    pub area: String,
    /// Field 2: the area's five-digit postcodes.
    pub postcodes: Vec<String>,
    /// Field 3: one per area and month; netzorientiert in a month that had both.
    pub regime: Regime,
    /// Field 4: the month's maximum number of simultaneously reduced SteuVE.
    pub steuve: usize,
    /// Field 5: hours of the month with any reduction in the area.
    pub hours: f64,
    /// Field 6: Eingriffsintensität in per cent — the month's mean of the daily
    /// `Σᵢ (installed − allowedᵢ)/installed · durationᵢ/24 h` over the area's SteuVE.
    pub intensity_percent: f64,
    /// Field 7.
    pub measure: Measure,
    /// Field 8.
    pub completion: Option<String>,
}

/// The month's publication and what a reviewer should look at before it is uploaded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Publication {
    /// The month (its first day).
    pub month: Date,
    /// One row per area with control; areas without control are not listed.
    pub rows: Vec<PublicationRow>,
    /// Format rules a row breaks, or defaults that were assumed.
    pub warnings: Vec<String>,
}

/// The publication for the calendar month containing `month` (Europe/Berlin), from the log and
/// the master data alone.
#[must_use]
pub fn publication(log: &Log, connections: &[Connection], month: Date, plans: &BTreeMap<String, AreaPlan>) -> Publication {
    let first = month.first_of_month();
    let next = first.last_of_month().tomorrow().unwrap_or(first);
    let start = first.to_zoned(berlin()).map(|z| z.timestamp()).unwrap_or(Timestamp::UNIX_EPOCH);
    let end = next.to_zoned(berlin()).map(|z| z.timestamp()).unwrap_or(start);
    let days = f64::from(first.days_in_month());
    let by_id: BTreeMap<&ConnectionId, &Connection> = connections.iter().map(|c| (&c.id, c)).collect();
    let installed_area = |area: &str, on: Date| -> f64 { connections.iter().filter(|c| c.area == area).map(|c| c.steuve_nominal(on).0).sum() };

    #[derive(Default)]
    struct Acc {
        regimes: BTreeSet<Regime>,
        events: Vec<(Timestamp, i64)>,
        spans: Vec<(Timestamp, Timestamp)>,
        reduced_kwh_days: f64,
    }
    let mut acc: BTreeMap<String, Acc> = BTreeMap::new();
    // A log describes the grid only up to its last record: a reduction still running then is
    // counted to that moment, not to its end time, which the log cannot know it reached.
    let horizon = log.records().last().map_or(end, |r| r.at).min(end);
    for iv in interventions(log, horizon) {
        let (s, e) = (iv.start.max(start), iv.end.min(horizon));
        if e <= s {
            continue;
        }
        let Some(c) = by_id.get(&iv.connection) else { continue };
        let on = local_date(s);
        let units = i64::try_from(c.steuve(on).len()).unwrap_or(0);
        let a = acc.entry(c.area.clone()).or_default();
        a.regimes.insert(iv.regime);
        a.events.push((s, units));
        a.events.push((e, -units));
        a.spans.push((s, e));
        let cut = (c.steuve_nominal(on).0 - iv.limit.0).max(0.0);
        let area_installed = installed_area(&c.area, on);
        if area_installed > 0.0 {
            a.reduced_kwh_days += cut / area_installed * (e.duration_since(s).as_secs_f64() / 86_400.0);
        }
    }
    let mut warnings = Vec::new();
    // Assets the floors could not hold for at least an hour this month: a case for Netzausbau.
    let mut short_open: BTreeMap<String, Timestamp> = BTreeMap::new();
    let mut short_hours: BTreeMap<String, f64> = BTreeMap::new();
    for r in log.records().iter().filter(|r| r.at >= start && r.at < end) {
        match &r.event {
            Output::Alarm(crate::engine::Alarm::Shortfall { asset, .. }) => {
                short_open.insert(asset.clone(), r.at);
            }
            Output::Alarm(crate::engine::Alarm::ShortfallCleared { asset }) => {
                if let Some(s) = short_open.remove(asset) {
                    *short_hours.entry(asset.clone()).or_default() += r.at.duration_since(s).as_secs_f64() / 3600.0;
                }
            }
            _ => {}
        }
    }
    for (asset, s) in short_open {
        *short_hours.entry(asset).or_default() += end.min(log.records().last().map_or(end, |r| r.at)).duration_since(s).as_secs_f64() / 3600.0;
    }
    let shortfalls: BTreeSet<String> = short_hours.into_iter().filter(|(_, h)| *h >= 1.0).map(|(a, _)| a).collect();
    let rows = acc
        .into_iter()
        .map(|(area, mut a)| {
            let regime = if a.regimes.contains(&Regime::Netzorientiert) { Regime::Netzorientiert } else { Regime::Praeventiv };
            a.events.sort_by(|x, y| x.0.cmp(&y.0).then(x.1.cmp(&y.1)));
            let (mut cur, mut peak) = (0i64, 0i64);
            for (_, d) in &a.events {
                cur += d;
                peak = peak.max(cur);
            }
            let hours = round2(union_hours(a.spans));
            let cap = if regime == Regime::Praeventiv { 62.0 } else { 744.0 };
            if hours > cap {
                warnings.push(format!("{area}: {hours} h exceeds the format's {cap} h for {}", regime.label()));
            }
            let plan = plans.get(&area).cloned().unwrap_or_else(|| {
                // A month in which the floors could not hold an asset is a month that argues for Netzausbau.
                let m = if !shortfalls.is_empty() {
                    Measure::Netzausbau
                } else if regime == Regime::Praeventiv {
                    Measure::Netzdigitalisierung
                } else {
                    Measure::KeineMassnahme
                };
                let why = if shortfalls.is_empty() {
                    String::new()
                } else {
                    format!(" (shortfall on {})", shortfalls.iter().cloned().collect::<Vec<_>>().join(", "))
                };
                warnings.push(format!("{area}: no measure recorded — proposed \"{}\"{why}; confirm it and its completion", m.label()));
                AreaPlan { measure: m, completion: None }
            });
            if !plan.measure.allowed(regime) {
                warnings.push(format!("{area}: \"{}\" is not allowed for {}", plan.measure.label(), regime.label()));
            }
            let postcodes: BTreeSet<String> = connections.iter().filter(|c| c.area == area).map(|c| c.postcode.clone()).collect();
            if let Some(bad) = postcodes.iter().find(|p| p.len() != 5 || !p.bytes().all(|b| b.is_ascii_digit())) {
                warnings.push(format!("{area}: postcode {bad:?} is not five digits"));
            }
            PublicationRow {
                postcodes: postcodes.into_iter().collect(),
                regime,
                steuve: usize::try_from(peak).unwrap_or(0),
                hours,
                intensity_percent: round2((a.reduced_kwh_days / days * 100.0).min(100.0)),
                measure: plan.measure,
                completion: plan.completion,
                area,
            }
        })
        .collect();
    Publication { month: first, rows, warnings }
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

fn union_hours(mut spans: Vec<(Timestamp, Timestamp)>) -> f64 {
    spans.sort();
    let mut total = 0.0;
    let mut cur: Option<(Timestamp, Timestamp)> = None;
    for (s, e) in spans {
        match cur {
            Some((cs, ce)) if s <= ce => cur = Some((cs, ce.max(e))),
            Some((cs, ce)) => {
                total += ce.duration_since(cs).as_secs_f64();
                cur = Some((s, e));
            }
            None => cur = Some((s, e)),
        }
    }
    if let Some((cs, ce)) = cur {
        total += ce.duration_since(cs).as_secs_f64();
    }
    total / 3600.0
}

fn csv_field(s: &str) -> String {
    if s.contains([';', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_owned() }
}

/// The publication as CSV in the BDEW field order: semicolon-separated, decimal commas, several
/// postcodes in one field separated by `;` (and therefore quoted).
#[must_use]
pub fn publication_csv(p: &Publication) -> String {
    let mut s = String::from(
        "NeBe-ID;PLZ;Art der Steuerung;Anzahl betroffener SteuVE;Eingriffsdauer [h];Eingriffsintensität [%];Maßnahme zur Reduzierung;Voraussichtlicher Abschluss\n",
    );
    let dec = |x: f64| format!("{x:.2}").replace('.', ",");
    for r in &p.rows {
        let fields = [
            csv_field(&r.area),
            csv_field(&r.postcodes.join(";")),
            r.regime.label().to_owned(),
            r.steuve.to_string(),
            dec(r.hours),
            dec(r.intensity_percent),
            csv_field(r.measure.label()),
            r.completion.clone().unwrap_or_else(|| "-".into()),
        ];
        s.push_str(&fields.join(";"));
        s.push('\n');
    }
    s
}

/// Everything the log says about one connection — what its customer may see, and nothing else.
#[must_use]
pub fn documentation(log: &Log, connection: &ConnectionId, until: Timestamp) -> Vec<Intervention> {
    interventions(log, until).into_iter().filter(|i| &i.connection == connection).collect()
}
