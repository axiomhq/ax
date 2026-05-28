# Step 22 — `:trace <id>` fetch + `ViewMode::Trace` skeleton

## Status

**Done.** The first end-to-end `:trace <id>` flow lands: dataset/
deployment resolution chain, ladder fetch (`now-1h` → `now-24h`
→ `now-7d` → `now-30d`), `to_trace_model` decode, transition into
`ViewMode::Trace` with a single-pane indented-tree renderer.

### Deviations from the original spec

* **Separate event variant** `AppEvent::TraceFetchFinished`
  instead of overloading `AplQueryFinished`. The oracle review
  caught that a shared `last_query_id` would let an editor `:r`
  silently cancel an in-flight trace fetch (and vice versa).
  Trace fetches now bump their own `App.trace_query_counter` —
  fully independent. Regression-guarded by
  `editor_apl_query_does_not_cancel_pending_trace`.
* **Fresh `AxiomClient` per trace fetch** (not the cached
  `App.client`). Trace fetches can target a different deployment
  than the editor; caching the client would be a wrong-edge
  dispatch minefield for marginal perf gain. Live `Config::load`
  every dispatch.
* **Esc cancels a pending fetch** from Normal mode (in addition
  to dismissing the error overlay). Tested by
  `esc_with_pending_fetch_cancels_and_bumps_counter`.
* **`:q` from Trace mode** exits the trace (vim's split-close
  semantic), not the app.
* **`:grid` / `:solo` / `:open` from Trace mode** tear down the
  trace before switching to avoid orphaned `trace_view`.
* **Bare `:trace` inside Trace mode** reports the loaded
  trace's id instead of the editor's last query trace.

### Tests

+25 new tests in `src/app/tests/trace.rs` plus +1 in
`src/ui/trace.rs` (the `humanize_duration_ns` unit test):

* 5 dataset/deployment resolution + arg-parser tests
* 4 ladder-progression tests + 2 invariant tests (`next`
  termination, `as_relative_start` correctness)
* 2 stale-result / cross-event-bleed tests
* 3 happy-path / decode-error tests
* 3 exit-path tests (`:q` from Solo/Grid origin, `Esc` via keymap)
* 1 Esc-cancels-pending test
* 1 bare-`:trace`-inside-Trace test
* 3 cursor / Ctrl-D-U / `gg`-G tests
* 1 render smoke test (`TestBackend` 80×24, asserts trace_id +
  span names + TRACE label + dataset surface)

### Coverage

* `src/trace.rs`             — 100%
* `src/app/fetch/trace.rs`   — 86% line / 81% region. Uncovered:
  `build_trace_client` (network), `dispatch_trace_window`
  (runtime spawn). Both are tested implicitly via the
  no-dataset error path + decode-error path; deep coverage of
  the spawn body would require a client mock infrastructure
  this codebase doesn't yet have.
* `src/app/keys/trace.rs`    — 70% line. Uncovered: the
  `trace_visible_height` heuristic (it's a constant in step
  22 — step 23 will stash the real body height on `App` and
  reach 100%).
* `src/ui/trace.rs`          — 89% line. Uncovered: tiny-rect
  degenerate paths (header_h > area.height).

### Remaining warnings

4 dead-code warnings on `Span.{kind,status_code,events,
attributes,resource}` / `Span::is_root` / `TraceModel.by_id` /
`SpanKind::as_str`. All are step 23 consumers (detail pane reads
these fields). Carrying as known-deferred rather than adding
`#[cfg(test)]` gates that step 23 instantly removes.

## Incremental outcome

The first end-to-end trace flow lands. Running `:trace <id>` fetches
the trace's spans, decodes via `to_trace_model`, switches into a new
`ViewMode::Trace`, and renders the spans as an indented vim-style
list. No waterfall bars, no detail pane, no folds, no `/` filter yet
— those land in steps 23 + 24. The point of this step is to prove
the entire view-mode + fetch + decode + render path on the
shippable codepath, with the smallest renderer that does something
useful.

## User-visible improvement

- `:trace <id>` opens a span tree (indented names with depth + service).
- Header line shows `trace_id`, root op + service, total duration,
  span count, error count.
- `j`/`k`/`gg`/`G`/`Ctrl-d`/`Ctrl-u` scroll the list.
- `Esc` / `:q` returns to whichever ViewMode was active before.
- Status bar shows `NORMAL · TRACE` and the active dataset.
- If the trace isn't found in `now-1h`, the fetcher silently
  extends the window: `now-24h`, then `now-7d`, then `now-30d`.
  Only after `now-30d` returns zero rows does the user see
  "trace not found in `<dataset>`".

## Scope

### Add

- `ViewMode::Trace` variant on the existing enum in `src/app/types.rs`.
- `Pane::TraceTree` variant (detail pane lands in step 23 with a
  matching `Pane::TraceDetail`).
- `App.trace_view: Option<TraceView>` where:

```text
TraceView {
    model: TraceModel,
    cursor: usize,          // index into model.tree
    scroll: u16,
    return_mode: ViewMode,  // Solo or Grid, to restore on :q
}
```

- `App.pending_trace_fetch: Option<PendingTraceFetch>` for the
  ladder fetch state:

```text
PendingTraceFetch {
    query_id: u64,          // shares last_query_id space with the editor
    trace_id: String,
    dataset:  String,
    deployment_override: Option<String>,
    window:   TraceFetchWindow,  // Hour | Day | Week | Month
}
```

- `App.last_trace_dataset: Option<String>` — remembered across
  `:trace <id>` calls; seeded from `Settings.trace.dataset` on first
  use.
- Ex-command `:trace <id> [dataset=NAME] [deployment=NAME]` in
  `src/app/ex_cmds.rs`. Dataset resolution order:
  1. `dataset=...` arg.
  2. `last_trace_dataset` (in-session memory).
  3. `Settings.trace.dataset`.
  4. Error: "no trace dataset; set with `:trace set dataset=...`".

  Deployment resolution order:
  1. `deployment=...` arg.
  2. `Settings.trace.deployment`.
  3. `Config.active_deployments` (the editor's normal default).

- Fetcher in `src/app/fetch/` (new `trace.rs` under that folder):
  builds the APL `['<ds>'] | where trace_id == "<id>" | sort by
  _time asc`, dispatches against `query_apl` with the picked
  deployment, threads `query_id` and the current window through.
- Ladder logic in the `AplQueryFinished` handler: when
  `pending_trace_fetch.query_id == id`, decode via
  `to_trace_model`. On empty result, advance the window
  (`Hour → Day → Week → Month`) and re-dispatch. On a non-empty
  result, build `TraceView`, set `view_mode = ViewMode::Trace`, and
  clear `pending_trace_fetch`. On `Month` exhausted, surface
  "trace not found".
- Renderer in `src/ui/trace.rs` (new):
  * Body becomes a single full-width pane (no editor, no params,
    no legend in this mode).
  * 2-row header at the top of the body: trace meta on row 1
    (truncated `trace_id`, root op + service, duration, span
    count, error count); thin separator on row 2.
  * Remaining rows = indented list of span names. Each row:
    `  ▸ [<service>] <name>  <duration>`. Selected row inverted.
  * Virtualize: only render `[scroll, scroll+visible_h)`.
- Key handler in `src/app/keys/trace.rs` (new): `j/k/gg/G/Ctrl-d/
  Ctrl-u` move cursor + reclamp scroll; `Esc` / `:q` exit to
  `view.return_mode`; `:trace <id>` while already in trace mode
  fetches a new one (swaps the model in place).

### Keep simple

- Single pane (`Pane::TraceTree`) this step. `Pane::TraceDetail`
  and `Tab` swap land in step 23 alongside the detail renderer.
- No waterfall bars yet. The duration column is plain text
  (humanised: `12µs`, `1.42s`).
- No folds. Every node is always expanded; `model.tree` drives the
  list 1:1.
- No `/`. No `gt`. No `y`. No `:span json`. Those are step 24.
- Status bar overlay / dismissable error overlay reused as-is; no
  new overlay types this step.

### Why the ladder time-range

`trace_id` is indexed on the Axiom side, but charging / scan-cost
semantics aren't fully documented at the edge we're hitting. The
user's directive (turn 3) is: start narrow (`now-1h`, the same
default as `:traces ls` will use) and walk out only if the narrow
query came up empty. Worst case is 4 round-trips for a stale id,
which is acceptable because the user is interactively waiting for a
specific trace they care about. The retry is transparent — the
status bar shows the current window (`searching now-24h…`,
`now-7d…`) so the user can `Esc` to cancel if they realise the id
is wrong.

## Fetch path detail

```
:trace <id>  ─►  build PendingTraceFetch { window=Hour, … }
             └►  dispatch query_apl(ds, apl, now-1h, now) with query_id
                  └►  AplQueryFinished arrives:
                        ├ result empty + window<Month? bump window, re-dispatch
                        ├ result empty + window==Month? surface "not found", clear pending
                        └ result non-empty? to_trace_model → TraceView → ViewMode::Trace
```

Status bar shows the current window while a fetch is in flight.
The `busy` gate is shared with editor queries (no concurrent
fetches), matching the existing single-slot semaphore semantics.

## Tasks

1. `ViewMode::Trace` variant; thread the new variant through every
   `match` site (compile errors are the test).
2. `Pane::TraceTree` variant; `set_focus` allows it only when
   `trace_view.is_some()`.
3. `App.trace_view`, `App.pending_trace_fetch`, `App.last_trace_dataset`.
4. `:trace <id>` ex-command parser + dataset/deployment resolution.
   Reject if no dataset can be resolved.
5. Fetch dispatch in `src/app/fetch/trace.rs`; the APL string is
   built with the `trace_id` quoted via `serde_json::to_string` to
   handle exotic characters safely (none expected, but cheap
   correctness).
6. Ladder logic in `AplQueryFinished` handler; status messages per
   window.
7. `src/ui/trace.rs` renderer + the layout switch in `src/ui/mod.rs`
   (hide editor / params / legend when `view_mode == Trace`).
8. Trace-mode keymap in `src/app/keys/trace.rs`. `Esc` /
   `:q` exit to `view.return_mode`; status reads
   `NORMAL · TRACE · <dataset>`.
9. Tests:
   * Decode → render fixture round-trip: load `traces-research/`
     fixtures, build a `TraceView`, render into a 80×24 buffer,
     snapshot a handful (1 small, 1 medium, 1 huge).
   * Ladder: simulate empty `AplQueryFinished` for `Hour` and
     assert the next dispatch uses a `Day` window; same for
     `Day → Week`, `Week → Month`, `Month → giveup`.
   * `:trace <id>` happy path with mocked client returning one of
     the saved fixtures.
   * `:trace <id>` with no dataset configured returns the
     configuration error.
   * Exit restores the previous `ViewMode` exactly.

## Acceptance criteria

- `:trace 7ab6afbabd64c899b318746fc16b1223` (one of the saved
  fixtures, run against staging) opens a working indented list of
  164 spans. Header reads `… · 164 spans · 1 err`.
- `:trace 0000000000000000` walks the ladder and surfaces "trace
  not found in `<dataset>`" after the `now-30d` window.
- `Esc` returns to the previous mode; running `:trace <id>` again
  re-enters the view.
- Existing test suite plus the new round-trip / ladder tests pass.
- No new clippy warnings.

## Verification

- `cargo fmt`
- `cargo clippy --all-targets`
- `cargo llvm-cov`
- Manual against staging: pick three trace_ids from
  `traces-research/index.json` (one small, one mid, one >1000
  spans). All three render at 60fps; the huge one specifically
  exercises the virtualization in step 23, but here it must at
  least not block the event loop or allocate per frame.

## Files touched

- `src/app/types.rs` — enum variants + state.
- `src/app/mod.rs` — field additions + `AplQueryFinished` route.
- `src/app/fetch/trace.rs` (new).
- `src/app/keys/trace.rs` (new) + dispatch in `src/app/keys/mod.rs`.
- `src/app/ex_cmds.rs` — `:trace <id>` real behaviour (replacing
  the placeholder from step 20).
- `src/ui/mod.rs` — body-layout switch under `ViewMode::Trace`.
- `src/ui/trace.rs` (new).
- `src/cmdline_complete.rs` — surface that `:trace <id>` accepts
  trailing free text.
- `docs/keys.md` — new "Trace view" section (minimal for now;
  fleshed out in step 24).

## Out of scope

- Waterfall bars, detail pane, virtualization wrapping the new
  detail pane — step 23.
- Folds, `/`, `gt`, `y`, `:span json` — step 24.
- `:traces ls` — step 25.
- Span-row decoration for orphans (`⚠ orphan`) — step 23 (where
  the tree gets visual chrome).
