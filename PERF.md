# Performance

Review Buddy is built to stay fast on very large pull requests. The reference workload is a
synthetic PR with **2,000 files, 200,000 diff lines and 212,080 layout rows**
(`--synthetic huge`), measured in a release build on Apple silicon.

## How to measure

```sh
Scripts/bundle.sh release      # build ReviewBuddy.app
Scripts/perf-check.sh huge     # scripted run against budgets; exits 1 on a miss
swift test --filter Performance   # parser / layout / highlighter budgets (debug build)
```

`--perf-run <out.json>` drives the real window: smooth scrolling (60 pt per frame with
an 8 ms gap between frames, like a 120 Hz trackpad scroll), flings (a full page per frame),
random jumps between files, split/unified toggles, adding a comment composer and
collapsing a file. Every step forces layout, display and a Core Animation commit, so the
numbers include rendering, not just model work. `Review ▸ Copy Performance Report` copies
in-app counters, and the code emits `os_signpost` intervals ("Points of Interest") for
Instruments.

## Budgets and current numbers (huge PR)

| Metric | Budget | Measured |
|---|---|---|
| Load: fetch (in-memory) + parse + layout | 300 ms | ~105 ms |
| Smooth scroll, per frame p50 / p95 | 2 / 8.3 ms | 0.6–0.8 / ~5 ms |
| Split-view scroll p95 | 8.3 ms | 4.7–6.2 ms |
| Fling (new page every frame) p50 / p95 | 8.3 / 16.7 ms | 3.7 / 7.7 ms |
| Jump to a random file p50 | 16.7 ms | ~11 ms |
| Toggle split ↔ unified (median of 5) | 33.3 ms | 13–18 ms |
| Open a comment composer on a visible line (median of 9) | 33.3 ms | ~25 ms (17–19 ms in isolation) |
| Collapse a file (median of 9) | 33.3 ms | 15–17 ms |
| Resident memory | 400 MB | ~250 MB |

Discrete actions are measured before the long scrolling phases. After about 20 s of
continuous heavy work the same code measured 1.5–2× slower, which is a machine-state effect
and not something the user sees on a single click.

Continuous interactions (scrolling and flinging) must fit in one 120 Hz frame. Discrete
actions (a click that opens a composer, collapses a file or switches the view) get two
60 Hz frames. That is far below the ~100 ms at which a click stops feeling instant.

## What made it fast

Each item was found with Time Profiler traces of the scripted run (`xctrace record`):

1. **Byte-level diff parser.** One pass with `memchr` over the raw bytes; lines are byte
   ranges into a single buffer, so a 200k-line diff allocates nothing per line (~75 ms
   in a *debug* build).
2. **Flat 16-byte row model** (`DiffLayout`). It is rebuilt in ~1 ms on every structural
   change, and split-view pairs are cached. Per-file "generated" detection is computed once:
   it used to be 13% of toggle time.
3. **Custom canvas instead of `NSTableView`.** With 212k variable-height rows, the table's
   per-row view and layer setup cost about 1 ms per new row. The canvas keeps a prefix-sum
   height index (binary search for row lookup) and draws rows into **512 pt bitmap tiles**:
   - Tiles render straight into a `CGContext` in the *window's color space* and become
     `layer.contents`. AppKit's draw → display-list → replay path re-rasterized each tile
     about 6 times, and an sRGB/display mismatch made Core Animation convert every tile
     (half of all frame time).
   - Off-screen overscan tiles render in idle run-loop turns, so a scroll frame normally
     draws nothing.
   - Translucent theme colors are pre-composited to opaque, because blended fills were 22%
     of tile rendering.
4. **Only the inline SwiftUI that is needed.** Threads, composers and AI findings are
   SwiftUI views created only near the viewport. The file header, sticky header, hunk rows,
   hover "+" and the file tree are drawn natively. A SwiftUI `List` diffing 2,500 rows on
   every selection change was 28% of jump time.
5. **Observation hygiene.** Values that change on every scroll (current file, the layout,
   view options) are `@ObservationIgnored` and pushed to the few interested views directly,
   so SwiftUI never re-evaluates the window while you scroll.
6. **Tiles that slide.** Tiles are free-floating strips, not a fixed grid. After a
   structural change, the canvas diffs the old and new row lists from both ends. Tiles above
   the change are kept, tiles below it move by the height delta without re-rendering, and
   only the tile spanning the change is redrawn. Inserting a composer or collapsing a file
   re-renders about one tile instead of all of them.
7. **Color-space-native drawing.** Colors are converted once into the tile bitmap's color
   space. Drawing sRGB colors into a Display P3 bitmap forced color matching on every fill
   and glyph run, and row fills were a third of tile rendering time.
8. **Fine-grained observation for inline views.** Each composer, and each thread's reply or
   edit composer, is observed through its own small observable cell. Opening one composer
   no longer re-renders every thread on screen.
9. **Prewarming.** One composer is built offscreen in idle time after the first render, so
   the first real "Comment" doesn't pay SwiftUI's one-time setup cost.
10. **Synchronous highlighting.** Highlighting is cheap (~0.1 ms per file), so tiles are
   colored on their first render. Earlier, async highlighting caused a second render of
   every visible tile. Only files over 4,000 lines are highlighted in the background.

## Gotchas when scripting runs

- **Put value-less flags last.** AppKit treats a bare word after a flag without a value
  (`--perf-quit jump`) as a document to open, and SwiftUI then never creates the main
  window, so the run hangs. Write `--perf-only jump --perf-quit`, not the reverse.
- `--ui-script "cmd;cmd;…" --snap-dir <dir>` drives the app and saves PNG snapshots with
  `cacheDisplay`. Liquid Glass areas (toolbar, sidebar material, inspector) don't capture
  that way. `snappanel` renders the Discuss panel offscreen instead, and `tree` dumps the
  view hierarchy with frames, which is how the "window stretched to 13,000 pt" layout bug
  was found.
- Representable scroll views must implement `sizeThatFits` and return the proposed size.
  Otherwise SwiftUI can size them to their document height. Never put `fixedSize(vertical:)`
  on long text in the window's own view tree: an early layout pass with a tiny width turns
  it into an enormous minimum height.
