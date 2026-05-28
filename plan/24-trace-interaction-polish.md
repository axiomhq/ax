# Step 24 — Folds + `/` filter + service jump + yank/inspect

## Status

**Pending.** Requires step 23.

## Incremental outcome

Trace view becomes feature-complete for v1. Vim-style folds collapse
and expand subtrees. A `/`-driven filter narrows the visible tree to
spans whose name, service, or any attribute/resource value matches
the query — ancestors of matches stay visible so the structural
context is preserved. `gt` jumps to the next span on a different
service. `y` yanks the selected span as JSON. `:span json` opens
the inspect overlay (reusing the `tile_inspect_json` widget).

## User-visible improvement

### Folds

- `h` — collapse the selected node (its descendants disappear from
  the tree). Fold marker flips to `▾`.
- `l` — expand the selected node. Fold marker flips back to `▸`.
- `zM` — collapse everything. Cursor walks up to the highest
  collapsed ancestor so it stays visible.
- `zR` — expand everything. Scroll re-clamps.
- Visual: collapsed parents show `▾`, expanded parents `▸`, leaves
  `·`.
- Fold state is per-`TraceView`; switching to a different trace
  resets it.

### `/` filter

- `/` enters filter input (status-bar style, like vim search).
  Live-updates as the user types.
- Match rule: a row matches if its `name`, `service`, or **any**
  string-renderable value in `attributes` / `resource` contains
  the query (case-insensitive). Per user directive (turn 3): full
  attribute search is on, performance be damned for v1; we measure
  and trim later if needed.
- The tree shows matching rows **plus every ancestor of a
  matching row** so the structural context survives the filter.
  Non-matching siblings / descendants of non-matching nodes are
  hidden.
- `Enter` commits (filter stays active). `Esc` clears the filter.
- A filter status badge in the trace header reads `[/ <query>]`
  while active.
- Cursor moves to the first matching row on commit; clamps if no
  match.

### Navigation: `gt`

- `gt` jumps the cursor to the next tree row whose `service`
  differs from the currently-selected row's service. `gT` jumps
  backward. Wraps at the ends.
- Useful for "show me where the request crosses service
  boundaries" — the most common ask on a multi-service trace.

### Yank + inspect

- `y` yanks the selected span as JSON onto the existing app-wide
  yank register (`App.yank`), with `linewise = false`. Same
  register the editor uses, so `p` in editor mode pastes the JSON
  there.
- `:span json` opens an overlay containing the span's JSON
  (pretty-printed). Reuses `App.tile_inspect_json` storage and
  the `overlays::draw_tile_inspect_overlay` widget verbatim — any
  key dismisses, as today.
- The JSON shape is the same dict the decoder built (typed core
  + `attributes` map + `resource` map + `events` list), not the
  raw 118-column row. The user wants the model, not the wire.

## Scope

### Add

- `TraceView.collapsed: HashSet<usize>` (span_idx of collapsed
  parents) and `TraceView.filter: Option<String>`.
- A computed `visible_rows: Vec<usize>` per render — indices into
  `model.tree` — built by walking `tree` and skipping rows whose
  any ancestor is in `collapsed`, then intersecting with the
  filter's match set + ancestors.
- Helper `ensure_cursor_visible(&mut self)`: after collapse, walk
  up the ancestor chain from the cursor and snap it to the
  highest collapsed ancestor.
- Helper `reclamp_scroll(&mut self, viewport_h: u16)`: after
  expand/collapse-all or filter change, ensure `scroll ∈ [0,
  max(0, visible_rows.len() - viewport_h)]`.
- Filter match function with a documented predicate (recursive
  walk over the span's typed core + attribute/resource map values
  converted via `serde_json::Value::to_string`-style render; pre-
  computed *index* per span built at first filter activation so
  typing more chars is O(n) over the prior match set, not O(n)
  over the full span set).
- Filter input handling in `src/app/keys/trace.rs`: a small modal
  state (`TraceInputMode::Normal | Filter`) so `/` enters input
  mode, every char appended to `filter`, `Backspace` shortens,
  `Esc` cancels, `Enter` commits.
- `gt` / `gT` motion using the `g_prefix` state machine in
  `command.rs` — but scoped to trace mode so it doesn't collide
  with the editor's future `gd`/`gt`/`gT` (the editor's
  motion parser already gates by pane).
- `y` and `:span json`: yank handler uses `serde_json::to_string_pretty`
  on a `SpanJson` view struct (a `#[derive(Serialize)]` projection
  of `Span` that flattens the maps).

### Edge cases the implementation must handle

These came out of the architectural review (turn 2, oracle); each
gets its own test.

1. **Cursor inside collapsed subtree.** After `zM` or after
   collapsing an ancestor via `h` while the cursor was on a
   descendant, the cursor lands on the deepest still-visible
   ancestor.
2. **Filter clears cursor target.** If the filter hides the
   current row, cursor snaps to the first visible row; status
   bar mentions "filter hid cursor row".
3. **Filter typed slowly.** Filtering on `auth` then `authz`
   doesn't re-scan from scratch — narrowed match set is reused.
4. **`gt` on a single-service trace.** No motion; status reads
   "single service".
5. **Collapsed subtree contains the filter match.** Matches are
   *not* auto-expanded; the user sees only the visible ancestor
   chain. Add `zv` (vim's "open just enough folds to see the
   cursor") as a small bonus: `zv` expands every ancestor of the
   selected row.
6. **Scroll past end after `zM`.** Reclamped to last visible
   row.

### Keep simple

- No regex in `/`; substring only. Power users can run `:trace
  <id>` against a richer APL query themselves.
- Fold persistence: dropped on trace swap. Not worth a cache for
  v1.
- Yank goes to a single register (same as editor). No `"a..z`.

## Tasks

1. `TraceView.collapsed` + `TraceView.filter` + `TraceInputMode`.
2. `visible_rows` builder + `ensure_cursor_visible` +
   `reclamp_scroll`. Unit tests for each edge case above.
3. Fold key handlers (`h`, `l`, `zM`, `zR`, `zv`) in
   `src/app/keys/trace.rs`.
4. Filter input mode + status badge in the trace header (extend
   `src/ui/trace.rs`).
5. Filter match function + per-span pre-rendered "search blob"
   built lazily on first `/`. Property test: any row matched by
   filter `X` is also matched by every prefix of `X`.
6. `gt`/`gT` motion via the `g_prefix` parser, pane-gated.
7. `y` yank + `:span json` overlay wiring.
8. Tests:
   * Each edge case from the list above.
   * Filter property: `filter("foo").is_match(row) =>
     filter("f").is_match(row)`.
   * `gt` cycles through services in DFS order; on a 7-service
     trace from the fixture set, `gt` 7 times returns to start.
   * `y` writes a parseable JSON span; `:span json` overlay
     opens and dismisses.

## Acceptance criteria

- All v1 keybindings from the trace view spec work and pass
  tests.
- `/` filter on the 1,498-span fixture filters in <50ms per
  keystroke on the developer machine; document actual numbers in
  Confirmed.
- Folds + filter compose: filtering inside a collapsed subtree
  still produces a correct visible-rows list.
- `:span json` overlay matches the existing tile-json overlay
  visually.
- No new clippy warnings; all step-23 criteria still pass.

## Verification

- `cargo fmt`
- `cargo clippy --all-targets`
- `cargo llvm-cov`
- Manual against staging: pick the 1,498-span fixture, exercise
  every new binding, including `/` with multi-character
  refinement and full-attribute matches like `/http.status_code`.

## Files touched

- `src/trace.rs` — `visible_rows` builder; filter match function.
- `src/ui/trace.rs` — fold markers, filter badge, filter input
  line.
- `src/app/keys/trace.rs` — `h/l/zM/zR/zv`, `/`, `gt/gT`, `y`.
- `src/app/ex_cmds.rs` — `:span json`.
- `src/command.rs` — register `gt/gT` as trace-pane motions.
- `docs/keys.md` — full trace-view keymap reference.

## Out of scope

- Picker — step 25.
- `gd`-style "jump from client to matching server span" — backlog.
- Saving filter as a named view — backlog.
- Mouse / click-to-select — backlog.

## Confirmed (during implementation)

* **Search blob cache lives on `TraceView`** (not `TraceModel`).
  Built lazily on first `/` so traces that are never filtered
  don't pay the build cost; reused for the lifetime of the
  `TraceView` so subsequent keystrokes are pure substring scans.
  Trace swap = fresh `TraceView` = fresh cache; no
  invalidation logic needed.
* **Filter narrows incrementally** via `filter_push_char` — each
  appended character re-scans only the prior match set
  (`Vec<usize>` of span_idx). `Backspace` widens by re-scanning
  the full span list because shrinking a substring query can
  re-admit previously-rejected spans.
* **Cursor stays in `model.tree` index space.** `visible_rows()`
  is computed each render (and each motion key) by
  `crate::trace::visible_rows`. The renderer + keymap both call
  it; on a 1,498-span fixture this is comfortably inside the
  per-frame budget (the perf smoke test from step 23 stays
  green at `<10ms/frame` debug-build with the new path).
* **`gt` / `gT` use the existing `App.table_pending_g`** two-step
  latch instead of plumbing trace-pane motions through
  `command.rs::g_prefix`. The editor parser stays editor-only.
* **Folds inside a collapsed subtree stay hidden** even when the
  filter would match a descendant. `zv` is the explicit
  opener (vim-shape).
* **`:span json` re-uses `App.tile_inspect_json`** + the existing
  `overlays::draw_tile_inspect_overlay` widget. The global key
  dispatcher in `keys/mod.rs` already dismisses on any key.
* **`y` writes charwise to `App.yank`** so `p` in the editor
  splices the JSON at the cursor (vs. opening a new line).
  Single register, shared with the editor's text yank.
* **Pre-commit gate.** `cargo fmt` + `cargo clippy --all-targets`
  clean; `cargo llvm-cov`: 866 unit tests + 1 integration test
  pass. New code coverage: `src/trace.rs` 99.21% lines / 100%
  functions / 100% regions; `src/app/keys/trace.rs` 93.31% lines
  / 100% functions / 94.46% regions; `src/ui/trace.rs` 81.62%
  lines / 90.48% functions. Uncovered lines that remain are
  pre-existing step-22 fetch error branches and detail-pane `:`/
  `?` bindings carried over from step 23 — nothing new from
  this step is left uncovered.
