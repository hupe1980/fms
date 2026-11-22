//! `fms` — run the reference days, compute floors, verify evidence, produce the publication.

use clap::{Parser, Subcommand, ValueEnum};
use fms::evidence::{AreaPlan, Log, documentation, publication, publication_csv};
use fms::model::{Connection, ConnectionId, ControlMode, Device, DeviceKind, SrId};
use fms::rules::RuleSet;
use fms::sim::{self, Report};
use fms::units::Kw;
use jiff::civil::{Date, date};
use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "fms", version, about = "A Flexibilitätsmanagementsystem: the DSO side of § 14a EnWG", long_about = None)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List the reference days.
    Days,
    /// Run a reference day (or `all`) with and without the FMS and show what happened.
    Sim {
        /// The day, e.g. `cold-snap`; `all` runs every day as a summary table.
        #[arg(default_value = "cold-snap")]
        day: String,
        /// Seed for the feeder and the weather noise.
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// Print the report as JSON.
        #[arg(long)]
        json: bool,
        /// Write `evidence.jsonl`, `connections.json` and `report.json` into this directory.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Who is drawn on: every SteuVE of the Netzbereich (Ziff. 4.3) or only those behind the asset.
        #[arg(long, value_enum, default_value_t = Scope::Netzbereich)]
        selection: Scope,
    },
    /// Compute a connection's Mindestleistung under BK6-22-300.
    Floor {
        /// Direct control or one value for an EMS.
        #[arg(long, value_enum, default_value_t = Mode::Ems)]
        mode: Mode,
        /// Devices as `kind:kW`, e.g. `heat-pump:9 wallbox:11 cooling:3 storage:5`.
        #[arg(required = true)]
        devices: Vec<String>,
    },
    /// Verify an evidence log's hash chain — and, with an anchored head, that nothing was cut off.
    Verify {
        /// The `evidence.jsonl` file.
        log: PathBuf,
        /// The head hash anchored elsewhere; a chain can prove its prefix, only this proves its end.
        #[arg(long)]
        expect_head: Option<String>,
    },
    /// The month's VNBdigital publication from an evidence log (BDEW format v1.0).
    Publish {
        /// The `evidence.jsonl` file.
        log: PathBuf,
        /// The master data (`connections.json`).
        #[arg(long)]
        connections: PathBuf,
        /// The month, `YYYY-MM`.
        #[arg(long)]
        month: String,
        /// Per-area measures as JSON: `{"NB-1": {"measure": "netzausbau", "completion": "H2 2027"}}`.
        #[arg(long)]
        plans: Option<PathBuf>,
        /// JSON instead of CSV.
        #[arg(long)]
        json: bool,
    },
    /// Every reduction of one connection, with its reason — what its customer may see.
    Explain {
        /// The `evidence.jsonl` file.
        log: PathBuf,
        /// The connection.
        connection: String,
    },
    /// Check an SR-ID (steuerbare Ressource) and its check digit.
    SrId {
        /// The ID, e.g. `C1137355925`, or ten characters to compute the check digit for.
        id: String,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Scope {
    Netzbereich,
    Downstream,
}

impl From<Scope> for fms::rules::Selection {
    fn from(s: Scope) -> Self {
        match s {
            Scope::Netzbereich => fms::rules::Selection::Netzbereich,
            Scope::Downstream => fms::rules::Selection::Downstream,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum Mode {
    Ems,
    Direct,
}

struct Style {
    on: bool,
}

impl Style {
    fn detect() -> Style {
        Style { on: std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() }
    }
    fn paint(&self, code: &str, s: &str) -> String {
        if self.on { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_owned() }
    }
    fn bold(&self, s: &str) -> String {
        self.paint("1", s)
    }
    fn dim(&self, s: &str) -> String {
        self.paint("2", s)
    }
    fn good(&self, s: &str) -> String {
        self.paint("32", s)
    }
    fn bad(&self, s: &str) -> String {
        self.paint("31", s)
    }
    fn accent(&self, s: &str) -> String {
        self.paint("36", s)
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let st = Style::detect();
    match run(cli.cmd, &st) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{} {e}", st.bad("error:"));
            ExitCode::from(2)
        }
    }
}

fn run(cmd: Cmd, st: &Style) -> Result<ExitCode, String> {
    match cmd {
        Cmd::Days => {
            for d in sim::days() {
                println!("{}  {}", st.accent(&format!("{:<16}", d.name)), d.title);
            }
            println!("\n{}", st.dim("Run one with `fms sim <day>`, or all of them with `fms sim all`."));
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Sim { day, seed, json, out, selection } => {
            if day == "all" {
                let mut rows = Vec::new();
                for d in sim::days() {
                    rows.push(sim::run_with(&d, seed, selection.into()).map_err(|e| e.to_string())?.report);
                }
                if json {
                    println!("{}", serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())?);
                } else {
                    summary(&rows, st);
                }
                let ok = rows.iter().all(|r| r.floor_violations == 0 && r.below_minimum == 0);
                return Ok(if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE });
            }
            let d = sim::day(&day).ok_or_else(|| format!("no day called {day:?} — `fms days` lists them"))?;
            let run = sim::run_with(&d, seed, selection.into()).map_err(|e| e.to_string())?;
            if let Some(dir) = out {
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                std::fs::write(dir.join("evidence.jsonl"), run.log.to_jsonl()).map_err(|e| e.to_string())?;
                std::fs::write(dir.join("connections.json"), serde_json::to_string_pretty(&run.connections).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
                std::fs::write(dir.join("report.json"), serde_json::to_string_pretty(&run.report).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
                eprintln!("{} wrote evidence.jsonl, connections.json and report.json to {}", st.good("✓"), dir.display());
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&run.report).map_err(|e| e.to_string())?);
            } else {
                report(&run.report, st);
            }
            Ok(if run.report.floor_violations == 0 && run.report.below_minimum == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE })
        }
        Cmd::Floor { mode, devices } => {
            let on = date(2026, 1, 1);
            let mut devs = Vec::new();
            for (i, spec) in devices.iter().enumerate() {
                let (kind, kw) = spec.split_once(':').ok_or_else(|| format!("{spec:?}: expected kind:kW, e.g. heat-pump:9"))?;
                let kind = match kind {
                    "heat-pump" | "hp" | "wp" => DeviceKind::HeatPump,
                    "wallbox" | "ev" | "charging-point" => DeviceKind::ChargingPoint,
                    "cooling" | "klima" => DeviceKind::Cooling,
                    "storage" | "battery" => DeviceKind::Storage,
                    other => return Err(format!("unknown device kind {other:?} (heat-pump, wallbox, cooling, storage)")),
                };
                let kw: f64 = kw.replace(',', ".").parse().map_err(|_| format!("{spec:?}: {kw:?} is not a number of kW"))?;
                devs.push(Device {
                    id: format!("{}{}", kind.label().replace(' ', "-"), i + 1),
                    kind,
                    nominal: Kw(kw),
                    commissioned: date(2025, 1, 1),
                    opted_in: false,
                    exempt: false,
                });
            }
            let c = Connection {
                id: ConnectionId::from("cli"),
                resource: None,
                node: "N".into(),
                mode: match mode {
                    Mode::Ems => ControlMode::Ems,
                    Mode::Direct => ControlMode::Direct,
                },
                area: String::new(),
                postcode: String::new(),
                devices: devs,
                relay_only: false,
                failsafe: None,
                failsafe_for: jiff::SignedDuration::from_hours(2),
                fuse: Kw(100.0),
            };
            let rules = RuleSet::bk6_22_300();
            let steuve = c.steuve(on);
            for s in &steuve {
                println!("  {} {:<18} {}", st.good("●"), s.id, st.dim(&format!("{} · {:.1} kW", s.kind.label(), s.nominal.0)));
            }
            for d in c.devices.iter().filter(|d| !steuve.iter().any(|s| s.id.split('+').any(|p| p == d.id))) {
                println!("  {} {:<18} {}", st.dim("○"), d.id, st.dim(&format!("{} · {:.1} kW — not a SteuVE (≤ 4.2 kW)", d.kind.label(), d.nominal.0)));
            }
            match rules.floor(&c, on) {
                Some(f) => {
                    println!("\n  Mindestleistung  {}", st.bold(&format!("{:.2} kW", f.total.round_w().0)));
                    println!("  {}", st.dim(&f.explanation));
                }
                None => println!("\n  {}", st.dim("No SteuVE — § 14a does not apply to this connection.")),
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Verify { log, expect_head } => {
            let text = read(&log)?;
            match Log::from_jsonl(&text) {
                Ok(l) => {
                    if let Some(h) = expect_head
                        && h.trim() != l.head()
                    {
                        let known = l.records().iter().position(|r| r.hash == h.trim());
                        match known {
                            Some(i) => println!(
                                "{} the anchored head is record {}; the log continues to {} — later records are not covered by the anchor",
                                st.paint("33", "!"),
                                i + 1,
                                l.records().len()
                            ),
                            None => println!("{} the anchored head is not in this log — records were removed from its end, or it is another log", st.bad("✗")),
                        }
                        return Ok(ExitCode::FAILURE);
                    }
                    println!("{} {} records, chain intact", st.good("✓"), l.records().len());
                    println!("  head {}", st.dim(l.head()));
                    Ok(ExitCode::SUCCESS)
                }
                Err(b) => {
                    println!("{} {b}", st.bad("✗ broken at"));
                    Ok(ExitCode::FAILURE)
                }
            }
        }
        Cmd::Publish { log, connections, month, plans, json } => {
            let l = Log::from_jsonl(&read(&log)?).map_err(|b| format!("evidence log broken at {b}"))?;
            let cs: Vec<Connection> = serde_json::from_str(&read(&connections)?).map_err(|e| format!("{}: {e}", connections.display()))?;
            let m: Date = format!("{month}-01").parse().map_err(|_| format!("{month:?}: expected YYYY-MM"))?;
            let plans: BTreeMap<String, AreaPlan> = match plans {
                Some(p) => serde_json::from_str(&read(&p)?).map_err(|e| format!("{}: {e}", p.display()))?,
                None => BTreeMap::new(),
            };
            let p = publication(&l, &cs, m, &plans);
            if json {
                println!("{}", serde_json::to_string_pretty(&p).map_err(|e| e.to_string())?);
            } else {
                print!("{}", publication_csv(&p));
            }
            for w in &p.warnings {
                eprintln!("{} {w}", st.paint("33", "warning:"));
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Explain { log, connection } => {
            let l = Log::from_jsonl(&read(&log)?).map_err(|b| format!("evidence log broken at {b}"))?;
            let until = l.records().last().map_or(jiff::Timestamp::UNIX_EPOCH, |r| r.at);
            let ivs = documentation(&l, &ConnectionId(connection.clone()), until);
            if ivs.is_empty() {
                println!("{} was never reduced.", st.bold(&connection));
                return Ok(ExitCode::SUCCESS);
            }
            println!("{} — {} reductions\n", st.bold(&connection), ivs.len());
            let tz = fms::time::berlin();
            for iv in &ivs {
                let (s, e) = (iv.start.to_zoned(tz.clone()), iv.end.to_zoned(tz.clone()));
                let proof = iv
                    .confirmed
                    .map_or_else(|| st.paint("33", "not acknowledged"), |c| st.good(&format!("acknowledged {}", c.to_zoned(tz.clone()).strftime("%H:%M:%S"))));
                println!(
                    "  {} {}–{}  {}  floor {:.2} kW  {}  {}  {}",
                    s.strftime("%d.%m."),
                    s.strftime("%H:%M"),
                    e.strftime("%H:%M"),
                    st.bold(&format!("≤ {:.2} kW", iv.limit.0)),
                    iv.floor.0,
                    st.dim(&format!("{:?} · {}", iv.cause, iv.regime.label()).to_lowercase()),
                    st.dim(&format!("on {} · {} measurements", iv.asset, iv.inputs)),
                    proof,
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::SrId { id } => {
            if id.len() == 10 {
                let d = SrId::check_digit(&id).ok_or("ten characters, digits or capital letters")?;
                println!("{id}{d}");
                return Ok(ExitCode::SUCCESS);
            }
            match id.parse::<SrId>() {
                Ok(s) => {
                    println!("{} {s} is a valid SR-ID", st.good("✓"));
                    Ok(ExitCode::SUCCESS)
                }
                Err(e) => {
                    println!("{} {e}", st.bad("✗"));
                    Ok(ExitCode::FAILURE)
                }
            }
        }
    }
}

/// Input files above this are refused before they are read.
const MAX_INPUT: u64 = 1 << 30;

fn read(path: &std::path::Path) -> Result<String, String> {
    let len = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?.len();
    if len > MAX_INPUT {
        return Err(format!("{}: {len} bytes is above the 1 GiB this tool reads", path.display()));
    }
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn check(st: &Style, n: usize) -> String {
    if n == 0 { st.good("0 ✓") } else { st.bad(&format!("{n} ✗")) }
}

fn summary(rows: &[Report], st: &Style) {
    println!(
        "{}",
        st.bold(&format!("{:<16} {:>14} {:>14} {:>9} {:>9} {:>10} {:>7}", "day", "congested min", "peak load %", "commands", "reduced", "kWh cut", "floor"))
    );
    println!("{}", st.dim(&format!("{:<16} {:>14} {:>14}", "", "without → with", "without → with")));
    for r in rows {
        let cmds: usize = r.commands.values().sum();
        let cong = format!("{:>5} → {:<5}", r.without.congested_minutes, r.with.congested_minutes);
        let peak = format!("{:>5.0} → {:<5.0}", r.without.peak_loading_percent, r.with.peak_loading_percent);
        let cong = if r.with.congested_minutes <= r.without.congested_minutes * 0.1 { st.good(&cong) } else { st.bad(&cong) };
        println!(
            "{:<16} {:>14} {:>14} {:>9} {:>9} {:>10.1} {:>7}",
            r.day,
            cong,
            peak,
            cmds,
            r.households_reduced,
            r.curtailed_kwh,
            check(st, r.floor_violations + r.below_minimum)
        );
    }
}

const BLOCKS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

fn chart(r: &Report, st: &Style) {
    // Two series over 48 half-hour columns; rows of 10 %, a rule at 100 %.
    let cols: Vec<(f64, f64)> = r
        .series
        .chunks(2)
        .map(|c| {
            let n = c.len() as f64;
            (c.iter().map(|p| p.without).sum::<f64>() / n, c.iter().map(|p| p.with).sum::<f64>() / n)
        })
        .collect();
    let peak = cols.iter().map(|c| c.0.max(c.1)).fold(100.0f64, f64::max);
    // Rows of 20 %, the rule drawn where 100 % falls.
    let per = 20.0;
    let rows = ((peak / per).ceil() as usize).clamp(5, 10);
    for row in (0..rows).rev() {
        let lo = row as f64 * per;
        let top = lo + per;
        let label = if row == rows - 1 || (top - 100.0).abs() < 1e-9 || row == 0 { format!("{top:>4.0}%") } else { "     ".into() };
        let mut line = String::new();
        for &(without, with) in &cols {
            let cell = |v: f64| -> usize { (((v - lo) / per).clamp(0.0, 1.0) * 8.0).round() as usize };
            let (cw, cb) = (cell(with), cell(without));
            let rule = (lo + per - 100.0).abs() < 1e-9;
            let ch = if cw > 0 {
                let c = BLOCKS.get(cw).copied().unwrap_or('█').to_string();
                if with > 100.0 { st.bad(&c) } else { st.accent(&c) }
            } else if cb > 0 {
                st.dim(&BLOCKS.get(cb).copied().unwrap_or('█').to_string().replace(['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'], "░"))
            } else if rule {
                st.dim("┄")
            } else {
                " ".into()
            };
            line.push_str(&ch);
        }
        println!("  {} {}{line}", st.dim(&label), st.dim("│"));
    }
    println!("  {} {}", " ".repeat(5), st.dim(&format!("└{}", "─".repeat(cols.len()))));
    println!("  {}  {}", " ".repeat(5), st.dim("0     3     6     9     12    15    18    21    h"));
    println!("  {}  {} with the FMS   {} without", " ".repeat(5), st.accent("█"), st.dim("░"));
}

fn wrap(s: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for w in s.split_whitespace() {
        let last = lines.last_mut().map_or(0, |l| l.chars().count());
        if last + w.len() + 1 > width && last > 0 {
            lines.push(String::new());
        }
        if let Some(l) = lines.last_mut() {
            if !l.is_empty() {
                l.push(' ');
            }
            l.push_str(w);
        }
    }
    lines
}

fn report(r: &Report, st: &Style) {
    println!();
    println!(
        "  {}  {}",
        st.bold(&r.day),
        st.dim(&format!("{} · seed {} · {} households, {} SteuVE", r.date.strftime("%a %d %b %Y"), r.seed, r.households, r.steuve))
    );
    println!("  {}", r.title);
    for l in wrap(&r.story, 92) {
        println!("  {}", st.dim(&l));
    }
    println!();
    println!("  {}", st.bold("Transformer loading over the day"));
    chart(r, st);
    println!();
    let w = &r.without;
    let f = &r.with;
    println!("  {:<22}{:>16}{:>16}", "", st.dim("without FMS"), st.dim("with FMS"));
    let mark = |good: bool, s: String| if good { st.good(&s) } else { st.bad(&s) };
    println!(
        "  {:<22}{:>16}{:>16}",
        "congested",
        format!("{} min", w.congested_minutes),
        mark(f.congested_minutes <= w.congested_minutes * 0.1, format!("{} min", f.congested_minutes))
    );
    println!(
        "  {:<22}{:>16}{:>16}",
        "peak loading",
        format!("{:.0} % {}", w.peak_loading_percent, w.peak_asset),
        format!("{:.0} % {}", f.peak_loading_percent, f.peak_asset)
    );
    println!("  {:<22}{:>16}{:>16}", "lowest voltage", format!("{:.3} p.u.", w.min_voltage), format!("{:.3} p.u.", f.min_voltage));
    println!("  {:<22}{:>16}{:>16}", "EV energy still owed", format!("{:.1} kWh", w.ev_unserved_kwh), format!("{:.1} kWh", f.ev_unserved_kwh));
    println!();
    let cmds: usize = r.commands.values().sum();
    let parts: Vec<String> = r.commands.iter().map(|(k, v)| format!("{v} {k}")).collect();
    println!(
        "  {:<12} {} {}",
        st.bold("FMS"),
        st.bold(&format!("{cmds} commands")),
        st.dim(&format!("({}) · {} releases · {} retries", parts.join(" · "), r.releases, r.retries))
    );
    println!(
        "  {:<12} {} of {} households reduced · {:.1} kWh curtailed · at most {:.1} kWh for one household",
        "", r.households_reduced, r.households, r.curtailed_kwh, r.worst_household_kwh
    );
    println!("  {:<12} below the floor {} · below the EMS's own minimum {}", "", check(st, r.floor_violations), check(st, r.below_minimum));
    let req = format!("FNN 2e asks {:.0} %", r.required_share * 100.0);
    let enough = r.metered_share + 1e-9 >= r.required_share;
    println!(
        "  {:<12} {:.0} % of households metered{} · {} · error {:.1} kW, 95 % interval held {:.0} %",
        st.bold("Grid state"),
        r.metered_share * 100.0,
        match r.ons {
            fms::state::MeteringSetup::Feeders => " + every LV feeder at the ONS",
            fms::state::MeteringSetup::Transformer => " + the transformer at the ONS",
            fms::state::MeteringSetup::None => ", nothing at the ONS",
        },
        if enough { st.good(&req) } else { st.paint("33", &format!("{req} — not determined")) },
        r.estimator_mae_kw,
        r.coverage_95 * 100.0
    );
    if r.rejected > 0 || !r.alarms.is_empty() {
        let a: Vec<String> = r.alarms.iter().map(|(k, v)| format!("{k} ×{v}")).collect();
        println!(
            "  {:<12} {}{}",
            st.bold("Alarms"),
            if r.rejected > 0 { format!("{} measurements refused · ", r.rejected) } else { String::new() },
            a.join(" · ")
        );
    }
    for p in &r.publication {
        println!(
            "  {:<12} {} · {} · {} SteuVE · {:.2} h · intensity {:.2} %",
            st.bold("VNBdigital"),
            p.area,
            p.regime.label(),
            p.steuve,
            p.hours,
            p.intensity_percent
        );
    }
    println!("  {:<12} {} records · rule set {} · head {}…", st.bold("Evidence"), r.records, r.rule_set, r.head.get(..12).unwrap_or(""));
    println!();
}
