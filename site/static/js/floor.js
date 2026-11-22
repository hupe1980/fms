// The Mindestleistung of BK6-22-300 Ziff. 4.5, in the browser. The same rule as fms::rules —
// and on every load it is checked against vectors the Rust engine generated, so the two cannot
// drift apart silently.
var fmsFloor = (function () {
  var MIN = 4.2, LARGE = 11, SHARE = 0.4, GZF = [1, 1, 0.8, 0.75, 0.7, 0.65, 0.6, 0.55, 0.5, 0.45];
  function gzf(n) { return GZF[Math.min(n, 9)]; }
  // Devices: [{kind: "heat-pump"|"cooling"|"charging-point"|"storage", kw}]. Returns SteuVE.
  function steuve(devices) {
    var out = [];
    ["heat-pump", "cooling"].forEach(function (k) {
      var sum = devices.filter(function (d) { return d.kind === k; }).reduce(function (s, d) { return s + d.kw; }, 0);
      if (sum > MIN) out.push({ kind: k, kw: sum });
    });
    devices.forEach(function (d) { if ((d.kind === "charging-point" || d.kind === "storage") && d.kw > MIN) out.push({ kind: d.kind, kw: d.kw }); });
    return out;
  }
  function thermal(k) { return k === "heat-pump" || k === "cooling"; }
  function floor(mode, devices) {
    var s = steuve(devices), n = s.length;
    if (n === 0) return null;
    if (mode === "direct") {
      var parts = s.map(function (d) { return thermal(d.kind) && d.kw > LARGE ? SHARE * d.kw : Math.min(MIN, d.kw); });
      var total = parts.reduce(function (a, b) { return a + b; }, 0);
      return { total: total, steuve: s, how: parts.map(function (p) { return p.toFixed(2); }).join(" + ") + " = " + total.toFixed(2) + " kW  [Ziff. 4.5.1]" };
    }
    var hp = s.filter(function (d) { return d.kind === "heat-pump"; }).reduce(function (a, d) { return a + d.kw; }, 0);
    var cool = s.filter(function (d) { return d.kind === "cooling"; }).reduce(function (a, d) { return a + d.kw; }, 0);
    var large = hp > LARGE || cool > LARGE;
    var base = large ? Math.max(SHARE * hp, SHARE * cool) : MIN;
    var total = base + (n - 1) * gzf(n) * MIN;
    var b = large ? "max(0,4 · " + hp.toFixed(1) + "; 0,4 · " + cool.toFixed(1) + ")" : MIN.toFixed(1);
    return { total: total, steuve: s, how: b + " + (" + n + " − 1) · " + gzf(n) + " · 4.2 = " + total.toFixed(2) + " kW  [Ziff. 4.5.2]" };
  }
  function num(id) { var v = parseFloat(document.getElementById(id).value); return isFinite(v) && v > 0 ? v : 0; }
  function render() {
    var d = [{ kind: "heat-pump", kw: num("hp") }, { kind: "heat-pump", kw: num("hp2") }, { kind: "charging-point", kw: num("wb") }, { kind: "charging-point", kw: num("wb2") }, { kind: "cooling", kw: num("cool") }, { kind: "storage", kw: num("bat") }].filter(function (x) { return x.kw > 0; });
    var mode = document.querySelector("input[name=mode]:checked").value;
    var f = floor(mode, d), out = document.getElementById("out"), how = document.getElementById("how"), list = document.getElementById("list");
    list.innerHTML = "";
    if (!f) { out.textContent = "no SteuVE"; how.textContent = "Nothing above 4.2 kW: § 14a does not apply to this connection."; return; }
    out.textContent = f.total.toFixed(2) + " kW";
    how.textContent = f.how;
    f.steuve.forEach(function (s) { var li = document.createElement("li"); var name = { "heat-pump": "heat pumps", "cooling": "cooling", "charging-point": "wallbox", "storage": "storage" }[s.kind];
      li.textContent = name + " · " + s.kw.toFixed(1) + " kW — one SteuVE"; list.appendChild(li); });
  }
  function boot(vectorsUrl) {
    document.getElementById("calc").addEventListener("input", render);
    render();
    var el = document.getElementById("selfcheck");
    fetch(vectorsUrl).then(function (r) { return r.json(); }).then(function (vs) {
      var bad = vs.filter(function (v) { var f = floor(v.mode, v.devices); var got = f ? Math.round(f.total * 1000) / 1000 : null; return got !== v.floor; });
      el.innerHTML = bad.length === 0
        ? "✓ This calculator agrees with the Rust engine on all " + vs.length + " test vectors it generated."
        : "✗ This calculator disagrees with the Rust engine on " + bad.length + " of " + vs.length + " vectors — please report it.";
      el.className = "selfcheck " + (bad.length === 0 ? "good" : "warn");
    }).catch(function () { el.textContent = ""; });
  }
  return { floor: floor, boot: boot };
})();
if (typeof module !== "undefined") module.exports = fmsFloor;
