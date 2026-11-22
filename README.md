# fms

**A Flexibilitätsmanagementsystem in Rust — the distribution grid operator's side of § 14a EnWG.**

[![crates.io](https://img.shields.io/crates/v/fms)](https://crates.io/crates/fms)
[![docs.rs](https://img.shields.io/docsrs/fms)](https://docs.rs/fms)
[![CI](https://github.com/hupe1980/fms/actions/workflows/ci.yml/badge.svg)](https://github.com/hupe1980/fms/actions/workflows/ci.yml)
[![license](https://img.shields.io/crates/l/fms)](#license)

[Documentation](https://hupe1980.github.io/fms) ·
[Getting started](https://hupe1980.github.io/fms/docs/getting-started/) ·
[Floor calculator](https://hupe1980.github.io/fms/floor/)

Since 2024 a German DSO may no longer refuse to connect a heat pump or a wallbox for lack of grid
capacity. In return it may *temporarily* reduce such steuerbare Verbrauchseinrichtungen (SteuVE)
when the grid is actually congested, and never below a guaranteed minimum. `fms` is the system
that makes that bargain work, from the DSO's side:

- **Knows the grid.** It estimates every cable and the transformer from smart-meter values,
  Ortsnetzstation measurements and profiles. Each estimate comes with its uncertainty, and the
  estimator agrees with pandapower's AC power flow to 0.03 % of rating.
- **Acts only when it is sure.** A reduction starts only when the grid state is *determined* in
  the sense of VDE FNN Hinweis 2e and the trigger is crossed. A lying sensor is refused as bad
  data.
- **Never goes below the floor.** The Mindestleistung of BK6-22-300 is part of the command
  type: a limit below it cannot be constructed.
- **Treats everyone alike.** Every SteuVE drawn on gets the same depth above its own floor. The
  return is stepwise, and the rule that chose a household is published.
- **Proves what it did.** Every decision goes into a SHA-256-chained log with its reason. The
  customer's documentation and the monthly VNBdigital publication (BDEW format v1.0) are computed
  from that log alone.

It is the other end of the wire from [hems](https://github.com/hupe1980/hems): hems receives the
limit, `fms` decides it.

## See it in one command

```console
$ brew install hupe1980/tap/fms      # or: cargo install fms
$ fms sim cold-snap
```

Prebuilt binaries for Linux (static), macOS and Windows are attached to every
[release](https://github.com/hupe1980/fms/releases).

```text
  cold-snap  Thu 15 Jan 2026 · seed 1 · 96 households, 76 SteuVE
  January cold snap: heat pumps overload the transformer

  Transformer loading over the day
   140% │                                      ░
        │                                    ░░░░░
   100% │┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄▁▁▃▅▄▄▂┄┄┄┄┄┄
        │                                  ▃███████▄▆▁
        │            ▁▃▅▄▂                ▄███████████▂
        │▃▄▄▃▄▄▃▄▄▄▅▆█████▇▄▃▂▂▂▂▂▂▂▂▂▃▃▅██████████████▅▄
    20% │████████████████████████████████████████████████
        └────────────────────────────────────────────────
         0     3     6     9     12    15    18    21    h
         █ with the FMS   ░ without

                             without FMS        with FMS
  congested                      135 min           0 min
  peak loading                  125 % T1         93 % T1

  FMS          408 commands (81 relieve · 177 renew · 150 restore) · 54 releases
               54 of 96 households reduced · 513 kWh curtailed · at most 31.8 kWh for one household
               below the floor 0 ✓ · below the EMS's own minimum 0 ✓
  Grid state   30 % of households metered + the transformer at the ONS · FNN 2e asks 30 % · error 0.4 kW, 95 % interval held 100 %
  VNBdigital   NB-34117-0042 · netzorientiert · 76 SteuVE · 4.58 h · intensity 0.24 %
```

The simulator runs a whole day twice, with and without the FMS, against a hidden "true" feeder.
The FMS sees only what a DSO would see: a metered share, one minute late. `fms sim all` runs
every reference day:

| Day | What it shows |
|---|---|
| `cold-snap` | heat pumps overload the transformer; relieved to zero congested minutes |
| `wallbox-evening` | a new estate's cars load one cable and sag its voltage; measured per feeder |
| `pv-noon` | nothing to do — and nothing done |
| `channel-outage` | a branch loses its CLS channel; the FMS plans with the Steuerbox failsafe |
| `lying-sensor` | a spoofed transformer reading and a lying meter are refused; no command is sent |
| `beyond-14a` | the floors cannot hold the transformer; the FMS says how many kW are missing and the publication proposes Netzausbau |
| `blind-feeder` | no measurement at all: präventive Steuerung, two hours a day, and the rebound when it ends |

## Everything else the CLI does

```console
$ fms floor heat-pump:9 wallbox:11 cooling:3        # what is this connection owed?
  ● heat-pump1         heat pump · 9.0 kW
  ● wallbox2           wallbox · 11.0 kW
  ○ cooling3           cooling · 3.0 kW — not a SteuVE (≤ 4.2 kW)

  Mindestleistung  7.56 kW
  EMS: 4.2 + (2 − 1) · 0.8 · 4.2 = 7.56 kW [Ziff. 4.5.2]

$ fms sim cold-snap --out day/                       # keep the evidence
$ fms verify day/evidence.jsonl --expect-head <hash> # nothing changed, nothing cut off
$ fms explain day/evidence.jsonl H101c               # what one customer may see
$ fms publish day/evidence.jsonl --connections day/connections.json --month 2026-01
NeBe-ID;PLZ;Art der Steuerung;Anzahl betroffener SteuVE;Eingriffsdauer [h];Eingriffsintensität [%];Maßnahme zur Reduzierung;Voraussichtlicher Abschluss
NB-34117-0042;"34117;34119";netzorientiert;76;4,58;0,24;Keine Maßnahme;-
$ fms sr-id C113735592                               # complete an SR-ID's check digit
C1137355923
$ fms sim wallbox-evening --selection downstream     # the narrower reading of Ziff. 4.3
```

## As a library

The engine is sans-IO: it takes an input and the time, and returns outputs. Sockets, storage
and the clock belong to you, so the same engine runs in a service, an ADMS or a simulator.

```rust,ignore
use fms::prelude::*;

let mut engine = Engine::new(Topology::new(grid)?, connections, RuleSet::bk6_22_300())?;
engine.step(Input::Measurements(batch), now);
engine.step(Input::Profiles(forecast), now);
for out in engine.step(Input::Tick, now) {
    log.append(now, out.clone());                 // the evidence log
    if let Output::Command(c) = out {
        // c.limit is never below c.floor; c.reason says why, from what, under which rule set
        msb.send(&c)?;
    }
}
```

## What it implements

| Area | Source |
|---|---|
| SteuVE: the four kinds above 4.2 kW, heat-pump and cooling groups as one, 2024 cut-off, exemptions | BK6-22-300 Ziff. 2.4, 3.1 |
| Mindestleistung: Direkt, EMS with Gleichzeitigkeitsfaktor, > 11 kW thermal, relay | Ziff. 4.5, FNN Hinweis 2a |
| Determined grid state: P99 accuracy 10 % current, 2 % voltage | FNN Hinweis 2e |
| Trigger, stepwise return in 15-minute slots, an end time on every command | FNN Hinweis 2c, Ziff. 4.3 |
| Non-discriminatory selection across the Netzbereich | Ziff. 4.3 |
| Präventive Steuerung: 2 h a day, 24 months, never after 2028 | Ziff. 10.5 |
| Documentation per connection, acknowledgement as proof of execution | Ziff. 7 |
| VNBdigital publication | Ziff. 8.4, BDEW format v1.0 |
| SR-ID check digit | BDEW Bildungsvorschrift v1.1 |

Rule sets are validated against these bounds. fms ships no transport to the meter operator:
embed the engine and bring your own.

## How it is checked

- **pandapower** solves the AC power flow on the same feeders. The estimator agrees within
  0.03 % of rating on flows and 0.003 p.u. on voltages, and the test fails above 1 % / 0.005. Run
  it with `FMS_REQUIRE_PANDAPOWER=1 cargo test --test pandapower_oracle` after
  `pip install pandapower`.
- **Every reference day, several seeds:** no limit below a floor (checked by the household's own
  independent computation too), relievable days relieved even with 10 % message loss, quiet days
  and lying sensors silent, the 95 % interval holding the truth, and the publication matching
  what the Steuerboxen actually applied.
- **Random input** to the engine never panics or goes below a floor, and replays identically.
- **Every clause is a test**, from each Gleichzeitigkeitsfaktor step to the BDEW intensity.
- `#![forbid(unsafe_code)]`, and `unwrap`, `expect`, `panic` and indexing are denied in the
  library.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.
