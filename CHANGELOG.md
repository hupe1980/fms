# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the project follows semantic
versioning, and before 1.0 a minor release may break the API. Entries marked ⚠ change what
reaches a device.

## [Unreleased]

## [0.2.0] — 2026-11-22

The first release with code: the core, a closed-loop simulator and the `fms` CLI.

### Added

- **Rules** (`fms::model`, `fms::rules`). SteuVE classification per BK6-22-300 Ziff. 2.4 and 3.1:
  - heat-pump and cooling groups behind one connection count as one SteuVE;
  - devices commissioned before 2024 take part only if opted in;
  - exempt devices are left out.

  Also:
  - the Mindestleistung for Direktansteuerung and EMS, with the Gleichzeitigkeitsfaktor and the
    > 11 kW thermal rule;
  - relay-only connections;
  - dated `RuleSet`s for the netzorientiert and präventiv regimes, validated against the legal
    bounds;
  - `SrId` with the BDEW check digit.
- **Grid state** (`fms::grid`, `fms::state`):
  - a radial LV topology;
  - a Gaussian estimator that folds smart-meter, transformer and LV-feeder measurements into
    profile priors, with a second-order loss correction;
  - bad-data refusal of Ortsnetzstation values, and fuse plausibility for meters;
  - determination per VDE FNN Hinweis 2e, and its metering-share table.
- **Decision** (`fms::engine`) ⚠:
  - one controller per congested asset;
  - the same depth above the floor for every SteuVE drawn on, across the Netzbereich by default;
  - trigger, margin and hysteresis;
  - stepwise return in 15-minute slots;
  - a mass-switching limit;
  - präventiv daily budgets that taper;
  - shortfall alarms judged against the rating;
  - operator instructions, floored and attributed.
- **Dispatch** (`fms::dispatch`): a per-resource state machine with acknowledgement, retries,
  `Unknown`, failsafe and its lapse, and expiry.
- **Evidence** (`fms::evidence`):
  - a SHA-256-chained log;
  - documentation per connection, with the acknowledgement as proof of execution;
  - the VNBdigital publication in the BDEW format v1.0, as CSV or JSON.
- **Simulator** (`fms::sim`): a SimBench-like feeder with households, a simulated Steuerbox, a
  household guard with EMS semantics, and seven reference days.
- **CLI**: `fms days`, `sim`, `floor`, `verify`, `publish`, `explain` and `sr-id`.
- **Distribution**: prebuilt binaries (Linux static, macOS, Windows) with checksums on every
  release, and `brew install hupe1980/tap/fms`.

### Changed

- 0.1.0 was a name reservation without an API.
