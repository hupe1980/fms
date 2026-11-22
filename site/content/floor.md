+++
title = "Floor calculator"
description = "What a connection is owed under § 14a: the Mindestleistung of BK6-22-300 Ziff. 4.5, for one EMS or Direktansteuerung."
template = "floor.html"
+++

## How it is computed

- **Who counts.** Only a device above 4.2 kW is a SteuVE: a non-public charging point, a heat
  pump including its Heizstab, space cooling, or storage when it charges. Several heat pumps
  behind one connection form one SteuVE when their sum is above 4.2 kW, and so do several cooling
  units (Ziff. 2.4.2).
- **Direktansteuerung** (Ziff. 4.5.1). Each SteuVE is owed 4.2 kW. A heat pump or cooling above
  11 kW is owed 0.4 × its power instead.
- **One EMS** (Ziff. 4.5.2). The connection is owed 4.2 kW + (n − 1) · GZF · 4.2 kW. The
  Gleichzeitigkeitsfaktor runs 0.8, 0.75, 0.7 … down to 0.45 from nine SteuVE. When heat pumps or
  cooling above 11 kW are in the EMS, the first term is 0.4 × their summed power.

fms never sends a limit below this number. If the grid needs more than the floors give, it says
how many kilowatts are missing; it does not cut deeper. The same computation runs on the command
line:

```console
$ fms floor heat-pump:9 wallbox:11
$ fms floor --mode direct heat-pump:15
```
