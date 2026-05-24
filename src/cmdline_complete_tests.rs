use super::*;

fn ctx() -> Context<'static> {
    Context { dashboards: &[] }
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
    assert!(r.items.contains(&"dashboards".to_string()));
    // 'q' doesn't have the `d` prefix.
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
    assert_eq!(
        r.items,
        vec![
            "new".to_string(),
            "rm".to_string(),
            "save".to_string(),
            "save!".to_string()
        ]
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
fn dash_subcommands_filter_by_prefix() {
    let r = completions_for("dash sa", 7, &ctx()).unwrap();
    assert_eq!(r.items, vec!["save".to_string(), "save!".to_string()]);
    assert_eq!(r.range, (5, 7));
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
fn common_prefix_finds_longest_shared_start() {
    let r = CompletionRequest {
        items: vec!["save".to_string(), "save!".to_string()],
        range: (0, 0),
    };
    assert_eq!(r.common_prefix(), "save");
}

#[test]
fn common_prefix_returns_empty_when_no_overlap() {
    let r = CompletionRequest {
        items: vec!["alpha".into(), "beta".into()],
        range: (0, 0),
    };
    assert_eq!(r.common_prefix(), "");
}

#[test]
fn open_completes_against_cached_dashboards() {
    let list = vec![ds("prod-1"), ds("prod-2"), ds("staging")];
    let ctx = Context { dashboards: &list };
    let r = completions_for("open prod", 9, &ctx).unwrap();
    assert_eq!(r.items, vec!["prod-1", "prod-2"]);
}

#[test]
fn dash_rm_completes_uids() {
    let list = vec![ds("only-one")];
    let ctx = Context { dashboards: &list };
    let r = completions_for("dash rm ", 8, &ctx).unwrap();
    assert_eq!(r.items, vec!["only-one"]);
}

#[test]
fn unknown_third_token_returns_none() {
    // `:tile rm <foo>` has no defined completion source.
    let r = completions_for("tile rm something", 17, &ctx());
    assert!(r.is_none());
}
