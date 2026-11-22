"""AC power flow with pandapower for the fms oracle test.

Reads a feeder and its loads as JSON on stdin, writes line flows (kW, sending end) and bus
voltages (p.u.) as JSON on stdout. Balanced three-phase, unity power factor, slack at 1.0 p.u.
"""
import json
import math
import sys

import pandapower as pp

spec = json.load(sys.stdin)
net = pp.create_empty_network()
v = spec["v_nom_kv"]
buses = {}


def bus(name):
    if name not in buses:
        buses[name] = pp.create_bus(net, vn_kv=v, name=name)
    return buses[name]


root = spec["transformer"]["node"]
pp.create_ext_grid(net, bus(root), vm_pu=1.0)
lines = {}
for line in spec["lines"]:
    max_i_ka = line["rating"] / (math.sqrt(3) * v) / 1000.0
    lines[line["id"]] = pp.create_line_from_parameters(
        net, bus(line["from"]), bus(line["to"]), length_km=1.0,
        r_ohm_per_km=line["r_ohm"], x_ohm_per_km=1e-6, c_nf_per_km=0.0, max_i_ka=max_i_ka, name=line["id"])
for node, kw in spec["loads"].items():
    pp.create_load(net, bus(node), p_mw=kw / 1000.0, q_mvar=0.0)
pp.runpp(net, algorithm="nr", init="flat", tolerance_mva=1e-9)
out = {
    "lines": {name: float(net.res_line.at[i, "p_from_mw"] * 1000.0) for name, i in lines.items()},
    "voltages": {name: float(net.res_bus.at[i, "vm_pu"]) for name, i in buses.items()},
    "transformer": float(net.res_ext_grid.at[0, "p_mw"] * 1000.0),
    "losses": float(net.res_line.pl_mw.sum() * 1000.0),
    "load": float(net.res_load.p_mw.sum() * 1000.0),
}
json.dump(out, sys.stdout)
