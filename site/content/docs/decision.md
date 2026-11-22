+++
title = "Decision"
description = "One controller per asset, one depth for everyone, stepwise back — and an honest shortfall."
weight = 5
+++

## One depth, for everyone drawn on

When a cable or the transformer is congested, its controller computes one depth α from 0 to 1.
Every SteuVE it draws on may then take

```text
allowed = floor + (1 − α) · (installed − floor)
```

α = 0 is no reduction. α = 1 is every SteuVE at its Mindestleistung. The formula does not depend
on what a household happens to draw at that moment, which the DSO mostly cannot see. It treats a
heat pump and a wallbox alike. And it measures exactly the "installed − allowed" that the
publication reports.

α is solved each cycle against the *unconstrained* projection: the measured flow, plus what every
reduced household would draw back if released. The target is `trigger · rating · (1 − margin)`.
When several controllers want the same household, it takes the deepest.

## Back, step by step

α rises at once when the grid needs it. It falls at most once per 15-minute slot, by
`release_step` of what remains, and only if the asset would stay below
`trigger · rating · (1 − hysteresis)`. The last few per cent are released only with that margin
to spare. Each step is a `restore` command, and the last is a release.

## No mass switching

At most `max_switch_share` of a feeder's resources (default half) change state in one cycle.
Deeper reductions go first, because the grid comes first, then restores, then releases. The rest
waits for the next cycle, and a `Deferred` alarm says how many. A präventiv budget that has run
out is a legal limit: its release is never deferred.

## When the floors are not enough

If even α = 1 cannot keep an asset within its *rating*, the controller stays at the floors and
raises `Shortfall` with the kilowatts missing. It never cuts deeper. A month with an hour of
shortfall proposes *Netzausbau* in the publication. The `beyond-14a` day shows it.

## Operators

An operator's instruction is an input with an author and a note. It is floored like any other
command, held until the operator releases it, and recorded.
