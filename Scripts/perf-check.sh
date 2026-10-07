#!/bin/zsh
# Runs the scripted performance pass on a synthetic PR and checks budgets.
#   Scripts/perf-check.sh [small|large|huge]    (default: huge = 2,000 files / 200k lines)
# Requires a release bundle (Scripts/bundle.sh release). Exits 1 on a budget miss.
set -euo pipefail
cd "$(dirname "$0")/.."
SIZE="${1:-huge}"
OUT="$(mktemp -d)/perf.json"
APP="build/ReviewBuddy.app/Contents/MacOS/ReviewBuddy"
[ -x "$APP" ] || Scripts/bundle.sh release >/dev/null
"$APP" --synthetic "$SIZE" --perf-run "$OUT" --perf-quit >/dev/null 2>&1
python3 - "$OUT" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
# Budgets in milliseconds (release build, Apple silicon). 8.3 ms = one 120 Hz frame.
# Continuous interactions (scroll, fling) must fit in a frame; discrete actions (a click
# that opens a composer or collapses a file) get two 60 Hz frames — far under the
# ~100 ms at which a click stops feeling instant.
budgets = [
    ("loadMs",                lambda d: d["loadMs"],                  300),
    ("scroll p50",            lambda d: d["scrollUnified"]["p50"],    2.0),
    ("scroll p95",            lambda d: d["scrollUnified"]["p95"],    8.3),
    ("split scroll p95",      lambda d: d["scrollSplit"]["p95"],      8.3),
    ("fling p50",             lambda d: d["flingUnified"]["p50"],     8.3),
    ("fling p95",             lambda d: d["flingUnified"]["p95"],     16.7),
    ("jump to file p50",      lambda d: d["jumpToFileMs"]["p50"],     16.7),
    ("toggle split",          lambda d: d["toggleSplitMs"],           33.3),
    ("toggle unified",        lambda d: d["toggleUnifiedMs"],         33.3),
    ("add composer",          lambda d: d["addComposerMs"],           33.3),
    ("collapse file",         lambda d: d["collapseFileMs"],          33.3),
    ("memory MB",             lambda d: d["memoryMB"],                400),
]
print(f"{d['files']} files, {d['lines']} lines, {d['rows']} rows")
failed = False
for name, get, limit in budgets:
    v = get(d)
    ok = v <= limit
    failed |= not ok
    print(f"  {'✓' if ok else '✗'} {name:<18} {v:8.2f}  (budget {limit})")
sys.exit(1 if failed else 0)
PY
