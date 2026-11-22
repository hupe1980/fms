+++
title = "Simulator"
description = "A hidden feeder, honest measurements, Steuerboxen that lose things, and households that push back."
weight = 7
+++

## The feeder

The generated feeder has a 400 kVA transformer and four branches of NAYY 4×150 turning into
4×95, with three households per node. Each seed gives another feeder. Households have:

- base load with morning and evening peaks;
- heat pumps whose registered power includes the Heizstab, drawing about a third of it at
  −12 °C and three quarters at the evening peak;
- wallboxes with a real energy need, so curtailed charging comes back later;
- PV and storage.

Heat that was not delivered is half made up later. The truth is the loss-corrected flow model
that the pandapower test checks.

## What the FMS sees

- the metered share of households, with 1 % noise, one minute late;
- the Ortsnetzstation's transformer, or every feeder, depending on the day;
- profiles for everyone, with an honest spread — a wallbox is either off or at 11 kW,
  whatever the mean says.

## The other end

Each household has a simulated Steuerbox. It applies commands, loses 2 % of them and of the
acknowledgements, and falls back to its failsafe on a lost channel. Behind it, a household guard
applies the limit the way an energy manager such as hems does, and counts every limit below the
floor it computes itself.

## The days

| Day | What it shows |
|---|---|
| `cold-snap` | relief at the transformer |
| `wallbox-evening` | a cable and the voltage at the end of one branch, measured per feeder |
| `pv-noon` | nothing to do, nothing done |
| `channel-outage` | planning with the failsafe |
| `lying-sensor` | a spoofed RTU and a lying meter refused |
| `beyond-14a` | a shortfall, and Netzausbau proposed |
| `blind-feeder` | präventiv control and its rebound |

`cargo test` runs every day on four seeds. It asserts:

- zero limits below a floor;
- relievable days relieved;
- quiet days silent;
- präventiv within two hours a day;
- that a seed replays bit for bit.
