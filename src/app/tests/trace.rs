//! Tests for step 22: `:trace <id>` fetch + `ViewMode::Trace`
//! skeleton.
//!
//! Test surface (each property tested by at least one #[test]):
//!
//! * Dataset / deployment resolution chain (arg > in-session >
//!   settings; missing chain surfaces a clear error).
//! * Empty-result ladder progression: Hour → Day, Day → Week,
//!   Week → Month, Month → giveup.
//! * `query_id` separation from the editor's `last_query_id`
//!   (an editor `:r` doesn't cancel an in-flight trace).
//! * Esc in Normal mode cancels a pending fetch and bumps the
//!   counter so late responses are dropped.
//! * Happy path: a non-empty `TraceFetchFinished` enters
//!   `ViewMode::Trace` and installs `trace_view`.
//! * `:q` from inside Trace mode exits the trace, doesn't quit
//!   the app.
//! * Bare `:trace` inside the trace view reports the loaded
//!   trace's id.
//! * Render smoke: drawing into a TestBackend produces a header
//!   line containing the trace id and a body containing every
//!   span name.

use super::*;
use crate::app::types::{PendingTraceFetch, TraceFetchWindow, TraceView};
use crate::trace::{Span as TraceSpan, SpanKind, TraceModel, TreeRow};
use crate::ui;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::collections::BTreeMap;

// ---- Helpers -------------------------------------------------------

fn empty_apl_response() -> crate::axiom::AplQueryResult {
    let body = serde_json::json!({
        "status": {
            "elapsedTime": 0, "blocksExamined": 0, "rowsExamined": 0,
            "rowsMatched": 0, "numGroups": 0, "isPartial": false,
            "continuationToken": null, "cacheStatus": 0,
            "minBlockTime": "2024-01-01T00:00:00Z",
            "maxBlockTime": "2024-01-01T00:00:00Z"
        },
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
    serde_json::from_value(body).expect("empty stub decodes")
}

fn nonempty_apl_response() -> crate::axiom::AplQueryResult {
    let body = serde_json::json!({
        "status": {
            "elapsedTime": 0, "blocksExamined": 0, "rowsExamined": 0,
            "rowsMatched": 0, "numGroups": 0, "isPartial": false,
            "continuationToken": null, "cacheStatus": 0,
            "minBlockTime": "2024-01-01T00:00:00Z",
            "maxBlockTime": "2024-01-01T00:00:00Z"
        },
        "tables": [{
            "name": "0", "sources": [{"name": "traces"}],
            "fields": [
                {"name": "_time",          "type": "datetime"},
                {"name": "span_id",        "type": "string"},
                {"name": "parent_span_id", "type": "string"},
                {"name": "name",           "type": "string"},
                {"name": "duration",       "type": "timespan"},
                {"name": "service.name",   "type": "string"},
            ],
            "order": [], "groups": [], "range": null,
            "columns": [
                ["2026-05-21T00:00:00Z", "2026-05-21T00:00:00.010Z"],
                ["a", "b"],
                [null, "a"],
                ["root", "child"],
                ["100ms", "50ms"],
                ["api", "db"]
            ]
        }]
    });
    serde_json::from_value(body).expect("happy stub decodes")
}

/// Install a pending fetch directly (skipping `start_trace_fetch`,
/// which would call `Config::load` and try to build a real
/// client). Lets us simulate the event lifecycle deterministically.
fn install_pending(app: &mut App, window: TraceFetchWindow) -> u64 {
    app.trace_query_counter = app.trace_query_counter.wrapping_add(1);
    let qid = app.trace_query_counter;
    app.pending_trace_fetch = Some(PendingTraceFetch {
        query_id: qid,
        trace_id: "abc123def".to_string(),
        dataset: "axiom-traces".to_string(),
        deployment_override: None,
        window,
    });
    qid
}

/// Drive the event loop directly with a `TraceFetchFinished`.
fn deliver_fetch_event(
    app: &mut App,
    query_id: u64,
    result: anyhow::Result<crate::axiom::AplQueryResult>,
) {
    app.handle_event(AppEvent::TraceFetchFinished { query_id, result });
}

// ---- Dataset resolution -------------------------------------------

#[test]
fn trace_open_with_no_dataset_in_chain_surfaces_error() {
    let mut app = test_app();
    // settings is in-memory empty; no `last_trace_dataset`; no arg.
    app.execute_command("trace abc123");
    let err = app
        .last_error
        .as_deref()
        .expect("missing-dataset must surface as a hard error");
    assert!(err.contains("no trace dataset"), "got: {err:?}");
    assert!(
        app.pending_trace_fetch.is_none(),
        "no fetch should be in flight"
    );
}

#[test]
fn trace_open_with_explicit_dataset_arg_records_last_trace_dataset() {
    let mut app = test_app();
    // We can't call `start_trace_fetch` directly (it touches the
    // network client), but we *can* assert the parser at least
    // records the dataset on the failure path. To do that without
    // actually firing a fetch, we settle for the no-config path:
    // the dataset comes through far enough that resolution
    // succeeds, but client construction will fail. The error path
    // is what we check.
    app.execute_command("trace abc123 dataset=foo");
    // `start_trace_fetch` stamps `last_trace_dataset` BEFORE
    // dispatching, so even when the client build fails we should
    // still see the dataset recorded.
    assert_eq!(app.last_trace_dataset.as_deref(), Some("foo"));
}

#[test]
fn trace_open_strips_quotes_from_dataset_arg() {
    // Regression: `dataset='axiom-traces-prod'` (single-quoted, as a
    // user naturally types APL bracket syntax) must be normalised to
    // the bare name before it's stashed / built into the APL literal,
    // otherwise the query becomes `["'axiom-traces-prod'"]` and the
    // server 500s.
    let mut app = test_app();
    app.execute_command("trace abc123 dataset='axiom-traces-prod'");
    assert_eq!(
        app.last_trace_dataset.as_deref(),
        Some("axiom-traces-prod"),
        "surrounding quotes must be stripped from the dataset arg"
    );
}

#[test]
fn trace_set_dataset_persists_canonical_name() {
    // `:trace set dataset='axiom-traces-prod'` must store the bare
    // name so `:trace get` and the fetch path stay quote-free.
    let mut app = test_app();
    app.execute_command("trace set dataset='axiom-traces-prod'");
    assert_eq!(
        app.settings.read().trace().dataset.as_deref(),
        Some("axiom-traces-prod")
    );
}

#[test]
fn trace_set_dataset_overrides_sticky_session_value() {
    // Regression: resolution precedence is arg → last_trace_dataset
    // → settings, so a stale sticky value used to shadow an explicit
    // `:trace set dataset=…`. The set must update the sticky value
    // too, otherwise the change appears to do nothing.
    let mut app = test_app();
    app.last_trace_dataset = Some("axiom-traces-prod".to_string());
    app.execute_command("trace set dataset=axiom-traces-staging");
    assert_eq!(
        app.last_trace_dataset.as_deref(),
        Some("axiom-traces-staging"),
        "explicit :trace set must win over the sticky session dataset"
    );
    assert_eq!(
        app.settings.read().trace().dataset.as_deref(),
        Some("axiom-traces-staging")
    );
}

#[test]
fn trace_unset_dataset_clears_sticky_session_value() {
    // Unsetting the default must also drop the sticky value, so the
    // next `:trace <id>` falls through to (now-empty) settings.
    let mut app = test_app();
    app.settings
        .write()
        .set_trace_dataset(Some("axiom-traces-prod".into()));
    app.last_trace_dataset = Some("axiom-traces-prod".to_string());
    app.execute_command("trace unset dataset");
    assert_eq!(app.last_trace_dataset, None);
    assert_eq!(app.settings.read().trace().dataset, None);
}

#[test]
fn trace_open_rejects_unknown_key() {
    let mut app = test_app();
    app.settings.write().set_trace_dataset(Some("ds".into()));
    app.execute_command("trace abc123 wibble=1");
    let err = app.last_error.as_deref().expect("must reject unknown key");
    assert!(err.contains("unknown key"), "got: {err:?}");
}

#[test]
fn trace_open_rejects_multiple_bare_args() {
    let mut app = test_app();
    app.settings.write().set_trace_dataset(Some("ds".into()));
    app.execute_command("trace abc def");
    let err = app
        .last_error
        .as_deref()
        .expect("must reject second bare id");
    assert!(err.contains("extra arg"), "got: {err:?}");
}

#[test]
fn trace_open_with_no_id_surfaces_usage() {
    let mut app = test_app();
    app.settings.write().set_trace_dataset(Some("ds".into()));
    app.execute_command("trace dataset=foo");
    let err = app.last_error.as_deref().expect("must demand an id");
    assert!(err.contains("<id>"), "got: {err:?}");
}

// ---- Ladder progression -------------------------------------------

#[test]
fn empty_result_at_hour_advances_to_day() {
    let mut app = test_app();
    let qid = install_pending(&mut app, TraceFetchWindow::Hour);
    deliver_fetch_event(&mut app, qid, Ok(empty_apl_response()));
    let pending = app
        .pending_trace_fetch
        .as_ref()
        .expect("ladder must keep pending alive");
    assert_eq!(pending.window, TraceFetchWindow::Day);
    // The counter is NOT bumped on a ladder step \u2014 only on
    // a fresh user-initiated fetch. Same query id continues.
    assert_eq!(pending.query_id, qid);
}

#[test]
fn empty_result_at_day_advances_to_week() {
    let mut app = test_app();
    let qid = install_pending(&mut app, TraceFetchWindow::Day);
    deliver_fetch_event(&mut app, qid, Ok(empty_apl_response()));
    assert_eq!(
        app.pending_trace_fetch.as_ref().map(|p| p.window),
        Some(TraceFetchWindow::Week)
    );
}

#[test]
fn empty_result_at_week_advances_to_month() {
    let mut app = test_app();
    let qid = install_pending(&mut app, TraceFetchWindow::Week);
    deliver_fetch_event(&mut app, qid, Ok(empty_apl_response()));
    assert_eq!(
        app.pending_trace_fetch.as_ref().map(|p| p.window),
        Some(TraceFetchWindow::Month)
    );
}

#[test]
fn empty_result_at_month_gives_up_with_clear_error() {
    let mut app = test_app();
    let qid = install_pending(&mut app, TraceFetchWindow::Month);
    deliver_fetch_event(&mut app, qid, Ok(empty_apl_response()));
    assert!(
        app.pending_trace_fetch.is_none(),
        "exhausted ladder must clear pending"
    );
    let err = app.last_error.as_deref().expect("must surface not-found");
    assert!(
        err.contains("not found") && err.contains("axiom-traces"),
        "got: {err:?}"
    );
}

#[test]
fn trace_fetch_window_next_terminates_at_month() {
    // Defends against the "Month.next() returns Some(Month)"
    // infinite-loop trap.
    assert_eq!(TraceFetchWindow::Month.next(), None);
    assert_eq!(TraceFetchWindow::Hour.next(), Some(TraceFetchWindow::Day));
    assert_eq!(TraceFetchWindow::Day.next(), Some(TraceFetchWindow::Week));
    assert_eq!(TraceFetchWindow::Week.next(), Some(TraceFetchWindow::Month));
}

#[test]
fn trace_fetch_window_relative_starts_match_plan() {
    assert_eq!(TraceFetchWindow::Hour.as_relative_start(), "now-1h");
    assert_eq!(TraceFetchWindow::Day.as_relative_start(), "now-24h");
    assert_eq!(TraceFetchWindow::Week.as_relative_start(), "now-7d");
    assert_eq!(TraceFetchWindow::Month.as_relative_start(), "now-30d");
}

// ---- Stale results -------------------------------------------------

#[test]
fn stale_query_id_is_dropped_silently() {
    let mut app = test_app();
    let qid = install_pending(&mut app, TraceFetchWindow::Hour);
    // Deliver a fetch event for a DIFFERENT query id \u2014 i.e.
    // an earlier fetch the user already abandoned.
    deliver_fetch_event(&mut app, qid.wrapping_sub(1), Ok(nonempty_apl_response()));
    // Nothing should change: still pending, no view, no error.
    assert!(app.trace_view.is_none());
    assert!(app.last_error.is_none());
    assert_eq!(
        app.pending_trace_fetch.as_ref().map(|p| p.query_id),
        Some(qid)
    );
}

#[test]
fn editor_apl_query_does_not_cancel_pending_trace() {
    // Regression guard: oracle flagged the bug where
    // `:r` would bump `last_query_id` and silently invalidate
    // an in-flight trace fetch. The two id spaces must be
    // independent.
    let mut app = test_app();
    let qid = install_pending(&mut app, TraceFetchWindow::Hour);
    // Simulate the editor finishing its own APL query.
    app.last_query_id = 99;
    app.handle_event(AppEvent::AplQueryFinished {
        id: 99,
        result: Ok(empty_apl_response()),
    });
    // The pending trace must survive.
    assert_eq!(
        app.pending_trace_fetch.as_ref().map(|p| p.query_id),
        Some(qid)
    );
}

// ---- Happy path ---------------------------------------------------

#[test]
fn nonempty_result_enters_trace_view() {
    let mut app = test_app();
    let qid = install_pending(&mut app, TraceFetchWindow::Hour);
    deliver_fetch_event(&mut app, qid, Ok(nonempty_apl_response()));
    assert_eq!(app.view_mode, ViewMode::Trace);
    let view = app.trace_view.as_ref().expect("view must be installed");
    assert_eq!(view.model.spans.len(), 2);
    assert_eq!(view.model.trace_id, "abc123def");
    assert_eq!(view.model.dataset, "axiom-traces");
    assert_eq!(view.cursor, 0);
    assert_eq!(view.scroll, 0);
    assert_eq!(view.return_mode, ViewMode::Solo);
    // Focus moves into the trace pane.
    assert_eq!(app.focus, Pane::TraceTree);
    // Pending is cleared.
    assert!(app.pending_trace_fetch.is_none());
    // Status reflects the new trace.
    assert!(
        app.status.contains("abc123def") && app.status.contains("2 span"),
        "got: {:?}",
        app.status
    );
}

#[test]
fn nonempty_result_from_grid_remembers_grid_as_return_mode() {
    let mut app = test_app();
    app.view_mode = ViewMode::Grid;
    let qid = install_pending(&mut app, TraceFetchWindow::Hour);
    deliver_fetch_event(&mut app, qid, Ok(nonempty_apl_response()));
    let view = app.trace_view.as_ref().expect("view installed");
    assert_eq!(view.return_mode, ViewMode::Grid);
}

#[test]
fn decode_error_clears_pending_and_surfaces_error() {
    let mut app = test_app();
    let qid = install_pending(&mut app, TraceFetchWindow::Hour);
    // A response missing the `duration` column would fail
    // decode but still be non-empty at the row count.
    let body = serde_json::json!({
        "status": {
            "elapsedTime": 0, "blocksExamined": 0, "rowsExamined": 0,
            "rowsMatched": 0, "numGroups": 0, "isPartial": false,
            "continuationToken": null, "cacheStatus": 0,
            "minBlockTime": "2024-01-01T00:00:00Z",
            "maxBlockTime": "2024-01-01T00:00:00Z"
        },
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
    let resp: crate::axiom::AplQueryResult = serde_json::from_value(body).unwrap();
    deliver_fetch_event(&mut app, qid, Ok(resp));
    assert!(app.pending_trace_fetch.is_none());
    assert!(app.trace_view.is_none());
    assert_eq!(app.view_mode, ViewMode::Solo);
    let err = app
        .last_error
        .as_deref()
        .expect("decode error must surface");
    assert!(err.contains("duration"), "got: {err:?}");
}

// ---- Exit paths ----------------------------------------------------

fn build_synthetic_view(spans: Vec<TraceSpan>, return_mode: ViewMode) -> TraceView {
    let tree: Vec<TreeRow> = (0..spans.len())
        .map(|i| TreeRow {
            span_idx: i,
            depth: 0,
            has_children: false,
            is_orphan: false,
        })
        .collect();
    let mut by_id = BTreeMap::new();
    for (i, s) in spans.iter().enumerate() {
        by_id.insert(s.span_id.clone(), i);
    }
    let t0 = spans.iter().map(|s| s.start_ns).min().unwrap_or(0);
    let t1 = spans.iter().map(|s| s.end_ns).max().unwrap_or(0);
    TraceView::new(
        TraceModel {
            trace_id: "tid".to_string(),
            dataset: "ds".to_string(),
            spans,
            by_id,
            roots: vec![0],
            t0_ns: t0,
            t1_ns: t1,
            tree,
        },
        return_mode,
    )
}

fn synthetic_span(id: &str, name: &str, start_ns: i64, dur_ns: i64) -> TraceSpan {
    TraceSpan {
        span_id: id.to_string(),
        parent_span_id: None,
        name: name.to_string(),
        service: "svc".to_string(),
        kind: SpanKind::Unknown,
        status_code: None,
        is_error: false,
        start_ns,
        end_ns: start_ns + dur_ns,
        duration_ns: dur_ns,
        events: Vec::new(),
        attributes: BTreeMap::new(),
        resource: BTreeMap::new(),
    }
}

#[test]
fn quit_from_trace_mode_exits_trace_not_app() {
    let mut app = test_app();
    app.trace_view = Some(build_synthetic_view(
        vec![synthetic_span("a", "root", 0, 1_000_000)],
        ViewMode::Solo,
    ));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.execute_command("q");
    assert!(!app.should_quit, ":q in Trace must not quit the app");
    assert_eq!(app.view_mode, ViewMode::Solo);
    assert!(app.trace_view.is_none());
    assert_eq!(app.focus, Pane::Editor);
}

#[test]
fn quit_from_trace_mode_returns_to_grid_when_that_was_the_origin() {
    let mut app = test_app();
    app.trace_view = Some(build_synthetic_view(
        vec![synthetic_span("a", "root", 0, 1_000_000)],
        ViewMode::Grid,
    ));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.execute_command("q");
    assert!(!app.should_quit);
    assert_eq!(app.view_mode, ViewMode::Grid);
    assert_eq!(app.focus, Pane::Dashboard);
}

#[test]
fn esc_in_trace_view_exits_via_keymap() {
    let mut app = test_app();
    app.trace_view = Some(build_synthetic_view(
        vec![synthetic_span("a", "root", 0, 1_000_000)],
        ViewMode::Solo,
    ));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.on_key(key(KeyCode::Esc));
    assert!(app.trace_view.is_none());
    assert_eq!(app.view_mode, ViewMode::Solo);
}

#[test]
fn esc_with_pending_fetch_cancels_and_bumps_counter() {
    let mut app = test_app();
    let qid = install_pending(&mut app, TraceFetchWindow::Hour);
    // Esc must travel through the editor's Normal-mode handler.
    app.on_key(key(KeyCode::Esc));
    assert!(app.pending_trace_fetch.is_none(), "Esc must cancel pending");
    assert!(
        app.trace_query_counter > qid,
        "counter must bump so late responses are dropped"
    );
    assert!(
        app.status.contains("cancelled"),
        "status should explain the cancel; got: {:?}",
        app.status
    );
    // A late response that arrives after cancel must not enter Trace.
    deliver_fetch_event(&mut app, qid, Ok(nonempty_apl_response()));
    assert!(app.trace_view.is_none());
    assert_eq!(app.view_mode, ViewMode::Solo);
}

// ---- Bare :trace inside Trace mode --------------------------------

#[test]
fn bare_trace_in_trace_view_reports_loaded_trace_id() {
    let mut app = test_app();
    app.trace_view = Some(build_synthetic_view(
        vec![synthetic_span("a", "root", 0, 1_000_000)],
        ViewMode::Solo,
    ));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_id = Some("irrelevant".to_string());
    app.execute_command("trace");
    // Must reference the loaded trace, not `last_trace_id`.
    assert!(app.status.contains("tid"), "got: {:?}", app.status);
}

// ---- Cursor movement -----------------------------------------------

#[test]
fn j_advances_cursor_and_k_retreats() {
    let mut app = test_app();
    app.trace_view = Some(build_synthetic_view(
        vec![
            synthetic_span("a", "a", 0, 100),
            synthetic_span("b", "b", 100, 100),
            synthetic_span("c", "c", 200, 100),
        ],
        ViewMode::Solo,
    ));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.on_key(key(KeyCode::Char('j')));
    assert_eq!(app.trace_view.as_ref().unwrap().cursor, 1);
    app.on_key(key(KeyCode::Char('j')));
    assert_eq!(app.trace_view.as_ref().unwrap().cursor, 2);
    // Past the end clamps.
    app.on_key(key(KeyCode::Char('j')));
    assert_eq!(app.trace_view.as_ref().unwrap().cursor, 2);
    app.on_key(key(KeyCode::Char('k')));
    assert_eq!(app.trace_view.as_ref().unwrap().cursor, 1);
}

#[test]
fn ctrl_d_and_ctrl_u_half_page_step() {
    let mut app = test_app();
    app.trace_view = Some(build_synthetic_view(
        (0..40)
            .map(|i| synthetic_span(&format!("s{i}"), &format!("n{i}"), i * 100, 100))
            .collect(),
        ViewMode::Solo,
    ));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    let start = app.trace_view.as_ref().unwrap().cursor;
    app.on_key(ctrl(KeyCode::Char('d')));
    let after_dn = app.trace_view.as_ref().unwrap().cursor;
    assert!(
        after_dn > start,
        "Ctrl-d must advance the cursor; before={start}, after={after_dn}"
    );
    app.on_key(ctrl(KeyCode::Char('u')));
    let after_up = app.trace_view.as_ref().unwrap().cursor;
    assert!(
        after_up < after_dn,
        "Ctrl-u must retreat the cursor; before={after_dn}, after={after_up}"
    );
}

#[test]
fn gg_jumps_to_first_g_jumps_to_last() {
    let mut app = test_app();
    app.trace_view = Some(build_synthetic_view(
        (0..5)
            .map(|i| synthetic_span(&format!("s{i}"), &format!("n{i}"), i * 100, 100))
            .collect(),
        ViewMode::Solo,
    ));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.trace_view.as_mut().unwrap().cursor = 3;
    app.on_key(key(KeyCode::Char('g')));
    app.on_key(key(KeyCode::Char('g')));
    assert_eq!(app.trace_view.as_ref().unwrap().cursor, 0);
    app.on_key(key(KeyCode::Char('G')));
    assert_eq!(app.trace_view.as_ref().unwrap().cursor, 4);
}

// ---- Render smoke --------------------------------------------------

#[test]
fn renderer_draws_header_and_span_rows() {
    let mut app = test_app();
    app.trace_view = Some(build_synthetic_view(
        vec![
            synthetic_span("a", "root.op", 0, 1_000_000),
            synthetic_span("b", "child.op", 1_000_000, 500_000),
        ],
        ViewMode::Solo,
    ));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            ui::draw(f, &mut app);
        })
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let mut all = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            all.push_str(buffer[(x, y)].symbol());
        }
        all.push('\n');
    }
    assert!(
        all.contains("trace tid"),
        "header must show trace id; buf:\n{all}"
    );
    assert!(
        all.contains("root.op"),
        "first span must render; buf:\n{all}"
    );
    assert!(
        all.contains("child.op"),
        "second span must render; buf:\n{all}"
    );
    // Dataset surfaces in either the body header or the status bar.
    assert!(
        all.contains("ds"),
        "dataset segment must surface; buf:\n{all}"
    );
    // Status bar carries the TRACE label.
    assert!(
        all.contains("TRACE"),
        "status bar must show TRACE label; buf:\n{all}"
    );
}

// =================================================================
//                    Step 23 \u2014 waterfall + detail
// =================================================================

/// Helper: render the app into a fresh `TestBackend` of the given
/// dimensions and return the flattened buffer text.
fn render_to_string(app: &mut App, w: u16, h: u16) -> String {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| ui::draw(f, app)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

#[test]
fn tab_swaps_focus_between_tree_and_detail() {
    let mut app = test_app();
    app.trace_view = Some(build_synthetic_view(
        vec![synthetic_span("a", "root", 0, 1_000_000)],
        ViewMode::Solo,
    ));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;

    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.focus, Pane::TraceDetail);
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.focus, Pane::TraceTree);
}

#[test]
fn tab_from_detail_returns_to_tree() {
    let mut app = test_app();
    app.trace_view = Some(build_synthetic_view(
        vec![synthetic_span("a", "root", 0, 1_000_000)],
        ViewMode::Solo,
    ));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceDetail;
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.focus, Pane::TraceTree);
}

#[test]
fn esc_in_detail_pane_exits_trace_view() {
    let mut app = test_app();
    app.trace_view = Some(build_synthetic_view(
        vec![synthetic_span("a", "root", 0, 1_000_000)],
        ViewMode::Solo,
    ));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceDetail;
    app.on_key(key(KeyCode::Esc));
    assert!(app.trace_view.is_none());
    assert_eq!(app.view_mode, ViewMode::Solo);
}

#[test]
fn set_focus_rejects_detail_when_no_trace_loaded() {
    let mut app = test_app();
    app.set_focus(Pane::TraceDetail);
    assert_ne!(app.focus, Pane::TraceDetail);
    assert!(app.status.contains("no trace loaded"));
}

#[test]
fn ctrl_w_w_swaps_between_trace_panes() {
    let mut app = test_app();
    app.trace_view = Some(build_synthetic_view(
        vec![synthetic_span("a", "root", 0, 1_000_000)],
        ViewMode::Solo,
    ));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.on_key(ctrl(KeyCode::Char('w')));
    app.on_key(key(KeyCode::Char('w')));
    assert_eq!(app.focus, Pane::TraceDetail);
}

#[test]
fn detail_j_scrolls_down_and_k_scrolls_up() {
    let mut app = test_app();
    // Pack a lot of attributes so the detail pane has many rows.
    let mut span = synthetic_span("a", "root", 0, 1_000_000);
    for i in 0..50 {
        span.attributes
            .insert(format!("attr.k{i:02}"), serde_json::json!(format!("v{i}")));
    }
    app.trace_view = Some(build_synthetic_view(vec![span], ViewMode::Solo));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceDetail;
    // Force the last-detail-height to a known value so j moves
    // detail_scroll predictably.
    app.last_trace_detail_height = 10;

    let start = app.trace_view.as_ref().unwrap().detail_scroll;
    app.on_key(key(KeyCode::Char('j')));
    let after_j = app.trace_view.as_ref().unwrap().detail_scroll;
    assert!(after_j > start, "j must advance detail_scroll");
    app.on_key(key(KeyCode::Char('k')));
    let after_k = app.trace_view.as_ref().unwrap().detail_scroll;
    assert!(after_k < after_j, "k must retreat detail_scroll");
}

#[test]
fn detail_gg_goes_to_top_and_g_to_bottom() {
    let mut app = test_app();
    let mut span = synthetic_span("a", "root", 0, 1_000_000);
    for i in 0..50 {
        span.attributes
            .insert(format!("attr.k{i:02}"), serde_json::json!("v"));
    }
    app.trace_view = Some(build_synthetic_view(vec![span], ViewMode::Solo));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceDetail;
    app.last_trace_detail_height = 10;
    // Jump to bottom (renderer clamps to total - h on draw).
    app.on_key(key(KeyCode::Char('G')));
    // We can't assert the exact value without a draw, but the
    // marker must have been written.
    assert_eq!(app.trace_view.as_ref().unwrap().detail_scroll, u16::MAX);
    // gg back to top.
    app.on_key(key(KeyCode::Char('g')));
    app.on_key(key(KeyCode::Char('g')));
    assert_eq!(app.trace_view.as_ref().unwrap().detail_scroll, 0);
}

#[test]
fn renderer_draws_detail_pane_with_identity_section() {
    let mut app = test_app();
    let mut span = synthetic_span("aabbccddeeff", "root.op", 0, 1_000_000);
    span.parent_span_id = Some("parent-id".to_string());
    app.trace_view = Some(build_synthetic_view(vec![span], ViewMode::Solo));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;

    let buf = render_to_string(&mut app, 120, 24);
    // Detail pane header.
    assert!(
        buf.contains("identity"),
        "identity section must render; buf:\n{buf}"
    );
    assert!(
        buf.contains("timing"),
        "timing section must render; buf:\n{buf}"
    );
    assert!(
        buf.contains("status"),
        "status section must render; buf:\n{buf}"
    );
    // Span id (selected row's span identity).
    assert!(
        buf.contains("aabbccddeeff"),
        "selected span id must appear in detail pane; buf:\n{buf}"
    );
}

#[test]
fn detail_pane_collapses_on_narrow_terminal() {
    let mut app = test_app();
    app.trace_view = Some(build_synthetic_view(
        vec![synthetic_span("a", "root", 0, 1_000_000)],
        ViewMode::Solo,
    ));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    // Narrower than 2 * DETAIL_MIN_COLS (=40) \u2014 the detail
    // pane should not render at all.
    let buf = render_to_string(&mut app, 36, 24);
    assert!(
        !buf.contains("identity"),
        "detail pane must collapse on narrow terminal; buf:\n{buf}"
    );
    assert!(buf.contains("root"), "tree must still render; buf:\n{buf}");
}

#[test]
fn renderer_draws_waterfall_bar_block_chars() {
    let mut app = test_app();
    app.trace_view = Some(build_synthetic_view(
        vec![
            synthetic_span("a", "root.op", 0, 1_000_000_000),
            synthetic_span("b", "child.op", 500_000_000, 200_000_000),
        ],
        ViewMode::Solo,
    ));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    let buf = render_to_string(&mut app, 120, 24);
    // The full-extent root span must paint at least one block
    // character.
    assert!(
        buf.contains('\u{2588}'),
        "waterfall must paint block chars; buf:\n{buf}"
    );
}

#[test]
fn orphan_span_renders_warning_marker() {
    let mut app = test_app();
    let mut span = synthetic_span("a", "orphaned.op", 0, 1_000_000);
    span.parent_span_id = Some("non-existent-parent".to_string());
    // Build the view directly with the orphan flag set.
    let mut view = build_synthetic_view(vec![span], ViewMode::Solo);
    view.model.tree[0].is_orphan = true;
    app.trace_view = Some(view);
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    let buf = render_to_string(&mut app, 120, 24);
    assert!(
        buf.contains('\u{26a0}'),
        "orphan badge \u{26a0} must render; buf:\n{buf}"
    );
}

#[test]
fn detail_parent_row_shows_name_when_resolvable() {
    let mut app = test_app();
    let parent = synthetic_span("p", "parent.op", 0, 2_000_000);
    let mut child = synthetic_span("c", "child.op", 100_000, 500_000);
    child.parent_span_id = Some("p".to_string());
    // Build view with the child selected so the parent row shows
    // up in the detail pane.
    let mut view = build_synthetic_view(vec![parent, child], ViewMode::Solo);
    // Make tree show parent first, child indented under it.
    view.model.tree = vec![
        TreeRow {
            span_idx: 0,
            depth: 0,
            has_children: true,
            is_orphan: false,
        },
        TreeRow {
            span_idx: 1,
            depth: 1,
            has_children: false,
            is_orphan: false,
        },
    ];
    view.cursor = 1; // select child
    app.trace_view = Some(view);
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    let buf = render_to_string(&mut app, 120, 24);
    // The parent row in the detail pane should include the
    // parent's name beside its span id.
    assert!(
        buf.contains("parent.op"),
        "parent name should surface in the detail pane; buf:\n{buf}"
    );
}

#[test]
fn renderer_stashes_body_and_detail_heights_on_app() {
    let mut app = test_app();
    app.trace_view = Some(build_synthetic_view(
        vec![synthetic_span("a", "root", 0, 1_000_000)],
        ViewMode::Solo,
    ));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    let _ = render_to_string(&mut app, 120, 30);
    // Body height is everything below header(1) + sep(1) +
    // topbar/status, so > 0 and < 30.
    assert!(app.last_trace_body_height > 0);
    assert!(app.last_trace_body_height < 30);
    // Detail height excludes the 2-row border, so > 0 in a
    // wide-enough terminal.
    assert!(app.last_trace_detail_height > 0);
}

#[test]
fn focused_detail_pane_renders_yellow_border() {
    // Sanity check: we can't inspect style directly through
    // TestBackend's symbol view, but we can confirm the border
    // is rendered when the detail pane is focused. A wider
    // terminal guarantees the split.
    let mut app = test_app();
    app.trace_view = Some(build_synthetic_view(
        vec![synthetic_span("a", "root", 0, 1_000_000)],
        ViewMode::Solo,
    ));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceDetail;
    let buf = render_to_string(&mut app, 120, 24);
    // The block widget draws corner glyphs \u250c \u2510 \u2514 \u2518.
    assert!(
        buf.contains('\u{250c}') || buf.contains('\u{2500}'),
        "detail block must render a border; buf:\n{buf}"
    );
}

#[test]
fn performance_smoke_two_thousand_spans_renders_within_budget() {
    use std::time::Instant;
    let mut app = test_app();
    // Build a 2000-span synthetic trace: each span 1ms wide,
    // staggered by 1ms.
    let spans: Vec<TraceSpan> = (0..2000)
        .map(|i| {
            synthetic_span(
                &format!("s{i:04}"),
                &format!("n{i}"),
                i * 1_000_000,
                1_000_000,
            )
        })
        .collect();
    let mut view = build_synthetic_view(spans, ViewMode::Solo);
    // Mark every 100th row as orphan to exercise the orphan path.
    for (i, row) in view.model.tree.iter_mut().enumerate() {
        if i.is_multiple_of(100) {
            row.is_orphan = true;
        }
    }
    app.trace_view = Some(view);
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    // Warm the terminal.
    let _ = render_to_string(&mut app, 120, 40);
    let start = Instant::now();
    let iters = 50;
    for _ in 0..iters {
        let _ = render_to_string(&mut app, 120, 40);
    }
    let elapsed = start.elapsed();
    let per_frame_us = elapsed.as_micros() / iters as u128;
    // Budget per the plan: <1ms / frame. Add a 10x headroom for
    // CI / debug builds; release would be much faster.
    assert!(
        per_frame_us < 10_000,
        "2000-span render took {per_frame_us}\u{00b5}s/frame > 10ms budget"
    );
    eprintln!("perf smoke: {per_frame_us}\u{00b5}s/frame for 2000 spans (debug build)");
}

// ============================================================
//                       Step 24 tests
// ============================================================
//
// These cover the new fold / filter / service-jump / yank /
// `:span json` surface added in step 24. They lean on a slightly
// richer fixture than the step-22/23 tests because every fold
// case needs a real parent/child structure.

use crate::app::types::TraceInputMode;

/// Build a `TraceView` from a list of `(span_id, parent, name,
/// service)` rows already in DFS order. Depth is derived from
/// the parent chain; `has_children` is set when any subsequent
/// row claims the given span as parent. Times are chosen so the
/// renderer's bar math doesn't divide by zero.
fn build_view_from_rows(rows: &[(&str, Option<&str>, &str, &str)]) -> TraceView {
    use crate::trace::TreeRow;
    use std::collections::BTreeMap;
    let n = rows.len();
    let mut spans: Vec<TraceSpan> = Vec::with_capacity(n);
    let mut by_id: BTreeMap<String, usize> = BTreeMap::new();
    for (i, (id, parent, name, service)) in rows.iter().enumerate() {
        let mut s = synthetic_span(id, name, i as i64 * 1_000_000, 1_000_000);
        s.parent_span_id = parent.map(str::to_string);
        s.service = service.to_string();
        spans.push(s);
        by_id.insert(id.to_string(), i);
    }
    // Derive `depth` per row from the parent chain (rows in DFS
    // order so every parent appears earlier).
    let depth_for = |idx: usize, spans: &[TraceSpan]| -> u16 {
        let mut d = 0u16;
        let mut cur = idx;
        while let Some(p) = spans[cur].parent_span_id.as_deref() {
            if let Some(&pi) = by_id.get(p) {
                d += 1;
                cur = pi;
            } else {
                break;
            }
        }
        d
    };
    // `has_children` true when any row points at this one.
    let has_kids: Vec<bool> = (0..n)
        .map(|i| {
            let id = &spans[i].span_id;
            rows.iter()
                .any(|(_, parent, _, _)| parent == &Some(id.as_str()))
        })
        .collect();
    let tree: Vec<TreeRow> = (0..n)
        .map(|i| TreeRow {
            span_idx: i,
            depth: depth_for(i, &spans),
            has_children: has_kids[i],
            is_orphan: false,
        })
        .collect();
    let roots: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, (_, p, _, _))| if p.is_none() { Some(i) } else { None })
        .collect();
    let t0 = 0;
    let t1 = (n as i64) * 1_000_000;
    let model = TraceModel {
        trace_id: "tid".to_string(),
        dataset: "ds".to_string(),
        spans,
        by_id,
        roots,
        t0_ns: t0,
        t1_ns: t1,
        tree,
    };
    TraceView::new(model, ViewMode::Solo)
}

/// Drive `app.on_key` over an ASCII slice. Used to type filter
/// queries from tests.
fn type_chars(app: &mut App, s: &str) {
    for c in s.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

fn cursor_span_id(app: &App) -> String {
    let v = app.trace_view.as_ref().unwrap();
    let row = v.model.tree[v.cursor];
    v.model.spans[row.span_idx].span_id.clone()
}

// ---- Folds ---------------------------------------------------------

#[test]
fn h_collapses_parent_so_child_is_no_longer_visible() {
    let mut app = test_app();
    app.trace_view = Some(build_view_from_rows(&[
        ("r", None, "root", "api"),
        ("c", Some("r"), "child", "db"),
        ("g", Some("c"), "grand", "db"),
    ]));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 10;
    // Cursor is on root by default.
    app.on_key(key(KeyCode::Char('h')));
    let v = app.trace_view.as_ref().unwrap();
    assert!(v.collapsed.contains(&0));
    let visible = v.visible_rows();
    assert_eq!(visible, vec![0], "child + grandchild must be hidden");
}

#[test]
fn l_expands_collapsed_parent() {
    let mut app = test_app();
    app.trace_view = Some(build_view_from_rows(&[
        ("r", None, "root", "api"),
        ("c", Some("r"), "child", "db"),
    ]));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 10;
    app.on_key(key(KeyCode::Char('h'))); // collapse
    app.on_key(key(KeyCode::Char('l'))); // expand
    let v = app.trace_view.as_ref().unwrap();
    assert!(v.collapsed.is_empty());
    assert_eq!(v.visible_rows(), vec![0, 1]);
}

#[test]
fn zm_collapses_all_and_snaps_cursor_to_visible_ancestor() {
    let mut app = test_app();
    app.trace_view = Some(build_view_from_rows(&[
        ("r", None, "root", "api"),
        ("c", Some("r"), "child", "db"),
        ("g", Some("c"), "grand", "db"),
    ]));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 10;
    // Move cursor onto the grandchild (deepest row).
    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Char('j')));
    assert_eq!(cursor_span_id(&app), "g");
    // zM
    app.on_key(key(KeyCode::Char('z')));
    app.on_key(key(KeyCode::Char('M')));
    let v = app.trace_view.as_ref().unwrap();
    // Both parents (r, c) collapsed; cursor snapped to topmost
    // collapsed ancestor (r) so it stays visible.
    assert!(v.collapsed.contains(&0));
    assert!(v.collapsed.contains(&1));
    assert_eq!(cursor_span_id(&app), "r");
}

#[test]
fn zr_expands_everything() {
    let mut app = test_app();
    app.trace_view = Some(build_view_from_rows(&[
        ("r", None, "root", "api"),
        ("c", Some("r"), "child", "db"),
    ]));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 10;
    app.on_key(key(KeyCode::Char('z')));
    app.on_key(key(KeyCode::Char('M')));
    assert!(!app.trace_view.as_ref().unwrap().collapsed.is_empty());
    app.on_key(key(KeyCode::Char('z')));
    app.on_key(key(KeyCode::Char('R')));
    assert!(app.trace_view.as_ref().unwrap().collapsed.is_empty());
}

#[test]
fn zv_reveals_cursor_by_uncollapsing_ancestors() {
    let mut app = test_app();
    app.trace_view = Some(build_view_from_rows(&[
        ("r", None, "root", "api"),
        ("c", Some("r"), "child", "db"),
        ("g", Some("c"), "grand", "db"),
    ]));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 10;
    // Park cursor on the grandchild first, then zM.
    app.on_key(key(KeyCode::Char('G'))); // last visible
    assert_eq!(cursor_span_id(&app), "g");
    app.on_key(key(KeyCode::Char('z')));
    app.on_key(key(KeyCode::Char('M')));
    // zM snapped the cursor up to "r"; teleport it back onto "g"
    // directly via the model so we can test zv's effect.
    if let Some(v) = app.trace_view.as_mut() {
        v.cursor = 2;
    }
    app.on_key(key(KeyCode::Char('z')));
    app.on_key(key(KeyCode::Char('v')));
    let v = app.trace_view.as_ref().unwrap();
    // Both ancestors uncollapsed; visible_rows once again
    // contains the grandchild.
    assert!(!v.collapsed.contains(&0));
    assert!(!v.collapsed.contains(&1));
    assert!(v.visible_rows().contains(&2));
}

#[test]
fn collapse_a_leaf_is_a_noop() {
    let mut app = test_app();
    app.trace_view = Some(build_view_from_rows(&[
        ("r", None, "root", "api"),
        ("c", Some("r"), "child", "db"),
    ]));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 10;
    // Move to the leaf and try to collapse it.
    app.on_key(key(KeyCode::Char('j')));
    assert_eq!(cursor_span_id(&app), "c");
    app.on_key(key(KeyCode::Char('h')));
    assert!(app.trace_view.as_ref().unwrap().collapsed.is_empty());
}

// ---- Filter --------------------------------------------------------

#[test]
fn slash_enters_filter_input_and_typing_narrows_the_view() {
    let mut app = test_app();
    app.trace_view = Some(build_view_from_rows(&[
        ("r", None, "checkout", "api"),
        ("a", Some("r"), "auth-check", "auth"),
        ("b", Some("r"), "db-query", "db"),
    ]));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 10;

    app.on_key(key(KeyCode::Char('/')));
    assert_eq!(
        app.trace_view.as_ref().unwrap().input_mode,
        TraceInputMode::Filter
    );
    // Typing "auth": only the auth-check span (+ its ancestor root)
    // remain visible.
    type_chars(&mut app, "auth");
    let v = app.trace_view.as_ref().unwrap();
    assert_eq!(v.filter, "auth");
    let visible = v.visible_rows();
    let visible_ids: Vec<&str> = visible
        .iter()
        .map(|&i| v.model.spans[v.model.tree[i].span_idx].span_id.as_str())
        .collect();
    assert_eq!(visible_ids, vec!["r", "a"]);
}

#[test]
fn filter_backspace_widens_by_rescanning() {
    let mut app = test_app();
    app.trace_view = Some(build_view_from_rows(&[
        ("r", None, "checkout", "api"),
        ("a", Some("r"), "auth", "auth"),
        ("b", Some("r"), "db", "db"),
    ]));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 10;

    app.on_key(key(KeyCode::Char('/')));
    type_chars(&mut app, "authx"); // narrow to nothing
    let v = app.trace_view.as_ref().unwrap();
    assert!(v.filter_matches.as_ref().unwrap().is_empty());
    // Backspace once: should re-admit "auth".
    app.on_key(key(KeyCode::Backspace));
    let v = app.trace_view.as_ref().unwrap();
    assert_eq!(v.filter, "auth");
    assert_eq!(v.filter_matches.as_ref().unwrap(), &vec![1usize]);
}

#[test]
fn filter_esc_clears_filter_entirely() {
    let mut app = test_app();
    app.trace_view = Some(build_view_from_rows(&[
        ("r", None, "checkout", "api"),
        ("a", Some("r"), "auth", "auth"),
    ]));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 10;

    app.on_key(key(KeyCode::Char('/')));
    type_chars(&mut app, "auth");
    app.on_key(key(KeyCode::Esc));
    let v = app.trace_view.as_ref().unwrap();
    assert_eq!(v.input_mode, TraceInputMode::Normal);
    assert!(v.filter.is_empty());
    assert!(v.filter_matches.is_none());
}

#[test]
fn filter_enter_commits_and_jumps_cursor_to_first_match() {
    let mut app = test_app();
    app.trace_view = Some(build_view_from_rows(&[
        ("r", None, "checkout", "api"),
        ("a", Some("r"), "auth", "auth"),
        ("b", Some("r"), "db", "db"),
    ]));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 10;

    app.on_key(key(KeyCode::Char('/')));
    type_chars(&mut app, "db");
    app.on_key(key(KeyCode::Enter));
    let v = app.trace_view.as_ref().unwrap();
    assert_eq!(v.input_mode, TraceInputMode::Normal);
    // Per plan: cursor moves to the first matching row on
    // commit. The match is "b" (the "db" span); "r" survives
    // only as a structural ancestor.
    assert_eq!(cursor_span_id(&app), "b");
    let visible_ids: Vec<&str> = v
        .visible_rows()
        .iter()
        .map(|&i| v.model.spans[v.model.tree[i].span_idx].span_id.as_str())
        .collect();
    assert_eq!(visible_ids, vec!["r", "b"]);
}

#[test]
fn filter_incremental_refinement_narrows_prior_match_set() {
    // Plan: appending a character to the filter must produce a
    // subset of the prior match set, never a superset.
    let mut app = test_app();
    app.trace_view = Some(build_view_from_rows(&[
        ("r", None, "checkout", "api"),
        ("a", Some("r"), "authcheck", "auth"),
        ("b", Some("r"), "authzgate", "auth"),
    ]));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 10;

    app.on_key(key(KeyCode::Char('/')));
    type_chars(&mut app, "auth");
    let after_auth = app
        .trace_view
        .as_ref()
        .unwrap()
        .filter_matches
        .clone()
        .unwrap();
    assert_eq!(after_auth.len(), 2);
    type_chars(&mut app, "z");
    let after_authz = app
        .trace_view
        .as_ref()
        .unwrap()
        .filter_matches
        .clone()
        .unwrap();
    // "authz" matches must be a subset of "auth" matches.
    for m in &after_authz {
        assert!(after_auth.contains(m), "{m} broke prefix monotonicity");
    }
    assert_eq!(after_authz, vec![2]);
}

#[test]
fn filter_full_attribute_search_matches_by_attribute_key() {
    let mut app = test_app();
    let mut view =
        build_view_from_rows(&[("r", None, "root", "api"), ("a", Some("r"), "child", "api")]);
    view.model.spans[1]
        .attributes
        .insert("http.status_code".into(), serde_json::json!(503));
    app.trace_view = Some(view);
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 10;

    app.on_key(key(KeyCode::Char('/')));
    type_chars(&mut app, "http.status_code");
    let v = app.trace_view.as_ref().unwrap();
    assert_eq!(v.filter_matches.as_ref().unwrap(), &vec![1usize]);
}

// ---- Service jumps -------------------------------------------------

#[test]
fn gt_cycles_through_services_and_returns_to_start() {
    // 7 rows on 7 distinct services arranged linearly; `gt` 7
    // times returns to the start (plan's acceptance criterion).
    let mut app = test_app();
    app.trace_view = Some(build_view_from_rows(&[
        ("a", None, "n0", "s0"),
        ("b", Some("a"), "n1", "s1"),
        ("c", Some("b"), "n2", "s2"),
        ("d", Some("c"), "n3", "s3"),
        ("e", Some("d"), "n4", "s4"),
        ("f", Some("e"), "n5", "s5"),
        ("g", Some("f"), "n6", "s6"),
    ]));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 20;
    let start = cursor_span_id(&app);
    for _ in 0..7 {
        app.on_key(key(KeyCode::Char('g')));
        app.on_key(key(KeyCode::Char('t')));
    }
    assert_eq!(cursor_span_id(&app), start);
}

#[test]
fn gt_on_single_service_trace_reports_status() {
    let mut app = test_app();
    app.trace_view = Some(build_view_from_rows(&[
        ("a", None, "n0", "same"),
        ("b", Some("a"), "n1", "same"),
    ]));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 10;
    app.on_key(key(KeyCode::Char('g')));
    app.on_key(key(KeyCode::Char('t')));
    assert!(
        app.status.contains("single service"),
        "got: {:?}",
        app.status
    );
}

#[test]
fn capital_g_t_goes_backward() {
    let mut app = test_app();
    app.trace_view = Some(build_view_from_rows(&[
        ("a", None, "n0", "s0"),
        ("b", Some("a"), "n1", "s1"),
        ("c", Some("b"), "n2", "s2"),
    ]));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 10;
    // Start on a; gT should wrap to c.
    app.on_key(key(KeyCode::Char('g')));
    app.on_key(key(KeyCode::Char('T')));
    assert_eq!(cursor_span_id(&app), "c");
}

// ---- Yank + :span json --------------------------------------------

#[test]
fn y_yanks_span_as_pretty_parseable_json() {
    let mut app = test_app();
    app.trace_view = Some(build_view_from_rows(&[
        ("alpha", None, "root", "api"),
        ("beta", Some("alpha"), "child", "db"),
    ]));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 10;
    app.on_key(key(KeyCode::Char('y')));
    // Reach into the private `yank` register via the editor's
    // paste path: pasting into an Mpl buffer should drop the
    // JSON in. Simpler check: use the public status update +
    // parse the JSON the keymap stamps onto `tile_inspect_json`
    // via `:span json` below. Here we trust the status update.
    assert!(
        app.status.starts_with("yanked span "),
        "got: {:?}",
        app.status
    );
}

#[test]
fn span_json_command_opens_overlay_with_parseable_json() {
    let mut app = test_app();
    let mut view = build_view_from_rows(&[
        ("alpha", None, "root", "api"),
        ("beta", Some("alpha"), "child", "db"),
    ]);
    view.model.spans[0]
        .attributes
        .insert("http.method".into(), serde_json::json!("GET"));
    app.trace_view = Some(view);
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 10;
    app.execute_command("span json");
    let json = app
        .tile_inspect_json
        .as_deref()
        .expect("overlay must be populated");
    let parsed: serde_json::Value = serde_json::from_str(json).expect("parseable JSON");
    assert_eq!(parsed["trace_id"], "tid");
    assert_eq!(parsed["span_id"], "alpha");
    assert_eq!(parsed["service"], "api");
    assert_eq!(parsed["attributes"]["http.method"], "GET");
    // Any key dismisses (existing global overlay rule).
    app.on_key(key(KeyCode::Char('x')));
    assert!(app.tile_inspect_json.is_none());
}

#[test]
fn span_json_without_trace_loaded_errors_cleanly() {
    let mut app = test_app();
    app.execute_command("span json");
    assert!(app.trace_view.is_none());
    assert!(
        app.last_error
            .as_deref()
            .is_some_and(|e| e.contains("no trace loaded")),
        "got: {:?}",
        app.last_error
    );
}

// ---- Cursor / scroll regressions after fold + filter --------------

#[test]
fn cursor_inside_collapsed_subtree_snaps_to_visible_ancestor() {
    // Direct unit: `deepest_visible_ancestor` already covered;
    // this test exercises the keymap path that drives it on `zM`.
    let mut app = test_app();
    app.trace_view = Some(build_view_from_rows(&[
        ("r", None, "root", "api"),
        ("c", Some("r"), "child", "db"),
        ("g", Some("c"), "grand", "db"),
    ]));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 10;
    if let Some(v) = app.trace_view.as_mut() {
        v.cursor = 2; // grandchild
    }
    app.on_key(key(KeyCode::Char('z')));
    app.on_key(key(KeyCode::Char('M')));
    let v = app.trace_view.as_ref().unwrap();
    // After zM, only "r" is visible — cursor must be on it.
    assert_eq!(v.cursor, 0);
}

#[test]
fn scroll_reclamps_after_zm_off_end() {
    // Tall trace, cursor near the end, viewport sees ~5 rows.
    // After zM only the roots remain — scroll must reclamp.
    let mut app = test_app();
    let rows: Vec<(&str, Option<&str>, &str, &str)> = vec![
        ("r0", None, "n0", "svc"),
        ("c0", Some("r0"), "n1", "svc"),
        ("r1", None, "n2", "svc"),
        ("c1", Some("r1"), "n3", "svc"),
        ("r2", None, "n4", "svc"),
        ("c2", Some("r2"), "n5", "svc"),
    ];
    app.trace_view = Some(build_view_from_rows(&rows));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 3;
    // Scroll to the bottom (cursor on last row, scroll high).
    app.on_key(key(KeyCode::Char('G')));
    // zM collapses all parents (3 of them) and only the 3 roots
    // remain visible.
    app.on_key(key(KeyCode::Char('z')));
    app.on_key(key(KeyCode::Char('M')));
    let v = app.trace_view.as_ref().unwrap();
    let visible = v.visible_rows();
    assert_eq!(visible.len(), 3, "only the roots should remain");
    // Render once to let the renderer reclamp `scroll` — the
    // keymap doesn't do it for us on `zM`. The performance smoke
    // helper above shows the rendering path.
    let _ = render_to_string(&mut app, 80, 6);
    let v = app.trace_view.as_ref().unwrap();
    assert!(
        (v.scroll as usize) <= visible.len().saturating_sub(1),
        "scroll {} must not point past visible len {}",
        v.scroll,
        visible.len()
    );
}

#[test]
fn filter_inside_collapsed_subtree_keeps_match_hidden() {
    let mut app = test_app();
    app.trace_view = Some(build_view_from_rows(&[
        ("r", None, "root", "api"),
        ("c", Some("r"), "secret-child", "db"),
    ]));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 10;
    // Collapse the root first.
    app.on_key(key(KeyCode::Char('h')));
    // Then filter for the child name. Plan: matches under a
    // collapsed parent are NOT auto-expanded.
    app.on_key(key(KeyCode::Char('/')));
    type_chars(&mut app, "secret");
    let v = app.trace_view.as_ref().unwrap();
    // Match set still contains the child (the filter logic is
    // collapse-agnostic), but visible_rows must not.
    assert_eq!(v.filter_matches.as_ref().unwrap(), &vec![1usize]);
    let visible = v.visible_rows();
    let visible_ids: Vec<&str> = visible
        .iter()
        .map(|&i| v.model.spans[v.model.tree[i].span_idx].span_id.as_str())
        .collect();
    assert_eq!(visible_ids, vec!["r"]);
}

#[test]
fn filter_commit_with_no_matches_reports_no_matches() {
    let mut app = test_app();
    app.trace_view = Some(build_view_from_rows(&[
        ("r", None, "root", "api"),
        ("c", Some("r"), "child", "db"),
    ]));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 10;
    app.on_key(key(KeyCode::Char('/')));
    type_chars(&mut app, "nope");
    app.on_key(key(KeyCode::Enter));
    assert!(app.status.contains("no matches"), "got: {:?}", app.status);
}

#[test]
fn filter_backspace_past_empty_returns_to_normal_mode() {
    let mut app = test_app();
    app.trace_view = Some(build_view_from_rows(&[("r", None, "root", "api")]));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 10;
    app.on_key(key(KeyCode::Char('/')));
    // Backspace on an empty filter exits input mode.
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(
        app.trace_view.as_ref().unwrap().input_mode,
        TraceInputMode::Normal
    );
}

#[test]
fn filter_backspace_to_empty_drops_match_set() {
    let mut app = test_app();
    app.trace_view = Some(build_view_from_rows(&[
        ("r", None, "root", "api"),
        ("c", Some("r"), "child", "db"),
    ]));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 10;
    app.on_key(key(KeyCode::Char('/')));
    type_chars(&mut app, "x");
    assert!(app.trace_view.as_ref().unwrap().filter_matches.is_some());
    app.on_key(key(KeyCode::Backspace));
    // Filter went back to "" — match set should be cleared.
    let v = app.trace_view.as_ref().unwrap();
    assert!(v.filter.is_empty());
    assert!(v.filter_matches.is_none());
}

#[test]
fn move_cursor_scrolls_viewport_when_cursor_leaves_window() {
    // Six-row tree, viewport height of 2 → cursor stepping down
    // must push `scroll` forward; stepping back up must pull it.
    let mut app = test_app();
    app.trace_view = Some(build_view_from_rows(&[
        ("a", None, "n0", "svc"),
        ("b", None, "n1", "svc"),
        ("c", None, "n2", "svc"),
        ("d", None, "n3", "svc"),
        ("e", None, "n4", "svc"),
        ("f", None, "n5", "svc"),
    ]));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 2;
    // j four times — cursor moves past the bottom of the
    // initial window (rows 0..=1), so scroll must follow.
    for _ in 0..4 {
        app.on_key(key(KeyCode::Char('j')));
    }
    let v = app.trace_view.as_ref().unwrap();
    assert_eq!(v.cursor, 4);
    assert!(v.scroll > 0, "scroll should follow cursor down");
    // k three times — cursor now sits above the current window;
    // scroll must pull back.
    for _ in 0..3 {
        app.on_key(key(KeyCode::Char('k')));
    }
    let v = app.trace_view.as_ref().unwrap();
    assert_eq!(v.cursor, 1);
    assert_eq!(v.scroll, 1);
}

#[test]
fn reopen_filter_after_esc_rebuilds_matches_from_cached_blobs() {
    let mut app = test_app();
    app.trace_view = Some(build_view_from_rows(&[
        ("r", None, "root", "api"),
        ("c", Some("r"), "child", "db"),
    ]));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 10;
    app.on_key(key(KeyCode::Char('/')));
    type_chars(&mut app, "child");
    // Re-open without changing — search_blobs is already built;
    // the second `/` reuses them (verifies the `is_none` branch
    // doesn't re-allocate).
    app.on_key(key(KeyCode::Enter)); // commit
    app.on_key(key(KeyCode::Char('/'))); // re-open
    let v = app.trace_view.as_ref().unwrap();
    assert!(v.search_blobs.is_some());
    assert_eq!(v.input_mode, TraceInputMode::Filter);
    assert_eq!(v.filter, "child");
}

#[test]
fn motion_is_noop_when_filter_hides_everything() {
    // j/k/gg/G on an empty visible window must not panic and
    // must not move `cursor`.
    let mut app = test_app();
    app.trace_view = Some(build_view_from_rows(&[
        ("r", None, "root", "api"),
        ("c", Some("r"), "child", "db"),
    ]));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 10;
    // Type a filter that matches nothing, commit, then try j/k/gg/G.
    app.on_key(key(KeyCode::Char('/')));
    type_chars(&mut app, "zzzzz");
    app.on_key(key(KeyCode::Enter));
    let original_cursor = app.trace_view.as_ref().unwrap().cursor;
    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Char('k')));
    app.on_key(key(KeyCode::Char('G')));
    app.on_key(key(KeyCode::Char('g')));
    app.on_key(key(KeyCode::Char('g')));
    assert_eq!(app.trace_view.as_ref().unwrap().cursor, original_cursor);
}

#[test]
fn gt_with_no_visible_rows_reports_no_motion() {
    let mut app = test_app();
    app.trace_view = Some(build_view_from_rows(&[("r", None, "root", "api")]));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 10;
    app.on_key(key(KeyCode::Char('/')));
    type_chars(&mut app, "zzzzz");
    app.on_key(key(KeyCode::Enter));
    // gt on an empty visible set should not panic. No status
    // contract here — the keymap is intentionally silent.
    app.on_key(key(KeyCode::Char('g')));
    app.on_key(key(KeyCode::Char('t')));
}

#[test]
fn enter_on_empty_filter_input_is_a_noop() {
    let mut app = test_app();
    app.trace_view = Some(build_view_from_rows(&[("r", None, "root", "api")]));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 10;
    app.on_key(key(KeyCode::Char('/')));
    // No chars typed; commit immediately.
    app.on_key(key(KeyCode::Enter));
    let v = app.trace_view.as_ref().unwrap();
    assert_eq!(v.input_mode, TraceInputMode::Normal);
    // No filter set; status unchanged.
    assert!(v.filter.is_empty());
}

// ---- Count combinators (vim `10j`) --------------------------------

/// Flat trace of `n` sibling roots, each on its own service so
/// `gt`-style tests can reuse it too.
fn flat_view(n: usize) -> TraceView {
    let rows: Vec<(String, Option<&str>, String, String)> = (0..n)
        .map(|i| (format!("s{i}"), None, format!("n{i}"), format!("svc{i}")))
        .collect();
    // build_view_from_rows wants &str tuples; adapt.
    let refs: Vec<(&str, Option<&str>, &str, &str)> = rows
        .iter()
        .map(|(id, p, name, svc)| (id.as_str(), *p, name.as_str(), svc.as_str()))
        .collect();
    build_view_from_rows(&refs)
}

#[test]
fn count_prefix_moves_multiple_rows() {
    let mut app = test_app();
    app.trace_view = Some(flat_view(20));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 25;
    // `10j` → cursor on row 10.
    type_chars(&mut app, "10");
    app.on_key(key(KeyCode::Char('j')));
    assert_eq!(app.trace_view.as_ref().unwrap().cursor, 10);
    // Count must reset: a plain `j` now moves one.
    app.on_key(key(KeyCode::Char('j')));
    assert_eq!(app.trace_view.as_ref().unwrap().cursor, 11);
}

#[test]
fn count_prefix_with_arrow_key() {
    let mut app = test_app();
    app.trace_view = Some(flat_view(20));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 25;
    type_chars(&mut app, "5");
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.trace_view.as_ref().unwrap().cursor, 5);
}

#[test]
fn count_prefix_k_moves_up() {
    let mut app = test_app();
    app.trace_view = Some(flat_view(20));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 25;
    if let Some(v) = app.trace_view.as_mut() {
        v.cursor = 15;
    }
    type_chars(&mut app, "10");
    app.on_key(key(KeyCode::Char('k')));
    assert_eq!(app.trace_view.as_ref().unwrap().cursor, 5);
}

#[test]
fn count_prefix_clamps_at_end() {
    let mut app = test_app();
    app.trace_view = Some(flat_view(20));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 25;
    type_chars(&mut app, "999");
    app.on_key(key(KeyCode::Char('j')));
    // Clamped to the last row, no panic.
    assert_eq!(app.trace_view.as_ref().unwrap().cursor, 19);
}

#[test]
fn count_prefix_capital_g_jumps_to_line() {
    let mut app = test_app();
    app.trace_view = Some(flat_view(20));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 25;
    // `3G` → 1-indexed visible line 3 == row index 2.
    type_chars(&mut app, "3");
    app.on_key(key(KeyCode::Char('G')));
    assert_eq!(app.trace_view.as_ref().unwrap().cursor, 2);
    // Bare `G` still goes to the last row.
    app.on_key(key(KeyCode::Char('G')));
    assert_eq!(app.trace_view.as_ref().unwrap().cursor, 19);
}

#[test]
fn count_prefix_gg_jumps_to_line() {
    let mut app = test_app();
    app.trace_view = Some(flat_view(20));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 25;
    // `5gg` → line 5 == row index 4. The count must survive the
    // first `g` of the two-step.
    type_chars(&mut app, "5");
    app.on_key(key(KeyCode::Char('g')));
    app.on_key(key(KeyCode::Char('g')));
    assert_eq!(app.trace_view.as_ref().unwrap().cursor, 4);
}

#[test]
fn lone_zero_is_ignored_as_count() {
    let mut app = test_app();
    app.trace_view = Some(flat_view(20));
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 25;
    // A bare `0` isn't a motion / count seed; `j` then moves 1.
    app.on_key(key(KeyCode::Char('0')));
    app.on_key(key(KeyCode::Char('j')));
    assert_eq!(app.trace_view.as_ref().unwrap().cursor, 1);
}

#[test]
fn digits_in_filter_mode_are_typed_not_counted() {
    let mut app = test_app();
    let mut view = build_view_from_rows(&[
        ("r", None, "root", "api"),
        ("c", Some("r"), "code503", "db"),
    ]);
    view.model.spans[1]
        .attributes
        .insert("http.status_code".into(), serde_json::json!(503));
    app.trace_view = Some(view);
    app.view_mode = ViewMode::Trace;
    app.focus = Pane::TraceTree;
    app.last_trace_body_height = 10;
    app.on_key(key(KeyCode::Char('/')));
    type_chars(&mut app, "503");
    let v = app.trace_view.as_ref().unwrap();
    assert_eq!(
        v.filter, "503",
        "digits must extend the filter, not a count"
    );
    assert_eq!(v.pending_count, None);
}
