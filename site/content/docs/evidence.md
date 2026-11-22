+++
title = "Evidence"
description = "A hash-chained log, the customer's documentation, and the BDEW publication — all from the same records."
weight = 6
+++

## The log

Every output of the engine is a record: commands, restores, releases, alarms, refused
measurements, state changes and ignored acknowledgements. Each record carries its sequence
number, its time, the SHA-256 of the record before it, and its own hash.

```console
$ fms verify evidence.jsonl
✓ 1351 records, chain intact
$ fms verify evidence.jsonl --expect-head 9f3a…
✗ the anchored head is not in this log — records were removed from its end, or it is another log
```

A changed, removed, inserted or moved record is found, and its line is named. A chain can only
prove its prefix, though. To prove that nothing was cut from the end, anchor the head hash
somewhere the operator cannot rewrite, and verify against it with `--expect-head`.

## Documentation for one connection

```console
$ fms explain evidence.jsonl H101c
H101c — 3 reductions

  15.01. 17:35–18:20  ≤ 6.10 kW  floor 4.20 kW  relieve · netzorientiert  on T1 · 31 measurements  acknowledged 17:35:00
```

For each reduction it shows:

- when it started and ended;
- the limit and the floor it respected;
- why, and under which regime;
- the measurements behind it;
- when the Steuerbox acknowledged it — the proof of execution that Ziff. 7.2 asks for.

Nothing about other connections appears.

## The monthly publication

```console
$ fms publish evidence.jsonl --connections connections.json --month 2026-01 [--plans plans.json] [--json]
```

The output is CSV in the BDEW field order: semicolon-separated, with decimal commas, and several
PLZ quoted in one field. Areas without control are not listed. The command warns about anything
it had to assume, most often the measure and its completion; pass those in `--plans`. It also
flags any field that breaks a format limit, such as more than 62 hours of präventive control.
