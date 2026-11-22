+++
title = "Getting started"
description = "Install the CLI, run a reference day, keep its evidence, publish the month."
weight = 1
+++

## Install

```console
$ brew install hupe1980/tap/fms
```

Or with Rust 1.88 or newer, `cargo install fms`. Prebuilt binaries for Linux (static), macOS and
Windows, with checksums, are on the [releases page](https://github.com/hupe1980/fms/releases). The library alone is `cargo add fms --no-default-features`.

## Run a day

```console
$ fms days
$ fms sim cold-snap
```

The simulator builds a 400 kVA feeder with 96 households, gives some of them heat pumps,
wallboxes, PV and batteries, and runs a January day twice: once without an FMS, once with one.
The FMS sees what a DSO sees:

- 30 % of the households through their smart meter gateways, one minute late;
- the transformer measured at the Ortsnetzstation;
- profiles for everyone else.

It never sees the hidden truth it is judged against.

Read the report from the top:

1. **The chart.** The transformer over the day, with (solid) and without (light).
2. **Congested.** Minutes any cable or the transformer spent above its rating, or a node below
   the voltage limit.
3. **FMS.** Every command by cause: relieve, restore, renew. Below it, how many households were
   reduced, the energy curtailed, and two checks that must be zero. "Below the floor" is
   recomputed from the log. "Below the EMS's own minimum" is what each household's energy
   manager counted.
4. **Grid state.** The metered share against what VDE FNN Hinweis 2e asks for this measurement
   setup, and how far the FMS's belief was from the truth.
5. **VNBdigital.** The row this day would add to the monthly publication.

`fms sim all` runs all seven days as a table. `--seed` gives another feeder and other weather,
and `--json` gives the full report.

## Keep the evidence

```console
$ fms sim cold-snap --out day/
$ fms verify day/evidence.jsonl
✓ 1351 records, chain intact
  head fb147b5f4f73…
$ fms explain day/evidence.jsonl H101c
$ fms publish day/evidence.jsonl --connections day/connections.json --month 2026-01
```

`evidence.jsonl` is the log a real deployment keeps: every command, release, refusal, alarm and
state change, each record chained to the one before it. `explain` shows one customer what the
Festlegung entitles them to see. `publish` writes the month's row in the BDEW format for
VNBdigital.

## Next

- [How it works](@/docs/how-it-works.md): the four planes.
- [The rules](@/docs/rules.md): which clause each number comes from.
- [Floor calculator](@/floor.md): what one connection is owed.
