use super::*;
use crate::axiom::{DatasetSummary, MetricInfo};
use std::collections::BTreeMap;

fn cache_with(datasets: &[&str], metrics: &[(&str, &[&str])]) -> Cache {
    let mut c = Cache::in_memory(String::new());
    c.replace_datasets(
        datasets
            .iter()
            .map(|n| DatasetSummary {
                name: n.to_string(),
                description: None,
                edge_deployment: None,
                kind: None,
            })
            .collect(),
    );
    for (ds, ms) in metrics {
        let mut map: BTreeMap<String, MetricInfo> = BTreeMap::new();
        for m in *ms {
            map.insert(m.to_string(), MetricInfo::default());
        }
        c.replace_metrics(ds, map);
    }
    c
}

/// Find an item by label, panicking with the available labels listed.
fn find<'a>(items: &'a [CompletionItem], label: &str) -> &'a CompletionItem {
    items.iter().find(|i| i.label == label).unwrap_or_else(|| {
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        panic!("no item {label:?} in {labels:?}")
    })
}

#[test]
fn empty_query_yields_dataset_completion() {
    let cache = cache_with(&["home", "logs"], &[]);
    let p = compute("", 0, &[], &cache).expect("payload");
    assert_eq!(p.kind, CompletionKind::Dataset);
    let labels: Vec<&str> = p.items.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&"home"));
    assert!(labels.contains(&"logs"));
}

#[test]
fn after_colon_yields_metric_with_dataset() {
    let cache = cache_with(&["home"], &[("home", &["temp", "switch"])]);
    let p = compute("home:t", 6, &[], &cache).expect("payload");
    match &p.kind {
        CompletionKind::Metric { dataset } => assert_eq!(dataset, "home"),
        other => panic!("unexpected {other:?}"),
    }
    let labels: Vec<&str> = p.items.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&"temp"));
    assert!(!labels.contains(&"switch"));
}

#[test]
fn after_align_using_yields_align_fns_from_stdlib() {
    let cache = cache_with(&[], &[]);
    let q = "home:temp | align to 1m using ";
    let p = compute(q, q.len(), &[], &cache).expect("payload");
    assert_eq!(p.kind, CompletionKind::AlignFn);
    let avg = find(&p.items, "avg");
    assert_eq!(avg.apply, "avg");
    find(&p.items, "sum");
}

#[test]
fn after_map_yields_map_fns_from_stdlib() {
    let cache = cache_with(&[], &[]);
    let q = "home:temp | map ";
    let p = compute(q, q.len(), &[], &cache).expect("payload");
    assert_eq!(p.kind, CompletionKind::MapFn);
    find(&p.items, "rate");
}

#[test]
fn keyword_apply_includes_trailing_space() {
    // The engine ships `apply: "where "` so accepting positions the
    // cursor ready to type the predicate.
    let cache = cache_with(&[], &[]);
    let q = "home:temp | wh";
    let p = compute(q, q.len(), &[], &cache).expect("payload");
    assert_eq!(p.kind, CompletionKind::Keyword);
    let where_item = find(&p.items, "where");
    assert_eq!(where_item.apply, "where ");
}

#[test]
fn dataset_apply_is_plain_when_name_is_plain() {
    let cache = cache_with(&["home"], &[]);
    let p = compute("", 0, &[], &cache).expect("payload");
    let item = find(&p.items, "home");
    assert_eq!(item.apply, "home");
}

#[test]
fn dataset_apply_is_backticked_when_name_has_dashes() {
    let cache = cache_with(&["homeassistant-metrics"], &[]);
    let p = compute("", 0, &[], &cache).expect("payload");
    let item = find(&p.items, "homeassistant-metrics");
    assert_eq!(item.apply, "`homeassistant-metrics`");
}

#[test]
fn dataset_apply_with_opened_backtick_emits_only_closing() {
    let cache = cache_with(&["home", "logs"], &[]);
    // The engine advances `span.from` past the opening backtick. With
    // `opened_backtick = true` the apply text is just `body` + closing.
    let p = compute("`hom", 4, &[], &cache).expect("payload");
    assert_eq!(p.replace_range.0, 1, "engine should skip past backtick");
    let item = find(&p.items, "home");
    assert_eq!(item.apply, "home`");
}

#[test]
fn metric_apply_dotted_is_backticked() {
    let cache = cache_with(&["home"], &[("home", &["ha.sensor.temperature"])]);
    let p = compute("home:ha", 7, &[], &cache).expect("payload");
    let item = find(&p.items, "ha.sensor.temperature");
    assert_eq!(item.apply, "`ha.sensor.temperature`");
}

#[test]
fn metric_apply_embedded_backtick_escaped() {
    let cache = cache_with(&["home"], &[("home", &["weird`name"])]);
    let p = compute("home:w", 6, &[], &cache).expect("payload");
    let item = find(&p.items, "weird`name");
    assert_eq!(item.apply, "`weird\\`name`");
}

// ── tag + tag-value completion ─────────────────────────────────

/// Build a cache with one (dataset, metric) plus a tag list and a
/// value list for one of those tags.
fn cache_with_tags(
    dataset: &str,
    metric: &str,
    tags: &[&str],
    tag_values: Option<(&str, &[&str])>,
) -> Cache {
    let mut c = cache_with(&[dataset], &[(dataset, &[metric])]);
    c.replace_tags(
        dataset,
        metric,
        tags.iter().map(|s| s.to_string()).collect(),
    );
    if let Some((tag, values)) = tag_values {
        c.replace_tag_values(
            dataset,
            metric,
            tag,
            values.iter().map(|s| s.to_string()).collect(),
        );
    }
    c
}

#[test]
fn tag_completion_offers_cached_tag_names() {
    let cache = cache_with_tags("home", "temp", &["host", "region"], None);
    let q = "home:temp | where ";
    let p = compute(q, q.len(), &[], &cache).expect("payload");
    match &p.kind {
        CompletionKind::Tag { dataset, metric } => {
            assert_eq!(dataset, "home");
            assert_eq!(metric, "temp");
        }
        other => panic!("unexpected {other:?}"),
    }
    let labels: Vec<&str> = p.items.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&"host"), "got {labels:?}");
    assert!(labels.contains(&"region"), "got {labels:?}");
}

#[test]
fn tag_apply_backticks_dotted_name() {
    let cache = cache_with_tags("home", "temp", &["host.name"], None);
    let q = "home:temp | where ho";
    let p = compute(q, q.len(), &[], &cache).expect("payload");
    let item = find(&p.items, "host.name");
    assert_eq!(item.apply, "`host.name`");
}

#[test]
fn tag_completion_empty_when_cache_miss() {
    // No tags cached for (home, temp); engine still emits the Tag
    // variant but our adapter surfaces an empty item list. The popup
    // hides on empty.
    let cache = cache_with(&["home"], &[("home", &["temp"])]);
    let q = "home:temp | where ";
    let p = compute(q, q.len(), &[], &cache).expect("payload");
    assert!(matches!(p.kind, CompletionKind::Tag { .. }));
    assert!(p.items.is_empty(), "got {:?}", p.items);
}

#[test]
fn tag_value_completion_in_open_string() {
    let cache = cache_with_tags(
        "home",
        "temp",
        &["host"],
        Some(("host", &["web-1", "web-2", "db-1"])),
    );
    let q = "home:temp | where host == \"we";
    let p = compute(q, q.len(), &[], &cache).expect("payload");
    match &p.kind {
        CompletionKind::TagValue {
            dataset,
            metric,
            tag,
        } => {
            assert_eq!(dataset, "home");
            assert_eq!(metric, "temp");
            assert_eq!(tag, "host");
        }
        other => panic!("unexpected {other:?}"),
    }
    let labels: Vec<&str> = p.items.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&"web-1"), "got {labels:?}");
    assert!(labels.contains(&"web-2"), "got {labels:?}");
    assert!(!labels.contains(&"db-1"), "db-1 should be filtered out");
}

#[test]
fn tag_value_apply_emits_closing_quote_only_when_quote_opened() {
    let cache = cache_with_tags("home", "temp", &["host"], Some(("host", &["web-1"])));
    // Quote already open → apply is just `body"` (no leading quote).
    let q = "home:temp | where host == \"we";
    let p = compute(q, q.len(), &[], &cache).expect("payload");
    let item = find(&p.items, "web-1");
    assert_eq!(item.apply, "web-1\"");
}

#[test]
fn tag_value_apply_wraps_in_quotes_when_no_quote_yet() {
    let cache = cache_with_tags("home", "temp", &["host"], Some(("host", &["web-1"])));
    let q = "home:temp | where host == ";
    let p = compute(q, q.len(), &[], &cache).expect("payload");
    let item = find(&p.items, "web-1");
    assert_eq!(item.apply, "\"web-1\"");
}

#[test]
fn tag_value_position_respects_backticked_tag() {
    let cache = cache_with_tags(
        "home",
        "temp",
        &["host.name"],
        Some(("host.name", &["web-1"])),
    );
    let q = "home:temp | where `host.name` == \"";
    let p = compute(q, q.len(), &[], &cache).expect("payload");
    match &p.kind {
        CompletionKind::TagValue { tag, .. } => assert_eq!(tag, "host.name"),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn tag_value_skipped_when_cache_miss() {
    // No values cached → override doesn't fire; we fall through to the
    // engine, which returns Keywords for the string position.
    let cache = cache_with_tags("home", "temp", &["host"], None);
    let q = "home:temp | where host == \"we";
    match compute(q, q.len(), &[], &cache) {
        None => {}
        Some(p) => {
            assert!(
                !matches!(p.kind, CompletionKind::TagValue { .. }),
                "unexpected TagValue payload: {:?}",
                p.kind
            );
        }
    }
}

#[test]
fn system_param_surfaces_as_param_kind() {
    let cache = cache_with(&[], &[]);
    let sys = vec![SystemParam {
        name: "__interval".to_string(),
        kind: ParamKind::Duration,
    }];
    let q = "home:temp | align to $ using avg";
    let cursor = q.find('$').unwrap() + 1;
    let p = compute(q, cursor, &sys, &cache).expect("payload");
    assert_eq!(p.kind, CompletionKind::Param);
    let item = find(&p.items, "$__interval");
    assert_eq!(item.apply, "$__interval");
}
