//! Note tile rendering: a hand-rolled markdown subset over `body`.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};

/// Render `body` as a hand-rolled markdown subset. The supported subset:
///
///   * `# ` / `## ` / `### ` headings
///   * `- ` / `* ` unordered list items
///   * Inline `**bold**`, `*italic*`, and `` `code` ``
///   * Fenced code blocks delimited by ``` lines (rendered with a dim
///     background; nested formatting is suppressed inside).
///
/// Anything outside this set renders as plain text. Pulling in a full
/// markdown crate is overkill for the kind of notes a TUI dashboard
/// typically holds; the subset above is what Axiom's own dashboard
/// notes use in practice.
pub(super) fn draw_note(f: &mut Frame, body: &str, block: Block<'_>, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    // Skip the `// @viz note` pragma line if present; the user's note
    // starts on the line after it.
    let stripped = strip_leading_pragma(body);

    // Empty-note rendering: collapse the bordered 4-row box down to a
    // single thicker horizontal divider line. Skip the `block` entirely
    // so the row reads as a section break rather than an empty tile.
    if stripped.trim().is_empty() {
        let rule_y = area.y + area.height / 2;
        let rule = Rect {
            x: area.x,
            y: rule_y,
            width: area.width,
            height: 1,
        };
        let glyphs: String = "━".repeat(rule.width as usize);
        f.render_widget(
            Paragraph::new(glyphs).style(Style::default().fg(Color::DarkGray)),
            rule,
        );
        return;
    }

    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let lines: Vec<Line<'_>> = render_markdown(stripped);
    f.render_widget(Paragraph::new(lines), inner);
}

pub(super) fn strip_leading_pragma(body: &str) -> &str {
    let bytes = body.as_bytes();
    let mut i = 0usize;
    // Walk past any leading comment lines that contain `@viz`.
    while i < bytes.len() {
        // Trim leading whitespace on the line.
        let mut j = i;
        while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
            j += 1;
        }
        // Comment lines that mention `@viz` are pragmas; skip them.
        if bytes[j..].starts_with(b"//") && body[j..].contains("@viz") {
            // Advance to next newline.
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            }
            continue;
        }
        break;
    }
    &body[i..]
}

pub(super) fn render_markdown(body: &str) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let mut in_code_block = false;
    for raw in body.lines() {
        if raw.trim_start().starts_with("```") {
            in_code_block = !in_code_block;
            // The fence itself isn't rendered.
            continue;
        }
        if in_code_block {
            out.push(Line::from(Span::styled(
                raw.to_string(),
                Style::default()
                    .fg(Color::Yellow)
                    .bg(Color::Rgb(20, 20, 20)),
            )));
            continue;
        }
        let trimmed = raw.trim_start();
        // Headings.
        if let Some(rest) = trimmed.strip_prefix("### ") {
            out.push(Line::from(Span::styled(
                format!("  {rest}"),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("## ") {
            out.push(Line::from(Span::styled(
                format!(" {rest}"),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )));
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# ") {
            out.push(Line::from(Span::styled(
                rest.to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED | Modifier::REVERSED),
            )));
            continue;
        }
        // Unordered list.
        if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            let mut spans = vec![Span::raw("  • ")];
            spans.extend(render_inline(rest));
            out.push(Line::from(spans));
            continue;
        }
        out.push(Line::from(render_inline(raw)));
    }
    out
}

/// Render inline markdown: `**bold**`, `*italic*`, `` `code` ``. Naive
/// tokeniser — no nesting, no escapes. Matches the smallest subset
/// people actually use in dashboard notes.
fn render_inline(s: &str) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut i = 0usize;
    let bytes = s.as_bytes();
    let mut plain_start = 0usize;
    let flush_plain = |out: &mut Vec<Span<'static>>, s: &str, start: usize, end: usize| {
        if end > start {
            out.push(Span::raw(s[start..end].to_string()));
        }
    };
    while i < bytes.len() {
        if bytes[i] == b'*'
            && i + 1 < bytes.len()
            && bytes[i + 1] == b'*'
            && let Some(end_rel) = s[i + 2..].find("**")
        {
            flush_plain(&mut out, s, plain_start, i);
            let inner = &s[i + 2..i + 2 + end_rel];
            out.push(Span::styled(
                inner.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            i += 2 + end_rel + 2;
            plain_start = i;
            continue;
        }
        if bytes[i] == b'*'
            && let Some(end_rel) = s[i + 1..].find('*')
        {
            flush_plain(&mut out, s, plain_start, i);
            let inner = &s[i + 1..i + 1 + end_rel];
            out.push(Span::styled(
                inner.to_string(),
                Style::default().add_modifier(Modifier::ITALIC),
            ));
            i += 1 + end_rel + 1;
            plain_start = i;
            continue;
        }
        if bytes[i] == b'`'
            && let Some(end_rel) = s[i + 1..].find('`')
        {
            flush_plain(&mut out, s, plain_start, i);
            let inner = &s[i + 1..i + 1 + end_rel];
            out.push(Span::styled(
                inner.to_string(),
                Style::default()
                    .fg(Color::Yellow)
                    .bg(Color::Rgb(20, 20, 20)),
            ));
            i += 1 + end_rel + 1;
            plain_start = i;
            continue;
        }
        i += 1;
    }
    flush_plain(&mut out, s, plain_start, bytes.len());
    if out.is_empty() {
        out.push(Span::raw(String::new()));
    }
    out
}
