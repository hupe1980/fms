+++
title = "CLI reference"
description = "Every command of the fms binary."
weight = 8
+++

| Command | What it does |
|---|---|
| `fms days` | list the reference days |
| `fms sim <day\|all> [--seed N] [--json] [--out DIR] [--selection netzbereich\|downstream]` | run a day with and without the FMS; `--out` writes `evidence.jsonl`, `connections.json`, `report.json` |
| `fms floor [--mode ems\|direct] kind:kW…` | the Mindestleistung of one connection, with its clause; kinds are `heat-pump`, `wallbox`, `cooling`, `storage` |
| `fms verify LOG [--expect-head HASH]` | verify the chain, and that nothing was cut from its end |
| `fms explain LOG CONNECTION` | every reduction of one connection, with reason and acknowledgement |
| `fms publish LOG --connections FILE --month YYYY-MM [--plans FILE] [--json]` | the VNBdigital publication |
| `fms sr-id ID` | check an SR-ID, or complete ten characters with the check digit |

Colour is used only on a terminal and never when `NO_COLOR` is set. The exit code is 0 on
success, 1 when a check fails (a broken log, a floor violation in a simulation), and 2 on an
error. Input files above 1 GiB are refused before they are read.
