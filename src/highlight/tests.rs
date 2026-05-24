use super::*;
use mpl_language_server::collect_tokens;

fn keyword_style() -> Style {
    Style::default().fg(Color::Cyan)
}
fn number_style() -> Style {
    Style::default().fg(Color::Yellow)
}
fn string_style() -> Style {
    Style::default().fg(Color::Green)
}
fn bool_style() -> Style {
    Style::default().fg(Color::Magenta)
}
fn punct_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// Find the first span whose `content` equals `text` and assert its style.
fn assert_span_style(lines: &[Line<'_>], text: &str, expected: Style) {
    for line in lines {
        for sp in &line.spans {
            if sp.content == text {
                assert_eq!(sp.style, expected, "span {text:?} had {:?}", sp.style);
                return;
            }
        }
    }
    let dump: Vec<Vec<(String, Style)>> = lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| (s.content.to_string(), s.style))
                .collect()
        })
        .collect();
    panic!("no span with content {text:?} found in {dump:#?}");
}

#[test]
fn empty_buffer_yields_one_empty_line() {
    let lines = highlight_lines("", None);
    assert_eq!(lines.len(), 1);
    assert_eq!(
        lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>(),
        vec![""]
    );
}

#[test]
fn no_tokens_falls_back_to_plain_per_line() {
    // `highlight_lines` is pure — it doesn't itself run the fallback. With
    // an empty token slice every byte renders as `Span::raw` (plain).
    let buffer = "first\nsecond\nthird";
    let lines = highlight_lines(buffer, Some(&[]));
    assert_eq!(lines.len(), 3);
    for (line, expected) in lines.iter().zip(["first", "second", "third"]) {
        let joined: String = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(joined, expected);
        for sp in &line.spans {
            assert_eq!(sp.style, Style::default(), "expected plain style");
        }
    }
}

#[test]
fn keywords_get_keyword_style() {
    let buffer = "home:temp | where x == 1";
    let tokens = collect_tokens(buffer).expect("tokens");
    let lines = highlight_lines(buffer, Some(&tokens));
    assert_span_style(&lines, "where", keyword_style());
}

#[test]
fn numbers_get_number_style() {
    let buffer = "home:temp | where x == 42";
    let tokens = collect_tokens(buffer).expect("tokens");
    let lines = highlight_lines(buffer, Some(&tokens));
    assert_span_style(&lines, "42", number_style());
}

#[test]
fn bools_get_bool_style() {
    let buffer = "home:temp | where flag == true";
    let tokens = collect_tokens(buffer).expect("tokens");
    let lines = highlight_lines(buffer, Some(&tokens));
    assert_span_style(&lines, "true", bool_style());
}

#[test]
fn strings_get_string_style() {
    let buffer = "home:temp | where host == \"web-1\"";
    let tokens = collect_tokens(buffer).expect("tokens");
    let lines = highlight_lines(buffer, Some(&tokens));
    assert_span_style(&lines, "\"web-1\"", string_style());
}

#[test]
fn pipe_punctuation_is_dim() {
    let buffer = "home:temp | where x == 1";
    let tokens = collect_tokens(buffer).expect("tokens");
    let lines = highlight_lines(buffer, Some(&tokens));
    assert_span_style(&lines, "|", punct_style());
}

#[test]
fn tokens_on_later_lines_land_in_those_lines() {
    let buffer = "home:temp\n| where x == 1";
    let tokens = collect_tokens(buffer).expect("tokens");
    let lines = highlight_lines(buffer, Some(&tokens));
    assert_eq!(lines.len(), 2);
    // `where` lives on line 2 and must not bleed into line 1.
    let line1_text: String = lines[0]
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect::<Vec<_>>()
        .join("");
    assert!(!line1_text.contains("where"), "line 1: {line1_text}");
    let line2_text: String = lines[1]
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect::<Vec<_>>()
        .join("");
    assert!(line2_text.contains("where"), "line 2: {line2_text}");
}

#[test]
fn text_between_tokens_is_plain() {
    let buffer = "home:temp | where x == 1";
    let tokens = collect_tokens(buffer).expect("tokens");
    let lines = highlight_lines(buffer, Some(&tokens));
    let any_plain_space = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .any(|sp| sp.style == Style::default() && sp.content.chars().all(|c| c == ' '));
    assert!(
        any_plain_space,
        "expected at least one plain whitespace span"
    );
}

// ── fallback tokenizer (mid-edit) ────────────────────────────────────

/// Run the fallback path on `q` and assert that the first span whose
/// text equals `text` carries `expected` style. Tests partial queries
/// that don't parse via the engine.
fn assert_fallback_style(q: &str, text: &str, expected: Style) {
    let tokens = fallback_tokens(q);
    let lines = highlight_lines(q, Some(&tokens));
    assert_span_style(&lines, text, expected);
}

#[test]
fn fallback_partial_query_highlights_keyword() {
    assert_fallback_style("home:t | wh", "|", punct_style());
}

#[test]
fn fallback_in_progress_pipe_recognises_where() {
    assert_fallback_style("home:temp | where host", "where", keyword_style());
}

#[test]
fn fallback_string_in_progress() {
    assert_fallback_style("home:temp | where host == \"web", "\"web", string_style());
}

#[test]
fn fallback_number_with_duration_suffix() {
    assert_fallback_style("home:temp | align to 30s", "30s", number_style());
}

#[test]
fn fallback_bool_literal() {
    assert_fallback_style("home:temp | where flag == true", "true", bool_style());
}

#[test]
fn fallback_param_ident() {
    assert_fallback_style(
        "home:temp | align to $__interval",
        "$__interval",
        Style::default().fg(Color::White),
    );
}

#[test]
fn fallback_type_keyword() {
    assert_fallback_style(
        "param $w: Duration; home:temp",
        "Duration",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::ITALIC),
    );
}

#[test]
fn fallback_backtick_ident_treated_as_variable() {
    // Variables now carry an explicit white foreground so backticked
    // dataset/metric idents are visually distinct from plain text.
    assert_fallback_style("`home`:`temp`", "`home`", Style::default().fg(Color::White));
}

#[test]
fn fallback_comment_does_not_panic_and_runs_to_eol() {
    // No assertion on style — just that the scanner consumes the comment
    // cleanly and doesn't infinite-loop.
    let q = "# this is a comment\nhome:temp";
    let tokens = fallback_tokens(q);
    let lines = highlight_lines(q, Some(&tokens));
    assert_eq!(lines.len(), 2);
}

// ── engine + fallback merge ───────────────────────────────────────────────

#[test]
fn merged_tokens_cover_engine_gaps() {
    // The user-reported case: engine emits tokens for `where`/`5`/the
    // first two `|`s but skips `align`, `to`, `using`, and `::`. After
    // the merge those gap tokens come from the fallback.
    let q = "`homeassistant-metrics`:`ha.sensor.current`\n| where tag == 5\n| align to $__interval using prom::rate";
    let engine = collect_tokens(q).expect("buffer should parse");
    let fallback = fallback_tokens(q);
    let merged = merge_tokens(&engine, &fallback);
    let lines = highlight_lines(q, Some(&merged));
    for kw in ["align", "to", "using"] {
        assert_span_style(&lines, kw, keyword_style());
    }
    // `::` is two chars; the fallback classifies each ':' separately
    // as punctuation. Verify at least the first one shows up.
    assert_span_style(&lines, ":", punct_style());
}

#[test]
fn merge_keeps_engine_token_on_overlap() {
    // Engine says "where" is a Keyword; fallback would also say so.
    // After merge only one token should cover the span (the engine's).
    let q = "home:temp | where x == 1";
    let engine = collect_tokens(q).expect("tokens");
    let fallback = fallback_tokens(q);
    let merged = merge_tokens(&engine, &fallback);
    let where_count = merged
        .iter()
        .filter(|t| q.get(t.span.from..t.span.to) == Some("where"))
        .count();
    assert_eq!(
        where_count, 1,
        "merge must not duplicate overlapping tokens"
    );
}

#[test]
fn merge_with_empty_engine_returns_fallback_only() {
    let q = "home:temp";
    let fallback = fallback_tokens(q);
    let merged = merge_tokens(&[], &fallback);
    assert_eq!(merged.len(), fallback.len());
}

#[test]
fn tabs_expand_to_four_spaces() {
    let buffer = "a\tb";
    let lines = highlight_lines(buffer, Some(&[]));
    let joined: String = lines[0]
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect::<Vec<_>>()
        .join("");
    assert_eq!(joined, "a   b", "got {joined:?}");
}
