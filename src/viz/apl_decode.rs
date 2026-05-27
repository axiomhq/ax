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
}
