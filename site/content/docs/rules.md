+++
title = "The rules"
description = "BNetzA BK6-22-300 and the VDE FNN Hinweise, as the engine executes them — clause by clause."
weight = 3
+++

Every number below is a field of a `RuleSet` and a test. `RuleSet::validate` refuses any value
outside these bounds: a rule set is configuration, and configuration can be edited.

## Who is controllable

| Rule | Clause |
|---|---|
| Non-public charging points, heat pumps incl. Heizstab, space cooling, storage when charging | Ziff. 2.4.1 |
| Netzanschlussleistung **above** 4.2 kW, on low voltage | Ziff. 2.4.1 |
| Several heat pumps (or cooling units) behind one connection are **one** SteuVE if their sum is above 4.2 kW | Ziff. 2.4.2 |
| Devices commissioned after 31.12.2023 take part; older ones may opt in; Sonderrechte and non-residential heating are exempt | Ziff. 3.1 |

## What is owed: the Mindestleistung

| Mode | Floor | Clause |
|---|---|---|
| Direktansteuerung | 4.2 kW per SteuVE; 0.4 × power for heat pumps or cooling above 11 kW | Ziff. 4.5.1 |
| One EMS | 4.2 kW + (n − 1) · GZF · 4.2 kW, GZF 0.8 … 0.45 | Ziff. 4.5.2 |
| Relay (W4) | holds every SteuVE at the minimum | FNN Hinweis 2a |

A `Limit` can only be built through the floor computation. A request below the floor is
raised to it, and the command records that it was.

## When a reduction may happen

- **Netzorientiert** (Ziff. 4). A reduction needs a congestion that the
  Netzzustandsermittlung establishes. It lasts as long as it is necessary — there is no fixed
  maximum. fms starts one only when the estimate is *determined* in the sense of FNN Hinweis
  2e: its 99th-percentile error is within 10 % of the rating for current, or 2 % for voltage.
  A result is valid for five minutes.
- **Trigger** (FNN Hinweis 2c). The DSO may act from 80 % of the permissible loading or voltage
  drop. fms acts at 90 % by default and aims a few per cent below that.
- **Präventiv** (Ziff. 10.5). Planning data are allowed only where netzorientierte control is
  not yet possible:
  - at most two hours per day;
  - for at most 24 months per Netzbereich;
  - never after 31.12.2028.

  In fms the depth tapers over the last half hour of a household's daily budget, and a release
  forced by the budget is never deferred.

## How it is shared

- **Non-discriminatory** (Ziff. 4.3). Ziff. 4.3 presumes that every SteuVE of the
  Netzbereich relieves the grid equally, and it prescribes no formula. fms gives every SteuVE
  it draws on the same depth α: each may draw `floor + (1 − α) · (installed − floor)`. By
  default that is the whole Netzbereich. `--selection downstream` limits it to the SteuVE behind
  the congested asset, and the CLI shows what each reading costs.
- **Stepwise back** (Ziff. 4.3, FNN 2c). The depth returns in 15-minute slots, by half of
  what remains, and only with margin to spare. Every command carries an end time.

## What is recorded and published

- **Documentation** (Ziff. 7). Every command carries its reason: the asset, the estimate and
  its uncertainty, the relief needed, the measurements used, and the rule set. The Steuerbox's
  acknowledgement is the proof that the command was carried out. Records are kept two years.
- **VNBdigital** (Ziff. 8.4, BDEW format v1.0). The publication is monthly, by the 15th, with
  one row per Netzbereich:
  - PLZ;
  - type of control;
  - the month's peak of affected SteuVE;
  - hours with any intervention;
  - Eingriffsintensität, the monthly mean of the daily
    `Σ (installed − allowed)/installed · duration/24 h`;
  - the measure and its expected completion.
