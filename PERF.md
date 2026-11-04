# Performance

Review Buddy is built to stay fast on very large pull requests. The reference workload is a
synthetic PR with **2,000 files, 200,000 diff lines and 214,080 layout rows**
(`--synthetic huge`), measured in a release build on Apple silicon.

## How to measure

```sh
Scripts/perf-check.sh huge          # scripted run against budgets; exits 1 on a miss
cargo test -p review_buddy_core     # includes parser / layout / highlighter budget tests
```

`--perf-run <out.json>` drives the real window: smooth scrolling (60 pt per frame), flings
(900 pt per frame, a new page every frame), random jumps between files, split/unified
toggles, opening a comment composer and collapsing a file. Each sample is the time egui
spends building that frame on the CPU (layout, text shaping, hit testing and tessellation of
the visible rows). GPU work happens afterwards on the render thread and is not included.
`--perf-only scroll|jump|toggle|composer` runs a single phase.

## Budgets and current numbers (huge PR)

| Metric | Budget | Measured |
|---|---|---|
| Load: fetch (in-memory) + parse + split pairs + layout | 300 ms | ~33 ms |
| Smooth scroll, per frame p50 / p95 | 2 / 8.3 ms | 0.85 / 1.1 ms |
| Split-view scroll p95 | 8.3 ms | 1.2 ms |
| Fling (new page every frame) p50 / p95 | 8.3 / 16.7 ms | 1.3 / 2.1 ms |
| Jump to a random file p50 | 16.7 ms | 1.3 ms |
| Toggle split ↔ unified | 33.3 ms | ~3 ms |
| Open a comment composer on a visible line (median of 9) | 33.3 ms | ~1.7 ms |
| Collapse a file (median of 9) | 33.3 ms | ~1 ms |
| Resident memory | 400 MB | ~250 MB |

Continuous interactions (scrolling and flinging) must fit in one 120 Hz frame. Discrete
actions (a click that opens a composer, collapses a file or switches the view) get two
60 Hz frames, far below the ~100 ms at which a click stops feeling instant.

## What makes it fast

1. **Byte-level diff parser.** One pass over the raw bytes; lines are byte ranges into a
   single buffer, so a 200k-line diff allocates nothing per line.
2. **Flat 16-byte row model** (`DiffLayout`). It is rebuilt in a few milliseconds on every
   structural change, and split-view pairs are computed once off the UI thread and reused.
   Per-file "generated" detection is computed once per diff.
3. **Virtualized canvas.** The diff is one egui scroll area over a prefix-sum height index
   (binary search for row lookup). Only rows intersecting the viewport are laid out and
   painted; egui caches the shaped text of repeated lines between frames.
4. **Inline blocks are real widgets placed only near the viewport.** Threads, composers,
   Discuss markers and AI findings start with an estimated height; their measured height
   feeds back into the index on the next frame, and growth above the viewport is
   compensated so the content doesn't jump.
5. **Lazy syntax highlighting.** A single-pass table-driven lexer runs per file on first
   draw (~0.1 ms for a typical file); files over 4,000 lines are highlighted on a worker
   thread and appear plain until then.
6. **Nothing blocks the UI thread.** GitHub, git and Claude calls run on worker threads and
   hand their results back as small jobs applied between frames; a load generation counter
   drops results that a newer load has superseded.
