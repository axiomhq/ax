use super::*;

#[test]
fn viz_kind_round_trips_through_as_str_and_parse() {
    for k in [
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
    ] {
        assert_eq!(VizKind::parse(k.as_str()), Some(k), "round-trip for {k:?}");
    }
}

#[test]
fn viz_kind_parse_accepts_aliases() {
    assert_eq!(VizKind::parse("stat"), Some(VizKind::Statistic));
    assert_eq!(VizKind::parse("toplist"), Some(VizKind::TopList));
    assert_eq!(VizKind::parse("logs"), Some(VizKind::LogStream));
    assert_eq!(VizKind::parse("logstream"), Some(VizKind::LogStream));
    assert_eq!(VizKind::parse("monitors"), Some(VizKind::MonitorList));
}

#[test]
fn viz_kind_parse_rejects_unknown() {
    assert_eq!(VizKind::parse(""), None);
    assert_eq!(VizKind::parse("nope"), None);
    assert_eq!(VizKind::parse("LINE"), None); // case-sensitive
}

#[test]
fn implemented_set_matches_current_scope() {
    let implemented: Vec<_> = [
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
    ]
    .into_iter()
    .filter(|k| k.is_implemented())
    .collect();
    assert_eq!(
        implemented,
        vec![
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
        ]
    );
}

#[test]
fn single_tile_dashboard_carries_kind_and_opts() {
    let mut opts = BTreeMap::new();
    opts.insert("n".to_string(), "10".to_string());
    let d = Dashboard::single_tile_from_mpl("foo:bar".to_string(), VizKind::Bar, opts);
    assert_eq!(d.tiles.len(), 1);
    let t = d.focused_tile();
    assert_eq!(t.kind, VizKind::Bar);
    assert_eq!(t.opts.get("n").map(String::as_str), Some("10"));
    assert!(matches!(&t.query, Query::Mpl(s) if s == "foo:bar"));
}

use crate::axiom::{ChartBase, DashboardDocument, DashboardSummary, LayoutItem};
use serde_json::json;

fn chart_with_mpl(id: &str, name: &str, mpl: &str) -> Chart {
    Chart::TimeSeries(ChartBase {
        id: id.to_string(),
        name: Some(name.to_string()),
        query: Some(json!({ "mpl": mpl })),
        extras: Default::default(),
    })
}

// Fixtures lifted verbatim from `GET /v2/dashboards/uid/…` against
// a real account. Two MPL examples (home overview) and one APL
// example (probe-* dashboards).
const REAL_MPL_BACKTICK_STAT: &str = "`home`:temp\n| where type == \"temperature\"\n| where room != \"Außen\"\n| group using avg";
const REAL_MPL_BACKTICK_TIMESERIES: &str =
    "`home`:power\n| group by circuit using sum\n| align to 5m using avg";
const REAL_APL_BRACKET: &str =
    "[\"axiom-audit-logs\"] | summarize n=count() by bin_auto(_time)";

fn statistic_with_apl(text: &str) -> Chart {
    Chart::Statistic(crate::axiom::ChartBase {
        id: "c1".into(),
        name: None,
        query: Some(json!({ "apl": text })),
        extras: Default::default(),
    })
}

#[test]
fn real_home_overview_mpl_statistic_classifies_as_mpl() {
    // ``home`:temp | where … | group using avg` — stored under
    // the `apl` key on a Statistic chart. The discriminator is
    // the leading backtick.
    let chart = statistic_with_apl(REAL_MPL_BACKTICK_STAT);
    assert!(matches!(extract_query(&chart), Query::Mpl(_)));
}

#[test]
fn real_home_overview_mpl_timeseries_classifies_as_mpl() {
    let chart = Chart::TimeSeries(crate::axiom::ChartBase {
        id: "c1".into(),
        name: None,
        query: Some(json!({ "apl": REAL_MPL_BACKTICK_TIMESERIES })),
        extras: Default::default(),
    });
    assert!(matches!(extract_query(&chart), Query::Mpl(_)));
}

#[test]
fn real_probe_apl_with_bracketed_dataset_classifies_as_apl() {
    // `["axiom-audit-logs"] | summarize n=count() by …` — stored
    // under `apl` on a TimeSeries chart. Pipes don't make this
    // MPL; the leading `[` does make it APL.
    let chart = Chart::TimeSeries(crate::axiom::ChartBase {
        id: "c1".into(),
        name: None,
        query: Some(json!({ "apl": REAL_APL_BRACKET })),
        extras: Default::default(),
    });
    assert!(matches!(extract_query(&chart), Query::Apl(_)));
}

#[test]
fn bare_metric_classifies_as_mpl() {
    // Bare `metric:agg` shape — valid MPL the engine accepts.
    let chart = statistic_with_apl("cpu:rate");
    assert!(matches!(extract_query(&chart), Query::Mpl(_)));
}

#[test]
fn invalid_mpl_syntax_classifies_as_apl() {
    // Anything the engine rejects — even if it textually looks
    // metric-shaped — falls through to APL so the metrics endpoint
    // doesn't get pinged with garbage.
    let chart = statistic_with_apl("this is definitely not a valid query");
    assert!(matches!(extract_query(&chart), Query::Apl(_)));
}

#[test]
fn bare_identifier_dataset_with_pipes_classifies_as_apl() {
    // `axiom-history | count` is valid APL (dataset name without
    // brackets). No colon before the pipe → not MPL.
    let chart = statistic_with_apl("axiom-history | count");
    assert!(matches!(extract_query(&chart), Query::Apl(_)));
}

#[test]
fn explicit_mpl_key_still_wins_when_present() {
    let chart = Chart::TimeSeries(crate::axiom::ChartBase {
        id: "c1".into(),
        name: None,
        query: Some(json!({
            "mpl": "correct:value",
            "apl": "['logs'] | count",
        })),
        extras: Default::default(),
    });
    match extract_query(&chart) {
        Query::Mpl(s) => assert_eq!(s, "correct:value"),
        other => panic!("expected Mpl, got {other:?}"),
    }
}

#[test]
fn from_resource_maps_chart_types_to_viz_kinds() {
    let resource = DashboardSummary {
        uid: "u1".into(),
        id: None,
        updated_at: None,
        updated_by: None,
        version: None,
        dashboard: DashboardDocument {
            name: Some("d".into()),
            charts: vec![
                chart_with_mpl("c1", "latency", "http_latency:p99"),
                Chart::Pie(ChartBase {
                    id: "c2".into(),
                    name: Some("by-region".into()),
                    query: Some(json!({ "apl": "['logs'] | summarize count() by region" })),
                    extras: Default::default(),
                }),
                Chart::TopK(ChartBase {
                    id: "c3".into(),
                    name: Some("errors".into()),
                    query: None,
                    extras: Default::default(),
                }),
            ],
            ..Default::default()
        },
    };
    let d = Dashboard::from_resource(&resource);
    assert_eq!(d.id.as_deref(), Some("u1"));
    assert_eq!(d.tiles.len(), 3);
    assert_eq!(d.tiles[0].kind, VizKind::Line);
    assert_eq!(d.tiles[1].kind, VizKind::Pie);
    assert_eq!(d.tiles[2].kind, VizKind::TopList);
    assert!(matches!(
        &d.tiles[0].query,
        Query::Mpl(s) if s == "http_latency:p99"
    ));
    assert!(matches!(
        &d.tiles[1].query,
        Query::Apl(s) if s.starts_with("['logs']")
    ));
    assert!(matches!(d.tiles[2].query, Query::Empty));
}

#[test]
fn from_resource_pairs_layout_by_chart_id() {
    let resource = DashboardSummary {
        uid: "u".into(),
        id: None,
        updated_at: None,
        updated_by: None,
        version: None,
        dashboard: DashboardDocument {
            name: Some("d".into()),
            charts: vec![chart_with_mpl("c1", "x", "a:b")],
            layout: vec![LayoutItem {
                i: "c1".into(),
                x: 3,
                y: Some(2),
                w: 6,
                h: 4,
                extras: Default::default(),
            }],
            ..Default::default()
        },
    };
    let d = Dashboard::from_resource(&resource);
    let pos = d.tiles[0].pos;
    assert_eq!(pos.x, 3);
    assert_eq!(pos.y, 2);
    assert_eq!(pos.w, 6);
    assert_eq!(pos.h, 4);
}

#[test]
fn from_resource_creates_placeholder_tile_when_no_charts() {
    let resource = DashboardSummary {
        uid: "u".into(),
        id: None,
        updated_at: None,
        updated_by: None,
        version: None,
        dashboard: DashboardDocument {
            name: Some("empty".into()),
            ..Default::default()
        },
    };
    let d = Dashboard::from_resource(&resource);
    // Invariant: focused_tile never panics.
    assert_eq!(d.tiles.len(), 1);
    assert_eq!(d.focused_tile().kind, VizKind::Note);
}

#[test]
fn from_resource_carries_time_window() {
    let resource = DashboardSummary {
        uid: "u".into(),
        id: None,
        updated_at: None,
        updated_by: None,
        version: None,
        dashboard: DashboardDocument {
            name: Some("d".into()),
            time_window_start: Some("qr-now-7d".into()),
            time_window_end: Some("qr-now".into()),
            ..Default::default()
        },
    };
    let d = Dashboard::from_resource(&resource);
    assert_eq!(d.time_range.start, "qr-now-7d");
    assert_eq!(d.time_range.end, "qr-now");
}

#[test]
fn default_time_range_matches_legacy_constants() {
    let r = TimeRange::default();
    assert_eq!(r.start, "now-1h");
    assert_eq!(r.end, "now");
}
