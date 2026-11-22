+++
title = "Security"
description = "The system that can switch off a district, and what keeps it honest."
weight = 10
+++

An FMS can reduce thousands of heat pumps at once. The threats that matter, and what fms does
about each:

| Threat | Control |
|---|---|
| Mass false reduction | floor in the type, determination before action, the mass-switching limit, bad-data refusal |
| A spoofed or lying sensor | 5 σ innovation test on Ortsnetzstation values, fuse plausibility for meters, the `Inconsistent` alarm |
| An unlawful configuration | `RuleSet::validate` against BK6-22-300 and the FNN Hinweise |
| A synchronised release | stepwise return in 15-minute slots, with hysteresis |
| A forged acknowledgement | applied only to the resource's current message, otherwise recorded and ignored |
| A rewritten log | SHA-256 chain; `--expect-head` against an anchored head |
| One customer seeing another | documentation filtered to one connection; the publication is aggregates only |
| Hostile input files | strict parsing, no panics, a 1 GiB limit |

fms ships no transport to the MSB: connect it over mutual TLS with pinned certificates, and keep
its credentials in a vault.

The library forbids `unsafe` and denies `unwrap`, `expect`, `panic` and indexing. To report a
vulnerability, see the
[security policy](https://github.com/hupe1980/fms/blob/main/SECURITY.md).
