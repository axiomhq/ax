//! Per-pane / per-mode key handlers + the `on_key` dispatch entry
//! point.
//!
//! `App::on_key` is the only public method here — it consumes a raw
//! `KeyEvent`, decides which surface owns the keystroke (overlay,
//! pane, mode), and delegates to the corresponding `handle_*_key`
//! method. The handlers themselves are private; they mutate `App`
//! state and call back into editing / command / completion paths
//! that live in other submodules.

use super::*;

impl App {

    pub fn on_key(&mut self, key: KeyEvent) {
        // Dashboard picker takes precedence over every other key handler
        // when it's visible. Owns its own keymap (arrows + Enter +
        // printable for the filter); only Esc closes it.
        if self.dashboards.visible {
            self.handle_dashboards_picker_key(key);
            return;
        }

        // `:time` quick-select overlay. Owns its own modal keymap so
        // motion keys don't bleed through to the editor/dashboard.
        if self.time_picker.is_some() {
            self.handle_time_picker_key(key);
            return;
        }

        // Help modal: owns its own scroll-friendly keymap. j/k/Ctrl-d/u
        // scroll, g/G jump to top/bottom, any other key dismisses.
        // Handled here so the modal works from every pane and mode,
        // not just the few that had ad-hoc guards before.
        if self.help_visible {
            self.handle_help_key(key);
            return;
        }

        // `:dashinfo` overlay: any key dismisses. Sits above the picker
        // logically but below it in priority — they're mutually
        // exclusive in practice (picker hides itself on Enter).
        if self.dashinfo_visible {
            self.dashinfo_visible = false;
            return;
        }

        // `:tile json` inspect overlay: any key dismisses.
        if self.tile_inspect_json.is_some() {
            self.tile_inspect_json = None;
            return;
        }

        // `Ctrl-w` is the window-prefix in any mode; the next key picks
        // the target pane. Handled before mode dispatch so it works from
        // Insert, Visual, and the legend itself.
        if self.pending_ctrl_w {
            self.pending_ctrl_w = false;
            self.handle_ctrl_w_followup(key);
            return;
        }
        if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('w') {
            self.pending_ctrl_w = true;
            return;
        }

        // Legend / params / dashboard own their own bindings when
        // focused; the modal editor's mode is irrelevant on those
        // surfaces.
        if self.focus == Pane::Legend {
            self.handle_legend_key(key);
            return;
        }
        if self.focus == Pane::Params {
            self.handle_params_key(key);
            return;
        }
        if self.focus == Pane::Dashboard {
            self.handle_dashboard_key(key);
            return;
        }

        match self.mode {
            Mode::Insert => self.handle_insert_key(key),
            Mode::Normal => self.handle_normal_key(key),
            Mode::Command => self.handle_command_key(key),
            Mode::Visual | Mode::VisualLine => self.handle_visual_key(key),
        }
    }

    /// Keymap for the dashboard grid pane. The dispatch order is:
    ///
    ///   1. Active sub-mode (Move/Resize/ConfirmDelete/AddPick) owns
    ///      every key while engaged — Esc cancels back to Idle.
    ///   2. `Idle` accepts the navigation + entry-point shortcuts
    ///      (m, s, d, a, v, R, Enter, hjkl/arrows, Tab).
    fn handle_dashboard_key(&mut self, key: KeyEvent) {
        // Sub-mode takes precedence.
        match self.tile_submode.clone() {
            TileSubMode::Move { original } => return self.handle_move_key(key, original),
            TileSubMode::Resize { original } => return self.handle_resize_key(key, original),
            TileSubMode::ConfirmDelete => return self.handle_confirm_delete_key(key),
            TileSubMode::AddPick { cursor } => return self.handle_add_pick_key(key, cursor),
            TileSubMode::Idle => {}
        }
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                self.focus = Pane::Editor;
            }
            (KeyCode::Left, _) | (KeyCode::Char('h'), KeyModifiers::NONE) => {
                self.move_dashboard_selection_spatial(SpatialDir::Left);
            }
            (KeyCode::Right, _) | (KeyCode::Char('l'), KeyModifiers::NONE) => {
                self.move_dashboard_selection_spatial(SpatialDir::Right);
            }
            (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::NONE) => {
                self.move_dashboard_selection_spatial(SpatialDir::Up);
            }
            (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::NONE) => {
                self.move_dashboard_selection_spatial(SpatialDir::Down);
            }
            (KeyCode::Tab, _) => {
                self.move_dashboard_selection(1);
            }
            (KeyCode::BackTab, _) => {
                self.move_dashboard_selection(-1);
            }
            (KeyCode::Enter, _) | (KeyCode::Char('v'), KeyModifiers::NONE) => {
                self.zoom_selected_chart();
            }
            // `:` drops into the ex-command line while preserving the
            // current pane so Enter/Esc returns to the grid. Without
            // this arm the colon was silently swallowed by the final
            // `_ => {}` and the user had to Esc back to the editor to
            // run any `:` command from grid view.
            (KeyCode::Char(':'), KeyModifiers::NONE)
            | (KeyCode::Char(':'), KeyModifiers::SHIFT) => self.prefill_command(""),
            // `?` opens the help modal. Centralised dismissal in
            // `on_key` means we just trigger here — scrolling and
            // closing happen above pane dispatch.
            (KeyCode::Char('?'), _) => self.open_help(),
            (KeyCode::Char('m'), KeyModifiers::NONE) => self.enter_tile_move(),
            (KeyCode::Char('s'), KeyModifiers::NONE) => self.enter_tile_resize(),
            (KeyCode::Char('d'), KeyModifiers::NONE) => self.enter_tile_confirm_delete(),
            (KeyCode::Char('a'), KeyModifiers::NONE) => self.enter_tile_add_pick(),
            (KeyCode::Char('R'), KeyModifiers::SHIFT)
            | (KeyCode::Char('R'), KeyModifiers::NONE) => {
                self.run_focused_tile_query();
            }
            (KeyCode::Char('r'), KeyModifiers::CONTROL) => {
                self.run_tile_queries();
                self.status = format!("refetching {} tile(s)…", self.tile_results.len().max(1));
            }
            // Vertical scroll. `j`/`k` are owned by spatial nav above
            // so we use vim's scroll-by-screen bindings here. The
            // renderer clamps to valid range each frame.
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                self.dashboard_scroll = self.dashboard_scroll.saturating_add(10);
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                self.dashboard_scroll = self.dashboard_scroll.saturating_sub(10);
            }
            (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                self.dashboard_scroll = self.dashboard_scroll.saturating_add(20);
            }
            (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                self.dashboard_scroll = self.dashboard_scroll.saturating_sub(20);
            }
            (KeyCode::Char('g'), KeyModifiers::NONE) => {
                self.dashboard_scroll = 0;
            }
            (KeyCode::Char('G'), KeyModifiers::NONE)
            | (KeyCode::Char('G'), KeyModifiers::SHIFT) => {
                self.dashboard_scroll = u16::MAX; // renderer clamps to max
            }
            _ => {}
        }
    }

    fn enter_tile_move(&mut self) {
        let Some(original) = self.snapshot_selected_layout() else {
            self.status = "no tile selected".to_string();
            return;
        };
        self.tile_submode = TileSubMode::Move { original };
        self.status = "MOVE: arrows = nudge, Enter = commit, Esc = cancel".to_string();
    }

    fn enter_tile_resize(&mut self) {
        let Some(original) = self.snapshot_selected_layout() else {
            self.status = "no tile selected".to_string();
            return;
        };
        self.tile_submode = TileSubMode::Resize { original };
        self.status =
            "RESIZE: Right/Down grow, Left/Up shrink, Enter = commit, Esc = cancel".to_string();
    }

    fn enter_tile_confirm_delete(&mut self) {
        if self.current_chart_id().is_none() {
            self.status = "no tile selected".to_string();
            return;
        }
        self.tile_submode = TileSubMode::ConfirmDelete;
        self.status = "DELETE: y to confirm, any other key cancels".to_string();
    }

    fn enter_tile_add_pick(&mut self) {
        if self.loaded_dashboard.is_none() {
            self.status = "no dashboard loaded".to_string();
            return;
        }
        self.tile_submode = TileSubMode::AddPick { cursor: 0 };
        self.status = "ADD: arrows pick kind, Enter inserts, Esc cancels".to_string();
    }

    fn handle_move_key(&mut self, key: KeyEvent, original: crate::axiom::LayoutItem) {
        let Some(id) = self.current_chart_id() else {
            self.tile_submode = TileSubMode::Idle;
            return;
        };
        let mut translate = |dx: i32, dy: i32| {
            let Some(resource) = self.loaded_dashboard.as_mut() else {
                return;
            };
            match tile_ops::translate(&mut resource.dashboard.layout, &id, dx, dy) {
                Ok(()) => {
                    self.dashboard_dirty = true;
                }
                Err(reason) => {
                    self.status = format!("move blocked: {reason}");
                }
            }
        };
        match (key.code, key.modifiers) {
            (KeyCode::Left, _) | (KeyCode::Char('h'), KeyModifiers::NONE) => translate(-1, 0),
            (KeyCode::Right, _) | (KeyCode::Char('l'), KeyModifiers::NONE) => translate(1, 0),
            (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::NONE) => translate(0, -1),
            (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::NONE) => translate(0, 1),
            (KeyCode::Enter, _) => {
                self.tile_submode = TileSubMode::Idle;
                self.status = "move committed".to_string();
            }
            (KeyCode::Esc, _) => self.revert_layout(original),
            _ => {}
        }
    }

    fn handle_resize_key(&mut self, key: KeyEvent, original: crate::axiom::LayoutItem) {
        let Some(id) = self.current_chart_id() else {
            self.tile_submode = TileSubMode::Idle;
            return;
        };
        let mut resize = |dw: i32, dh: i32| {
            let Some(resource) = self.loaded_dashboard.as_mut() else {
                return;
            };
            match tile_ops::resize(&mut resource.dashboard.layout, &id, dw, dh) {
                Ok(()) => {
                    self.dashboard_dirty = true;
                }
                Err(reason) => {
                    self.status = format!("resize blocked: {reason}");
                }
            }
        };
        match (key.code, key.modifiers) {
            (KeyCode::Right, _) | (KeyCode::Char('l'), KeyModifiers::NONE) => resize(1, 0),
            (KeyCode::Left, _) | (KeyCode::Char('h'), KeyModifiers::NONE) => resize(-1, 0),
            (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::NONE) => resize(0, 1),
            (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::NONE) => resize(0, -1),
            (KeyCode::Enter, _) => {
                self.tile_submode = TileSubMode::Idle;
                self.status = "resize committed".to_string();
            }
            (KeyCode::Esc, _) => self.revert_layout(original),
            _ => {}
        }
    }

    fn handle_confirm_delete_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let Some(id) = self.current_chart_id() else {
                    self.tile_submode = TileSubMode::Idle;
                    return;
                };
                if let Some(resource) = self.loaded_dashboard.as_mut()
                    && let Ok(()) = tile_ops::delete(
                        &mut resource.dashboard.charts,
                        &mut resource.dashboard.layout,
                        &id,
                    )
                {
                    self.dashboard_dirty = true;
                    let n = resource.dashboard.charts.len();
                    if self.selected_chart_idx >= n {
                        self.selected_chart_idx = n.saturating_sub(1);
                    }
                    self.status = format!("deleted tile {id}");
                }
                self.tile_submode = TileSubMode::Idle;
            }
            _ => {
                self.tile_submode = TileSubMode::Idle;
                self.status = "delete cancelled".to_string();
            }
        }
    }

    fn handle_add_pick_key(&mut self, key: KeyEvent, cursor: usize) {
        // The picker shows every implemented `VizKind`.
        let kinds = add_pick_kinds();
        let n = kinds.len();
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                self.tile_submode = TileSubMode::Idle;
                self.status = "add cancelled".to_string();
            }
            (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                let next = (cursor + n - 1) % n;
                self.tile_submode = TileSubMode::AddPick { cursor: next };
            }
            (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                let next = (cursor + 1) % n;
                self.tile_submode = TileSubMode::AddPick { cursor: next };
            }
            (KeyCode::Enter, _) => {
                let kind = kinds[cursor];
                if let Some(resource) = self.loaded_dashboard.as_mut() {
                    let id = tile_ops::insert_tile(
                        &mut resource.dashboard.charts,
                        &mut resource.dashboard.layout,
                        kind,
                        "new tile",
                    );
                    self.dashboard_dirty = true;
                    self.selected_chart_idx = resource.dashboard.charts.len() - 1;
                    self.status = format!("added {} tile {id}", kind.as_str());
                }
                self.tile_submode = TileSubMode::Idle;
            }
            _ => {}
        }
    }

    fn handle_ctrl_w_followup(&mut self, key: KeyEvent) {
        // Spatial layout (matches the rendered grid):
        //   +---------+---+
        //   |  graph  | L |   (top:    Legend)
        //   +---------+---+
        //   |  editor | P |   (bottom: Params)
        //   +---------+---+
        // In Grid view the graph slot is the Dashboard pane, so the
        // top-left neighbour of Legend is Dashboard (not Editor).
        // `w` cycles Editor → Legend → Params → (Dashboard if Grid)
        // → Editor; directional keys use the layout to pick the
        // spatial neighbour and fall back to the source pane when
        // there's no neighbour in that direction.
        let cycle = || -> Pane {
            match self.focus {
                Pane::Editor => Pane::Legend,
                Pane::Legend => Pane::Params,
                Pane::Params => {
                    if self.view_mode == ViewMode::Grid {
                        Pane::Dashboard
                    } else {
                        Pane::Editor
                    }
                }
                Pane::Dashboard => Pane::Editor,
            }
        };
        let next = match (key.code, key.modifiers) {
            (KeyCode::Char('w'), _) => cycle(),
            // `Ctrl-w d` jumps straight to the dashboard pane. No-op if
            // no dashboard is loaded.
            (KeyCode::Char('d'), _) => {
                if self.loaded_dashboard.is_some() && self.view_mode == ViewMode::Grid {
                    Pane::Dashboard
                } else {
                    self.status = ":Ctrl-w d: no grid view".to_string();
                    return;
                }
            }
            (KeyCode::Char('h'), KeyModifiers::NONE) | (KeyCode::Left, _) => match self.focus {
                // In Grid view, Legend's left neighbour is the
                // Dashboard tile area (the graph slot); in Solo
                // there's no top-left pane, so fall back to Editor.
                Pane::Legend => {
                    if self.view_mode == ViewMode::Grid && self.loaded_dashboard.is_some() {
                        Pane::Dashboard
                    } else {
                        Pane::Editor
                    }
                }
                Pane::Params => Pane::Editor,
                Pane::Editor => Pane::Editor,
                // Dashboard is already leftmost — no-op.
                Pane::Dashboard => Pane::Dashboard,
            },
            (KeyCode::Char('l'), KeyModifiers::NONE) | (KeyCode::Right, _) => match self.focus {
                Pane::Editor => Pane::Params,
                Pane::Legend => Pane::Legend,
                Pane::Params => Pane::Params,
                // Dashboard's right neighbour is the Legend column.
                Pane::Dashboard => Pane::Legend,
            },
            (KeyCode::Char('j'), KeyModifiers::NONE) | (KeyCode::Down, _) => match self.focus {
                Pane::Legend => Pane::Params,
                Pane::Editor => Pane::Editor,
                Pane::Params => Pane::Params,
                Pane::Dashboard => Pane::Editor,
            },
            (KeyCode::Char('k'), KeyModifiers::NONE) | (KeyCode::Up, _) => match self.focus {
                Pane::Params => Pane::Legend,
                Pane::Editor => {
                    if self.view_mode == ViewMode::Grid {
                        Pane::Dashboard
                    } else {
                        Pane::Legend
                    }
                }
                Pane::Legend => Pane::Legend,
                Pane::Dashboard => Pane::Dashboard,
            },
            (KeyCode::Esc, _) => return,
            _ => return,
        };
        self.set_focus(next);
    }

    pub(super) fn set_focus(&mut self, pane: Pane) {
        if pane == Pane::Legend && self.series.is_empty() {
            self.status = "no series to focus".to_string();
            return;
        }
        self.focus = pane;
        if pane != Pane::Legend {
            self.legend_details_visible = false;
        }
        if pane == Pane::Params {
            // Clamp on entry so a stale index from a previous buffer
            // shape doesn't render off the end.
            let n = self.param_rows().len();
            if n == 0 {
                self.params_selected = 0;
            } else if self.params_selected >= n {
                self.params_selected = n - 1;
            }
        }
    }

    fn handle_params_key(&mut self, key: KeyEvent) {
        let rows = self.param_rows();
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('h'), KeyModifiers::NONE) | (KeyCode::Left, _) => {
                self.set_focus(Pane::Editor);
            }
            (KeyCode::Char('j'), KeyModifiers::NONE) | (KeyCode::Down, _) => {
                self.move_params_selection(1, &rows);
            }
            (KeyCode::Char('k'), KeyModifiers::NONE) | (KeyCode::Up, _) => {
                self.move_params_selection(-1, &rows);
            }
            (KeyCode::Char('g'), KeyModifiers::NONE) => {
                self.params_selected = 0;
            }
            (KeyCode::Char('G'), _) if !rows.is_empty() => {
                self.params_selected = rows.len() - 1;
            }
            // `a` / `i` — add new param. Drop into command mode with a
            // bare `p ` prefix so the user types `NAME=VALUE`.
            (KeyCode::Char('a'), KeyModifiers::NONE) | (KeyCode::Char('i'), KeyModifiers::NONE) => {
                self.prefill_command("p ");
            }
            // `e` / `Enter` — edit selected row. Pre-fills with the
            // current value so the user can tweak in place.
            (KeyCode::Char('e'), KeyModifiers::NONE) | (KeyCode::Enter, _) => {
                if let Some(row) = rows.get(self.params_selected) {
                    let v = row.value.as_deref().unwrap_or("");
                    self.prefill_command(&format!("p {}={}", row.name, v));
                }
            }
            // `x` / `dd` — clear the selected value.
            (KeyCode::Char('x'), KeyModifiers::NONE) => {
                if let Some(row) = rows.get(self.params_selected).cloned() {
                    if self.cli_params.remove(&row.name).is_some() {
                        self.status = format!("cleared ${}", row.name);
                    } else {
                        self.status = format!("${} not set", row.name);
                    }
                }
            }
            (KeyCode::Char('?'), _) => self.open_help(),
            (KeyCode::Char('q'), KeyModifiers::NONE) => self.cmd_quit(false),
            _ => {}
        }
    }

    fn move_params_selection(&mut self, delta: i32, rows: &[crate::params::ParamRow]) {
        if rows.is_empty() {
            self.params_selected = 0;
            return;
        }
        let n = rows.len() as i32;
        let cur = self.params_selected as i32;
        let next = (cur + delta).rem_euclid(n);
        self.params_selected = next as usize;
    }

    /// Drop into Command mode with `text` already on the line and the
    /// cursor at the end. Shared by the params pane's add/edit bindings.
    /// Remembers the current pane so the cmdline can return focus to it
    /// once the command is submitted or cancelled.
    fn prefill_command(&mut self, text: &str) {
        self.cmdline_return_focus = Some(self.focus);
        self.cmdline.reset();
        self.cmdline.buf = text.to_string();
        self.cmdline.cursor = self.cmdline.buf.chars().count();
        self.mode = Mode::Command;
        self.status = String::new();
        // The cmdline lives at the bottom of the screen and consumes
        // keys through `handle_command_key` while `mode == Command`;
        // pane focus is irrelevant during that period. We drop to
        // Editor so any pane-specific key handlers stop firing.
        self.focus = Pane::Editor;
    }

    /// Restore pane focus after the command line closes. Used by both
    /// the Enter and Esc paths so cancelling a prefilled `:p` also
    /// brings the user back to the pane they came from.
    fn restore_cmdline_focus(&mut self) {
        if let Some(pane) = self.cmdline_return_focus.take() {
            // `set_focus` enforces the same invariants as any other
            // focus change (e.g. won't focus Legend with no series).
            self.set_focus(pane);
        }
    }

    fn handle_legend_key(&mut self, key: KeyEvent) {
        // Details modal owns its own bindings while open.
        if self.legend_details_visible {
            self.handle_legend_details_key(key);
            return;
        }

        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('h'), KeyModifiers::NONE) | (KeyCode::Left, _) => {
                self.set_focus(Pane::Editor)
            }
            (KeyCode::Char('j'), KeyModifiers::NONE) | (KeyCode::Down, _) => {
                self.move_legend_selection(1);
            }
            (KeyCode::Char('k'), KeyModifiers::NONE) | (KeyCode::Up, _) => {
                self.move_legend_selection(-1);
            }
            (KeyCode::Char('g'), KeyModifiers::NONE) => {
                // `gg` to top — simple two-key here; pending_g lives in the
                // parser but the legend has its own little state.
                self.legend_selected = 0;
            }
            (KeyCode::Char('G'), _) if !self.active_legend_series().is_empty() => {
                self.legend_selected = self.active_legend_series().len() - 1;
            }
            (KeyCode::Char(' '), KeyModifiers::NONE) | (KeyCode::Enter, _) => {
                self.legend_toggle_current();
            }
            (KeyCode::Char('a'), KeyModifiers::NONE) => {
                self.legend_toggle_all();
            }
            (KeyCode::Char('e'), KeyModifiers::NONE)
                if !self.active_legend_series().is_empty() =>
            {
                self.legend_details_visible = true;
                self.details_cursor = 0;
            }
            (KeyCode::Char('?'), _) => self.open_help(),
            (KeyCode::Char('q'), KeyModifiers::NONE) => self.cmd_quit(false),
            _ => {}
        }
    }

    fn move_legend_selection(&mut self, delta: i32) {
        let n = self.active_legend_series().len();
        if n == 0 {
            return;
        }
        let n = n as i32;
        let cur = self.legend_selected as i32;
        let next = (cur + delta).rem_euclid(n);
        self.legend_selected = next as usize;
    }

    fn legend_toggle_current(&mut self) {
        if let Some(flag) = self.legend_hidden.get_mut(self.legend_selected) {
            *flag = !*flag;
        }
    }

    fn handle_legend_details_key(&mut self, key: KeyEvent) {
        let tag_count = self
            .active_legend_index()
            .and_then(|i| self.active_legend_series().get(i))
            .map(|s| s.tags.len())
            .unwrap_or(0);
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _)
            | (KeyCode::Char('e'), KeyModifiers::NONE)
            | (KeyCode::Char('q'), KeyModifiers::NONE) => {
                self.legend_details_visible = false;
            }
            (KeyCode::Char('j'), KeyModifiers::NONE) | (KeyCode::Down, _) if tag_count > 0 => {
                self.details_cursor = (self.details_cursor + 1) % tag_count;
            }
            (KeyCode::Char('k'), KeyModifiers::NONE) | (KeyCode::Up, _) if tag_count > 0 => {
                self.details_cursor = if self.details_cursor == 0 {
                    tag_count - 1
                } else {
                    self.details_cursor - 1
                };
            }
            (KeyCode::Char('g'), KeyModifiers::NONE) => self.details_cursor = 0,
            (KeyCode::Char('G'), _) if tag_count > 0 => {
                self.details_cursor = tag_count - 1;
            }
            (KeyCode::Char(' '), KeyModifiers::NONE) | (KeyCode::Enter, _) => {
                self.toggle_label_tag_at_cursor();
            }
            _ => {}
        }
    }

    fn toggle_label_tag_at_cursor(&mut self) {
        // Clone the key first so we don't hold a borrow across the
        // mutation of `legend_label_tags`.
        let key = {
            let Some(idx) = self.active_legend_index() else {
                return;
            };
            let series_slice = self.active_legend_series();
            let Some(series) = series_slice.get(idx) else {
                return;
            };
            let Some((k, _)) = series.tags.get(self.details_cursor) else {
                return;
            };
            k.clone()
        };
        if let Some(pos) = self.legend_label_tags.iter().position(|kk| kk == &key) {
            self.legend_label_tags.remove(pos);
        } else {
            self.legend_label_tags.push(key);
        }
        self.persist_legend_label_tags();
    }

    /// Smart toggle: if any series is currently hidden, show all; otherwise
    /// hide all. Vim's `:hidden` toggle convention.
    fn legend_toggle_all(&mut self) {
        if self.legend_hidden.is_empty() {
            return;
        }
        let any_hidden = self.legend_hidden.iter().any(|h| *h);
        let target = !any_hidden;
        for h in &mut self.legend_hidden {
            *h = target;
        }
    }

    fn handle_insert_key(&mut self, key: KeyEvent) {
        // Completion popup intercepts a small set of keys.
        if self.completions.visible {
            match (key.code, key.modifiers) {
                (KeyCode::Esc, _) => {
                    self.completions.hide();
                    return;
                }
                (KeyCode::Tab, KeyModifiers::NONE) | (KeyCode::Enter, KeyModifiers::NONE) => {
                    self.accept_completion();
                    return;
                }
                (KeyCode::Up, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                    self.move_completion_selection(-1);
                    return;
                }
                (KeyCode::Down, _) | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                    self.move_completion_selection(1);
                    return;
                }
                _ => {}
            }
        }

        // Trigger keys: Tab and Ctrl-Space.
        if matches!(
            (key.code, key.modifiers),
            (KeyCode::Tab, KeyModifiers::NONE) | (KeyCode::Char(' '), KeyModifiers::CONTROL),
        ) {
            self.open_completions();
            return;
        }

        if key.code == KeyCode::Esc {
            self.mode = Mode::Normal;
            return;
        }

        let consumed = self.editor.input(key);
        if consumed {
            if self.completions.visible {
                self.refresh_completions();
            }
            self.recompute_diagnostics();
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) {

        // Hover popup: any key other than `K` dismisses it (so the user can
        // also re-trigger by pressing `K` over a different ident).
        if self.hover.is_some() && !matches!((key.code, key.modifiers), (KeyCode::Char('K'), _)) {
            self.hover = None;
        }

        // The quick-fix picker takes over a small set of keys while visible.
        if self.quickfix.visible {
            match (key.code, key.modifiers) {
                (KeyCode::Esc, _) | (KeyCode::Char('q'), KeyModifiers::NONE) => {
                    self.quickfix.hide();
                    return;
                }
                (KeyCode::Enter, _) => {
                    self.accept_quickfix();
                    return;
                }
                (KeyCode::Up, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                    self.move_quickfix_selection(-1);
                    return;
                }
                (KeyCode::Down, _) | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                    self.move_quickfix_selection(1);
                    return;
                }
                _ => return,
            }
        }

        match self.cmd_parser.feed(key) {
            Step::Pending | Step::Cancel => {}
            Step::Emit(cmd) => self.run_command(cmd),
        }
        // Any keystroke may have moved the cursor or edited the buffer;
        // refresh the signature-help line so the status bar follows.
        self.recompute_sig_help();
    }

    /// Visual-mode key handler. Motion keys go through the same parser
    /// (we only consume `Command::Move` emissions); operator keys collapse
    /// the current selection into a range and apply it.
    fn handle_visual_key(&mut self, key: KeyEvent) {
        // Direct overrides.
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                self.exit_visual();
                return;
            }
            (KeyCode::Char('v'), KeyModifiers::NONE) => {
                self.exit_visual();
                return;
            }
            (KeyCode::Char('V'), _) => {
                self.mode = Mode::VisualLine;
                return;
            }
            (KeyCode::Char(op), _) if matches!(op, 'd' | 'c' | 'y' | 'x' | '>' | '<') => {
                let operator = match op {
                    'd' | 'x' => Operator::Delete,
                    'c' => Operator::Change,
                    'y' => Operator::Yank,
                    '>' => Operator::IndentRight,
                    '<' => Operator::IndentLeft,
                    _ => unreachable!(),
                };
                self.apply_visual(operator);
                return;
            }
            _ => {}
        }
        // Otherwise: feed the parser but only honour pure-motion emissions.
        // Anything else (operators, find-char, etc.) is dropped — the user
        // can always Esc and re-key in Normal mode.
        if let Step::Emit(Command::Move { motion, count }) = self.cmd_parser.feed(key) {
            self.apply_motion(motion, count);
        }
        self.recompute_sig_help();
    }

    pub(super) fn enter_command_mode(&mut self) {
        self.cmdline.reset();
        self.mode = Mode::Command;
        self.status = String::new();
    }

    /// Modal keymap for the help overlay. j/k/Up/Down/Ctrl-d/u scroll;
    /// g/G jump to top/bottom; any other key dismisses (including
    /// Esc, q, and `?` itself — the modal behaves like a peek).
    fn handle_help_key(&mut self, key: KeyEvent) {
        match (key.code, key.modifiers) {
            (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::NONE) => {
                self.help_scroll = self.help_scroll.saturating_add(1);
            }
            (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::NONE) => {
                self.help_scroll = self.help_scroll.saturating_sub(1);
            }
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                self.help_scroll = self.help_scroll.saturating_add(10);
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                self.help_scroll = self.help_scroll.saturating_sub(10);
            }
            (KeyCode::PageDown, _) | (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                self.help_scroll = self.help_scroll.saturating_add(20);
            }
            (KeyCode::PageUp, _) | (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                self.help_scroll = self.help_scroll.saturating_sub(20);
            }
            (KeyCode::Char('g'), KeyModifiers::NONE) => {
                self.help_scroll = 0;
            }
            (KeyCode::Char('G'), _) => {
                self.help_scroll = u16::MAX;
            }
            _ => {
                self.help_visible = false;
            }
        }
    }

    fn handle_command_key(&mut self, key: KeyEvent) {
        // Tab / Shift-Tab drive the completion popup; handled before
        // anything else so they never reach the printable-char path
        // below. Every other key resets the popup so successive
        // insert + tab cycles always start from a fresh candidate set.
        match (key.code, key.modifiers) {
            (KeyCode::Tab, _) => {
                self.handle_cmdline_tab(false);
                return;
            }
            (KeyCode::BackTab, _) => {
                self.handle_cmdline_tab(true);
                return;
            }
            _ => {
                // Hide the popup the moment the user does anything
                // other than navigation/accept keys. Up/Down navigate
                // the popup; Enter accepts; Esc/Ctrl-c hide it
                // explicitly via their own arms below.
                if !matches!(
                    (key.code, key.modifiers),
                    (KeyCode::Up, _) | (KeyCode::Down, _) | (KeyCode::Enter, _) | (KeyCode::Esc, _)
                ) && !matches!(
                    (key.code, key.modifiers),
                    (KeyCode::Char('c'), KeyModifiers::CONTROL)
                ) {
                    self.cmdline_completions.hide();
                }
            }
        }
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                self.cmdline.reset();
                self.cmdline_completions.hide();
                self.mode = Mode::Normal;
                self.restore_cmdline_focus();
            }
            (KeyCode::Up, _) if self.cmdline_completions.visible => {
                self.move_cmdline_completion(-1);
            }
            (KeyCode::Down, _) if self.cmdline_completions.visible => {
                self.move_cmdline_completion(1);
            }
            (KeyCode::Enter, _) => {
                // Enter accepts the highlighted completion if the
                // popup is up; otherwise it executes the cmdline.
                if self.cmdline_completions.visible {
                    self.accept_cmdline_completion();
                    return;
                }
                let cmd = std::mem::take(&mut self.cmdline.buf);
                self.cmdline.cursor = 0;
                self.mode = Mode::Normal;
                self.execute_command(cmd.trim());
                self.restore_cmdline_focus();
            }
            (KeyCode::Backspace, _) => {
                if self.cmdline.buf.is_empty() {
                    // Empty cmdline + Backspace cancels, like vim.
                    self.mode = Mode::Normal;
                } else {
                    self.cmdline.backspace();
                }
            }
            (KeyCode::Delete, _) => self.cmdline.delete_forward(),
            (KeyCode::Left, _) => self.cmdline.move_left(),
            (KeyCode::Right, _) => self.cmdline.move_right(),
            (KeyCode::Home, _) | (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                self.cmdline.move_home();
            }
            (KeyCode::End, _) | (KeyCode::Char('e'), KeyModifiers::CONTROL) => {
                self.cmdline.move_end();
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                // Clear from cursor to start — standard readline behaviour.
                let to = self.cmdline.byte_cursor();
                self.cmdline.buf.drain(..to);
                self.cmdline.cursor = 0;
            }
            (KeyCode::Char('k'), KeyModifiers::CONTROL) => {
                // Clear from cursor to end.
                let from = self.cmdline.byte_cursor();
                self.cmdline.buf.truncate(from);
            }
            (KeyCode::Char(c), m) if m == KeyModifiers::NONE || m == KeyModifiers::SHIFT => {
                self.cmdline.insert_char(c);
            }
            _ => {}
        }
    }

    /// Modal keymap for the `:time` overlay. Dispatches by sub-state:
    /// the preset list takes simple cursor motion + Enter (with the
    /// trailing "Custom…" row transitioning into the calendar view);
    /// the calendar view takes day/week/month navigation + Tab to
    /// switch focus between start and end.
    fn handle_time_picker_key(&mut self, key: KeyEvent) {
        let state = match self.time_picker.take() {
            Some(s) => s,
            None => return,
        };
        match state {
            TimePickerState::Presets { cursor } => {
                self.handle_time_preset_key(cursor, key);
            }
            TimePickerState::Custom(picker) => {
                self.handle_time_custom_key(picker, key);
            }
        }
    }

    fn handle_time_preset_key(&mut self, cursor: usize, key: KeyEvent) {
        // Cursor range is 0..=TIME_PRESETS.len() — the last index is
        // the synthetic "Custom…" row.
        let n = TIME_PRESETS.len() + 1;
        let mut next_cursor = cursor;
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                // Already taken out via `take()`; just leave None.
                return;
            }
            (KeyCode::Enter, _) => {
                if cursor == TIME_PRESET_CUSTOM_INDEX {
                    // Transition to the calendar overlay, seeded from
                    // whatever the dashboard's current window parses
                    // as (defaulting to yesterday→today).
                    let mut picker = CustomRangePicker::seed();
                    if let Some(d) = parse_iso_date(&self.time_range.start) {
                        picker.start = d;
                    }
                    if let Some(d) = parse_iso_date(&self.time_range.end) {
                        picker.end = d;
                    }
                    self.time_picker = Some(TimePickerState::Custom(picker));
                    return;
                }
                let (_, duration) = TIME_PRESETS[cursor];
                self.set_time_range(format!("now-{duration}"), "now".to_string());
                return;
            }
            (KeyCode::Up, _)
            | (KeyCode::Char('k'), KeyModifiers::NONE)
            | (KeyCode::BackTab, _) => {
                next_cursor = (cursor + n - 1) % n;
            }
            (KeyCode::Down, _)
            | (KeyCode::Char('j'), KeyModifiers::NONE)
            | (KeyCode::Tab, _) => {
                next_cursor = (cursor + 1) % n;
            }
            (KeyCode::Char('g'), KeyModifiers::NONE) => {
                next_cursor = 0;
            }
            (KeyCode::Char('G'), _) => {
                next_cursor = n - 1;
            }
            _ => {}
        }
        self.time_picker = Some(TimePickerState::Presets { cursor: next_cursor });
    }

    fn handle_time_custom_key(&mut self, mut picker: CustomRangePicker, key: KeyEvent) {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                // Step back to the preset list rather than closing
                // outright — lets the user undo Custom without losing
                // their place in the picker.
                self.time_picker = Some(TimePickerState::Presets {
                    cursor: TIME_PRESET_CUSTOM_INDEX,
                });
            }
            (KeyCode::Enter, _) => {
                let (start, end) = picker.to_range();
                self.set_time_range(start, end);
                // set_time_range doesn't touch time_picker; explicit None.
                self.time_picker = None;
            }
            (KeyCode::Tab, _)
            | (KeyCode::BackTab, _)
            | (KeyCode::Char('\t'), _) => {
                picker.focus = match picker.focus {
                    CustomField::Start => CustomField::End,
                    CustomField::End => CustomField::Start,
                };
                self.time_picker = Some(TimePickerState::Custom(picker));
            }
            (KeyCode::Left, _) | (KeyCode::Char('h'), KeyModifiers::NONE) => {
                picker.shift_days(-1);
                self.time_picker = Some(TimePickerState::Custom(picker));
            }
            (KeyCode::Right, _) | (KeyCode::Char('l'), KeyModifiers::NONE) => {
                picker.shift_days(1);
                self.time_picker = Some(TimePickerState::Custom(picker));
            }
            (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::NONE) => {
                picker.shift_days(-7);
                self.time_picker = Some(TimePickerState::Custom(picker));
            }
            (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::NONE) => {
                picker.shift_days(7);
                self.time_picker = Some(TimePickerState::Custom(picker));
            }
            (KeyCode::Char('<'), _)
            | (KeyCode::Char(','), KeyModifiers::SHIFT)
            | (KeyCode::Char('['), KeyModifiers::NONE) => {
                picker.shift_month(-1);
                self.time_picker = Some(TimePickerState::Custom(picker));
            }
            (KeyCode::Char('>'), _)
            | (KeyCode::Char('.'), KeyModifiers::SHIFT)
            | (KeyCode::Char(']'), KeyModifiers::NONE) => {
                picker.shift_month(1);
                self.time_picker = Some(TimePickerState::Custom(picker));
            }
            _ => {
                // Unrecognised key — keep the overlay open and the
                // picker state intact.
                self.time_picker = Some(TimePickerState::Custom(picker));
            }
        }
    }

    /// Keymap for the dashboard picker overlay. The filter is
    /// edit-as-you-type; printable characters extend it, Backspace
    /// removes the last char, and navigation keys scroll the filtered
    /// list.
    fn handle_dashboards_picker_key(&mut self, key: KeyEvent) {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                self.dashboards.hide();
            }
            (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::CONTROL) => {
                self.dashboards.move_cursor(-1);
            }
            (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::CONTROL) => {
                self.dashboards.move_cursor(1);
            }
            (KeyCode::PageUp, _) => {
                self.dashboards.move_cursor(-10);
            }
            (KeyCode::PageDown, _) => {
                self.dashboards.move_cursor(10);
            }
            (KeyCode::Enter, _) => {
                if let Some(sel) = self.dashboards.selected() {
                    let uid = sel.uid.clone();
                    let name = sel.name().to_string();
                    self.last_picked_dashboard = Some(uid.clone());
                    self.fetch_dashboard_by_uid(uid.clone());
                    self.status = format!("opening dashboard `{name}` …");
                }
                self.dashboards.hide();
            }
            (KeyCode::Backspace, _) => {
                self.dashboards.filter.pop();
                self.dashboards.cursor = 0;
            }
            (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) => {
                self.dashboards.filter.push(c);
                self.dashboards.cursor = 0;
            }
            _ => {}
        }
    }
}
