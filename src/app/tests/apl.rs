//! Phase 1 APL-language coverage:
//!
//! * `extract_query` honours the `mcuLang` sidecar over chart kind.
//! * `:apl` / `:mpl` flip the focused tile's query-object key and
//!   stamp the sidecar (or just flip `buffer_lang` in standalone).
//! * `:tile add <kind> apl` inserts a tile pre-marked as APL.
//! * APL-tile editor seeds are raw text (no `// APL` comment banner).
//! * `sync_buffer_to_focused_tile` round-trips APL edits.
//! * `normalize_queries_to_wire` leaves APL-keyed queries alone.

use super::*;
use crate::dashboard::{Lang, Query, extract_lang, extract_query};

fn timeseries_with_query(query: serde_json::Value) -> crate::axiom::Chart {
    use crate::axiom::{Chart, ChartBase, KnownChart};
    Chart::Known(KnownChart::TimeSeries(ChartBase {
        id: "c1".into(),
        name: Some("c1".into()),
        query: Some(query),
        extras: Default::default(),
    }))
}

fn timeseries_with_query_and_lang(query: serde_json::Value, lang: Lang) -> crate::axiom::Chart {
    use crate::axiom::{Chart, ChartBase, KnownChart};
    let mut extras: serde_json::Map<String, serde_json::Value> = Default::default();
    extras.insert(
        crate::dashboard::LANG_SIDECAR_KEY.to_string(),
        serde_json::Value::String(lang.as_sidecar().to_string()),
    );
    Chart::Known(KnownChart::TimeSeries(ChartBase {
        id: "c1".into(),
        name: Some("c1".into()),
        query: Some(query),
        extras,
    }))
}

fn dashboard_with_chart(chart: crate::axiom::Chart) -> crate::axiom::DashboardSummary {
    crate::axiom::DashboardSummary {
        uid: "u".into(),
        id: None,
        updated_at: None,
        updated_by: None,
        version: None,
        dashboard: crate::axiom::DashboardDocument {
            name: Some("d".into()),
            charts: vec![chart],
            ..Default::default()
        },
    }
}

#[test]
fn sidecar_apl_on_metrics_chart_overrides_kind_default() {
    // A TimeSeries chart with the `mcuLang=apl` sidecar must classify
    // as APL even though the chart-kind fallback would say MPL. This
    // is the whole point of the sidecar: deterministic language on
    // tiles mcu authored.
    let chart =
        timeseries_with_query_and_lang(serde_json::json!({ "apl": "['logs'] | count" }), Lang::Apl);
    assert!(matches!(extract_query(&chart), Query::Apl(_)));
    assert_eq!(extract_lang(&chart), Some(Lang::Apl));
}

#[test]
fn sidecar_mpl_on_logstream_overrides_kind_default() {
    // Mirror: a LogStream chart explicitly marked MPL classifies as
    // MPL. (Not a normal user flow but the sidecar must win both ways.)
    use crate::axiom::{Chart, ChartBase, KnownChart};
    let mut extras: serde_json::Map<String, serde_json::Value> = Default::default();
    extras.insert(
        crate::dashboard::LANG_SIDECAR_KEY.to_string(),
        serde_json::Value::String("mpl".into()),
    );
    let chart = Chart::Known(KnownChart::LogStream(ChartBase {
        id: "c1".into(),
        name: Some("c1".into()),
        query: Some(serde_json::json!({ "apl": "cpu:rate" })),
        extras,
    }));
    assert!(matches!(extract_query(&chart), Query::Mpl(_)));
}

#[test]
fn sidecar_apl_reads_text_from_either_key() {
    // A tile that was MPL yesterday and got `:apl`-flipped today
    // might still have its text under `mpl` until the next sync.
    // The sidecar wins, but we still surface the text.
    let chart =
        timeseries_with_query_and_lang(serde_json::json!({ "mpl": "some text" }), Lang::Apl);
    match extract_query(&chart) {
        Query::Apl(s) => assert_eq!(s, "some text"),
        other => panic!("expected APL, got {other:?}"),
    }
}

#[test]
fn cmd_lang_in_standalone_buffer_only_flips_buffer_lang() {
    // No dashboard loaded → `:apl` mutates `App.buffer_lang` and
    // touches nothing else.
    let mut app = test_app();
    assert_eq!(app.buffer_lang, Lang::Mpl);
    app.execute_command("apl");
    assert_eq!(app.buffer_lang, Lang::Apl);
    assert!(app.loaded_dashboard.is_none());
    app.execute_command("mpl");
    assert_eq!(app.buffer_lang, Lang::Mpl);
}

#[test]
fn cmd_lang_in_dashboard_mode_rewrites_key_and_stamps_sidecar() {
    // Adopt a one-chart dashboard with an MPL TimeSeries tile, then
    // `:apl`. Expected:
    //   * the query object now has the `apl` key, not `mpl`,
    //   * `chart.extras["mcuLang"] == "apl"`,
    //   * dashboard_dirty flips to true.
    let mut app = test_app();
    let resource = dashboard_with_chart(timeseries_with_query(
        serde_json::json!({ "mpl": "http_requests:rate" }),
    ));
    app.handle_event(AppEvent::DashboardOpened {
        uid: "u".into(),
        result: Ok(resource),
    });
    app.dashboard_dirty = false; // ignore adoption-side dirty.
    app.execute_command("apl");
    let chart = &app.loaded_dashboard.as_ref().unwrap().dashboard.charts[0];
    let base = chart.base().unwrap();
    let q = base.query.as_ref().unwrap();
    assert!(q.get("apl").is_some(), "query: {q}");
    assert!(q.get("mpl").is_none(), "mpl key must be dropped: {q}");
    assert_eq!(
        base.extras.get(crate::dashboard::LANG_SIDECAR_KEY),
        Some(&serde_json::Value::String("apl".into()))
    );
    assert!(app.dashboard_dirty);
    assert_eq!(app.active_lang(), Lang::Apl);
}

#[test]
fn cmd_lang_does_not_convert_buffer_text() {
    // `:apl` preserves the user's typed text verbatim. The user is
    // expected to rewrite it; we just flip the key.
    let mut app = test_app();
    let resource = dashboard_with_chart(timeseries_with_query(
        serde_json::json!({ "mpl": "http_requests:rate" }),
    ));
    app.handle_event(AppEvent::DashboardOpened {
        uid: "u".into(),
        result: Ok(resource),
    });
    app.execute_command("apl");
    let chart = &app.loaded_dashboard.as_ref().unwrap().dashboard.charts[0];
    let text = crate::dashboard::extract_query(chart);
    match text {
        Query::Apl(s) => assert_eq!(s, "http_requests:rate"),
        other => panic!("expected APL, got {other:?}"),
    }
}

#[test]
fn tile_add_apl_inserts_apl_marked_tile() {
    // `:tile add line apl my-chart` builds an APL tile with the
    // sidecar set and an empty `apl` key. The user can type APL
    // into the editor and `:w` round-trips correctly.
    let mut app = test_app();
    let resource = dashboard_with_chart(timeseries_with_query(
        serde_json::json!({ "mpl": "anchor:rate" }),
    ));
    app.handle_event(AppEvent::DashboardOpened {
        uid: "u".into(),
        result: Ok(resource),
    });
    app.execute_command("tile add line apl my-apl");
    let charts = &app.loaded_dashboard.as_ref().unwrap().dashboard.charts;
    assert_eq!(charts.len(), 2);
    let new_chart = &charts[1];
    let base = new_chart.base().unwrap();
    assert_eq!(base.name.as_deref(), Some("my-apl"));
    let q = base.query.as_ref().unwrap();
    assert!(q.get("apl").is_some());
    assert!(q.get("mpl").is_none());
    assert_eq!(
        base.extras.get(crate::dashboard::LANG_SIDECAR_KEY),
        Some(&serde_json::Value::String("apl".into()))
    );
    assert_eq!(extract_lang(new_chart), Some(Lang::Apl));
}

#[test]
fn apl_tile_seeds_editor_with_raw_text_no_banner() {
    // The pre-execution comment-banner is gone: APL tiles seed as
    // raw editable text so the user can type / save normally.
    let mut app = test_app();
    use crate::axiom::{Chart, ChartBase, KnownChart};
    let resource = crate::axiom::DashboardSummary {
        uid: "u".into(),
        id: None,
        updated_at: None,
        updated_by: None,
        version: None,
        dashboard: crate::axiom::DashboardDocument {
            name: Some("d".into()),
            charts: vec![Chart::Known(KnownChart::LogStream(ChartBase {
                id: "c1".into(),
                name: Some("logs".into()),
                query: Some(serde_json::json!({
                    "apl": "['logs'] | where severity == 'error' | limit 50"
                })),
                extras: Default::default(),
            }))],
            ..Default::default()
        },
    };
    app.handle_event(AppEvent::DashboardOpened {
        uid: "u".into(),
        result: Ok(resource),
    });
    let buf = buffer(&app);
    assert!(buf.contains("// @viz log_stream"));
    assert!(buf.contains("['logs'] | where severity == 'error' | limit 50"));
    assert!(
        !buf.contains("// APL"),
        "no comment banner allowed in buffer: {buf:?}"
    );
}

#[test]
fn sync_buffer_to_focused_tile_round_trips_apl_edits() {
    // Adopt an APL tile, type new APL text, sync, then verify the
    // chart's `apl` key carries the new text and dashboard is dirty.
    let mut app = test_app();
    let resource = dashboard_with_chart(timeseries_with_query_and_lang(
        serde_json::json!({ "apl": "['logs'] | count" }),
        Lang::Apl,
    ));
    app.handle_event(AppEvent::DashboardOpened {
        uid: "u".into(),
        result: Ok(resource),
    });
    app.dashboard_dirty = false;
    // Replace the editor with the pragma + new APL.
    set_buffer(
        &mut app,
        "// @viz line\n['logs'] | summarize n = count() by bin(_time, 1m)",
    );
    app.sync_buffer_to_focused_tile();
    let chart = &app.loaded_dashboard.as_ref().unwrap().dashboard.charts[0];
    match extract_query(chart) {
        Query::Apl(s) => assert_eq!(s, "['logs'] | summarize n = count() by bin(_time, 1m)"),
        other => panic!("expected APL, got {other:?}"),
    }
    assert!(app.dashboard_dirty);
}

#[test]
fn normalize_queries_to_wire_leaves_apl_keyed_queries_alone() {
    // Wire-shape pass: an APL tile (key = `apl`) needs no transform.
    // Only the legacy `mpl`-keyed convention gets rewritten.
    let mut doc = crate::axiom::DashboardDocument {
        charts: vec![
            timeseries_with_query(serde_json::json!({ "mpl": "cpu:rate" })),
            timeseries_with_query_and_lang(
                serde_json::json!({ "apl": "['logs'] | count" }),
                Lang::Apl,
            ),
        ],
        ..Default::default()
    };
    crate::dashboard::normalize_queries_to_wire(&mut doc);
    // MPL chart: text moved to `apl`, `mpl` gone.
    let q0 = doc.charts[0].base().unwrap().query.as_ref().unwrap();
    assert!(q0.get("mpl").is_none());
    assert_eq!(q0.get("apl").and_then(|v| v.as_str()), Some("cpu:rate"));
    // APL chart: untouched.
    let q1 = doc.charts[1].base().unwrap().query.as_ref().unwrap();
    assert_eq!(
        q1.get("apl").and_then(|v| v.as_str()),
        Some("['logs'] | count")
    );
    // Both charts: the `mcuLang` sidecar must have been scrubbed.
    // (Phase-1 stamped it via `timeseries_with_query_and_lang`; if
    // it leaked into the wire payload the Axiom server's PUT
    // schema validator would reject the request with an unknown-key
    // error — we hit this in the wild loading + saving an APL
    // dashboard.)
    for chart in &doc.charts {
        assert!(
            chart
                .base()
                .unwrap()
                .extras
                .get(crate::dashboard::LANG_SIDECAR_KEY)
                .is_none(),
            "mcuLang sidecar leaked into wire payload",
        );
    }
}

// ── Phase 2: APL dispatch + handler ────────────────────────────────

fn apl_query_status_stub() -> axiom_rs::datasets::QueryStatus {
    serde_json::from_value(serde_json::json!({
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
    }))
    .expect("status stub decodes")
}

fn apl_table_response(fixture_json: &str) -> crate::axiom::AplQueryResult {
    let raw: serde_json::Value = serde_json::from_str(fixture_json).expect("fixture parses");
    // The status wrapper is optional in our fixtures; merge if absent.
    let mut wrapper = serde_json::json!({
        "status": serde_json::to_value(apl_query_status_stub()).unwrap(),
        "tables": raw,
    });
    if raw.get("tables").is_some() {
        wrapper = raw;
        wrapper.as_object_mut().unwrap().insert(
            "status".into(),
            serde_json::to_value(apl_query_status_stub()).unwrap(),
        );
    }
    serde_json::from_value(wrapper).expect("response decodes")
}

fn one_chart_dashboard_apl_kind(
    kind: crate::dashboard::VizKind,
    apl: &str,
) -> crate::axiom::DashboardSummary {
    use crate::axiom::{ChartBase, DashboardDocument, DashboardSummary};
    let mut extras: serde_json::Map<String, serde_json::Value> = Default::default();
    extras.insert(
        crate::dashboard::LANG_SIDECAR_KEY.to_string(),
        serde_json::Value::String("apl".into()),
    );
    let base = ChartBase {
        id: "c1".into(),
        name: Some("apl-tile".into()),
        query: Some(serde_json::json!({ "apl": apl })),
        extras,
    };
    DashboardSummary {
        uid: "u".into(),
        id: None,
        updated_at: None,
        updated_by: None,
        version: None,
        dashboard: DashboardDocument {
            name: Some("d".into()),
            charts: vec![kind.to_chart(base)],
            ..Default::default()
        },
    }
}

/// Apl `TileAplFinished` handler on a `Line` viz kind routes the
/// response into `entry.series` (decoder produces Vec<Series>).
#[test]
fn apl_tile_finished_on_line_kind_populates_series() {
    let mut app = test_app();
    app.handle_event(AppEvent::DashboardOpened {
        uid: "u".into(),
        result: Ok(one_chart_dashboard_apl_kind(
            crate::dashboard::VizKind::Line,
            "['logs'] | summarize n=count() by bin(_time, 1h)",
        )),
    });
    // Pre-seed the in-flight tile entry so the handler's
    // "slot must exist" guard passes (mimics run_tile_queries).
    app.tile_results
        .insert("c1".into(), crate::app::types::TileQueryResult::default());
    let resp = apl_table_response(APL_TIME_SERIES_FIXTURE);
    app.handle_event(AppEvent::TileAplFinished {
        chart_id: "c1".into(),
        epoch: app.tile_query_epoch,
        result: Ok(resp),
    });
    let entry = app.tile_results.get("c1").expect("entry present");
    assert!(entry.error.is_none(), "unexpected error: {:?}", entry.error);
    assert_eq!(entry.series.len(), 1);
    assert!(entry.table.is_none());
    assert_eq!(entry.series[0].points.len(), 3);
}

/// On a `Table` viz kind the handler routes the response into
/// `entry.table` so the grid renderer shows the raw columns.
#[test]
fn apl_tile_finished_on_table_kind_populates_table() {
    let mut app = test_app();
    app.handle_event(AppEvent::DashboardOpened {
        uid: "u".into(),
        result: Ok(one_chart_dashboard_apl_kind(
            crate::dashboard::VizKind::Table,
            "['logs'] | summarize count() by level",
        )),
    });
    app.tile_results
        .insert("c1".into(), crate::app::types::TileQueryResult::default());
    let resp = apl_table_response(APL_TWO_COLUMN_FIXTURE);
    app.handle_event(AppEvent::TileAplFinished {
        chart_id: "c1".into(),
        epoch: app.tile_query_epoch,
        result: Ok(resp),
    });
    let entry = app.tile_results.get("c1").expect("entry present");
    assert!(entry.error.is_none(), "unexpected error: {:?}", entry.error);
    let table = entry.table.as_ref().expect("table populated");
    assert_eq!(table.columns, vec!["level", "n"]);
    assert_eq!(table.rows.len(), 2);
    assert!(entry.series.is_empty());
}

/// A series-kind tile whose APL response has no time column surfaces
/// the decoder's error in `entry.error` (no silent placeholder).
#[test]
fn apl_tile_finished_decoder_error_surfaces_in_entry() {
    let mut app = test_app();
    app.handle_event(AppEvent::DashboardOpened {
        uid: "u".into(),
        result: Ok(one_chart_dashboard_apl_kind(
            crate::dashboard::VizKind::Line,
            "['logs'] | summarize n=count() by level",
        )),
    });
    app.tile_results
        .insert("c1".into(), crate::app::types::TileQueryResult::default());
    // Fixture has only `level` + `n` — no time column. Series
    // decoder must reject and the handler surfaces the message.
    let resp = apl_table_response(APL_TWO_COLUMN_FIXTURE);
    app.handle_event(AppEvent::TileAplFinished {
        chart_id: "c1".into(),
        epoch: app.tile_query_epoch,
        result: Ok(resp),
    });
    let entry = app.tile_results.get("c1").expect("entry present");
    let err = entry.error.as_deref().expect("error populated");
    assert!(err.starts_with("APL:"), "err: {err}");
    assert!(err.contains("time column"), "err: {err}");
}

/// Stale (epoch mismatch) results are dropped silently — protects
/// against late results from a previous dashboard run overwriting a
/// fresh tile with the same id.
#[test]
fn apl_tile_finished_drops_stale_epoch() {
    let mut app = test_app();
    app.handle_event(AppEvent::DashboardOpened {
        uid: "u".into(),
        result: Ok(one_chart_dashboard_apl_kind(
            crate::dashboard::VizKind::Line,
            "['logs'] | count",
        )),
    });
    app.tile_results
        .insert("c1".into(), crate::app::types::TileQueryResult::default());
    let resp = apl_table_response(APL_TIME_SERIES_FIXTURE);
    let stale_epoch = app.tile_query_epoch.wrapping_sub(1);
    app.handle_event(AppEvent::TileAplFinished {
        chart_id: "c1".into(),
        epoch: stale_epoch,
        result: Ok(resp),
    });
    let entry = app.tile_results.get("c1").expect("entry preserved");
    assert!(entry.series.is_empty());
    assert!(entry.table.is_none());
    assert!(entry.error.is_none());
}

// Fixtures used by the dispatch tests above. Same shape as the
// decoder fixtures in `src/viz/apl_decode.rs`, kept inline so the
// test bodies stay self-contained.
const APL_TIME_SERIES_FIXTURE: &str = r#"{
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

const APL_TWO_COLUMN_FIXTURE: &str = r#"{
  "tables": [{
    "name": "0",
    "sources": [{"name": "logs"}],
    "fields": [
      {"name": "level", "type": "string"},
      {"name": "n", "type": "long"}
    ],
    "order": [],
    "groups": [{"name": "level"}],
    "range": null,
    "buckets": null,
    "columns": [
      ["error", "info"],
      [3, 75]
    ]
  }]
}"#;

/// Regression: loading a server-authored dashboard whose `apl`-keyed
/// query is genuinely APL on a non-LogStream chart kind must show
/// `APL` in the status bar and **not** flag MPL syntax errors on the
/// raw APL text. Two bugs were fixed at once: (a) the kind-fallback
/// mis-classified bracketed APL as MPL, (b) `recompute_diagnostics`
/// ran the MPL analyzer regardless of language.
#[test]
fn loading_apl_dashboard_does_not_report_mpl_errors() {
    use crate::axiom::{Chart, ChartBase, DashboardDocument, KnownChart};
    let mut app = test_app();
    let chart = Chart::Known(KnownChart::TimeSeries(ChartBase {
        id: "c1".into(),
        name: Some("errors-per-hour".into()),
        query: Some(serde_json::json!({
            "apl": "['logs'] | summarize n=count() by bin(_time, 1h)"
        })),
        extras: Default::default(),
    }));
    let resource = crate::axiom::DashboardSummary {
        uid: "u".into(),
        id: None,
        updated_at: None,
        updated_by: None,
        version: None,
        dashboard: DashboardDocument {
            name: Some("d".into()),
            charts: vec![chart],
            ..Default::default()
        },
    };
    app.handle_event(AppEvent::DashboardOpened {
        uid: "u".into(),
        result: Ok(resource),
    });
    // Language: APL (sniff caught the bracket prefix).
    assert_eq!(app.active_lang(), Lang::Apl);
    // No MPL diagnostics on an APL buffer.
    assert!(
        app.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        app.diagnostics
    );
    // Simulating a keystroke must not reintroduce MPL errors.
    app.recompute_diagnostics();
    assert!(
        app.diagnostics.is_empty(),
        "keystroke re-introduced MPL diagnostics: {:?}",
        app.diagnostics
    );
}

#[test]
fn active_lang_follows_focused_tile_in_dashboard_mode() {
    // Three-tile dashboard: tile 0 MPL, tile 1 APL (sidecar), tile 2 MPL.
    // Moving focus updates `active_lang` accordingly.
    let mut app = test_app();
    let mut doc = crate::axiom::DashboardDocument {
        name: Some("d".into()),
        ..Default::default()
    };
    doc.charts = vec![
        timeseries_with_query(serde_json::json!({ "mpl": "a:rate" })),
        timeseries_with_query_and_lang(serde_json::json!({ "apl": "['logs'] | count" }), Lang::Apl),
        timeseries_with_query(serde_json::json!({ "mpl": "c:rate" })),
    ];
    let resource = crate::axiom::DashboardSummary {
        uid: "u".into(),
        id: None,
        updated_at: None,
        updated_by: None,
        version: None,
        dashboard: doc,
    };
    app.handle_event(AppEvent::DashboardOpened {
        uid: "u".into(),
        result: Ok(resource),
    });
    assert_eq!(app.active_lang(), Lang::Mpl);
    app.set_focused_chart(1);
    assert_eq!(app.active_lang(), Lang::Apl);
    app.set_focused_chart(2);
    assert_eq!(app.active_lang(), Lang::Mpl);
}
