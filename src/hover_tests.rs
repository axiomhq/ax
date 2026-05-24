use super::*;

#[test]
fn resolve_function_at_returns_avg() {
    let q = "home:temp | align to 1m using avg";
    let cursor = q.len(); // cursor at end of `avg`
    let info = resolve_function_at(q, cursor).expect("avg should resolve");
    assert_eq!(info.label, "avg");
    assert!(!info.args.is_empty() || info.info.is_some(), "{info:?}");
}

#[test]
fn resolve_function_at_qualified() {
    let q = "home:temp | map prom::rate";
    let cursor = q.len();
    let info = resolve_function_at(q, cursor).expect("prom::rate should resolve");
    assert_eq!(info.label, "prom::rate");
}

#[test]
fn resolve_function_at_cursor_in_middle_of_ident() {
    let q = "avg";
    let info = resolve_function_at(q, 1).expect("cursor inside ident still resolves");
    assert_eq!(info.label, "avg");
}

#[test]
fn resolve_function_at_unknown_returns_none() {
    assert!(resolve_function_at("home:temp", 4).is_none());
}

#[test]
fn find_call_context_first_arg() {
    let q = "home:temp | bucket to 1m using histogram(0.99";
    let ctx = find_call_context(q, q.len()).expect("inside call");
    assert_eq!(ctx.label, "histogram");
    assert_eq!(ctx.active, 0);
}

#[test]
fn find_call_context_second_arg() {
    let q = "home:temp | bucket to 1m using histogram(0.99, ";
    let ctx = find_call_context(q, q.len()).expect("inside call");
    assert_eq!(ctx.label, "histogram");
    assert_eq!(ctx.active, 1);
}

#[test]
fn find_call_context_skips_string_commas() {
    // Comma inside the string literal must not bump `active`. `histogram`
    // is a real bucket function so the lookup succeeds.
    let q = "home:temp | bucket to 1m using histogram(\"a, b\", ";
    let ctx = find_call_context(q, q.len()).expect("inside call");
    assert_eq!(ctx.label, "histogram");
    assert_eq!(ctx.active, 1);
}

#[test]
fn find_call_context_handles_nested_parens() {
    // The inner `(...)` is a balanced subcall and its comma must not
    // count toward the outer call's active arg.
    let q = "home:temp | bucket to 1m using histogram(rate(0.5), ";
    let ctx = find_call_context(q, q.len()).expect("inside outer call");
    assert_eq!(ctx.label, "histogram");
    assert_eq!(ctx.active, 1);
}

#[test]
fn find_call_context_returns_none_outside_call() {
    assert!(find_call_context("home:temp | align to 1m using avg", 33).is_none());
}

#[test]
fn find_call_context_returns_none_inside_string() {
    let q = "home:temp | where x == \"hello, world";
    assert!(find_call_context(q, q.len()).is_none());
}
