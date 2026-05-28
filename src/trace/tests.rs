//! Unit tests for the small, pure surface that doesn't depend on
//! the APL decoder \u2014 enum parsing, accessors. The decoder itself
//! (and the fixture/synthetic round-trips) is tested in
//! `viz::apl_decode::tests`.

use super::*;

#[test]
fn span_kind_round_trips_canonical_names() {
    for kind in [
        SpanKind::Server,
        SpanKind::Client,
        SpanKind::Internal,
        SpanKind::Producer,
        SpanKind::Consumer,
        SpanKind::Unknown,
    ] {
        // Unknown is "lossy" on the parse side (every unknown
        // string maps to it), but the canonical name still
        // round-trips.
        assert_eq!(SpanKind::from_str(kind.as_str()), kind);
    }
}

#[test]
fn span_kind_accepts_upper_and_mixed_case() {
    // OTLP wire form is lower-case but some Java SDKs ship
    // upper-case; we accept both.
    assert_eq!(SpanKind::from_str("SERVER"), SpanKind::Server);
    assert_eq!(SpanKind::from_str("Client"), SpanKind::Client);
    assert_eq!(SpanKind::from_str(" internal "), SpanKind::Internal);
}

#[test]
fn span_kind_unknown_for_unrecognised_string() {
    // `SPAN_KIND_UNSPECIFIED` is the OTLP "no kind given" form.
    assert_eq!(
        SpanKind::from_str("SPAN_KIND_UNSPECIFIED"),
        SpanKind::Unknown
    );
    // Empty string and pure whitespace also.
    assert_eq!(SpanKind::from_str(""), SpanKind::Unknown);
    assert_eq!(SpanKind::from_str("   "), SpanKind::Unknown);
    // Future kinds we don't recognise yet.
    assert_eq!(SpanKind::from_str("nemesis"), SpanKind::Unknown);
}

#[test]
fn span_is_root_when_parent_id_missing() {
    let s = Span {
        span_id: "abc".into(),
        parent_span_id: None,
        name: String::new(),
        service: String::new(),
        kind: SpanKind::Unknown,
        status_code: None,
        is_error: false,
        start_ns: 0,
        end_ns: 0,
        duration_ns: 0,
        events: Vec::new(),
        attributes: BTreeMap::new(),
        resource: BTreeMap::new(),
    };
    assert!(s.is_root());
}

#[test]
fn span_with_parent_id_is_not_root_by_field() {
    // Note: `is_root()` is a field-level test. Whether a span is
    // *effectively* a root (its parent is outside the trace)
    // is the decoder's job to recompute when building `roots`.
    let s = Span {
        span_id: "abc".into(),
        parent_span_id: Some("parent".into()),
        name: String::new(),
        service: String::new(),
        kind: SpanKind::Unknown,
        status_code: None,
        is_error: false,
        start_ns: 0,
        end_ns: 0,
        duration_ns: 0,
        events: Vec::new(),
        attributes: BTreeMap::new(),
        resource: BTreeMap::new(),
    };
    assert!(!s.is_root());
}

#[test]
fn trace_model_duration_ns_is_saturating() {
    // The decoder rejects empty traces, but the accessor must
    // not panic on a degenerate model \u2014 the renderer reads it
    // unconditionally.
    let m = TraceModel {
        trace_id: "t".into(),
        dataset: "ds".into(),
        spans: vec![],
        by_id: BTreeMap::new(),
        roots: vec![],
        t0_ns: 0,
        t1_ns: 0,
        tree: vec![],
    };
    assert_eq!(m.duration_ns(), 0);

    let m = TraceModel {
        trace_id: "t".into(),
        dataset: "ds".into(),
        spans: vec![],
        by_id: BTreeMap::new(),
        roots: vec![],
        // Pathological inversion: shouldn't overflow.
        t0_ns: i64::MAX,
        t1_ns: i64::MIN,
        tree: vec![],
    };
    assert_eq!(m.duration_ns(), 0);

    let m = TraceModel {
        trace_id: "t".into(),
        dataset: "ds".into(),
        spans: vec![],
        by_id: BTreeMap::new(),
        roots: vec![],
        t0_ns: 100,
        t1_ns: 250,
        tree: vec![],
    };
    assert_eq!(m.duration_ns(), 150);
}

// ============================================================
//      Step-24 helper tests — filter / fold / visible_rows
// ============================================================

fn mk_span(id: &str, parent: Option<&str>, name: &str, service: &str) -> Span {
    Span {
        span_id: id.into(),
        parent_span_id: parent.map(str::to_string),
        name: name.into(),
        service: service.into(),
        kind: SpanKind::Unknown,
        status_code: None,
        is_error: false,
        start_ns: 0,
        end_ns: 0,
        duration_ns: 0,
        events: Vec::new(),
        attributes: BTreeMap::new(),
        resource: BTreeMap::new(),
    }
}

fn mk_row(span_idx: usize, depth: u16, has_children: bool) -> TreeRow {
    TreeRow {
        span_idx,
        depth,
        has_children,
        is_orphan: false,
    }
}

/// Build a tiny model. Tree shape:
///
/// ```text
/// 0 root                (service=api)
///   1 a                 (service=db)
///     2 a.a             (service=db)
///   3 b                 (service=cache)
/// 4 root2               (service=api)
/// ```
fn sample_model() -> TraceModel {
    let spans = vec![
        mk_span("r", None, "root", "api"),
        mk_span("a", Some("r"), "a", "db"),
        mk_span("aa", Some("a"), "a.a", "db"),
        mk_span("b", Some("r"), "b", "cache"),
        mk_span("r2", None, "root2", "api"),
    ];
    let mut by_id = BTreeMap::new();
    for (i, s) in spans.iter().enumerate() {
        by_id.insert(s.span_id.clone(), i);
    }
    let tree = vec![
        mk_row(0, 0, true),
        mk_row(1, 1, true),
        mk_row(2, 2, false),
        mk_row(3, 1, false),
        mk_row(4, 0, false),
    ];
    TraceModel {
        trace_id: "t".into(),
        dataset: "ds".into(),
        spans,
        by_id,
        roots: vec![0, 4],
        t0_ns: 0,
        t1_ns: 1,
        tree,
    }
}

#[test]
fn visible_rows_no_folds_no_filter_returns_everything() {
    let m = sample_model();
    let collapsed = HashSet::new();
    let rows = visible_rows(&m.tree, &collapsed, None);
    assert_eq!(rows, vec![0, 1, 2, 3, 4]);
}

#[test]
fn visible_rows_hides_collapsed_subtree() {
    let m = sample_model();
    let mut collapsed = HashSet::new();
    collapsed.insert(1); // collapse `a` — row 2 should drop
    let rows = visible_rows(&m.tree, &collapsed, None);
    assert_eq!(rows, vec![0, 1, 3, 4]);
}

#[test]
fn visible_rows_collapsed_root_drops_whole_subtree() {
    let m = sample_model();
    let mut collapsed = HashSet::new();
    collapsed.insert(0); // root hides 1, 2, 3 but not the sibling 4
    let rows = visible_rows(&m.tree, &collapsed, None);
    assert_eq!(rows, vec![0, 4]);
}

#[test]
fn visible_rows_collapsing_a_leaf_is_a_noop() {
    let m = sample_model();
    let mut collapsed = HashSet::new();
    collapsed.insert(2); // leaf — has_children=false
    let rows = visible_rows(&m.tree, &collapsed, None);
    assert_eq!(rows, vec![0, 1, 2, 3, 4]);
}

#[test]
fn visible_rows_filter_keeps_match_and_its_ancestors() {
    let m = sample_model();
    // Match span 2 (`a.a`); ancestors r and a must survive.
    let mut set = HashSet::new();
    set.insert(2);
    set.insert(1);
    set.insert(0);
    let rows = visible_rows(&m.tree, &HashSet::new(), Some(&set));
    assert_eq!(rows, vec![0, 1, 2]);
}

#[test]
fn visible_rows_filter_inside_collapsed_subtree_stays_hidden() {
    // Per plan: matches under a collapsed parent are NOT
    // auto-expanded. The user must `zv` to reveal them.
    let m = sample_model();
    let mut collapsed = HashSet::new();
    collapsed.insert(1);
    // Filter set thinks 2 should be visible (and its ancestors).
    let mut set = HashSet::new();
    set.insert(0);
    set.insert(1);
    set.insert(2);
    let rows = visible_rows(&m.tree, &collapsed, Some(&set));
    // 2 still suppressed because 1 is collapsed.
    assert_eq!(rows, vec![0, 1]);
}

#[test]
fn ancestor_closure_adds_every_ancestor() {
    let m = sample_model();
    let set = ancestor_closure(&m, &[2]);
    assert!(set.contains(&2));
    assert!(set.contains(&1));
    assert!(set.contains(&0));
    assert!(!set.contains(&3));
    assert!(!set.contains(&4));
}

#[test]
fn ancestor_closure_stops_at_orphan_parent() {
    // Construct a span whose parent_span_id points at nothing.
    let mut spans = vec![mk_span("orph", Some("missing"), "orphan", "api")];
    spans[0].parent_span_id = Some("missing".into());
    let mut by_id = BTreeMap::new();
    by_id.insert("orph".to_string(), 0);
    let m = TraceModel {
        trace_id: "t".into(),
        dataset: "ds".into(),
        spans,
        by_id,
        roots: vec![0],
        t0_ns: 0,
        t1_ns: 1,
        tree: vec![mk_row(0, 0, false)],
    };
    let set = ancestor_closure(&m, &[0]);
    // The orphan itself is in the set; the missing parent isn't
    // reachable so the walk simply stops.
    assert_eq!(set.iter().copied().collect::<Vec<_>>(), vec![0]);
}

#[test]
fn deepest_visible_ancestor_unchanged_when_nothing_collapsed() {
    let m = sample_model();
    let collapsed = HashSet::new();
    assert_eq!(deepest_visible_ancestor(&m, &collapsed, 2), 2);
}

#[test]
fn deepest_visible_ancestor_snaps_to_topmost_collapsed() {
    let m = sample_model();
    let mut collapsed = HashSet::new();
    collapsed.insert(0); // root collapsed — cursor on 2 must snap to 0
    assert_eq!(deepest_visible_ancestor(&m, &collapsed, 2), 0);
}

#[test]
fn deepest_visible_ancestor_inner_collapse() {
    let m = sample_model();
    let mut collapsed = HashSet::new();
    collapsed.insert(1); // a is collapsed; cursor on 2 must snap to 1
    assert_eq!(deepest_visible_ancestor(&m, &collapsed, 2), 1);
}

#[test]
fn span_matches_query_empty_matches_everything() {
    assert!(span_matches_query("anything", ""));
}

#[test]
fn span_matches_query_is_substring() {
    let blob = build_search_blob(&mk_span("a", None, "GET /checkout", "api"));
    assert!(span_matches_query(&blob, "checkout"));
    assert!(span_matches_query(&blob, "api"));
    assert!(!span_matches_query(&blob, "payments"));
}

#[test]
fn build_search_blob_includes_attribute_keys_and_values() {
    let mut span = mk_span("a", None, "work", "svc");
    span.attributes
        .insert("http.status_code".into(), serde_json::json!(200));
    span.attributes
        .insert("db.system".into(), serde_json::json!("postgres"));
    let blob = build_search_blob(&span);
    // The key-form match is the headline use case from the plan.
    assert!(blob.contains("http.status_code"));
    assert!(blob.contains("200"));
    assert!(blob.contains("db.system"));
    assert!(blob.contains("postgres"));
}

#[test]
fn build_search_blob_includes_resource_and_event_payload() {
    let mut span = mk_span("a", None, "work", "svc");
    span.resource
        .insert("deployment.env".into(), serde_json::json!("prod"));
    span.events.push(SpanEvent {
        time_ns: 1,
        name: "exception".into(),
        attributes: {
            let mut m = BTreeMap::new();
            m.insert("exception.type".into(), serde_json::json!("IOError"));
            m
        },
    });
    let blob = build_search_blob(&span);
    assert!(blob.contains("deployment.env"));
    assert!(blob.contains("prod"));
    assert!(blob.contains("exception"));
    assert!(blob.contains("ioerror"), "event attr value lowercased");
}

#[test]
fn filter_prefix_monotonicity_property() {
    // The plan's property test: if `query` matches a blob, every
    // prefix of `query` also matches. Sweep a handful of blobs
    // and queries; the property holds because we substring-scan
    // and `str::contains(needle)` implies `str::contains(prefix)`
    // for every needle prefix.
    let blobs: Vec<String> = [
        "GET /checkout\tapi\thttp.status_code=200",
        "db.query\tdb\tdb.system=postgres",
        "send.message\tkafka\tmessaging.system=kafka",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let queries = ["api", "checkout", "postgres", "http.status_code", "kafka"];
    for q in queries {
        for blob in &blobs {
            if span_matches_query(blob, q) {
                for take in 1..=q.len() {
                    let prefix = &q[..take];
                    assert!(
                        span_matches_query(blob, prefix),
                        "prefix `{prefix}` of matched query `{q}` must also match `{blob}`"
                    );
                }
            }
        }
    }
}

#[test]
fn span_json_round_trips_typed_core() {
    let mut span = mk_span("abc", Some("parent"), "work", "svc");
    span.start_ns = 10;
    span.end_ns = 30;
    span.duration_ns = 20;
    span.attributes.insert("k".into(), serde_json::json!("v"));
    let s = serde_json::to_string(&SpanJson::from_span("tid", &span)).unwrap();
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["trace_id"], "tid");
    assert_eq!(v["span_id"], "abc");
    assert_eq!(v["parent_span_id"], "parent");
    assert_eq!(v["name"], "work");
    assert_eq!(v["service"], "svc");
    assert_eq!(v["duration_ns"], 20);
    assert_eq!(v["attributes"]["k"], "v");
}

#[test]
fn span_json_omits_parent_when_none() {
    let span = mk_span("r", None, "root", "svc");
    let s = serde_json::to_string(&SpanJson::from_span("tid", &span)).unwrap();
    assert!(
        !s.contains("parent_span_id"),
        "None parent must be skipped: {s}"
    );
}

#[test]
fn build_search_blob_renders_null_bool_and_nested_json_values() {
    let mut span = mk_span("a", None, "n", "svc");
    span.attributes
        .insert("nullable".into(), serde_json::json!(null));
    span.attributes
        .insert("flag".into(), serde_json::json!(true));
    span.attributes.insert(
        "nested".into(),
        serde_json::json!({"a": [1, 2, 3], "b": "deep"}),
    );
    let blob = build_search_blob(&span);
    assert!(blob.contains("null"));
    assert!(blob.contains("true"));
    // Nested objects fall back to compact JSON; the inner string
    // value must still be lower-cased.
    assert!(blob.contains("deep"));
    assert!(blob.contains("\"a\""));
}

#[test]
fn ancestor_closure_handles_overlapping_walks() {
    // Two matches sharing ancestors should not panic and should
    // exercise the "already-walked" short-circuits.
    let m = sample_model();
    let set = ancestor_closure(&m, &[2, 3]);
    // span 2 contributes 0, 1, 2; span 3 contributes 0, 3.
    // The closure should include all of them; the dedup happens
    // via the HashSet.
    let mut got: Vec<usize> = set.into_iter().collect();
    got.sort();
    assert_eq!(got, vec![0, 1, 2, 3]);
}

#[test]
fn deepest_visible_ancestor_on_root_returns_self() {
    let m = sample_model();
    let collapsed = HashSet::new();
    // Span 0 is a root (parent_span_id None) — the parent walk
    // breaks immediately.
    assert_eq!(deepest_visible_ancestor(&m, &collapsed, 0), 0);
}

#[test]
fn span_json_includes_events_with_attributes() {
    let mut span = mk_span("a", None, "n", "svc");
    span.events.push(SpanEvent {
        time_ns: 42,
        name: "exception".into(),
        attributes: {
            let mut m = BTreeMap::new();
            m.insert("type".into(), serde_json::json!("IOError"));
            m
        },
    });
    let s = serde_json::to_string(&SpanJson::from_span("tid", &span)).unwrap();
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["events"][0]["time_ns"], 42);
    assert_eq!(v["events"][0]["name"], "exception");
    assert_eq!(v["events"][0]["attributes"]["type"], "IOError");
}

#[test]
fn ancestor_closure_skips_duplicate_match_entries() {
    // Defensive: API accepts &[usize] so callers could (in
    // principle) pass duplicates. The walk must skip them.
    let m = sample_model();
    let set = ancestor_closure(&m, &[2, 2, 2]);
    let mut got: Vec<usize> = set.into_iter().collect();
    got.sort();
    assert_eq!(got, vec![0, 1, 2]);
}

#[test]
fn deepest_visible_ancestor_breaks_at_orphan_parent() {
    // Span whose parent_span_id is set but unresolvable. The
    // walk must stop instead of looping forever.
    let mut spans = vec![mk_span("orph", Some("missing"), "orphan", "svc")];
    spans[0].parent_span_id = Some("missing".into());
    let mut by_id = BTreeMap::new();
    by_id.insert("orph".to_string(), 0);
    let m = TraceModel {
        trace_id: "t".into(),
        dataset: "ds".into(),
        spans,
        by_id,
        roots: vec![0],
        t0_ns: 0,
        t1_ns: 1,
        tree: vec![mk_row(0, 0, false)],
    };
    let collapsed = HashSet::new();
    // Returns the orphan itself; doesn't loop on the missing parent.
    assert_eq!(deepest_visible_ancestor(&m, &collapsed, 0), 0);
}
