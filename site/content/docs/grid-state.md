+++
title = "Grid state"
description = "A Gaussian posterior over every connection, with losses — within 0.03 % of an AC power flow."
weight = 4
+++

## The estimator

Every connection's load starts as a Gaussian: its profile's expected value and spread. Without a
profile, the prior is uninformative, bounded only by the fuse. Each fresh measurement is a
linear observation of those loads:

- a smart meter gateway's TAF 10 value observes one connection;
- the transformer at the Ortsnetzstation observes their sum;
- an LV feeder measured at the busbar observes the sum of its branch.

Each measurement is folded in by the exact Gaussian update over the full covariance. That is a
Kalman step, which on this problem is weighted least squares.

A measurement at a feeder's sending end includes the losses of everything beyond it. Those
predicted losses are taken off before the reading updates the loads, so they are not counted
twice.

Flows and voltages are linear in the loads: a cable carries everything behind it, and the voltage
drops by `R · P / V²` along the path. So their mean and standard deviation follow exactly from
the posterior. A second-order term adds each section's I²R losses, which grow as voltage sags.

## Checked against pandapower

The oracle test builds the same feeder in pandapower, loads it near its rating, and solves the
full AC equations:

| | Agreement | Test bound |
|---|---|---|
| line and transformer flows | 0.03 % of rating | 1 % |
| node voltages | 0.003 p.u. | 0.005 p.u. |
| losses | 0.1 kW of ~13 kW | — |

```console
$ pip install pandapower
$ FMS_REQUIRE_PANDAPOWER=1 cargo test --release --test pandapower_oracle -- --nocapture
```

In the simulator the transformer estimate is within 0.2–1.3 kW of the truth on a 400 kVA
transformer, and its stated 95 % interval holds the truth in at least 99 % of cycles. A test
fails if the interval ever covers less than 95 %.

## Determined or not

The estimate of every asset carries its σ. fms calls it *determined* when 2.326 σ — the 99th
percentile — is within 10 % of the rating, or within 2 % for voltage, as FNN Hinweis 2e asks. An
undetermined asset gets no new netzorientierte reduction. The report shows the metered share
against the 2e table for a radial feeder:

| At the Ortsnetzstation | Share of connections that must deliver TAF 10 values |
|---|---|
| every LV feeder measured | 15 % |
| the transformer total | 30 % |
| nothing | 70 % |

## Bad data

A smart meter is the only direct observation of its connection. There is nothing redundant to
test it against, so it is checked for plausibility: finite, within 1.1 × its fuse, and its
SteuVE share consistent. An Ortsnetzstation measurement *is* redundant. If its normalised
innovation against everything else exceeds 5 σ, it is refused before use. When it is refused
twice in a row, an `Inconsistent` alarm asks someone to look. On `lying-sensor` the transformer
reading is spoofed 80 % high and one meter claims 120 kW: both are refused, and nothing is sent.
