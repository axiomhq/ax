//! Viz-kind / query classification helpers shared by the editor, the
//! grid renderer, and the dashboard adoption path.
//!
//! Earlier revisions also held an internal `Dashboard`/`Tile`/`Layout`
//! model that mirrored the wire shape on `App`. That model was never
//! the source of truth — the grid renderer walks `axiom::Chart`
//! directly off `loaded_dashboard`, and only the focused tile's viz
//! kind / opts / time range ever changed during a session. Step 4 of
//! the cleanup plan collapsed those structures onto `App` directly
//! (`App.viz_kind`, `App.viz_opts`, `App.time.range`); what remained
//! here is the classifier surface every caller actually consumes.

use crate::axiom::{Chart, ChartBase, DashboardSummary, KnownChart};

/// Which Axiom dashboard element a tile renders. Variants outside
/// `Line/Bar/Area/Scatter` are accepted by the parser so files authored
/// ahead of the implementation produce an "unsupported yet" diagnostic
/// rather than "unknown kind".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VizKind {
    #[default]
    Line,
    Bar,
    Area,
    Scatter,
    Statistic,
    TopList,
    Pie,
    Heatmap,
    Table,
    LogStream,
    MonitorList,
    Note,
    Spacer,
}

impl VizKind {
    /// Map an Axiom wire `Chart` variant to our internal `VizKind`.
    ///
    /// Notes on the cross-mapping:
    /// * `TopK` (server) ↔ `TopList` (TUI). Naming difference, same
    ///   element.
    /// * `Scatter` is rendered today through the same series code path
    ///   as line/bar; the rendering is approximate (no per-point
    ///   markers in the metrics chart) but doesn't crash.
    /// * `Bar`, `Area`, `Spacer`, `MonitorList` are TUI-only sub-kinds
    ///   and never appear in the wire `Chart` enum, so they're not
    ///   reachable here.
    pub fn from_chart(chart: &Chart) -> Self {
        match chart {
            Chart::Known(KnownChart::TimeSeries(_)) => VizKind::Line,
            Chart::Known(KnownChart::Heatmap(_)) => VizKind::Heatmap,
            Chart::Known(KnownChart::LogStream(_)) => VizKind::LogStream,
            Chart::Known(KnownChart::Pie(_)) => VizKind::Pie,
            Chart::Known(KnownChart::Scatter(_)) => VizKind::Scatter,
            Chart::Known(KnownChart::Table(_)) => VizKind::Table,
            Chart::Known(KnownChart::TopK(_)) => VizKind::TopList,
            Chart::Known(KnownChart::Statistic(_)) => VizKind::Statistic,
            Chart::Known(KnownChart::Note(_)) => VizKind::Note,
            Chart::Unknown(_) => VizKind::Line,
        }
    }

    /// Lower-case identifier used in `// @viz <kind>` pragmas and `:viz` commands.
    pub fn as_str(self) -> &'static str {
        match self {
            VizKind::Line => "line",
            VizKind::Bar => "bar",
            VizKind::Area => "area",
            VizKind::Scatter => "scatter",
            VizKind::Statistic => "statistic",
            VizKind::TopList => "top_list",
            VizKind::Pie => "pie",
            VizKind::Heatmap => "heatmap",
            VizKind::Table => "table",
            VizKind::LogStream => "log_stream",
            VizKind::MonitorList => "monitor_list",
            VizKind::Note => "note",
            VizKind::Spacer => "spacer",
        }
    }

    /// Parse a pragma identifier. Accepts both `top_list` (canonical) and
    /// `toplist` (no underscore) for the multi-word kinds; same for the
    /// other compounds so older notes survive a rename.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "line" => VizKind::Line,
            "bar" => VizKind::Bar,
            "area" => VizKind::Area,
            "scatter" => VizKind::Scatter,
            "statistic" | "stat" => VizKind::Statistic,
            "top_list" | "toplist" => VizKind::TopList,
            "pie" => VizKind::Pie,
            "heatmap" => VizKind::Heatmap,
            "table" => VizKind::Table,
            "log_stream" | "logstream" | "logs" => VizKind::LogStream,
            "monitor_list" | "monitors" => VizKind::MonitorList,
            "note" => VizKind::Note,
            "spacer" => VizKind::Spacer,
            _ => return None,
        })
    }

    /// `true` for the kinds whose renderer is implemented today.
    /// Used by tests to assert that the dispatch table in
    /// [`crate::viz::draw`] covers every variant.
    #[cfg(test)]
    pub fn is_implemented(self) -> bool {
        matches!(
            self,
            VizKind::Line
                | VizKind::Bar
                | VizKind::Area
                | VizKind::Scatter
                | VizKind::Statistic
                | VizKind::TopList
                | VizKind::Pie
                | VizKind::Heatmap
                | VizKind::Table
                | VizKind::LogStream
                | VizKind::Note
                | VizKind::Spacer
                | VizKind::MonitorList
        )
    }

    /// Every variant in display order. Single source of truth for the
    /// add-tile / open-tile picker and the `:tile add` completion list.
    pub const ALL: &'static [VizKind] = &[
        VizKind::Line,
        VizKind::Bar,
        VizKind::Area,
        VizKind::Scatter,
        VizKind::Statistic,
        VizKind::TopList,
        VizKind::Pie,
        VizKind::Heatmap,
        VizKind::Table,
        VizKind::LogStream,
        VizKind::MonitorList,
        VizKind::Note,
        VizKind::Spacer,
    ];

    /// Wrap a [`ChartBase`] in the wire `Chart` variant matching this
    /// kind. Inverse of [`VizKind::from_chart`].
    ///
    /// TUI-only kinds (`Bar`, `Area`, `MonitorList`, `Spacer`) don't have
    /// a dedicated wire variant; they fall back to `Chart::TimeSeries`
    /// so PUT round-trips cleanly. The TUI-only intent is preserved in
    /// the editor buffer's `// @viz` pragma.
    pub fn to_chart(self, base: ChartBase) -> Chart {
        match self {
            VizKind::Line | VizKind::Bar | VizKind::Area => {
                Chart::Known(KnownChart::TimeSeries(base))
            }
            VizKind::Scatter => Chart::Known(KnownChart::Scatter(base)),
            VizKind::Pie => Chart::Known(KnownChart::Pie(base)),
            VizKind::Heatmap => Chart::Known(KnownChart::Heatmap(base)),
            VizKind::Table => Chart::Known(KnownChart::Table(base)),
            VizKind::TopList => Chart::Known(KnownChart::TopK(base)),
            VizKind::Statistic => Chart::Known(KnownChart::Statistic(base)),
            VizKind::LogStream => Chart::Known(KnownChart::LogStream(base)),
            VizKind::Note => Chart::Known(KnownChart::Note(base)),
            VizKind::MonitorList | VizKind::Spacer => Chart::Known(KnownChart::TimeSeries(base)),
        }
    }
}

/// What kind of query a tile runs. `Mpl` and `Apl` are the runtime
/// variants; `Empty` covers charts with no query body (notes,
/// spacers, monitor-list-without-filter, etc.) — their renderer
/// doesn't read this field.
#[derive(Clone, Debug)]
pub enum Query {
    /// Metrics MPL query, sent to `/v1/query/_mpl`.
    Mpl(String),
    /// APL query, sent to `/v1/datasets/_apl`.
    Apl(String),
    /// No query (note bodies, spacers, etc.).
    Empty,
}

/// Extract the executable query string from an Axiom `Chart`.
///
/// Discrimination strategy: **chart kind, not parser**. Earlier
/// revisions ran `mpl_lang::compile` on the text and inferred the
/// language from the parser's verdict. That doesn't work in
/// practice — the local `mpl_lang` crate's grammar and stdlib are
/// subsets of what the Axiom server accepts, so valid real-world
/// MPL (queries that work fine on the server) routinely failed the
/// local check, flipped to `Query::Apl`, and rendered as the
/// "not yet executable" banner with no data. Picking by chart kind
/// dodges that drift entirely.
///
/// Rules:
///   1. Explicit `mpl` key wins (set by local edits in
///      [`crate::app::App::sync_buffer_to_focused_tile`] to keep
///      the local model classifier-free).
///   2. `apl` key on a `LogStream` chart → `Query::Apl`. LogStream
///      is genuinely APL on the Axiom side.
///   3. `apl` key on any other chart kind (or `Chart::Unknown`)
///      → `Query::Mpl`. Metrics chart kinds (TimeSeries,
///      Statistic, TopK, Heatmap, Pie, Scatter, Table, Note) ship
///      with MPL queries.
///   4. No query → `Query::Empty`.
///
/// Trade-off: a `TimeSeries` chart whose user genuinely wrote APL
/// will now be dispatched to the MPL endpoint and fail with a
/// server-side error in `tile_results.error`. That's strictly
/// better than the previous behaviour, where the local classifier
/// also returned APL but the fetcher then refused to dispatch the
/// query at all — the user saw "APL (not yet executable)" with no
/// hint of what was actually wrong.
pub fn extract_query(chart: &Chart) -> Query {
    use crate::axiom::KnownChart;
    let Some(base) = chart.base() else {
        return Query::Empty;
    };
    let q = match base.query.as_ref() {
        Some(v) => v,
        None => return Query::Empty,
    };
    // Explicit `mpl` key wins. Used by local edits so
    // `sync_buffer_to_focused_tile` round-trips don't depend on any
    // classifier behaviour.
    if let Some(mpl) = q.get("mpl").and_then(|v| v.as_str()) {
        return Query::Mpl(mpl.to_string());
    }
    if let Some(text) = q.get("apl").and_then(|v| v.as_str()) {
        return match chart {
            Chart::Known(KnownChart::LogStream(_)) => Query::Apl(text.to_string()),
            _ => Query::Mpl(text.to_string()),
        };
    }
    Query::Empty
}

/// Convert the local-canonical query form into the wire form the v2
/// dashboards API expects. Locally, queries the editor mutated live
/// under the `mpl` key (so [`extract_query`] takes the explicit-key
/// shortcut and never has to ask the chart kind). On the wire,
/// every chart's query MUST live under the `apl` key regardless of
/// language — matching what the server returns on GET. Mutates the
/// passed document in place; intended to be called on a clone right
/// before PUT.
///
/// Any sibling extras on the query object are preserved. A chart
/// with no `mpl` key (e.g. a true APL banner, or already in wire
/// form) is left untouched.
pub fn normalize_queries_to_wire(doc: &mut crate::axiom::DashboardDocument) {
    for chart in &mut doc.charts {
        let Some(base) = chart.base_mut() else {
            continue;
        };
        let Some(query) = base.query.as_mut() else {
            continue;
        };
        let Some(obj) = query.as_object_mut() else {
            continue;
        };
        if let Some(mpl_val) = obj.remove("mpl") {
            // If both keys somehow co-existed at this point, the
            // `mpl` value is authoritative — it carries the user's
            // most recent edit. The pre-existing `apl` is dropped.
            obj.insert("apl".to_string(), mpl_val);
        }
    }
}

/// A time-range expression. Strings are stored verbatim so they
/// round-trip through the dashboard file format unchanged (Axiom accepts
/// `now-1h`, RFC3339, etc.).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeRange {
    pub start: String,
    pub end: String,
}

impl Default for TimeRange {
    fn default() -> Self {
        // Matches the legacy `DEFAULT_START` / `DEFAULT_END` constants
        // so file-mode startup is a no-op at runtime.
        Self {
            start: "now-1h".to_string(),
            end: "now".to_string(),
        }
    }
}

impl TimeRange {
    /// Build a `TimeRange` from a loaded dashboard resource, falling
    /// back to `now-1h` / `now` when the document omits either field.
    pub fn from_resource(resource: &DashboardSummary) -> Self {
        let doc = &resource.dashboard;
        Self {
            start: doc
                .time_window_start
                .clone()
                .unwrap_or_else(|| "now-1h".to_string()),
            end: doc
                .time_window_end
                .clone()
                .unwrap_or_else(|| "now".to_string()),
        }
    }
}

#[cfg(test)]
mod tests;
