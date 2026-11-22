+++
title = "How it works"
description = "Four planes over one sans-IO core: grid state, decision, dispatch, evidence."
weight = 2
+++

```text
measurements ─▶ grid state ──posterior──▶ decision ──commands──▶ dispatch ─▶ MSB ─▶ Steuerbox
profiles     ─▶ (estimator)  mean + σ     (controllers) + reasons (tracker)  ◀─ acknowledgements
                                             │ every output
                                             ▼
                                          evidence ─▶ documentation · VNBdigital
```

## One engine, no I/O

`Engine::step(input, now)` takes one input and the time, and returns what it caused. It owns no
socket, thread, database or clock. Measurements, profiles, acknowledgements, a lost channel and
an operator's instruction are inputs. A `Tick` runs a control cycle. Because nothing hidden goes
in, a reduction can be replayed from its log. The simulator runs the very same engine that a
service would.

## The cycle

1. **Bookkeeping.** Unacknowledged commands are retried. After the last attempt the resource is
   *unknown*. Expired limits end.
2. **Grid state.** The estimator turns the fresh measurements and the profiles into a posterior
   over every connection. Flows and voltages follow, each with a standard deviation. See
   [Grid state](@/docs/grid-state.md).
3. **Decision.** Every cable and the transformer has a controller, and so does the worst
   voltage. A controller acts when its estimate is *determined* and crosses the trigger. It
   holds a depth from 0 to 1 that applies to every SteuVE it draws on. See
   [Decision](@/docs/decision.md).
4. **Changes.** Deeper reductions go out first, then stepwise restores, then releases. At most
   half of a feeder's resources change state in one cycle. Renewals keep end times ahead.
5. **Evidence.** Every output goes to the log. See [Evidence](@/docs/evidence.md).

## What the FMS believes is in effect

| State | Meaning | Planning assumes |
|---|---|---|
| Idle | no reduction | nothing |
| Commanded | sent, not yet acknowledged | the commanded limit |
| Confirmed | the Steuerbox acknowledged it | the commanded limit |
| Failsafe | channel lost; the EMS holds its failsafe value for 2–24 h | the failsafe value, then nothing |
| Unknown | never acknowledged | nothing — the conservative assumption for the grid |
