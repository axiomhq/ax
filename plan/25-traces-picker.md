# Step 25 — `:traces ls` picker + per-dataset cache

## Status

**Pending.** Requires step 24.

## Incremental outcome

The last v1 entry surface lands. `:traces ls [<dataset>]` opens a
searchable picker over recent traces in the dataset — one row per
trace_id with root op, root service, span count, services count,
error flag, and start time. Selecting a row hands off to the
`:trace <id>` flow from step 22, which adopts the trace into the
existing `ViewMode::Trace`. Listings are cached per-dataset on disk
the same way dashboard listings are, so reopening the picker is
instant; a background refresh fires after the cached set is shown.

After this step the trace view has full v1 coverage: discovery
(`:traces ls`), direct open (`:trace <id>`), exploration, folds,
filter, span inspect. Steps 20–24 plus this one close out the
feature.

## User-visible improvement

- `:traces ls` — opens picker for the active trace dataset
  (`Settings.trace.dataset`).
- `:traces ls <dataset>` — opens picker for an explicit dataset
  override (and remembers it as `last_trace_dataset`).
- Picker layout mirrors `:dash ls`: fuzzy filter at the top,
  sortable list below, `Enter` selects, `Esc` dismisses.
- Each row shows: `time root_service root_name spans services
  errors duration`.
- Picker reads from cache on open (instant), kicks off a
  background refresh, and live-updates the list when the fresh
  response arrives — same `refresh_items` pattern as
  `DashboardPicker`.
- Default time window: `now-1h` per user directive (turn 3). The
  ladder-extend behaviour from step 22 only kicks in for `:trace
  <id>` (a specific id the user is hunting); the picker is for
  "what's been happening recently" and stays narrow.
- `:time` while the picker is open re-fetches the listing for the
  selected window.

## Scope

### Add

- `TracePicker` struct in `src/app/types.rs` cloning the shape of
  `DashboardPicker` (`visible`, `items`, `filter`, `cursor`,
  `dataset`).
- `TraceSummary` row type in `src/trace.rs`:
  `{ trace_id, root_name, root_service, spans, services, errors,
     t_start, t_end }`.
- Picker APL (run by a new `fetch::traces::list_for_dataset`):

```apl
['<dataset>']
| summarize
    root_name    = any(name),
    root_service = any(['service.name']),
    spans        = count(),
    services     = dcount(['service.name']),
    errors       = max(toint(is_error)),
    t_start      = min(_time),
    t_end        = max(_time)
    by trace_id
| order by t_start desc
| limit 200
```

`any()` is APL's "pick a representative non-null"; the root
filter (`isnull(parent_span_id)`) is omitted to keep the
listing query single-pass — a `where isnull(parent_span_id)
| project name, ['service.name'], trace_id | join ...` form
would be more correct but punishes the server. The summary
picks an arbitrary span's name and service, which empirically
(on `axiom-traces-dev`) is the root in well-formed traces.
If this proves misleading in real datasets, swap to a
`top 1 by iif(isnull(parent_span_id), 0, 1) asc` projection.

- Duration column rendered client-side as `t_end - t_start`
  (true wall-clock span, not `sum(duration)` which double-counts
  overlap).
- New `AppEvent::TracesListed { dataset: String, result:
  Result<Vec<TraceSummary>> }` (parallel to
  `DashboardsRefreshed`).
- Cache layer in `src/cache.rs`:
  * `set_trace_listing(dataset, Vec<TraceSummary>)`
  * `get_trace_listing(dataset) -> Option<Vec<TraceSummary>>`
  * On-disk format extends the existing discovery JSON with a
    `trace_listings: Map<String, Vec<TraceSummary>>` field —
    `#[serde(default)]` so old caches load.
- `:traces ls [<ds>]` ex-command + `:traces<Tab>` completion.
- Picker overlay renderer in `src/ui/overlays.rs` (a new
  `draw_traces_picker` next to `draw_dashboards_picker`,
  factored similarly).

### Keep simple

- Single sort order (descending `t_start`). Multi-column sort can
  layer in once we see real usage.
- No multi-select / batch open.
- No cross-trace columns (e.g. "session", "user") in v1 — those
  would need per-dataset schema awareness; the user can always
  drop to APL.
- Fuzzy filter targets `trace_id`, `root_name`, `root_service`
  (same fields the dashboard picker matches across).

### Picker / view interactions

- `Enter` on a picker row sets `view_mode = ViewMode::Trace` via
  the existing `:trace <id>` code path — the picker's selected
  row becomes a `pending_trace_fetch` with `dataset = picker.dataset`
  and the same APL fetch as step 22. The picker close fires as
  soon as the fetch is dispatched (matching `:dash ls` /
  `:open` semantics).
- `Esc` while the picker is open dismisses without affecting
  `view_mode`.
- The picker's `dataset` is recorded as `last_trace_dataset` on
  open so subsequent bare `:trace <id>` calls remember it.

## Tasks

1. `TraceSummary` + the summarize-style decoder in
   `viz::apl_decode::to_trace_summaries`. Unit tests against a
   small synthetic `AplQueryResult` containing 3 rows.
2. `fetch::traces::list_for_dataset` async fn.
3. `TracePicker` state + `open` / `refresh_items` / `selected`
   methods cloned from `DashboardPicker`.
4. Cache extension + migration test (load a step-19-era cache,
   confirm `trace_listings` defaults to empty without error).
5. Picker overlay renderer + key handler.
6. `:traces ls [<ds>]` ex-command + `:traces<Tab>` /
   `:traces ls <Tab>` completion (dataset names from cache).
7. `Enter` wiring: build the same `PendingTraceFetch` step 22
   uses; close picker; fetch fires.
8. Tests:
   * Picker cache hit → instant open with cached rows; bg refresh
     event updates them.
   * Picker cache miss → empty list shown immediately; fresh
     fetch populates.
   * `Enter` on a row dispatches a trace fetch with the picker's
     dataset (assert `pending_trace_fetch.dataset`).
   * `:traces ls` with no `Settings.trace.dataset` configured and
     no explicit arg fails with a clear error.
   * `:time` while picker open triggers a re-fetch for the new
     window.

## Acceptance criteria

- `:traces ls axiom-traces-dev` against staging shows ≥100
  recent traces; `Enter` on any row opens the trace view.
- Reopening the picker after a successful `:traces ls` is
  instant (cache); the bg refresh fires and live-updates the
  list without losing the cursor.
- `:traces ls` (no arg) uses `Settings.trace.dataset`; a missing
  setting surfaces the configuration error from step 20.
- `:traces<Tab>` completes to `ls`; `:traces ls <Tab>` completes
  to cached dataset names.
- Existing test suite plus the new picker tests pass.

## Verification

- `cargo fmt`
- `cargo clippy --all-targets`
- `cargo llvm-cov`
- Manual against staging: open `:traces ls`, scroll, filter,
  pick a row, return to picker via `:traces ls`, confirm the
  same cursor lands on the prior selection (or close-by).

## Files touched

- `src/trace.rs` — `TraceSummary` + decoder.
- `src/app/types.rs` — `TracePicker`.
- `src/app/mod.rs` — picker field + event handler.
- `src/app/fetch/traces.rs` (new) — list query.
- `src/app/ex_cmds.rs` — `:traces ls`.
- `src/cache.rs` — `trace_listings` map.
- `src/ui/overlays.rs` — picker overlay.
- `src/cmdline_complete.rs` — `:traces` sub-commands.
- `docs/keys.md` — short "Trace picker" subsection.

## Out of scope

- Per-row sparkline columns (latency vs time, error vs time).
- Pagination beyond the 200-row APL limit.
- Multi-dataset picker (showing traces from all configured
  trace datasets at once).
- Saved picker queries.
- Picker → multi-trace comparison view.

## Confirmed (during implementation)

(populate after the step lands: how the picker behaves on a
dataset with >200 traces in `now-1h` — does the user
realistically need pagination, or is `:time now-30m` enough?
What the real fetch latency for the picker query is.)
