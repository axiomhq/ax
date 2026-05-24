use super::*;
use crate::command::{Motion, TextObject};

fn r(start: usize, end: usize) -> Range {
    Range {
        start,
        end,
        linewise: false,
    }
}

// ── Word motions ──────────────────────────────────────────────────

#[test]
fn dw_includes_trailing_space() {
    // cursor on `f`, `dw` should delete `foo ` (4 bytes).
    let buf = "foo bar";
    let got = resolve_motion(buf, 0, Motion::WordForward, 1, false).unwrap();
    assert_eq!(got, r(0, 4));
}

#[test]
fn de_stops_at_word_end_inclusive() {
    let buf = "foo bar";
    let got = resolve_motion(buf, 0, Motion::WordEnd, 1, false).unwrap();
    assert_eq!(got, r(0, 3));
}

#[test]
fn cw_quirk_acts_like_ce() {
    // `cw` should behave like `ce` and stop at end of word.
    let buf = "foo bar";
    let got = resolve_motion(buf, 0, Motion::WordForward, 1, true).unwrap();
    assert_eq!(got, r(0, 3));
}

#[test]
fn db_from_word_end_deletes_current_word() {
    let buf = "foo bar";
    let got = resolve_motion(buf, 3, Motion::WordBack, 1, false).unwrap();
    assert_eq!(got, r(0, 3));
}

#[test]
fn dw_count_two_spans_two_words() {
    let buf = "foo bar baz";
    let got = resolve_motion(buf, 0, Motion::WordForward, 2, false).unwrap();
    assert_eq!(got, r(0, 8));
}

// ── Current-line range ────────────────────────────────────────────

#[test]
fn dd_includes_trailing_newline() {
    let buf = "foo\nbar\nbaz";
    let got = resolve_motion(buf, 1, Motion::CurrentLine, 1, false).unwrap();
    assert_eq!(got.start, 0);
    assert_eq!(got.end, 4);
    assert!(got.linewise);
}

#[test]
fn dd_on_last_line_pulls_in_leading_newline() {
    let buf = "foo\nbar";
    // Cursor on `bar` (last line, no trailing newline).
    let got = resolve_motion(buf, 4, Motion::CurrentLine, 1, false).unwrap();
    assert_eq!(got.start, 3);
    assert_eq!(got.end, 7);
    assert!(got.linewise);
}

#[test]
fn dd_count_three() {
    let buf = "a\nb\nc\nd\n";
    let got = resolve_motion(buf, 0, Motion::CurrentLine, 3, false).unwrap();
    assert_eq!(got.start, 0);
    assert_eq!(got.end, 6);
}

// ── Word text objects ─────────────────────────────────────────────

#[test]
fn iw_selects_inner_word() {
    let buf = "foo bar baz";
    let got = resolve_object(buf, 5, TextObject::Word { around: false }).unwrap();
    assert_eq!(got, r(4, 7));
}

#[test]
fn aw_extends_over_trailing_space() {
    let buf = "foo bar baz";
    let got = resolve_object(buf, 0, TextObject::Word { around: true }).unwrap();
    assert_eq!(got, r(0, 4));
}

#[test]
fn aw_extends_left_when_no_trailing() {
    let buf = "foo bar";
    // cursor on `b` of `bar` — no trailing whitespace, extend left.
    let got = resolve_object(buf, 4, TextObject::Word { around: true }).unwrap();
    assert_eq!(got, r(3, 7));
}

// ── Quote objects ─────────────────────────────────────────────────

#[test]
fn i_quote_excludes_quotes() {
    let buf = "x == \"hello, world\"";
    let got = resolve_object(
        buf,
        10,
        TextObject::Quote {
            quote: '"',
            around: false,
        },
    )
    .unwrap();
    assert_eq!(buf[got.start..got.end].to_string(), "hello, world");
}

#[test]
fn a_quote_includes_quotes() {
    let buf = "x == \"hi\"";
    let got = resolve_object(
        buf,
        6,
        TextObject::Quote {
            quote: '"',
            around: true,
        },
    )
    .unwrap();
    assert_eq!(buf[got.start..got.end].to_string(), "\"hi\"");
}

#[test]
fn quote_object_respects_escape() {
    let buf = "\"a\\\"b\"";
    // cursor inside; should select `a\"b` (4 chars).
    let got = resolve_object(
        buf,
        1,
        TextObject::Quote {
            quote: '"',
            around: false,
        },
    )
    .unwrap();
    assert_eq!(&buf[got.start..got.end], "a\\\"b");
}

// ── Bracket pairs ─────────────────────────────────────────────────

#[test]
fn i_paren_excludes_brackets() {
    let buf = "f(a, b)";
    let got = resolve_object(
        buf,
        3,
        TextObject::Pair {
            open: '(',
            around: false,
        },
    )
    .unwrap();
    assert_eq!(&buf[got.start..got.end], "a, b");
}

#[test]
fn a_paren_includes_brackets() {
    let buf = "f(a, b)";
    let got = resolve_object(
        buf,
        3,
        TextObject::Pair {
            open: '(',
            around: true,
        },
    )
    .unwrap();
    assert_eq!(&buf[got.start..got.end], "(a, b)");
}

#[test]
fn i_paren_handles_nesting() {
    let buf = "f(g(x), y)";
    // cursor inside outer paren but past inner; should select outer body.
    let got = resolve_object(
        buf,
        7,
        TextObject::Pair {
            open: '(',
            around: false,
        },
    )
    .unwrap();
    assert_eq!(&buf[got.start..got.end], "g(x), y");
}

#[test]
fn i_paren_inside_inner_selects_inner() {
    let buf = "f(g(x), y)";
    let got = resolve_object(
        buf,
        4,
        TextObject::Pair {
            open: '(',
            around: false,
        },
    )
    .unwrap();
    assert_eq!(&buf[got.start..got.end], "x");
}
