+++
title = "Library"
description = "Embed the engine: topology, connections, a rule set, and a loop."
weight = 9
+++

```rust,ignore
use fms::prelude::*;

let grid = GridSpec { v_nom_kv: 0.4, transformer, lines };          // e.g. from a CIM export
let mut engine = Engine::new(Topology::new(grid)?, connections, RuleSet::bk6_22_300())?;
let mut log = Log::new();

loop {
    let now = clock.now();
    log.extend(now, engine.step(Input::Measurements(meters.drain()), now));
    log.extend(now, engine.step(Input::Profiles(forecast.at(now)), now));
    for out in engine.step(Input::Tick, now) {
        if let Output::Command(c) = &out { transport.send(c)?; }
        if let Output::Release(r) = &out { transport.release(r)?; }
        log.append(now, out);
    }
    for ack in transport.acknowledgements() {
        log.extend(now, engine.step(Input::Ack { connection: ack.connection, msg: ack.msg }, now));
    }
}
```

- `Engine::new` refuses a rule set that breaks a legal bound, and master data that does not
  fit the topology.
- Every `Command` carries `limit ≥ floor`, an end time `valid_until`, and a `reason`.
- `engine.view()` gives the last cycle's transformer estimate and congestions for a display.
- `fms::evidence::{documentation, publication}` turn the log into reports.

The crate has two features, both on by default: `sim` (the simulator) and `cli` (the binary). The
API documentation is on [docs.rs](https://docs.rs/fms).
