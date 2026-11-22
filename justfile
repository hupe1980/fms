# fms — task runner (https://just.systems)
#
# `just` on its own lists every recipe.

set shell := ["bash", "-uc"]

# Keep in sync with `rust-version` in Cargo.toml and the msrv job in .github/workflows/ci.yml.
msrv := "1.88"

version := `sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1`

# 📋 List all recipes
default:
    @just --list

# ✅ Everything CI runs, in CI order (the oracle needs `just venv` once)
ci: fmt-check lint purity test oracle msrv deny doc site-check
    @echo "✅ all checks passed"

# 🎨 Format
fmt:
    cargo fmt --all

# 🎨 Fail if anything is unformatted
fmt-check:
    cargo fmt --all --check

# 🔍 Clippy, warnings as errors
lint:
    cargo clippy --all-targets --all-features -- -D warnings

# The engine takes time as an input and owns no socket, file or clock — that is what makes a
# reduction replayable from its inputs and the simulator the same program as production. Only
# the simulator and the CLI may touch the outside world.
#
# 🧊 Enforce "no I/O, no clock, no unsafe" in the core
purity:
    #!/usr/bin/env bash
    set -uo pipefail
    core="units time model rules grid state command engine dispatch evidence"
    fail=0
    for m in $core; do
        hits="$(grep -n -E 'SystemTime::now|Instant::now|Timestamp::now|Zoned::now|std::(fs|env|net|process|thread)|\bunsafe\b' "src/$m.rs" \
            | grep -vE ':[[:space:]]*(///|//!|//)' || true)"
        if [ -n "$hits" ]; then
            echo "❌ src/$m.rs reached for ambient state:" >&2
            echo "$hits" >&2
            fail=1
        fi
    done
    [ "$fail" -eq 0 ] && echo "🧊 pure: no clock, no I/O, no unsafe in the core"
    exit "$fail"

# 🧪 Every test, release mode (the reference days run several seeds), never stopping at the first failure
test:
    cargo test --release --all-features --no-fail-fast

# 🐍 A local Python with pandapower for the oracle (gitignored .venv)
venv:
    uv venv -q .venv
    uv pip install -q --python .venv/bin/python pandapower simbench

# ⚡ The estimator against pandapower's AC power flow — absence of pandapower is a failure
oracle:
    FMS_REQUIRE_PANDAPOWER=1 cargo test --release --test pandapower_oracle -- --nocapture

# 🦀 Build on the minimum supported Rust version
msrv:
    rustup toolchain install {{msrv}} --profile minimal --no-self-update >/dev/null
    cargo +{{msrv}} check --all-features

# 📜 Licences, advisories, sources
deny:
    cargo deny check

# 📚 API docs, warnings as errors
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features

# 🌐 Regenerate the site's data from the library (chart, reference days, floor vectors)
site-data:
    FMS_UPDATE_SITE=1 cargo test --release --test site

# 🌐 Fail if the site's data is stale, then build it
site-check:
    cargo test --release --test site
    cd site && zola build

# 🌐 Serve the site locally with live reload
site:
    cd site && zola serve

# 📦 The crate as crates.io would get it: its file list, then a build from only those files
publishable:
    cargo package --list --allow-dirty
    cargo publish --dry-run --allow-dirty

# 🍺 Render the Homebrew formula from fake checksums and syntax-check it — the tap job's rehearsal
formula:
    #!/usr/bin/env bash
    set -euo pipefail
    sums="$(mktemp)"
    for t in aarch64-apple-darwin aarch64-unknown-linux-musl x86_64-unknown-linux-musl; do
        printf '%064d  fms-v{{version}}-%s.tar.gz\n' 0 "$t" >> "$sums"
    done
    packaging/homebrew/formula.sh {{version}} "$sums" | ruby -c
    rm -f "$sums"

# 🗓  Run a reference day (or `all`) — e.g. `just sim cold-snap 3`
sim day="all" seed="1":
    cargo run --release --quiet -- sim {{day}} --seed {{seed}}

# 🔐 ThreatSpec gap check on every open feature with a threat model
threatspec:
    #!/usr/bin/env bash
    set -uo pipefail
    for m in specs/*/threat-model.yaml; do
        [ -e "$m" ] || continue
        d="$(dirname "$m")"
        echo "== $d"
        .specify/extensions/threatspec/scripts/bash/threatspec.sh --feature-dir "$d" check | tail -12
    done

# 🏷  Check that a release of the current version can be cut, then print the tag command
release-check: ci publishable formula
    #!/usr/bin/env bash
    set -euo pipefail
    grep -q "^## \[{{version}}\]" CHANGELOG.md || { echo "❌ CHANGELOG.md has no entry for {{version}}" >&2; exit 1; }
    if git rev-parse "v{{version}}" >/dev/null 2>&1; then echo "❌ tag v{{version}} exists" >&2; exit 1; fi
    echo "✅ ready: git tag -a v{{version}} -m 'v{{version}}' && git push origin v{{version}}"
