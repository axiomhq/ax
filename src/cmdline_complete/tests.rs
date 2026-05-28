use super::*;

fn ctx() -> Context<'static> {
    Context {
        dashboards: &[],
        datasets: &[],
        deployments: &[],
    }
}

fn ds(uid: &str) -> DashboardSummary {
    DashboardSummary {
        uid: uid.into(),
        id: None,
        updated_at: None,
        updated_by: None,
        version: None,
        dashboard: Default::default(),
    }
}

#[test]
fn head_completion_returns_matching_known_commands() {
    let r = completions_for("d", 1, &ctx()).unwrap();
    assert!(r.items.contains(&"dash".to_string()));
    assert!(r.items.contains(&"dashinfo".to_string()));
    // 'q' doesn't have a `d`.
    assert!(!r.items.contains(&"q".to_string()));
}

#[test]
fn head_completion_with_empty_buffer_offers_all_heads() {
    let r = completions_for("", 0, &ctx()).unwrap();
    assert!(r.items.contains(&"quit".to_string()));
    assert!(r.items.contains(&"tile".to_string()));
}

#[test]
fn dash_subcommands_after_head_and_space() {
    let r = completions_for("dash ", 5, &ctx()).unwrap();
    // Empty token: alphabetical order, full set. `save` was
    // collapsed into `:w` / `:w!` in step 19.
    assert_eq!(
        r.items,
        vec!["ls".to_string(), "new".to_string(), "rm".to_string()]
    );
    // Splice range is empty at the trailing position.
    assert_eq!(r.range, (5, 5));
}

#[test]
fn tile_subcommands_include_json_inspector() {
    let r = completions_for("tile ", 5, &ctx()).unwrap();
    assert!(r.items.contains(&"json".to_string()));
    assert!(r.items.contains(&"inspect".to_string()));
}

#[test]
fn dash_subcommands_filter_by_fuzzy_match() {
    // `n` matches `new` (single fuzzy hit).
    let r = completions_for("dash n", 6, &ctx()).unwrap();
    assert_eq!(r.items, vec!["new".to_string()]);
    assert_eq!(r.range, (5, 6));
}

#[test]
fn head_completion_is_fuzzy() {
    // Fuzzy matches non-prefix subsequences: `hp` matches `help`
    // (h_p) but not strict-prefix candidates that lack a `p` after
    // the leading `h`.
    let r = completions_for("hp", 2, &ctx()).unwrap();
    assert!(r.items.contains(&"help".to_string()));
    assert!(!r.items.contains(&"h".to_string()));
}

#[test]
fn head_completion_ranks_prefix_above_scattered() {
    // `da` should rank `dash` / `dashinfo` / `datasets` (prefix matches)
    // ahead of any pure-subsequence match.
    let r = completions_for("da", 2, &ctx()).unwrap();
    assert!(!r.items.is_empty());
    assert!(
        r.items[0].starts_with("da"),
        "prefix match should win first slot, got {:?}",
        r.items
    );
}

#[test]
fn tile_add_third_token_completes_viz_kinds() {
    let r = completions_for("tile add ", 9, &ctx()).unwrap();
    assert!(r.items.contains(&"line".to_string()));
    assert!(r.items.contains(&"top_list".to_string()));
}

#[test]
fn tile_add_third_token_filters() {
    let r = completions_for("tile add s", 10, &ctx()).unwrap();
    for want in ["scatter", "statistic", "spacer"] {
        assert!(r.items.contains(&want.to_string()), "missing {want}");
    }
    assert!(!r.items.contains(&"line".to_string()));
}

#[test]
fn viz_command_completes_kinds() {
    let r = completions_for("viz ", 4, &ctx()).unwrap();
    assert!(r.items.contains(&"heatmap".to_string()));
}

#[test]
fn open_completes_against_cached_dashboards() {
    let list = vec![ds("prod-1"), ds("prod-2"), ds("staging")];
    let ctx = Context {
        dashboards: &list,
        datasets: &[],
        deployments: &[],
    };
    let r = completions_for("open prod", 9, &ctx).unwrap();
    assert_eq!(r.items, vec!["prod-1", "prod-2"]);
}

#[test]
fn dash_rm_completes_uids() {
    let list = vec![ds("only-one")];
    let ctx = Context {
        dashboards: &list,
        datasets: &[],
        deployments: &[],
    };
    let r = completions_for("dash rm ", 8, &ctx).unwrap();
    assert_eq!(r.items, vec!["only-one"]);
}

#[test]
fn unknown_third_token_returns_none() {
    // `:tile rm <foo>` has no defined completion source.
    let r = completions_for("tile rm something", 17, &ctx());
    assert!(r.is_none());
}

// ---- :trace -----------------------------------------------------------

#[test]
fn trace_first_arg_offers_sub_commands() {
    let r = completions_for("trace ", 6, &ctx()).unwrap();
    // Whole `TRACE_SUBS` set on the empty token, alphabetically.
    assert_eq!(r.items, vec!["get", "set", "unset"]);
}

#[test]
fn trace_sub_command_fuzzy_filters() {
    let r = completions_for("trace s", 7, &ctx()).unwrap();
    // Both `set` and `unset` match `s`; `set` is a prefix so it
    // scores higher.
    assert_eq!(r.items.first().map(String::as_str), Some("set"));
    assert!(r.items.iter().any(|s| s == "unset"));
}

#[test]
fn trace_set_empty_token_offers_key_equals_pairs() {
    let r = completions_for("trace set ", 10, &ctx()).unwrap();
    assert_eq!(r.items, vec!["dataset=", "deployment="]);
}

#[test]
fn trace_set_dataset_value_slot_completes_from_cache() {
    let datasets = vec![
        "axiom-traces-dev".to_string(),
        "axiom-traces-prod".to_string(),
        "unrelated-logs".to_string(),
    ];
    let ctx = Context {
        dashboards: &[],
        datasets: &datasets,
        deployments: &[],
    };
    let buf = "trace set dataset=ax";
    let r = completions_for(buf, buf.chars().count(), &ctx).unwrap();
    // Both `ax`-matching datasets surface; `unrelated-logs` does not.
    assert!(r.items.iter().any(|s| s == "axiom-traces-dev"));
    assert!(r.items.iter().any(|s| s == "axiom-traces-prod"));
    assert!(!r.items.iter().any(|s| s == "unrelated-logs"));
    // Splice range covers only the value portion, leaving `dataset=`
    // intact when the candidate is applied.
    let value_start = buf.find('=').unwrap() + 1;
    assert_eq!(r.range, (value_start, buf.len()));
}

#[test]
fn trace_set_deployment_value_slot_completes_from_config() {
    let deployments = vec!["prod".to_string(), "staging".to_string()];
    let ctx = Context {
        dashboards: &[],
        datasets: &[],
        deployments: &deployments,
    };
    let buf = "trace set deployment=";
    let r = completions_for(buf, buf.chars().count(), &ctx).unwrap();
    // Empty value token — every deployment, alphabetical.
    assert_eq!(r.items, vec!["prod", "staging"]);
}

#[test]
fn trace_set_unknown_key_value_slot_returns_none() {
    // `:trace set foo=` has no value source; completer returns
    // None so the popup doesn't open with stale candidates.
    let buf = "trace set foo=";
    let r = completions_for(buf, buf.chars().count(), &ctx());
    assert!(r.is_none());
}

#[test]
fn trace_unset_arg_offers_bare_keys() {
    let r = completions_for("trace unset ", 12, &ctx()).unwrap();
    // No trailing `=`: unset takes keys, not pairs.
    assert_eq!(r.items, vec!["dataset", "deployment"]);
}

#[test]
fn trace_get_extra_arg_falls_through_to_none() {
    // `:trace get` takes no further args; the completer has no
    // candidates to offer for the 2nd slot.
    let r = completions_for("trace get x", 11, &ctx());
    assert!(r.is_none());
}
