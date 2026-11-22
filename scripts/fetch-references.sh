#!/usr/bin/env bash
# Fetch the freely available reference material into $FMS_REFERENCE (default: a git-ignored
# directory in the checkout; the tests use the same default). Idempotent.
# Public material only: German statutes (official works), pandapower and SimBench (the
# power-flow oracle and benchmark grids), openKONSEQUENZ (read for scope). BNetzA decisions,
# BSI technical guidelines and BDEW formats change URL with each publication and VDE FNN
# documents are paid, so those are listed for manual download at the end.
# Nothing fetched here is ever committed or redistributed.
set -euo pipefail
cd "$(dirname "$0")/.."
R="${FMS_REFERENCE:-concepts/reference}"
mkdir -p "$R"/{law,bnetza,bsi,bdew,vde-fnn,eebus,connect-plus,field}

dl() { # url dest
  if [ -s "$2" ]; then echo "skip   $2"; return; fi
  if curl -fsSL --retry 3 --max-time 180 -o "$2" "$1"; then echo "ok     $2"; else echo "FAILED $2 ($1)" >&2; rm -f "$2"; fi
}
clone() { # url dest
  if [ -d "$2/.git" ]; then echo "skip   $2"; return; fi
  git clone --depth 1 --quiet "$1" "$2" && echo "ok     $2" || echo "FAILED $2 ($1)" >&2
}

# Statutes (gesetze-im-internet.de publishes a PDF per law).
dl https://www.gesetze-im-internet.de/enwg_2005/EnWG.pdf "$R/law/enwg.pdf"
dl https://www.gesetze-im-internet.de/messbg/MessbG.pdf  "$R/law/msbg.pdf"
dl https://www.gesetze-im-internet.de/eeg_2014/EEG_2023.pdf "$R/law/eeg-2023.pdf"
dl https://www.gesetze-im-internet.de/nav/NAV.pdf        "$R/law/nav.pdf"

# The oracle and the benchmark grids (D11).
clone https://github.com/e2nIEE/pandapower.git "$R/pandapower"
clone https://github.com/e2nIEE/simbench.git   "$R/simbench"

cat <<EOT

Manual (changing URLs, registration or paid) — see the REFERENCES index in the architecture notes:
  BNetzA BK6-22-300 Anlage 1 + Beschluss, BK8-22/010-A, BK6-20-059/-060   into $R/bnetza/
  BSI TR-03109-1 (+ Anlagen), TR-03109-5, TR-03109-6                       into $R/bsi/
  BDEW VNBdigital publication format, Universalbestellprozess, EDI@Energy  into $R/bdew/
  VDE FNN Lastenheft Steuerbox, Hinweis Netzzustandsermittlung (paid)      into $R/vde-fnn/
  EEBUS LPC / LPP use-case specifications (registration)                   into $R/eebus/
  openKONSEQUENZ repositories of interest                                  into $R/openkonsequenz/
EOT
