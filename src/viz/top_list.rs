//! Top-list tile: sorted horizontal bars over per-series aggregates.

use std::collections::BTreeMap;

use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};

use super::agg::{Agg, format_value};
use crate::chart::Series;

/// Compute the sorted-and-truncated set of `(series_idx, agg_value)`
/// pairs that the top-list renders. Extracted from the renderer so it's
/// directly unit-testable.
pub(super) fn top_list_rows(
    series: &[Series],
    hidden: &[bool],
    agg: Agg,
    n: usize,
    ascending: bool,
) -> Vec<(usize, f64)> {
    let mut rows: Vec<(usize, f64)> = series
        .iter()
        .enumerate()
        .filter(|(i, _)| !hidden.get(*i).copied().unwrap_or(false))
        .filter_map(|(i, s)| agg.apply(&s.points).map(|v| (i, v)))
        .collect();
    if ascending {
        rows.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    } else {
        rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    }
    rows.truncate(n);
    rows
}

/// Sorted horizontal bars: one row per series, scaled to the largest
/// aggregated value in the visible set.
///
/// Options:
///   * `agg`        — default `avg`
///   * `n`          — max rows, default `10`
///   * `ascending`  — default `false` (largest first)
pub(super) fn draw_top_list(
    f: &mut Frame,
    series: &[Series],
    hidden: &[bool],
    opts: &BTreeMap<String, String>,
    block: Block<'_>,
    area: Rect,
) {
    let agg = opts
        .get("agg")
        .and_then(|s| Agg::parse(s))
        .unwrap_or(Agg::Avg);
    let n: usize = opts.get("n").and_then(|s| s.parse().ok()).unwrap_or(10);
    let ascending = opts
        .get("ascending")
        .map(|s| matches!(s.as_str(), "true" | "1" | "yes"))
        .unwrap_or(false);

    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let rows = top_list_rows(series, hidden, agg, n.min(inner.height as usize), ascending);
    if rows.is_empty() {
        let p = Paragraph::new("(no data)")
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(p, inner);
        return;
    }

    // Scale bars to the largest absolute value so a single negative series
    // doesn't render an empty row.
    let max_abs = rows.iter().map(|(_, v)| v.abs()).fold(0.0_f64, f64::max);

    // Layout per row: [bar 60%]  [label]  [value]
    let label_w = rows
        .iter()
        .map(|(i, _)| series[*i].name.chars().count() as u16)
        .max()
        .unwrap_or(8)
        // `(inner.width / 3).max(4)` keeps the clamp bounds ordered:
        // for narrow tiles (width 1..=11) `inner.width / 3 < 4`, and a
        // bare `.clamp(4, <4)` would panic (`min > max`).
        .clamp(4, (inner.width / 3).max(4));
    let value_w: u16 = 10;
    let bar_w = inner.width.saturating_sub(label_w + value_w + 4);

    let lines: Vec<Line<'_>> = rows
        .iter()
        .map(|(idx, v)| {
            let s = &series[*idx];
            let frac = if max_abs > 0.0 {
                v.abs() / max_abs
            } else {
                0.0
            };
            let fill = ((bar_w as f64) * frac).round() as u16;
            let mut bar = String::with_capacity(bar_w as usize);
            for _ in 0..fill {
                bar.push('▇');
            }
            for _ in fill..bar_w {
                bar.push('░');
            }
            Line::from(vec![
                Span::styled(bar, Style::default().fg(s.color)),
                Span::raw("  "),
                Span::styled(
                    format!("{:<width$}", s.name, width = label_w as usize),
                    Style::default(),
                ),
                Span::raw("  "),
                Span::styled(
                    format!(
                        "{:>width$}",
                        format_value(*v, 2, None),
                        width = value_w as usize
                    ),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
            ])
        })
        .collect();

    f.render_widget(Paragraph::new(lines), inner);
}
