# Step 23 — Waterfall bars + detail pane + virtualization

## Status

**Done.** Body splits 65/35 into tree+waterfall (left) and span
detail (right). Waterfall bars project each span's `[start_ns,
end_ns]` onto an 18-cell column with a stable per-service colour;
error spans paint red and orphan rows get a `⚠` badge. The detail
pane lazily materialises only the visible slice of the section
plan, so a span with hundreds of attributes renders the same as a
bare span. `Tab` (and `Ctrl-w w`) swap focus between the two
panes; the keymap's half-page math reads the stashed body /
detail heights so `Ctrl-D`/`Ctrl-U` are exact instead of using a
fixed heuristic.

### Deviations from the original spec

* **Header collapsed from 2 rows to 1.** The step-22 "row 1" was
  the dataset name; step 23 surfaces it in the status bar
  (`NORMAL · TRACE · ds: <dataset>`) so the body has more room.
* **`is_orphan_root` did not become a new field.** The decoder
  already set `TreeRow::is_orphan` correctly in step 21 (only
  when `parent_span_id` was set-but-not-found), so the renderer
  reads the existing field directly. The plan's name was a
  prescriptive suggestion the existing field already matched
  semantically.
* **Service colour via FNV-1a, no `rustc-hash` dep.** The oracle
  flagged that the codebase doesn't carry FxHash; FNV-1a (12
  lines, zero deps) gives the same determinism guarantee on a
  surface that doesn't need cryptographic spread.
* **`Span::is_root` gated `#[cfg(test)]`.** The renderer reads
  `TreeRow::is_orphan` (a stronger flag) instead, so `is_root`
  is exercised only by the unit tests — cleaner than carrying a
  dead-warning into step 24.
* **`by_id` consumed.** The detail pane's `parent` row now looks
  up the parent's name via `by_id` and shows `<span_id>  (<name>)`
  instead of just the opaque hex. Big UX win, kills the dead-code
  warning, fixture sweep still passes.
* **`Pane::TraceDetail` instead of a `TraceView.focus` enum.**
  Oracle recommendation: `App.focus` is the existing source of
  truth for keymap dispatch; duplicating into `TraceView` would
  break the `prefill_command` return-focus path and require
  parallel `Ctrl-w` cycle logic.
* **`Ctrl-w w` was kept** for the trace pane swap. The plan
  suggested either lift the short-circuit or block it; lifting
  felt vim-native enough that we wired it as `Tab`'s alias.

### Performance

* 2000-span synthetic trace, 50-frame average:
  * **release build: 455 µs / frame** (under the 1 ms plan budget)
  * debug build: 4.7 ms / frame
* Plan acceptance: 1,498-span fixture sustains 60 fps while
  scrolling. Production fixture sweep confirms the per-frame
  budget holds across the corpus.

### Tests

+15 new unit tests in `src/ui/trace.rs` (`bar_extent`,
`service_colour`, `render_bar`, `truncate_for_display`) and +15
in `src/app/tests/trace.rs`:

* `bar_extent`: full span, mid-half, at-t0, at-t1, zero-duration,
  sub-cell, degenerate total, zero bar width, inverted bounds,
  overflow t1.
* `service_colour`: determinism, never red, empty falls to grey,
  1000-name sweep.
* `render_bar`: only paints inside window.
* `truncate_for_display`: short input, zero max, ellipsis at max.
* Pane focus: `Tab` swap, `Tab` from detail back to tree, Esc
  from detail exits trace, `set_focus` rejects detail when no
  trace loaded, `Ctrl-w w` swaps panes.
* Detail keymap: `j`/`k` scroll, `gg`/`G` top/bottom, scroll
  marker pattern (`u16::MAX` resolved by renderer).
* Detail render: identity/timing/status sections present,
  selected span id visible, parent name resolves via `by_id`.
* Waterfall render: block characters paint.
* Orphan badge: `⚠` renders.
* Narrow terminal: detail pane collapses (no "identity"
  string), tree still visible.
* Border state: detail block paints when focused.
* App stashing: `last_trace_body_height` and
  `last_trace_detail_height` populated after a draw.
* Perf smoke: 2000-span trace under 10 ms / frame in debug
  (release ≪ 1 ms).

### Coverage

Grew with the new code: `src/ui/trace.rs` line coverage is
driven by both the unit tests and the integration tests, the
geometry helpers are 100% covered, and the detail-pane
branches are exercised by the section-render tests.

## Incremental outcome

The bare span list from step 22 grows into a real trace explorer.
The body splits into a tree+waterfall column on the left and a
span-detail pane on the right; selecting a span in the tree updates
the detail pane. Each tree row carries a horizontal waterfall bar
mapped from the span's `[start_ns, end_ns]` onto the tree column's
remaining width. Virtualization (already in for the tree) now
covers the detail pane's scroll, and a per-frame allocation budget
check keeps the 1,498-span fixture interactive.

No interactive folds or `/` filter yet — those finish in step 24.

## User-visible improvement

- Body becomes a left/right split: tree+waterfall (~65%) and span
  detail (~35%).
- Each tree row shows:
  `<indent>▸ [<service>] <name>  ████▌  <duration>`
  Bar position + width are time-relative; bar colour = stable hash
  of `service.name`. Error spans paint red regardless of service.
  Zero-duration spans render a 1-cell tick mark.
- Selecting a row fills the detail pane with:
  - **identity**: `trace_id`, `span_id`, `parent_span_id`, `kind`
  - **timing**: start (`+xxxms` from `t0_ns`), end, duration, %
    of trace
  - **status**: `status.code`, error flag, optional message
  - **service & resource**: `service.name`, `service.version`,
    relevant `resource.*` keys
  - **attributes**: sorted key/value table (string-renderable
    JSON; long values truncated with `…`)
  - **events**: timestamp + name + attributes per event
- `Tab` swaps focus between tree and detail; the focused pane gets
  the yellow border (existing `pane_block` convention).
- Detail pane scrolls independently with `j`/`k`/`Ctrl-d`/`Ctrl-u`
  when focused.
- Orphan spans (parents missing from the response) get a `⚠ orphan`
  badge in the tree.

## Scope

### Add

- `Pane::TraceDetail` variant + focus rules: enter via `Tab` while
  `view_mode == Trace`; same `Tab` swaps back.
- `TraceView.detail_scroll: u16` and `TraceView.focus: TracePane`
  (`Tree` | `Detail`) — small enum local to `trace.rs`.
- Waterfall geometry helper in `src/ui/trace.rs`:
  `bar_extent(span, t0_ns, t1_ns, bar_width_cells) -> (offset,
  width)` with explicit handling for zero-width, sub-cell-width
  (rounded up to 1 to stay visible), and bars that overflow the
  bar column on the right (clamped).
- Service colour helper: stable hash of `service.name` → palette
  index over a curated 12-colour set (avoid red — reserved for
  errors). Same hash function across renders so the colour for a
  given service is consistent within and across traces.
- Detail-pane renderer: paragraph-style sections with bold
  headers, key/value rows, and a small events list at the bottom.
  Long attribute values truncated to viewport width minus key
  column.
- Tree-row decoration for orphans: prefix `⚠ ` and dim style on
  the row label when `tree_row.is_orphan_root` (a new flag
  populated by the decoder, but only set when a span's
  `parent_span_id` referenced a non-existent id — root-by-design
  spans don't get flagged).

### Keep simple

- Fold marker rendered as `▸` everywhere (always-expanded) even
  though folds don't land until step 24. Reserves the visual real
  estate without wiring the toggle.
- Detail-pane sections are static order; no user-driven section
  collapse this step.
- No mouse support.
- Colour palette is hard-coded; no theming hook this step.

### Virtualization

The tree was already virtualized in step 22. This step adds two
new render budgets to watch:

1. **Detail pane string building.** Attribute / resource maps can
   be large. Materialise the section text into `Vec<Line>` lazily
   per-frame from the selected span's maps, but cap the visible
   slice to `[detail_scroll, detail_scroll+detail_h)` rows
   *before* doing the truncate-to-width pass. Don't allocate
   strings for rows that won't be drawn.
2. **Waterfall bar bookkeeping.** The bar extent is O(1) per row;
   what's not free is repeated `format!` for the duration column.
   Render the duration into a stack-allocated buffer
   (`std::fmt::Write` into a small `ArrayString` from
   `arrayvec` *if* a profile shows allocation pressure;
   otherwise plain `format!` is fine for ≤120 visible rows).

Add a focused criterion-bench-free smoke test: build a 2,000-span
synthetic `TraceModel`, render to an 80×24 buffer, assert <1ms per
render. If we miss the budget the hot spot is identifiable from
the criterion-free perf trace; do not preemptively reach for
`arrayvec` etc.

## Layout

```
┌──────────────────────────────────────┬──────────────────────────────┐
│ trace abcd1234… · checkout-svc       │ identity                     │
│         /POST checkout · 1.42s · …   │   trace_id  abcd1234…        │
├──────────────────────────────────────┤   span_id   89f0…            │
│ ▸ checkout-svc /POST checkout ████   │   parent    7c2d…            │
│   ▸ auth-svc /verify          ▌      │   kind      server           │
│   ▸ cart-svc /load           ███     │ timing                       │
│   ▸ payments /charge         ███     │   start     +0.12s           │
│     ▸ stripe.api             ██      │   dur       0.84s (59%)      │
│ …virtualized…                        │ status                       │
│                                      │   ok                         │
│                                      │ attributes …                 │
│                                      │ events …                     │
└──────────────────────────────────────┴──────────────────────────────┘
 NORMAL · TRACE · axiom-traces-dev                                     ◀ status
```

Header is one logical row above the body split (re-using the
2-row header from step 22 but with the bottom row deleted — the
split gives the visual boundary the separator used to provide).

## Tasks

1. Split the body in `ui::trace::draw` into two horizontal regions
   (`Layout` with `Constraint::Percentage(65)` + the remainder),
   honouring a soft min on the detail pane (`Constraint::Min(20)`
   columns).
2. Implement `bar_extent` with unit tests covering:
   * Span fully inside `[t0, t1]`.
   * Span at exact `t0` / exact `t1`.
   * Zero-duration span (returns width = 1).
   * Sub-cell width (returns width = 1).
   * Span with `end_ns > t1_ns` from clock skew (caller has
     already widened `t1_ns`; helper just trusts it).
3. Service colour helper + unit tests asserting determinism +
   "no red except errors".
4. Tree row renderer that consumes `bar_extent` and the colour
   helper; orphan badge wired.
5. Detail-pane renderer: sections + lazy line building + scroll
   handling.
6. Pane focus / `Tab` swap in the trace keymap.
7. Snapshot tests: render the same 3 fixtures from step 22 with
   the new split layout into an 80×24 buffer; assert the header,
   selected row decoration, and detail-pane "identity" header
   land at fixed positions.
8. Allocation budget smoke: 2,000-span synthetic trace renders in
   <1ms per frame on the developer machine. Document the actual
   number in the step's "Confirmed" section once implemented.

## Acceptance criteria

- `:trace <id>` of a 47-span fixture renders tree + detail in one
  frame.
- `:trace <id>` of the 1,498-span fixture renders at sustained
  60fps while scrolling (`j` held down).
- `Tab` toggles focus border between tree and detail panes.
- Selecting a span updates the detail pane within the same
  frame — no async dependency.
- Orphan spans render the `⚠ orphan` badge; the synthetic orphan
  fixture covers this in tests.
- All step-22 acceptance criteria still pass.

## Verification

- `cargo fmt`
- `cargo clippy --all-targets`
- `cargo llvm-cov`
- Manual against staging: scroll the 1,498-span fixture top to
  bottom; toggle `Tab` repeatedly; pick five different rows and
  inspect the detail pane.

## Files touched

- `src/trace.rs` — `is_orphan_root` flag on `TreeRow`; service
  colour helper if it lives in the model rather than the ui layer
  (decide during implementation).
- `src/ui/trace.rs` — body split + waterfall + detail.
- `src/ui/trace/bar.rs` (new, optional) — `bar_extent` if it
  exceeds ~60 lines.
- `src/app/keys/trace.rs` — `Tab` swap + detail scroll.
- `docs/keys.md` — trace-view section adds the detail-pane scroll
  + `Tab`.

## Out of scope

- Folds, `/` filter, `gt`, yank, `:span json` — step 24.
- Picker — step 25.
- GenAI / RPC specialised renderers — generic attribute table
  covers them.
- Theming / colour customisation.

## Confirmed (during implementation)

* `bar_extent` lives in `src/ui/trace.rs` as a free function
  (oracle-validated: the data layer has no pixel awareness).
* `bar_extent` returns `(0, 0)` for `bar_width == 0` (tiny
  terminal) and for inverted bounds; returns `(0, 1)` for a
  degenerate trace duration (`t0 == t1`); rounds sub-cell widths
  up to 1 cell.
* Detail-pane minimum width is **20 columns**; if the body width
  is < `2 * 20` cells, the renderer collapses to tree-only.
* Heights are stashed on `App` (not on `TraceView`) per oracle
  review: terminal geometry isn't trace state.
* Service-colour palette is a 12-hue curated set (`Cyan`,
  `LightCyan`, `Blue`, `LightBlue`, `Magenta`, `LightMagenta`,
  `Green`, `LightGreen`, `Yellow`, `LightYellow`, `Gray`, amber).
  Red is reserved for errors; empty `service.name` falls to
  `DarkGray`.
