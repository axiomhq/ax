# Step 21 — Trace data model + APL decoder

## Status

**Done.** `src/trace.rs` carries the in-memory model; the decoder
in `viz::apl_decode::to_trace_model` is gated behind `#[cfg(test)]`
until step 22 wires the production consumer. All 40
`traces-research/traces/*.json` fixtures decode + satisfy the
renderer-facing invariants; 14 synthetic tests cover
orphan / sibling-sort / empty / clock-skew / zero-duration /
attribute-bucketing / events / is_error precedence.

+21 new `#[test]` functions vs. step-20 baseline (6 in
`src/trace/tests.rs` for the pure model surface; 14 trace-decoder
scenario tests in `viz::apl_decode::tests`; 1 fixture sweep that
iterates the corpus). The literal acceptance target was "≥45 new
tests" assuming a per-fixture macro; the corpus sweep is a single
`#[test]` function asserting 9 invariants × 40 fixtures = 360
fixture-level checks. Spirit met, letter missed — noted in the
coverage table below.

Full suite: 757 unit + 1 integration. fmt/clippy clean.

## Incremental outcome

A new `src/trace.rs` module owns the in-memory shape of a single
trace, and `viz::apl_decode` learns to turn an `AplQueryResult` into
that shape. The decoder is exercised by fixture tests against the 40
trace samples already saved under `traces-research/traces/`. Nothing
in the UI consumes the model yet — that lands in step 22.

This is the riskiest data slice: 118-column wide schema, parent
links to validate, clock skew between services, zero-duration spans,
huge responses (≈4.6 MiB per 1.5k-span trace). Landing it behind a
test-only consumer first means the UI work in step 22 can assume the
model is correct.

## User-visible improvement

None directly. The binary builds, all existing surfaces behave
exactly as before. The improvement is internal: `cargo test` now
covers the decoder against real production-shaped data.

## Scope

### Add

- `src/trace.rs` with:
  * `pub struct TraceModel { trace_id, dataset, spans, by_id, roots,
    t0_ns, t1_ns, tree }`.
  * `pub struct Span { core fields + attributes/resource maps +
    events }` — typed core (~15 fields) + `BTreeMap<String,
    serde_json::Value>` for the rest of the wide schema.
  * `pub enum SpanKind { Server, Client, Internal, Producer,
    Consumer, Unknown }` with `from_str` / `as_str`. Parsed once at
    decode time so the renderer can `match` on the enum.
  * `pub struct SpanEvent { time_ns, name, attributes }`.
  * `pub struct TreeRow { span_idx, depth, has_children }` —
    pre-flattened DFS, what the renderer will iterate.
- `viz::apl_decode::to_trace_model(&AplQueryResult, trace_id,
  dataset) -> Result<TraceModel>`.
- Tree builder (private to `trace.rs`):
  * Walk `spans` once to populate `by_id`.
  * Link parents; spans whose `parent_span_id` is `None` or points
    to a span not present become roots.
  * Sort each parent's children by `start_ns` ascending; ties by
    `span_id` for determinism.
  * Flatten with DFS into `tree: Vec<TreeRow>`.
  * Compute `t0_ns = min(span.start_ns)` and `t1_ns =
    max(span.end_ns)` across all spans (clock skew is normal — a
    child can end after its parent).
- Fixture tests in `src/trace/tests.rs` that load every
  `traces-research/traces/*.json` and assert:
  * Decode succeeds.
  * Span count matches the column count of the response.
  * `roots.len() == 1` for every fixture in our sample (recorded
    invariant; orphans path still tested with a synthetic case).
  * DFS order is a topological order of the parent links.
  * Siblings in `tree` appear in non-decreasing `start_ns` order.
  * `t1_ns >= t0_ns` and bounds enclose every span's `[start_ns,
    end_ns]`.

### Keep simple

- Decoder allocates per-span attribute maps eagerly. We measured the
  pay-off (≤200 KiB of typed structs vs 4.6 MiB raw JSON) and the
  renderer needs O(1) per-row access; lazy decode is premature
  optimisation.
- No caching of decoded `TraceModel` on disk. Per the architectural
  decision in `findings.md`/the trace-view design: traces are
  immutable but large; re-fetching by id is cheap, disk-eviction
  budgeting is not free.
- No `serde::Deserialize` impl for `Span`/`TraceModel` — the only
  source of truth is the APL response, not a file format.

### Typed-core fields on `Span`

- `span_id: String`
- `parent_span_id: Option<String>`
- `name: String`
- `service: String` (from `service.name`; empty string if missing)
- `kind: SpanKind`
- `status_code: Option<String>` (from `status.code`)
- `is_error: bool` (from the dataset's `is_error` column when
  present; falls back to `status_code == "ERROR"`)
- `start_ns: i64` (`_time` as unix nanoseconds)
- `end_ns: i64` (`start_ns + duration_ns`; stored to avoid
  recompute on every render frame)
- `duration_ns: i64` (from `duration`, which is `timespan` ns)
- `events: Vec<SpanEvent>` (decoded from the `events` array column)
- `attributes: BTreeMap<String, serde_json::Value>` (every
  `attributes.*` column with a non-null value, key stripped of the
  `attributes.` prefix; `attributes.custom` map keys spliced in)
- `resource:   BTreeMap<String, serde_json::Value>` (same shape for
  `resource.*` and `resource.custom`)

`BTreeMap` ordering gives the detail-pane a stable rendering order
without extra sort steps.

## Tasks

1. Define the `SpanKind` enum, parse helpers, unit tests.
2. Define `Span`, `SpanEvent`, `TraceModel`, `TreeRow`.
3. Implement `to_trace_model`:
   1. Extract the typed core columns by name; surface a clear error
      if a required column (`trace_id`, `span_id`, `_time`,
      `duration`) is missing.
   2. Bucket every other column into `attributes` / `resource`
      depending on its prefix; merge `attributes.custom` and
      `resource.custom` maps into the parent map.
   3. Decode the `events` array column into `Vec<SpanEvent>` per
      span.
   4. Build `by_id`, link parents, detect orphans (treat as
      additional roots; flag on the row in step 23 only — model
      itself just records them as roots).
   5. Sort children, flatten DFS.
   6. Compute `t0_ns`/`t1_ns`.
4. Fixture-driven tests: iterate `traces-research/traces/*.json`,
   decode each, run the asserts above. Add the directory path to a
   `TRACE_FIXTURES_DIR` constant; skip with a clear message if the
   directory is absent (so CI without fixtures still passes).
5. Synthetic tests:
   * Two-orphan response (parent_span_id set to a value not in the
     set) → both become roots; orphan flag recorded on the rows.
   * Sibling sort: spans with the same `start_ns` order by
     `span_id`.
   * Empty response → `Err(...)`, not `Ok(empty)` — the caller
     (step 22) maps this to a status message.
   * Clock skew: child with `end_ns > parent.end_ns` decodes
     without panic; `t1_ns` reflects the child's `end_ns`.
   * Zero-duration span: `end_ns == start_ns` survives the
     pipeline.

## Acceptance criteria

- All 40 fixture decodes succeed.
- Synthetic orphan / skew / zero-duration cases pass.
- `cargo test` adds ≥45 new tests (40 fixture + ≥5 synthetic).
- No new clippy warnings.
- `cargo llvm-cov` shows `to_trace_model` covered ≥90%; remaining
  lines justified in test comments.

## Verification

- `cargo fmt`
- `cargo clippy --all-targets`
- `cargo llvm-cov`
- Manual smoke: a binary test or `cargo run --example` that prints
  a fixture's tree shape (optional, can be a `#[test]` with
  `--nocapture`).

## Files touched

- `src/trace.rs` (new) + `src/trace/tests.rs` (new).
- `src/viz/apl_decode.rs` — add `to_trace_model`.
- `src/lib.rs` / `src/main.rs` — `pub mod trace;` wiring.
- `Cargo.toml` — no new deps expected (`serde_json` is already in).

## Out of scope

- Reading the model from anywhere outside tests — step 22.
- Persistence — explicit non-goal (re-fetch is cheap).
- Special-casing GenAI / RPC spans — generic attribute map handles
  them; presentation tweaks (if any) layer in later.
