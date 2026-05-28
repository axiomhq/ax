//! Decode an `axiom_rs::datasets::QueryResult` (APL response) into
//! the internal viz-layer shapes:
//!
//! * [`TableResult`] for the `Table` and `LogStream` viz kinds, plus
//!   a textual `(no data)` fallback for everything else when the
//!   response isn't series-shaped.
//! * `Vec<Series>` for the time-series kinds (Line / Bar / Area /
//!   Scatter / Pie / Statistic / TopList / Heatmap), built around an
//!   `_time` (or `// @viz line x=…`-named) column plus optional
//!   `groups` for series splitting.
//!
//! The column-major → row-major transpose and the `JsonValue`
//! → [`TableCell`] coercion live here. Per-viz interpretation
//! (which column is x, which is y) lives in [`to_series`], guided
//! by `// @viz <kind> x=… y=… series=…` opts and `Table.groups()`
//! when opts are absent.

use std::collections::BTreeMap;

use axiom_rs::datasets::{Field, QueryResult, Table};
use chrono::{DateTime, Utc};
use serde_json::Value as Json;

use super::table::{TableCell, TableResult};
use crate::chart::{Series, color_for};

/// Error variants returned by [`to_series`]. Surface in
/// `tile_results.error` so the tile shows a real, fixable message
/// (`expected an _time column, got [foo, bar]`) instead of the old
/// generic "APL (not yet executable)" placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AplDecodeError {
    /// `tables[]` was empty. APL queries that match no events can
    /// land here when the server returns a truly empty body.
    NoTables,
    /// No column qualifies as the x-axis (no `_time`, no
    /// `Table.buckets()`, no `// @viz x=…` opt, no `datetime`-typed
    /// column).
    MissingTimeColumn { available: Vec<String> },
    /// No numeric column qualifies as the y-axis after taking
    /// `// @viz y=…` opts and the x / group columns into account.
    MissingValueColumn { available: Vec<String> },
    /// `// @viz x=foo` or `y=foo` named a column that doesn't exist
    /// in the response.
    UnknownColumn { which: &'static str, name: String },
    /// A column required by the trace decoder is absent. Trace
    /// queries must produce `_time`, `trace_id`, `span_id`, and
    /// `duration`; missing any of these means the user almost
    /// certainly forgot the `| extend …` or `project-keep` clause
    /// for that field.
    MissingTraceColumn { name: &'static str },
    /// The trace response carried no spans. The trace view treats
    /// this as a hard error rather than rendering an empty tree;
    /// callers map it to a clear status ("trace not found").
    EmptyTrace,
}

impl std::fmt::Display for AplDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AplDecodeError::NoTables => f.write_str("APL response had no tables"),
            AplDecodeError::MissingTimeColumn { available } => {
                write!(
                    f,
                    "no time column found (need _time or // @viz x=<col>); available: [{}]",
                    available.join(", ")
                )
            }
            AplDecodeError::MissingValueColumn { available } => {
                write!(
                    f,
                    "no numeric value column found (need // @viz y=<col>); available: [{}]",
                    available.join(", ")
                )
            }
            AplDecodeError::UnknownColumn { which, name } => {
                write!(f, "// @viz {which}={name}: no such column in APL response")
            }
            AplDecodeError::MissingTraceColumn { name } => {
                write!(f, "trace decode: missing required column `{name}`")
            }
            AplDecodeError::EmptyTrace => f.write_str("trace decode: response had no spans"),
        }
    }
}

impl std::error::Error for AplDecodeError {}

/// Convert the first table in an APL response into a [`TableResult`].
/// Used for `// @viz table` and `// @viz log_stream` rendering, and
/// as a graceful fallback when [`to_series`] rejects a response that
/// isn't series-shaped (caller can show the raw table instead of a
/// hard error).
///
/// Empty responses yield `TableResult { columns: [], rows: [] }`;
/// the table renderer's `(no data)` path covers that case.
pub fn to_table_result(resp: &QueryResult) -> TableResult {
    let Some(table) = resp.tables.first() else {
        return TableResult {
            columns: Vec::new(),
            rows: Vec::new(),
        };
    };
    table_to_table_result(table)
}

fn table_to_table_result(table: &Table) -> TableResult {
    let fields = table.fields();
    let columns: Vec<String> = fields.iter().map(|f| f.name().to_string()).collect();
    let cols = table.columns();
    let row_count = cols.first().map(Vec::len).unwrap_or(0);
    let mut rows = Vec::with_capacity(row_count);
    for r in 0..row_count {
        let mut row = Vec::with_capacity(columns.len());
        for (i, _name) in columns.iter().enumerate() {
            let raw = cols.get(i).and_then(|c| c.get(r));
            let ty = fields.get(i).map(|f| f.typ().as_ref()).unwrap_or("");
            row.push(json_to_cell(raw, ty));
        }
        rows.push(row);
    }
    TableResult { columns, rows }
}

/// Coerce a single column cell into a [`TableCell`] using the field
/// type as a hint. Unknown / mismatched types fall back to a string
/// rendering of the JSON value so nothing is lost.
fn json_to_cell(v: Option<&Json>, ty: &str) -> TableCell {
    let Some(v) = v else {
        return TableCell::Null;
    };
    if v.is_null() {
        return TableCell::Null;
    }
    // Type-guided path first; it handles the typed-but-string-encoded
    // wire forms (datetime strings, large integers as strings).
    match ty {
        "datetime" | "date" => {
            if let Some(s) = v.as_str()
                && let Some(ms) = parse_rfc3339_to_ms(s)
            {
                return TableCell::Time(ms);
            }
            if let Some(n) = v.as_i64() {
                return TableCell::Time(n);
            }
        }
        "integer" | "long" | "int" => {
            if let Some(n) = v.as_i64() {
                return TableCell::Int(n);
            }
            if let Some(s) = v.as_str()
                && let Ok(n) = s.parse::<i64>()
            {
                return TableCell::Int(n);
            }
        }
        "real" | "float" | "double" | "number" => {
            if let Some(f) = v.as_f64() {
                return TableCell::Float(f);
            }
        }
        "boolean" | "bool" => {
            if let Some(b) = v.as_bool() {
                return TableCell::Bool(b);
            }
        }
        "string" => {
            if let Some(s) = v.as_str() {
                return TableCell::Str(s.to_string());
            }
        }
        _ => {}
    }
    // Fallback: inspect the JSON variant directly. Catches columns
    // whose declared `FieldType` we don't recognise (the SDK accepts
    // anything; new types ship server-side over time).
    match v {
        Json::Bool(b) => TableCell::Bool(*b),
        Json::Number(n) => {
            if let Some(i) = n.as_i64() {
                TableCell::Int(i)
            } else if let Some(f) = n.as_f64() {
                TableCell::Float(f)
            } else {
                TableCell::Str(n.to_string())
            }
        }
        Json::String(s) => TableCell::Str(s.clone()),
        Json::Null => TableCell::Null,
        // Arrays / objects come from things like `make_list()` or
        // bag aggregations; surface their JSON encoding so the user
        // at least sees the value.
        other => TableCell::Str(other.to_string()),
    }
}

fn parse_rfc3339_to_ms(s: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc).timestamp_millis())
}

/// Build a `Vec<Series>` from the first table of an APL response.
///
/// Auto-detection rules (overridable per [`SeriesOpts`]):
///   * **x-axis** \u2014 the explicit `x=` opt; else the bucket field
///     from `Table.buckets()`; else a column named `_time`; else
///     the first column whose declared type is `datetime`.
///   * **y-axis** \u2014 the explicit `y=` opt; else the first numeric\n///     column that isn't the x-axis or a group column.
///   * **series split** \u2014 the explicit `series=` opt (one column
///     name); else every column listed in `Table.groups()`.
///
/// Errors with [`AplDecodeError`] when no time column / numeric
/// value column can be found, or when an explicit opt names a\n/// missing column. Callers can render the underlying [`TableResult`]\n/// as a fallback (see [`to_table_result`]) when this returns `Err`.
pub fn to_series(
    resp: &QueryResult,
    opts: &BTreeMap<String, String>,
) -> Result<Vec<Series>, AplDecodeError> {
    let Some(table) = resp.tables.first() else {
        return Err(AplDecodeError::NoTables);
    };
    let fields = table.fields();
    let names: Vec<&str> = fields.iter().map(|f| f.name()).collect();

    // x-axis resolution.
    let x_name_opt = opts.get("x").cloned();
    let x_idx =
        match x_name_opt {
            Some(name) => names.iter().position(|n| *n == name).ok_or_else(|| {
                AplDecodeError::UnknownColumn {
                    which: "x",
                    name: name.clone(),
                }
            })?,
            None => find_time_column(table).ok_or_else(|| AplDecodeError::MissingTimeColumn {
                available: names.iter().map(|s| s.to_string()).collect(),
            })?,
        };

    // Series split: groups by default, single column from opts otherwise.
    let group_idxs: Vec<usize> = match opts.get("series").cloned() {
        Some(name) => {
            let i = names.iter().position(|n| *n == name).ok_or_else(|| {
                AplDecodeError::UnknownColumn {
                    which: "series",
                    name: name.clone(),
                }
            })?;
            vec![i]
        }
        None => table
            .groups()
            .iter()
            .filter_map(|g| names.iter().position(|n| *n == g.name()))
            .collect(),
    };

    // y-axis resolution.
    let y_name_opt = opts.get("y").cloned();
    let y_idx =
        match y_name_opt {
            Some(name) => names.iter().position(|n| *n == name).ok_or_else(|| {
                AplDecodeError::UnknownColumn {
                    which: "y",
                    name: name.clone(),
                }
            })?,
            None => find_first_numeric_other_than(table, x_idx, &group_idxs).ok_or_else(|| {
                AplDecodeError::MissingValueColumn {
                    available: names.iter().map(|s| s.to_string()).collect(),
                }
            })?,
        };

    let cols = table.columns();
    let row_count = cols.first().map(Vec::len).unwrap_or(0);
    if row_count == 0 {
        return Ok(Vec::new());
    }

    // Group rows into series keyed by the group columns' values.
    // `BTreeMap` for stable iteration order so series colours and
    // legend ordering are deterministic across reruns.
    let mut buckets: BTreeMap<Vec<String>, Vec<(f64, f64)>> = BTreeMap::new();
    let mut tags_per_key: BTreeMap<Vec<String>, Vec<(String, Json)>> = BTreeMap::new();
    for r in 0..row_count {
        let x = match column_value_as_x(cols, fields, x_idx, r) {
            Some(v) => v,
            None => continue,
        };
        let y = match column_value_as_y(cols, y_idx, r) {
            Some(v) => v,
            None => continue,
        };
        let key: Vec<String> = group_idxs
            .iter()
            .map(|&i| {
                cols.get(i)
                    .and_then(|c| c.get(r))
                    .map(json_text)
                    .unwrap_or_default()
            })
            .collect();
        // Stash original Json values as tags the first time we see
        // a key — useful for the legend + tag picker downstream.
        tags_per_key.entry(key.clone()).or_insert_with(|| {
            group_idxs
                .iter()
                .map(|&i| {
                    let name = names[i].to_string();
                    let v = cols
                        .get(i)
                        .and_then(|c| c.get(r))
                        .cloned()
                        .unwrap_or(Json::Null);
                    (name, v)
                })
                .collect()
        });
        buckets.entry(key).or_default().push((x, y));
    }

    // Build Series objects with palette-assigned colours.
    let y_name = names[y_idx].to_string();
    let mut out = Vec::with_capacity(buckets.len());
    for (i, (key, mut points)) in buckets.into_iter().enumerate() {
        points.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let tags = tags_per_key.remove(&key).unwrap_or_default();
        let name = if key.is_empty() {
            y_name.clone()
        } else {
            format!("{y_name} {{{}}}", key.join(","))
        };
        out.push(Series {
            name,
            tags,
            points,
            color: color_for(i),
        });
    }
    Ok(out)
}

fn find_time_column(table: &Table) -> Option<usize> {
    let fields = table.fields();
    // Prefer the bucket field, when the query used `bin(_time, …)`.
    if let Some(b) = table.buckets() {
        let bname = b.field();
        if let Some(i) = fields.iter().position(|f| f.name() == bname) {
            return Some(i);
        }
    }
    if let Some(i) = fields.iter().position(|f| f.name() == "_time") {
        return Some(i);
    }
    // Any datetime-typed column.
    fields
        .iter()
        .position(|f| matches!(f.typ().as_ref(), "datetime" | "date"))
}

fn find_first_numeric_other_than(
    table: &Table,
    x_idx: usize,
    group_idxs: &[usize],
) -> Option<usize> {
    let fields = table.fields();
    let cols = table.columns();
    let is_excluded = |i: usize| i == x_idx || group_idxs.contains(&i);
    for (i, f) in fields.iter().enumerate() {
        if is_excluded(i) {
            continue;
        }
        let by_type = matches!(
            f.typ().as_ref(),
            "real" | "float" | "double" | "number" | "integer" | "long" | "int"
        );
        // Some servers don't tag the type strictly; sample the first
        // non-null cell to confirm it is numeric.
        let by_value = cols
            .get(i)
            .and_then(|c| c.iter().find(|v| !v.is_null()))
            .map(|v| v.is_number())
            .unwrap_or(false);
        if by_type || by_value {
            return Some(i);
        }
    }
    None
}

/// Pull a single x-axis value out of column `idx` row `r`, in unix
/// milliseconds. Strings are parsed as RFC3339; numeric values are
/// passed through as-is (assumed already milliseconds, matching the
/// MPL `MetricsSeries.start` convention).
fn column_value_as_x(cols: &[Vec<Json>], fields: &[Field], idx: usize, r: usize) -> Option<f64> {
    let v = cols.get(idx)?.get(r)?;
    if v.is_null() {
        return None;
    }
    let ty = fields.get(idx).map(|f| f.typ().as_ref()).unwrap_or("");
    match ty {
        "datetime" | "date" => {
            if let Some(s) = v.as_str() {
                return parse_rfc3339_to_ms(s).map(|n| n as f64);
            }
            v.as_f64()
        }
        _ => {
            if let Some(f) = v.as_f64() {
                Some(f)
            } else if let Some(s) = v.as_str() {
                parse_rfc3339_to_ms(s).map(|n| n as f64)
            } else {
                None
            }
        }
    }
}

fn column_value_as_y(cols: &[Vec<Json>], idx: usize, r: usize) -> Option<f64> {
    let v = cols.get(idx)?.get(r)?;
    if v.is_null() {
        return None;
    }
    v.as_f64().or_else(|| v.as_i64().map(|n| n as f64))
}

fn json_text(v: &Json) -> String {
    match v {
        Json::String(s) => s.clone(),
        Json::Null => String::new(),
        other => other.to_string(),
    }
}

// ============================================================
// Trace decoder — see [`crate::trace`] for the in-memory model.
// ============================================================

use crate::trace::{Span, SpanEvent, SpanKind, TraceModel, TreeRow};

/// Decode an APL response into a [`TraceModel`].
///
/// The response shape is the standard `axiom_rs::QueryResult`
/// (columnar `tables[0]`); we resolve each typed-core column by
/// name, bucket the wide tail into per-span `attributes` /
/// `resource` maps, link parents, sort children, DFS-flatten, and
/// compute the trace's `[t0_ns, t1_ns]` bounds.
///
/// The decoder is total against missing optional columns (e.g.
/// `parent_span_id` or `service.name`) — they degrade gracefully
/// to `None` / `""`. Only the four columns listed in
/// [`AplDecodeError::MissingTraceColumn`] are hard requirements.
///
/// `trace_id` and `dataset` are passed in (rather than re-read
/// from the response) because the caller already knows them —
/// the trace view is opened with `:trace <id>` and the active
/// dataset comes from settings/CLI — and the same response can in
/// principle carry multiple `trace_id` values (we ignore that
/// here: the caller is expected to filter).
pub fn to_trace_model(
    resp: &QueryResult,
    trace_id: String,
    dataset: String,
) -> Result<TraceModel, AplDecodeError> {
    let Some(table) = resp.tables.first() else {
        return Err(AplDecodeError::NoTables);
    };
    let fields = table.fields();
    let cols = table.columns();
    let row_count = cols.first().map(Vec::len).unwrap_or(0);
    if row_count == 0 {
        return Err(AplDecodeError::EmptyTrace);
    }

    // ---- Column index resolution ------------------------------------
    //
    // We look up every column by name once up front; per-row
    // access reuses these indices. Required columns surface as
    // `MissingTraceColumn`; optional ones are `None` and read as
    // "this span doesn't have that field".
    let idx_of = |name: &str| fields.iter().position(|f| f.name() == name);
    let req = |name: &'static str| idx_of(name).ok_or(AplDecodeError::MissingTraceColumn { name });

    let time_idx = req("_time")?;
    let span_id_idx = req("span_id")?;
    let duration_idx = req("duration")?;
    let name_idx = req("name")?;

    let parent_idx = idx_of("parent_span_id");
    let service_idx = idx_of("service.name");
    let kind_idx = idx_of("kind");
    let status_code_idx = idx_of("status.code");
    let is_error_idx = idx_of("is_error");
    let events_idx = idx_of("events");

    // Sort the "long tail" columns by prefix once. Each entry
    // records the column's source index, the key the value should
    // land under in the per-span map, and whether the source
    // column was the special `*.custom` map (which gets spliced
    // into the parent map rather than nested).
    let mut attr_cols: Vec<TailColumn> = Vec::new();
    let mut res_cols: Vec<TailColumn> = Vec::new();
    for (i, f) in fields.iter().enumerate() {
        let name = f.name();
        if let Some(key) = name.strip_prefix("attributes.") {
            attr_cols.push(TailColumn {
                idx: i,
                key: key.to_string(),
                is_custom_map: key == "custom",
            });
        } else if let Some(key) = name.strip_prefix("resource.") {
            res_cols.push(TailColumn {
                idx: i,
                key: key.to_string(),
                is_custom_map: key == "custom",
            });
        }
    }

    // ---- Per-row span construction ---------------------------------
    let mut spans: Vec<Span> = Vec::with_capacity(row_count);
    for r in 0..row_count {
        let span_id = read_string(cols, span_id_idx, r);
        if span_id.is_empty() {
            // A row without a span id can't be navigated to or
            // linked from; skip it rather than letting it pollute
            // `by_id` (the empty-string key would collide with
            // any other malformed row).
            continue;
        }
        let start_ns = read_time_ns(cols, time_idx, r);
        let duration_ns = read_duration_ns(cols, duration_idx, r);
        let parent_raw = parent_idx
            .map(|i| read_string(cols, i, r))
            .unwrap_or_default();
        let parent_span_id = if parent_raw.is_empty() {
            None
        } else {
            Some(parent_raw)
        };
        let service = service_idx
            .map(|i| read_string(cols, i, r))
            .unwrap_or_default();
        let kind = kind_idx
            .map(|i| read_string(cols, i, r))
            .map(|s| SpanKind::from_str(&s))
            .unwrap_or(SpanKind::Unknown);
        let status_code = status_code_idx.and_then(|i| {
            let s = read_string(cols, i, r);
            if s.is_empty() { None } else { Some(s) }
        });
        // `is_error` resolution: prefer the dedicated column; fall
        // back to status code semantics.
        let is_error = is_error_idx
            .and_then(|i| cols.get(i).and_then(|c| c.get(r)).and_then(Json::as_bool))
            .unwrap_or_else(|| status_code.as_deref() == Some("ERROR"));
        let events = events_idx
            .and_then(|i| cols.get(i).and_then(|c| c.get(r)))
            .map(decode_events)
            .unwrap_or_default();
        let attributes = collect_tail(cols, r, &attr_cols);
        let resource = collect_tail(cols, r, &res_cols);

        spans.push(Span {
            span_id,
            parent_span_id,
            name: read_string(cols, name_idx, r),
            service,
            kind,
            status_code,
            is_error,
            start_ns,
            end_ns: start_ns.saturating_add(duration_ns),
            duration_ns,
            events,
            attributes,
            resource,
        });
    }

    if spans.is_empty() {
        return Err(AplDecodeError::EmptyTrace);
    }

    // ---- Indices, parent linking, tree flatten ---------------------
    let mut by_id: BTreeMap<String, usize> = BTreeMap::new();
    for (idx, span) in spans.iter().enumerate() {
        // Last-write-wins on dup span_ids — the same span can in
        // principle appear twice if the user's APL didn't dedup;
        // we prefer the later row so the renderer at least sees
        // a consistent linkage.
        by_id.insert(span.span_id.clone(), idx);
    }

    // Build children adjacency. Orphans — spans whose parent_span_id
    // is set but not in `by_id` — join `roots` alongside the true
    // roots so the tree flattener visits them.
    let mut children: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut roots: Vec<usize> = Vec::new();
    let mut is_orphan_flags: Vec<bool> = vec![false; spans.len()];
    for (idx, span) in spans.iter().enumerate() {
        match span.parent_span_id.as_deref().and_then(|p| by_id.get(p)) {
            Some(&parent_idx) => children.entry(parent_idx).or_default().push(idx),
            None => {
                if span.parent_span_id.is_some() {
                    is_orphan_flags[idx] = true;
                }
                roots.push(idx);
            }
        }
    }

    // Sort roots + each child list by (start_ns, span_id) for
    // deterministic rendering across runs.
    let sort_key = |spans: &[Span], i: usize| (spans[i].start_ns, spans[i].span_id.clone());
    roots.sort_by_key(|&i| sort_key(&spans, i));
    for kids in children.values_mut() {
        kids.sort_by_key(|&i| sort_key(&spans, i));
    }

    // DFS flatten. Iterative to avoid blowing the stack on deep
    // traces (production traces with 50+ deep chains exist).
    let mut tree: Vec<TreeRow> = Vec::with_capacity(spans.len());
    // Stack entries: (span_idx, depth). Children are pushed in
    // reverse so the leftmost child is visited first.
    let mut stack: Vec<(usize, u16)> = Vec::with_capacity(roots.len());
    for &root in roots.iter().rev() {
        stack.push((root, 0));
    }
    while let Some((idx, depth)) = stack.pop() {
        let kids = children.get(&idx);
        tree.push(TreeRow {
            span_idx: idx,
            depth,
            has_children: kids.is_some_and(|v| !v.is_empty()),
            is_orphan: is_orphan_flags[idx],
        });
        if let Some(kids) = kids {
            for &kid in kids.iter().rev() {
                stack.push((kid, depth.saturating_add(1)));
            }
        }
    }

    // ---- Time bounds -----------------------------------------------
    //
    // Clock skew between services means we can't just take the
    // root's bounds; we min/max over every span. `i64::MAX` /
    // `i64::MIN` are safe sentinels because spans.is_empty() was
    // already rejected.
    let mut t0_ns = i64::MAX;
    let mut t1_ns = i64::MIN;
    for span in &spans {
        if span.start_ns < t0_ns {
            t0_ns = span.start_ns;
        }
        if span.end_ns > t1_ns {
            t1_ns = span.end_ns;
        }
    }

    Ok(TraceModel {
        trace_id,
        dataset,
        spans,
        by_id,
        roots,
        t0_ns,
        t1_ns,
        tree,
    })
}

/// One "long tail" column entry: bucketed once at decode-start
/// time, then read row-by-row inside the span loop.
struct TailColumn {
    idx: usize,
    /// Key into the per-span `attributes` / `resource` map. For
    /// the `*.custom` columns this is `"custom"` and is
    /// **ignored** (the map's keys are spliced into the parent
    /// map directly).
    key: String,
    is_custom_map: bool,
}

fn collect_tail(cols: &[Vec<Json>], row: usize, sources: &[TailColumn]) -> BTreeMap<String, Json> {
    let mut out: BTreeMap<String, Json> = BTreeMap::new();
    for tc in sources {
        let Some(v) = cols.get(tc.idx).and_then(|c| c.get(row)) else {
            continue;
        };
        if v.is_null() {
            continue;
        }
        if tc.is_custom_map {
            // `attributes.custom` / `resource.custom` is an
            // object whose keys land at the top level of the map.
            if let Json::Object(map) = v {
                for (k, sub) in map {
                    if !sub.is_null() {
                        out.insert(k.clone(), sub.clone());
                    }
                }
            }
            // Non-object `custom` values (shouldn't happen on a
            // well-formed response) are silently dropped: there's
            // no sensible key to put them under.
            continue;
        }
        out.insert(tc.key.clone(), v.clone());
    }
    out
}

fn decode_events(v: &Json) -> Vec<SpanEvent> {
    let Json::Array(arr) = v else {
        return Vec::new();
    };
    let mut out: Vec<SpanEvent> = Vec::with_capacity(arr.len());
    for entry in arr {
        let Json::Object(obj) = entry else {
            continue;
        };
        let name = obj
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_string();
        let time_ns = obj.get("timestamp").and_then(|t| t.as_i64()).unwrap_or(0);
        let attributes = obj
            .get("attributes")
            .and_then(|a| a.as_object())
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        out.push(SpanEvent {
            time_ns,
            name,
            attributes,
        });
    }
    // Events are presented in wall-clock order in the detail
    // pane; producers occasionally emit them out of order so we
    // sort once here rather than every render.
    out.sort_by_key(|e| e.time_ns);
    out
}

fn read_string(cols: &[Vec<Json>], idx: usize, row: usize) -> String {
    cols.get(idx)
        .and_then(|c| c.get(row))
        .map(json_text)
        .unwrap_or_default()
}

/// Convert an APL `_time` column cell (RFC3339 string with
/// nanosecond precision, e.g.
/// `"2026-05-21T00:28:28.181575627Z"`) into unix nanoseconds.
/// Falls back to integer parsing for the rare case where the
/// server hands us a raw `i64` instead of a string. Unparseable
/// values become `0` — the renderer treats that as the
/// epoch and the row sorts to the start, which is the safest
/// degenerate behaviour.
fn read_time_ns(cols: &[Vec<Json>], idx: usize, row: usize) -> i64 {
    let Some(v) = cols.get(idx).and_then(|c| c.get(row)) else {
        return 0;
    };
    if let Some(s) = v.as_str()
        && let Some(ns) = parse_rfc3339_to_ns(s)
    {
        return ns;
    }
    v.as_i64().unwrap_or(0)
}

fn parse_rfc3339_to_ns(s: &str) -> Option<i64> {
    let dt = DateTime::parse_from_rfc3339(s).ok()?;
    // `timestamp_nanos_opt` returns `None` past ~year 2262 — a
    // non-concern for live traces but worth degrading cleanly.
    dt.with_timezone(&Utc).timestamp_nanos_opt()
}

/// Parse the `duration` column's `timespan` string into
/// nanoseconds. The wire format is the Go-style suffixed decimal
/// the Axiom backend ships: `"130.944578ms"`, `"587.163µs"`,
/// `"27.461ns"`, `"3.5s"`. Unparseable cells become `0`.
fn read_duration_ns(cols: &[Vec<Json>], idx: usize, row: usize) -> i64 {
    let Some(v) = cols.get(idx).and_then(|c| c.get(row)) else {
        return 0;
    };
    if let Some(n) = v.as_i64() {
        return n;
    }
    let Some(s) = v.as_str() else {
        return 0;
    };
    parse_timespan_ns(s).unwrap_or(0)
}

/// Parse a Go-style duration string into nanoseconds. Returns `None`
/// when a segment's number or unit doesn't parse.
///
/// Go's `time.Duration.String()` — which the Axiom `duration` column
/// ships — emits a *compound* form for durations ≥ 1 minute
/// (`"1m0s"`, `"1h2m3.5s"`, `"90m"`) and a single-unit form below
/// (`"277.7ms"`, `"41µs"`, `"27ns"`). We parse the general grammar:
/// an optional leading sign, then one-or-more `<decimal><unit>`
/// segments summed together. Units: `h`, `m`, `s`, `ms`, `us`/`µs`,
/// `ns`. (A plain `0` with no unit — Go's zero-duration string — is
/// handled by the caller, which only reaches here for non-empty
/// string cells; `read_duration_ns` returns 0 for unparseable cells.)
fn parse_timespan_ns(s: &str) -> Option<i64> {
    let s = s.trim();
    let (neg, mut rest) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    if rest.is_empty() {
        return None;
    }
    let mut total_ns = 0_f64;
    let mut saw_segment = false;
    while !rest.is_empty() {
        // Leading decimal: ASCII digits and at most one '.'.
        let num_len = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        if num_len == 0 {
            return None; // unit with no number — malformed
        }
        let value: f64 = rest[..num_len].parse().ok()?;
        rest = &rest[num_len..];
        // Unit suffix: match 2-byte / multi-byte units before the
        // bare `s` / `m` so `ms` isn't read as `m` + stray `s`, etc.
        let (scale_ns, unit_len) = if rest.starts_with("ns") {
            (1_f64, 2)
        } else if rest.starts_with("µs") {
            (1_000_f64, "µs".len())
        } else if rest.starts_with("us") {
            (1_000_f64, 2)
        } else if rest.starts_with("ms") {
            (1_000_000_f64, 2)
        } else if rest.starts_with('s') {
            (1_000_000_000_f64, 1)
        } else if rest.starts_with('m') {
            (60_000_000_000_f64, 1)
        } else if rest.starts_with('h') {
            (3_600_000_000_000_f64, 1)
        } else {
            return None; // unrecognised unit
        };
        total_ns += value * scale_ns;
        rest = &rest[unit_len..];
        saw_segment = true;
    }
    if !saw_segment {
        return None;
    }
    let ns = total_ns as i64;
    Some(if neg { -ns } else { ns })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse_fixture(s: &str) -> QueryResult {
        serde_json::from_str(s).expect("fixture parses")
    }

    /// Two-column table: `_time` (datetime) + `count` (long). No
    /// groups. Should yield one Series.
    #[test]
    fn to_series_single_bucketed_count() {
        let resp = parse_fixture(SUMMARIZE_COUNT_NO_GROUPS);
        let series = to_series(&resp, &BTreeMap::new()).expect("decode ok");
        assert_eq!(series.len(), 1);
        let s = &series[0];
        assert_eq!(s.name, "n");
        assert_eq!(s.points.len(), 3);
        // x values are unix-ms, parsed from the RFC3339 strings.
        assert!(s.points[0].0 < s.points[1].0 && s.points[1].0 < s.points[2].0);
        assert_eq!(s.points[0].1, 5.0);
        assert_eq!(s.points[2].1, 12.0);
    }

    /// Two-column table + one group column (`level`). Should split
    /// into one Series per distinct group value.
    #[test]
    fn to_series_with_one_group_column_splits_per_group() {
        let resp = parse_fixture(SUMMARIZE_COUNT_BY_LEVEL);
        let series = to_series(&resp, &BTreeMap::new()).expect("decode ok");
        assert_eq!(series.len(), 2);
        let names: Vec<_> = series.iter().map(|s| s.name.clone()).collect();
        assert!(
            names.iter().any(|n| n.contains("error")),
            "names: {names:?}"
        );
        assert!(names.iter().any(|n| n.contains("info")), "names: {names:?}");
    }

    /// Empty response — no tables. Series decoder errors; table
    /// decoder returns an empty TableResult.
    #[test]
    fn empty_response_handling() {
        let resp = QueryResult {
            status: empty_status(),
            tables: vec![],
            saved_query_id: None,
            trace_id: None,
        };
        assert!(matches!(
            to_series(&resp, &BTreeMap::new()),
            Err(AplDecodeError::NoTables)
        ));
        let t = to_table_result(&resp);
        assert!(t.columns.is_empty());
        assert!(t.rows.is_empty());
    }

    /// `to_table_result` on the bucketed-count fixture gives one row
    /// per bucket, two columns (`_time`, `n`), with the right cell
    /// types.
    #[test]
    fn to_table_result_round_trip() {
        let resp = parse_fixture(SUMMARIZE_COUNT_NO_GROUPS);
        let t = to_table_result(&resp);
        assert_eq!(t.columns, vec!["_time".to_string(), "n".to_string()]);
        assert_eq!(t.rows.len(), 3);
        // First column is datetime → TableCell::Time
        assert!(matches!(t.rows[0][0], TableCell::Time(_)));
        // Second column is `long` → TableCell::Int
        assert!(matches!(t.rows[0][1], TableCell::Int(5)));
    }

    /// `// @viz line x=foo`: error when the named column is missing.
    #[test]
    fn unknown_x_column_errors_cleanly() {
        let resp = parse_fixture(SUMMARIZE_COUNT_NO_GROUPS);
        let mut opts = BTreeMap::new();
        opts.insert("x".to_string(), "nope".to_string());
        assert!(matches!(
            to_series(&resp, &opts),
            Err(AplDecodeError::UnknownColumn { which: "x", .. })
        ));
    }

    /// Pure string table (no numeric col) → MissingValueColumn.
    #[test]
    fn missing_value_column_errors_cleanly() {
        // Simulate a single-column string table; build from JSON.
        let resp: QueryResult = serde_json::from_value(json!({
            "status": status_stub(),
            "tables": [{
                "name": "0",
                "sources": [{"name": "logs"}],
                "fields": [{"name": "_time", "type": "datetime"}, {"name": "msg", "type": "string"}],
                "order": [],
                "groups": [],
                "range": null,
                "buckets": null,
                "columns": [
                    ["2024-01-01T00:00:00Z"],
                    ["hello"]
                ]
            }]
        }))
        .expect("decode fixture");
        match to_series(&resp, &BTreeMap::new()) {
            Err(AplDecodeError::MissingValueColumn { available }) => {
                assert_eq!(available, vec!["_time".to_string(), "msg".to_string()]);
            }
            other => panic!("expected MissingValueColumn, got {other:?}"),
        }
    }

    fn empty_status() -> axiom_rs::datasets::QueryStatus {
        serde_json::from_value(status_stub()).expect("status stub decodes")
    }

    fn status_stub() -> Json {
        json!({
            "elapsedTime": 0,
            "blocksExamined": 0,
            "rowsExamined": 0,
            "rowsMatched": 0,
            "numGroups": 0,
            "isPartial": false,
            "continuationToken": null,
            "cacheStatus": 0,
            "minBlockTime": "2024-01-01T00:00:00Z",
            "maxBlockTime": "2024-01-01T00:00:00Z"
        })
    }

    // ── Fixtures ─────────────────────────────────────────────────
    //
    // Captured from real `/v1/datasets/_apl` responses, trimmed to
    // the fields the decoder actually consumes. `name`, `sources`,
    // `order`, `range`, `saved_query_id`, `trace_id` aren't asserted
    // on \u2014 they're part of the wire shape so the SDK can decode.

    const SUMMARIZE_COUNT_NO_GROUPS: &str = r#"{
      "status": {
        "elapsedTime": 0,
        "blocksExamined": 0,
        "rowsExamined": 0,
        "rowsMatched": 0,
        "numGroups": 0,
        "isPartial": false,
        "continuationToken": null,
        "cacheStatus": 0,
        "minBlockTime": "2024-01-01T00:00:00Z",
        "maxBlockTime": "2024-01-01T00:00:00Z"
      },
      "tables": [{
        "name": "0",
        "sources": [{"name": "logs"}],
        "fields": [
          {"name": "_time", "type": "datetime"},
          {"name": "n", "type": "long"}
        ],
        "order": [],
        "groups": [],
        "range": null,
        "buckets": {"field": "_time", "size": 3600000000000},
        "columns": [
          ["2024-01-01T00:00:00Z", "2024-01-01T01:00:00Z", "2024-01-01T02:00:00Z"],
          [5, 8, 12]
        ]
      }]
    }"#;

    const SUMMARIZE_COUNT_BY_LEVEL: &str = r#"{
      "status": {
        "elapsedTime": 0,
        "blocksExamined": 0,
        "rowsExamined": 0,
        "rowsMatched": 0,
        "numGroups": 0,
        "isPartial": false,
        "continuationToken": null,
        "cacheStatus": 0,
        "minBlockTime": "2024-01-01T00:00:00Z",
        "maxBlockTime": "2024-01-01T00:00:00Z"
      },
      "tables": [{
        "name": "0",
        "sources": [{"name": "logs"}],
        "fields": [
          {"name": "_time", "type": "datetime"},
          {"name": "level", "type": "string"},
          {"name": "n", "type": "long"}
        ],
        "order": [],
        "groups": [{"name": "level"}],
        "range": null,
        "buckets": {"field": "_time", "size": 3600000000000},
        "columns": [
          ["2024-01-01T00:00:00Z", "2024-01-01T00:00:00Z", "2024-01-01T01:00:00Z", "2024-01-01T01:00:00Z"],
          ["error", "info", "error", "info"],
          [2, 50, 3, 70]
        ]
      }]
    }"#;

    // ================================================================
    // Trace decoder tests
    // ================================================================

    /// Build a minimal trace fixture from a list of
    /// `(span_id, parent_span_id, _time, duration, name)` tuples.
    /// Keeps the synthetic tests readable; missing optional columns
    /// (`service.name`, `kind`, etc.) exercise the decoder's
    /// degrade-to-default path.
    fn trace_fixture(spans: &[(&str, Option<&str>, &str, &str, &str)]) -> QueryResult {
        let times: Vec<Json> = spans.iter().map(|s| json!(s.2)).collect();
        let span_ids: Vec<Json> = spans.iter().map(|s| json!(s.0)).collect();
        let parents: Vec<Json> = spans
            .iter()
            .map(|s| match s.1 {
                Some(p) => json!(p),
                None => Json::Null,
            })
            .collect();
        let names: Vec<Json> = spans.iter().map(|s| json!(s.4)).collect();
        let durations: Vec<Json> = spans.iter().map(|s| json!(s.3)).collect();
        let body = json!({
            "status": status_stub(),
            "tables": [{
                "name": "0",
                "sources": [{"name": "traces"}],
                "fields": [
                    {"name": "_time",          "type": "datetime"},
                    {"name": "span_id",        "type": "string"},
                    {"name": "parent_span_id", "type": "string"},
                    {"name": "name",           "type": "string"},
                    {"name": "duration",       "type": "timespan"},
                ],
                "order": [], "groups": [], "range": null,
                "columns": [times, span_ids, parents, names, durations]
            }]
        });
        serde_json::from_value(body).expect("synthetic fixture decodes")
    }

    #[test]
    fn trace_decoder_links_simple_chain() {
        let resp = trace_fixture(&[
            ("a", None, "2026-05-21T00:00:00Z", "100ms", "root"),
            ("b", Some("a"), "2026-05-21T00:00:00.010Z", "50ms", "child"),
            ("c", Some("b"), "2026-05-21T00:00:00.020Z", "10ms", "grand"),
        ]);
        let m = to_trace_model(&resp, "trace1".into(), "ds".into()).unwrap();
        assert_eq!(m.spans.len(), 3);
        assert_eq!(m.roots.len(), 1);
        assert_eq!(m.tree.len(), 3);
        assert_eq!(m.tree[0].depth, 0);
        assert_eq!(m.tree[1].depth, 1);
        assert_eq!(m.tree[2].depth, 2);
        assert!(m.tree[0].has_children);
        assert!(m.tree[1].has_children);
        assert!(!m.tree[2].has_children);
        assert!(m.t0_ns <= m.spans[0].start_ns);
        for s in &m.spans {
            assert!(m.t1_ns >= s.end_ns, "t1 must enclose every span");
        }
    }

    #[test]
    fn trace_decoder_orphan_parent_becomes_root_with_flag() {
        let resp = trace_fixture(&[
            ("a", None, "2026-05-21T00:00:00Z", "50ms", "root"),
            (
                "b",
                Some("ghost"),
                "2026-05-21T00:00:00.001Z",
                "10ms",
                "orphan",
            ),
        ]);
        let m = to_trace_model(&resp, "t".into(), "d".into()).unwrap();
        assert_eq!(m.roots.len(), 2);
        assert_eq!(m.tree.iter().filter(|r| r.depth == 0).count(), 2);
        assert_eq!(m.tree.iter().filter(|r| r.is_orphan).count(), 1);
        let orphan_row = m.tree.iter().find(|r| r.is_orphan).unwrap();
        assert_eq!(m.spans[orphan_row.span_idx].span_id, "b");
    }

    #[test]
    fn trace_decoder_sibling_sort_ties_break_by_span_id() {
        let resp = trace_fixture(&[
            ("a", None, "2026-05-21T00:00:00Z", "100ms", "root"),
            ("c", Some("a"), "2026-05-21T00:00:00.010Z", "5ms", "c"),
            ("b", Some("a"), "2026-05-21T00:00:00.010Z", "5ms", "b"),
        ]);
        let m = to_trace_model(&resp, "t".into(), "d".into()).unwrap();
        let ids: Vec<&str> = m
            .tree
            .iter()
            .map(|r| m.spans[r.span_idx].span_id.as_str())
            .collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    #[test]
    fn trace_decoder_empty_response_errors() {
        let resp = QueryResult {
            status: empty_status(),
            tables: vec![],
            saved_query_id: None,
            trace_id: None,
        };
        assert!(matches!(
            to_trace_model(&resp, "t".into(), "d".into()),
            Err(AplDecodeError::NoTables)
        ));
    }

    #[test]
    fn trace_decoder_rejects_table_with_no_rows() {
        let body = json!({
            "status": status_stub(),
            "tables": [{
                "name": "0", "sources": [{"name": "t"}],
                "fields": [
                    {"name": "_time",    "type": "datetime"},
                    {"name": "span_id",  "type": "string"},
                    {"name": "name",     "type": "string"},
                    {"name": "duration", "type": "timespan"},
                ],
                "order": [], "groups": [], "range": null,
                "columns": [[], [], [], []]
            }]
        });
        let resp: QueryResult = serde_json::from_value(body).unwrap();
        assert!(matches!(
            to_trace_model(&resp, "t".into(), "d".into()),
            Err(AplDecodeError::EmptyTrace)
        ));
    }

    #[test]
    fn trace_decoder_missing_required_column_errors() {
        let body = json!({
            "status": status_stub(),
            "tables": [{
                "name": "0", "sources": [{"name": "t"}],
                "fields": [
                    {"name": "_time",   "type": "datetime"},
                    {"name": "span_id", "type": "string"},
                    {"name": "name",    "type": "string"},
                ],
                "order": [], "groups": [], "range": null,
                "columns": [["2026-05-21T00:00:00Z"], ["a"], ["only"]]
            }]
        });
        let resp: QueryResult = serde_json::from_value(body).unwrap();
        match to_trace_model(&resp, "t".into(), "d".into()) {
            Err(AplDecodeError::MissingTraceColumn { name }) => assert_eq!(name, "duration"),
            other => panic!("expected MissingTraceColumn(duration), got {other:?}"),
        }
    }

    #[test]
    fn trace_decoder_clock_skew_child_ends_after_parent() {
        let resp = trace_fixture(&[
            ("a", None, "2026-05-21T00:00:00Z", "10ms", "parent"),
            ("b", Some("a"), "2026-05-21T00:00:00.005Z", "20ms", "child"),
        ]);
        let m = to_trace_model(&resp, "t".into(), "d".into()).unwrap();
        let parent_end = m.spans.iter().find(|s| s.span_id == "a").unwrap().end_ns;
        let child_end = m.spans.iter().find(|s| s.span_id == "b").unwrap().end_ns;
        assert!(child_end > parent_end, "fixture must exercise skew");
        assert_eq!(m.t1_ns, child_end);
    }

    #[test]
    fn trace_decoder_zero_duration_survives() {
        let resp = trace_fixture(&[("a", None, "2026-05-21T00:00:00Z", "0ns", "instant")]);
        let m = to_trace_model(&resp, "t".into(), "d".into()).unwrap();
        assert_eq!(m.spans[0].duration_ns, 0);
        assert_eq!(m.spans[0].start_ns, m.spans[0].end_ns);
        assert_eq!(m.duration_ns(), 0);
    }

    #[test]
    fn trace_decoder_parses_all_timespan_units() {
        let resp = trace_fixture(&[
            ("a", None, "2026-05-21T00:00:00Z", "3.5s", "sec"),
            ("b", Some("a"), "2026-05-21T00:00:00Z", "500ms", "milli"),
            ("c", Some("a"), "2026-05-21T00:00:00Z", "250µs", "micro"),
            ("d", Some("a"), "2026-05-21T00:00:00Z", "42ns", "nano"),
            (
                "e",
                Some("a"),
                "2026-05-21T00:00:00Z",
                "75us",
                "micro-ascii",
            ),
        ]);
        let m = to_trace_model(&resp, "t".into(), "d".into()).unwrap();
        let dur = |id: &str| {
            m.spans
                .iter()
                .find(|s| s.span_id == id)
                .unwrap()
                .duration_ns
        };
        assert_eq!(dur("a"), 3_500_000_000);
        assert_eq!(dur("b"), 500_000_000);
        assert_eq!(dur("c"), 250_000);
        assert_eq!(dur("d"), 42);
        assert_eq!(dur("e"), 75_000);
    }

    #[test]
    fn parse_timespan_handles_compound_go_durations() {
        // Single-unit forms (durations < 1 minute).
        assert_eq!(parse_timespan_ns("277.731738ms"), Some(277_731_738));
        assert_eq!(parse_timespan_ns("41.5µs"), Some(41_500));
        assert_eq!(parse_timespan_ns("75us"), Some(75_000));
        assert_eq!(parse_timespan_ns("27ns"), Some(27));
        assert_eq!(parse_timespan_ns("3.5s"), Some(3_500_000_000));
        // Compound forms Go's Duration.String() emits for >= 1 minute.
        assert_eq!(parse_timespan_ns("90s"), Some(90_000_000_000));
        assert_eq!(parse_timespan_ns("1m0s"), Some(60_000_000_000));
        assert_eq!(parse_timespan_ns("2m30s"), Some(150_000_000_000));
        assert_eq!(
            parse_timespan_ns("1h2m3.5s"),
            Some(3_600_000_000_000 + 120_000_000_000 + 3_500_000_000)
        );
        // Malformed / unit-less cells degrade to None (caller → 0).
        assert_eq!(parse_timespan_ns("abc"), None);
        assert_eq!(parse_timespan_ns(""), None);
        assert_eq!(parse_timespan_ns("10"), None);
    }

    #[test]
    fn trace_decoder_dfs_is_topological_order_of_parent_links() {
        let resp = trace_fixture(&[
            ("a", None, "2026-05-21T00:00:00Z", "100ms", "root"),
            ("b", Some("a"), "2026-05-21T00:00:00.001Z", "50ms", "l1"),
            ("c", Some("b"), "2026-05-21T00:00:00.002Z", "5ms", "l2"),
            ("d", Some("a"), "2026-05-21T00:00:00.050Z", "5ms", "sib"),
        ]);
        let m = to_trace_model(&resp, "t".into(), "d".into()).unwrap();
        let pos: BTreeMap<&str, usize> = m
            .tree
            .iter()
            .enumerate()
            .map(|(i, r)| (m.spans[r.span_idx].span_id.as_str(), i))
            .collect();
        for s in &m.spans {
            if let Some(pid) = s.parent_span_id.as_deref()
                && let Some(parent_pos) = pos.get(pid)
                && let Some(child_pos) = pos.get(s.span_id.as_str())
            {
                assert!(
                    parent_pos < child_pos,
                    "child {} appeared before parent {pid}",
                    s.span_id
                );
            }
        }
    }

    #[test]
    fn trace_decoder_buckets_attributes_and_resource() {
        let body = json!({
            "status": status_stub(),
            "tables": [{
                "name": "0", "sources": [{"name": "t"}],
                "fields": [
                    {"name": "_time",                  "type": "datetime"},
                    {"name": "span_id",                "type": "string"},
                    {"name": "name",                   "type": "string"},
                    {"name": "duration",               "type": "timespan"},
                    {"name": "attributes.http.method", "type": "string"},
                    {"name": "attributes.custom",      "type": "map"},
                    {"name": "resource.host.arch",     "type": "string"},
                    {"name": "resource.custom",        "type": "map"},
                    {"name": "attributes.unused",      "type": "string"},
                ],
                "order": [], "groups": [], "range": null,
                "columns": [
                    ["2026-05-21T00:00:00Z"],
                    ["a"],
                    ["req"],
                    ["1ms"],
                    ["GET"],
                    [{"http.status_code": 200, "http.url": "/x"}],
                    ["x86_64"],
                    [{"k8s.pod.name": "pod-7"}],
                    [Json::Null]
                ]
            }]
        });
        let resp: QueryResult = serde_json::from_value(body).unwrap();
        let m = to_trace_model(&resp, "t".into(), "d".into()).unwrap();
        let s = &m.spans[0];
        assert_eq!(
            s.attributes.get("http.method").and_then(Json::as_str),
            Some("GET")
        );
        assert_eq!(
            s.attributes.get("http.status_code").and_then(Json::as_i64),
            Some(200)
        );
        assert_eq!(
            s.attributes.get("http.url").and_then(Json::as_str),
            Some("/x")
        );
        assert_eq!(
            s.resource.get("host.arch").and_then(Json::as_str),
            Some("x86_64")
        );
        assert_eq!(
            s.resource.get("k8s.pod.name").and_then(Json::as_str),
            Some("pod-7")
        );
        assert!(!s.attributes.contains_key("unused"));
        assert!(!s.attributes.contains_key("custom"));
        assert!(!s.resource.contains_key("custom"));
    }

    #[test]
    fn trace_decoder_decodes_events_in_time_order() {
        let body = json!({
            "status": status_stub(),
            "tables": [{
                "name": "0", "sources": [{"name": "t"}],
                "fields": [
                    {"name": "_time",    "type": "datetime"},
                    {"name": "span_id",  "type": "string"},
                    {"name": "name",     "type": "string"},
                    {"name": "duration", "type": "timespan"},
                    {"name": "events",   "type": "array"},
                ],
                "order": [], "groups": [], "range": null,
                "columns": [
                    ["2026-05-21T00:00:00Z"],
                    ["a"],
                    ["req"],
                    ["10ms"],
                    [[
                        {"name": "end",   "timestamp": 200, "attributes": {"ok": true}},
                        {"name": "start", "timestamp": 100, "attributes": {}},
                    ]]
                ]
            }]
        });
        let resp: QueryResult = serde_json::from_value(body).unwrap();
        let m = to_trace_model(&resp, "t".into(), "d".into()).unwrap();
        let events = &m.spans[0].events;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].name, "start");
        assert_eq!(events[0].time_ns, 100);
        assert_eq!(events[1].name, "end");
        assert_eq!(events[1].time_ns, 200);
        assert_eq!(
            events[1].attributes.get("ok").and_then(Json::as_bool),
            Some(true)
        );
    }

    #[test]
    fn trace_decoder_is_error_falls_back_to_status_code() {
        let body = json!({
            "status": status_stub(),
            "tables": [{
                "name": "0", "sources": [{"name": "t"}],
                "fields": [
                    {"name": "_time",       "type": "datetime"},
                    {"name": "span_id",     "type": "string"},
                    {"name": "name",        "type": "string"},
                    {"name": "duration",    "type": "timespan"},
                    {"name": "status.code", "type": "string"},
                ],
                "order": [], "groups": [], "range": null,
                "columns": [
                    ["2026-05-21T00:00:00Z", "2026-05-21T00:00:01Z"],
                    ["a", "b"],
                    ["ok", "bad"],
                    ["1ms", "1ms"],
                    ["OK", "ERROR"]
                ]
            }]
        });
        let resp: QueryResult = serde_json::from_value(body).unwrap();
        let m = to_trace_model(&resp, "t".into(), "d".into()).unwrap();
        let a = m.spans.iter().find(|s| s.span_id == "a").unwrap();
        let b = m.spans.iter().find(|s| s.span_id == "b").unwrap();
        assert!(!a.is_error);
        assert!(b.is_error);
    }

    #[test]
    fn trace_decoder_is_error_column_wins_over_status_code() {
        let body = json!({
            "status": status_stub(),
            "tables": [{
                "name": "0", "sources": [{"name": "t"}],
                "fields": [
                    {"name": "_time",       "type": "datetime"},
                    {"name": "span_id",     "type": "string"},
                    {"name": "name",        "type": "string"},
                    {"name": "duration",    "type": "timespan"},
                    {"name": "status.code", "type": "string"},
                    {"name": "is_error",    "type": "boolean"},
                ],
                "order": [], "groups": [], "range": null,
                "columns": [
                    ["2026-05-21T00:00:00Z"],
                    ["a"],
                    ["unusual"],
                    ["1ms"],
                    ["OK"],
                    [true]
                ]
            }]
        });
        let resp: QueryResult = serde_json::from_value(body).unwrap();
        let m = to_trace_model(&resp, "t".into(), "d".into()).unwrap();
        assert!(m.spans[0].is_error);
    }

    // Fixture sweep — every `traces-research/traces/*.json` must
    // decode and satisfy the structural invariants the renderer
    // assumes. The directory is gitignored so contributors without
    // the corpus see a clear skip rather than a red bar.
    const TRACE_FIXTURES_DIR: &str = "traces-research/traces";

    fn first_trace_id(resp: &QueryResult) -> String {
        let Some(table) = resp.tables.first() else {
            return String::new();
        };
        let Some(idx) = table.fields().iter().position(|f| f.name() == "trace_id") else {
            return String::new();
        };
        table
            .columns()
            .get(idx)
            .and_then(|c| c.first())
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    }

    #[test]
    fn trace_decoder_sweeps_research_fixtures() {
        let dir = std::path::Path::new(TRACE_FIXTURES_DIR);
        if !dir.is_dir() {
            eprintln!(
                "skipping fixture sweep: {TRACE_FIXTURES_DIR} not present (gitignored corpus)"
            );
            return;
        }
        let entries: Vec<_> = std::fs::read_dir(dir)
            .expect("read fixtures dir")
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        assert!(
            !entries.is_empty(),
            "fixture dir exists but contains no *.json files"
        );
        assert!(
            entries.len() >= 40,
            "expected ≥40 fixtures, found {}",
            entries.len()
        );

        for path in &entries {
            let label = path.file_name().unwrap().to_string_lossy().into_owned();
            let text =
                std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {label}: {e}"));
            let resp: QueryResult = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("parsing {label} as QueryResult: {e}"));
            let trace_id = first_trace_id(&resp);
            let m = to_trace_model(&resp, trace_id.clone(), "axiom-traces".into())
                .unwrap_or_else(|e| panic!("decoding {label}: {e}"));

            let row_count = resp.tables[0].columns().first().map(Vec::len).unwrap_or(0);
            assert_eq!(m.spans.len(), row_count, "{label}: span count != row count");
            assert_eq!(
                m.roots.len(),
                1,
                "{label}: expected 1 root, got {}",
                m.roots.len()
            );
            assert_eq!(m.tree.len(), m.spans.len(), "{label}: tree/span size drift");
            let mut seen = vec![false; m.spans.len()];
            for row in &m.tree {
                assert!(
                    !seen[row.span_idx],
                    "{label}: span {} visited twice in tree",
                    row.span_idx
                );
                seen[row.span_idx] = true;
            }
            let pos: BTreeMap<&str, usize> = m
                .tree
                .iter()
                .enumerate()
                .map(|(i, r)| (m.spans[r.span_idx].span_id.as_str(), i))
                .collect();
            for s in &m.spans {
                if let Some(pid) = s.parent_span_id.as_deref()
                    && let (Some(&pp), Some(&cp)) = (pos.get(pid), pos.get(s.span_id.as_str()))
                {
                    assert!(pp < cp, "{label}: child {} before parent {pid}", s.span_id);
                }
            }
            let mut prev_by_parent: BTreeMap<&str, i64> = BTreeMap::new();
            for row in &m.tree {
                let s = &m.spans[row.span_idx];
                let parent_key = s.parent_span_id.as_deref().unwrap_or("__root__");
                if let Some(&prev) = prev_by_parent.get(parent_key) {
                    assert!(
                        s.start_ns >= prev,
                        "{label}: siblings out of start_ns order under {parent_key}"
                    );
                }
                prev_by_parent.insert(parent_key, s.start_ns);
            }
            assert!(m.t1_ns >= m.t0_ns, "{label}: t1 < t0");
            for s in &m.spans {
                assert!(s.start_ns >= m.t0_ns, "{label}: span starts before t0");
                assert!(s.end_ns <= m.t1_ns, "{label}: span ends after t1");
            }
            assert_eq!(m.trace_id, trace_id, "{label}: trace_id field drift");
            assert_eq!(m.dataset, "axiom-traces", "{label}: dataset field drift");
            assert_eq!(m.by_id.len(), m.spans.len(), "{label}: by_id size != spans");
            for (idx, span) in m.spans.iter().enumerate() {
                assert_eq!(
                    m.by_id.get(&span.span_id).copied(),
                    Some(idx),
                    "{label}: by_id back-reference drift on span {}",
                    span.span_id
                );
            }
            let any_name = m.spans.iter().any(|s| !s.name.is_empty());
            let any_service = m.spans.iter().any(|s| !s.service.is_empty());
            assert!(any_name, "{label}: every span has an empty name");
            assert!(any_service, "{label}: every span has an empty service.name");
            for s in &m.spans {
                let _ = s.kind.as_str();
                let _ = s.status_code.as_deref();
            }
        }
    }
}
