# Security policy

`fms` decides when heat pumps and wallboxes are curtailed. A defect can leave homes cold or let a
transformer trip, so security reports are taken seriously and handled first.

## Reporting a vulnerability

Report privately through
[GitHub's private vulnerability reporting](https://github.com/hupe1980/fms/security/advisories/new).
Please do not open a public issue. Include:

- the version;
- the input or scenario (an `fms sim` seed and day is ideal);
- what happened, and what should have happened.

You will get an answer within five working days. A fix and an advisory follow as soon as the
issue is understood.

## What counts

- A command below the Mindestleistung, or one without a reason.
- A reduction caused by input that should have been refused.
- An evidence log that verifies although it was changed.
- A panic or unbounded allocation on input from outside.
- Data of one connection appearing in another's documentation or in the publication.

## Supported versions

Before 1.0 only the latest minor release receives fixes.
