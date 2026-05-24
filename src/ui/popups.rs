//! Cursor-anchored popups: code completion + quickfix menu.
//!
//! These don't go through `modal_frame` because they own their `Block`
//! (the `List` widget wants `.block()`); they share an anchor-near-cursor
//! geometry and width-clamp.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState},
};

use crate::app::App;

pub(super) const POPUP_MAX_ITEMS: usize = 10;
pub(super) const POPUP_MIN_WIDTH: u16 = 12;
pub(super) const POPUP_MAX_WIDTH: u16 = 40;

pub(super) fn draw_completion_popup(f: &mut Frame, app: &mut App, editor_area: Rect) {
    let items_len = app.completions.items.len();
    if items_len == 0 {
        return;
    }
    let width = compute_popup_width(&app.completions.items);
    let height = (items_len.min(POPUP_MAX_ITEMS) as u16) + 2; // +2 for borders

    // Place the popup just below the editor cursor. Fall back to the editor's
    // top-left when geometry would push it off-screen.
    let (cursor_row, cursor_col) = app.editor.cursor();
    // The editor block has 1-cell borders; cursor is relative to inner area.
    let anchor_x = editor_area
        .x
        .saturating_add(1 + cursor_col as u16)
        .min(editor_area.x + editor_area.width.saturating_sub(width));
    let mut anchor_y = editor_area.y.saturating_add(2 + cursor_row as u16);
    let screen = f.area();
    if anchor_y + height > screen.height {
        // Flip above the cursor if no room below.
        anchor_y = editor_area
            .y
            .saturating_add(1 + cursor_row as u16)
            .saturating_sub(height);
    }
    let popup = Rect {
        x: anchor_x,
        y: anchor_y,
        width: width.min(screen.width.saturating_sub(anchor_x)),
        height: height.min(screen.height.saturating_sub(anchor_y)),
    };
    if popup.width < 4 || popup.height < 3 {
        return;
    }

    let items: Vec<ListItem<'_>> = app
        .completions
        .items
        .iter()
        .map(|it| ListItem::new(Line::from(Span::raw(it.label.clone()))))
        .collect();
    let mut state = ListState::default();
    state.select(Some(app.completions.selected));

    let title = if app.completions.kind_label.is_empty() {
        "completions".to_string()
    } else {
        format!("completions · {}", app.completions.kind_label)
    };
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(title),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Cyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        );

    f.render_widget(Clear, popup);
    f.render_stateful_widget(list, popup, &mut state);
}

fn compute_popup_width(items: &[crate::completions::CompletionItem]) -> u16 {
    let max_item = items
        .iter()
        .map(|i| i.label.chars().count() as u16)
        .max()
        .unwrap_or(0);
    let with_padding = max_item.saturating_add(4); // borders + padding
    with_padding.clamp(POPUP_MIN_WIDTH, POPUP_MAX_WIDTH)
}

pub(super) fn draw_quickfix_popup(f: &mut Frame, app: &mut App, editor_area: Rect) {
    let items_len = app.quickfix.actions.len();
    if items_len == 0 {
        return;
    }
    let width = quickfix_popup_width(&app.quickfix);
    let height = (items_len.min(POPUP_MAX_ITEMS) as u16) + 2; // +2 for borders

    let (cursor_row, cursor_col) = app.editor.cursor();
    let anchor_x = editor_area
        .x
        .saturating_add(1 + cursor_col as u16)
        .min(editor_area.x + editor_area.width.saturating_sub(width));
    let mut anchor_y = editor_area.y.saturating_add(2 + cursor_row as u16);
    let screen = f.area();
    if anchor_y + height > screen.height {
        anchor_y = editor_area
            .y
            .saturating_add(1 + cursor_row as u16)
            .saturating_sub(height);
    }
    let popup = Rect {
        x: anchor_x,
        y: anchor_y,
        width: width.min(screen.width.saturating_sub(anchor_x)),
        height: height.min(screen.height.saturating_sub(anchor_y)),
    };
    if popup.width < 4 || popup.height < 3 {
        return;
    }

    let items: Vec<ListItem<'_>> = app
        .quickfix
        .actions
        .iter()
        .map(|a| ListItem::new(Line::from(Span::raw(a.name.clone()))))
        .collect();
    let mut state = ListState::default();
    state.select(Some(app.quickfix.selected));

    let title = format!("quick fix · {}", app.quickfix.title);
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Magenta))
                .title(title),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Magenta)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        );

    f.render_widget(Clear, popup);
    f.render_stateful_widget(list, popup, &mut state);
}

fn quickfix_popup_width(picker: &crate::app::QuickFixPicker) -> u16 {
    let max_item = picker
        .actions
        .iter()
        .map(|a| a.name.chars().count() as u16)
        .max()
        .unwrap_or(0);
    let title_w = picker.title.chars().count() as u16 + "quick fix · ".len() as u16;
    let with_padding = max_item.max(title_w).saturating_add(4);
    with_padding.clamp(POPUP_MIN_WIDTH, POPUP_MAX_WIDTH)
}
