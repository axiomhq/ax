use std::sync::mpsc;
use std::sync::{Arc, RwLock};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::runtime::Handle;
use tui_textarea::{CursorMove, TextArea};

use crate::axiom::{Client as AxiomClient, DashboardSummary};
use crate::cache::Cache;
use crate::chart::Series;
use crate::command::{self, Command, InsertAt, Motion, Operator, Step, Target};
use crate::completions;
use crate::config::Config;
use crate::dashboard::{TimeRange, VizKind};
use crate::editor;
use crate::hover;
use crate::motion::{self, Range};
use crate::mpl;
use crate::params;
use crate::share;
use crate::viz;

mod editing;
mod ex_cmds;
mod fetch;
mod keys;
mod helpers;
mod tile_layout;
mod types;

// Re-export the items external modules / tests reach into via
// `crate::app::*`. Internal helpers stay private and are pulled in
// through `use` below.
pub use tile_layout::{SpatialDir, build_dashboard_doc_from_buffer};
pub(crate) use tile_layout::{
    add_pick_kinds, pick_next_chart_in_direction, tile_ops,
};
pub use types::*;

use helpers::*;

pub struct App {
    pub mode: Mode,
    pub editor: TextArea<'static>,
    pub series: Vec<Series>,
    pub status: String,
    /// Most recent error in full. Surfaced as a centred overlay over the chart
    /// pane when present; dismissed with `Esc` in Normal mode.
    pub last_error: Option<String>,
    pub should_quit: bool,
    pub busy: bool,
    /// Shared discovery cache; persisted to disk by background tasks.
    pub cache: Arc<RwLock<Cache>>,
    pub completions: CompletionState,
    pub quickfix: QuickFixPicker,
    pub cmdline: CmdLine,
    /// Current cmdline completion popup state. `None` when no Tab has
    /// been pressed since the cmdline opened (or since the last
    /// non-Tab key reset it).
    pub cmdline_completions: CmdlineCompletionState,
    /// Live diagnostics for the current buffer.
    /// Recomputed by [`App::recompute_diagnostics`] on every buffer-mutating key.
    pub diagnostics: Vec<mpl::Diagnostic>,
    /// Trace identifier of the most recently completed query, surfaced on the
    /// right of the status bar so users can correlate against server logs.
    /// `None` before the first run or when the response carried no trace.
    pub last_trace_id: Option<String>,
    /// `true` while the help modal is on screen.
    pub help_visible: bool,
    /// Top row of the help modal that's currently visible. `0` puts
    /// the first line of `docs/keys.md` at the top; increased by
    /// j/Ctrl-d/G key handlers when the help modal is open so the
    /// content (now sourced from a file and longer than a screen) is
    /// scrollable instead of clipped.
    pub help_scroll: u16,
    /// Hover popup contents. `Some` when the user pressed `K` over a known
    /// stdlib function; any subsequent key dismisses it.
    pub hover: Option<hover::HoverInfo>,
    /// Active signature-help line, recomputed alongside diagnostics on
    /// every buffer-mutating or cursor-moving keystroke.
    pub sig_help: Option<hover::SigHelp>,
    /// Streaming parser for Normal-mode key chords. Holds whatever
    /// partial state has accumulated between keystrokes (count digits,
    /// pending operator, `g`-prefix flag, text-object selector).
    cmd_parser: command::Parser,
    /// Single-slot yank register populated by `y`/`d`/`c` operations and
    /// consumed by `p`/`P`. No named registers — vim's `"a.."z` are
    /// almost never used in practice and the cost-to-value ratio is poor.
    yank: Option<YankEntry>,
    /// Last `f`/`F`/`t`/`T` argument so `;` and `,` can repeat it.
    last_find: Option<FindMemo>,
    /// Last buffer-mutating command, replayed by `.`. We don't capture
    /// inserted text yet — `cw` then text + Esc replays only the delete.
    last_change: Option<Command>,
    /// In Visual mode, the byte offset where the selection started.
    /// `None` outside Visual mode.
    visual_anchor: Option<usize>,
    /// Which pane currently consumes keystrokes.
    pub focus: Pane,
    /// Index of the highlighted series in the legend (and the chart — the
    /// selected series is drawn with a brighter marker when the legend is
    /// focused so the user can see what they're about to toggle).
    pub legend_selected: usize,
    /// Per-series visibility flag, parallel to `series`. `true` means
    /// hidden from the chart. Resized on every successful query.
    pub legend_hidden: Vec<bool>,
    /// `true` while a details modal for the selected legend entry is open.
    pub legend_details_visible: bool,
    /// Cursor row inside the details modal (index into
    /// `series[legend_selected].tags`).
    pub details_cursor: usize,
    /// Tag keys, in selection order, that replace the auto-generated
    /// series label in the legend. Empty = use `series.name` as before.
    /// Reloaded from cache on every successful query (two-step fallback:
    /// AST hash, then dataset+metric); user toggles persist back via
    /// both keys so the next run remembers.
    pub legend_label_tags: Vec<String>,
    /// Identifies the query whose results currently sit in `series`.
    /// `None` before the first query completes. Captured at query
    /// dispatch so toggles persist to the right cache keys even if the
    /// user has since edited the buffer.
    pub last_query_context: Option<QueryContext>,
    /// Cursor row in the params pane. Index into the row list produced
    /// by [`crate::mpl::param_rows`] for the current buffer + provided
    /// values; clamped on every recompute so deletions don't dangle.
    pub params_selected: usize,
    /// When `Some`, the next time the command line is dismissed (either
    /// via `Enter` or `Esc`) focus is restored to this pane. Set by
    /// [`prefill_command`] so that `a`/`e` in the Params pane drop into
    /// `:p` but return the user to Params after submit. `None` for
    /// commands entered the normal way (`:` from Normal mode).
    cmdline_return_focus: Option<Pane>,
    /// `true` after `Ctrl-w` has been seen; the next key is interpreted
    /// as a window/pane command.
    pending_ctrl_w: bool,
    /// Host-supplied system parameters (e.g. `$__interval`). Substituted into
    /// the query text before validation and before sending to the API.
    pub system_params: Vec<params::SystemParam>,
    /// User-declared `param $name: type;` values supplied via `-p NAME=VALUE`
    /// on the command line. Sent verbatim to the server as `queryParams`;
    /// the server typechecks against the buffer's declared params.
    pub cli_params: std::collections::BTreeMap<String, String>,
    /// Path of the `.mpl` file currently being edited, if any. Set by `:e <path>`
    /// or the CLI argument; cleared when `:enew` (TODO) opens a fresh buffer.
    /// Searchable picker over the org's dashboards. Hidden by default;
    /// `:dashboards` (or `:db`) opens it.
    pub dashboards: DashboardPicker,
    /// Last dashboard uid the user picked from `:dashboards`. Captured
    /// so `:open` (without args) can re-fetch the same one.
    pub last_picked_dashboard: Option<String>,
    /// The dashboard currently loaded in memory. Set by
    /// `AppEvent::DashboardOpened`. Step 17b will adapt this into the
    /// internal `Dashboard` model and start rendering its tiles; for
    /// now it backs the `:dashinfo` overlay.
    pub loaded_dashboard: Option<DashboardSummary>,
    /// Toggle for the `:dashinfo` overlay. Closes on `Esc` (handled in
    /// `on_key`) and toggles via the Ex-command.
    pub dashinfo_visible: bool,
    /// `:time` overlay state. `Some(_)` while visible; the variant
    /// distinguishes the preset list from the custom date picker.
    pub time_picker: Option<TimePickerState>,
    /// When `Some`, an overlay shows the focused tile's raw chart
    /// JSON. Set by `:tile json` / `:tile inspect`; any key dismisses
    /// (handled in `on_key`).
    pub tile_inspect_json: Option<String>,
    /// Which mode the current buffer/file represents. `Mpl` is the
    /// long-standing default (a single MPL/MQL buffer is the source of
    /// truth); `Dashboard` means `loaded_dashboard` holds the canonical
    /// state and `:w` writes the dashboard JSON, not the buffer text.
    pub buffer_mode: BufferMode,
    /// Top-pane view: single-tile (`Solo`) or multi-tile (`Grid`).
    /// Auto-flips to `Grid` when a dashboard with ≥2 charts loads;
    /// `:solo` / `:grid` toggle manually.
    pub view_mode: ViewMode,
    /// Index into `loaded_dashboard.dashboard.charts` of the
    /// currently-selected tile in Grid mode. Wraps within bounds and
    /// resets to 0 when a new dashboard is adopted.
    pub selected_chart_idx: usize,
    /// Vertical scroll offset (in terminal rows) for the dashboard
    /// grid pane. Grid content is laid out at a minimum per-virtual-
    /// row height (see `MIN_GRID_ROW_HEIGHT` in `ui.rs`) so that
    /// large dashboards exceed the viewport and need scrolling. The
    /// renderer clamps this to `[0, max_scroll]` each frame; key
    /// handlers + auto-scroll only set a desired value.
    pub dashboard_scroll: u16,
    /// Active tile editing sub-mode. `Idle` outside of `m`/`s`/`d`/`a`.
    pub tile_submode: TileSubMode,
    /// Set whenever a tile mutation touches `loaded_dashboard`.
    /// Cleared on `DashboardSaved` and on `write_file` in dashboard
    /// mode. Surfaced as `[+]` in the status line.
    pub dashboard_dirty: bool,
    /// Per-tile query results, keyed by chart id (wire `ChartBase.id`).
    /// Populated by `run_tile_queries` after `adopt_dashboard`; read
    /// by the grid renderer to draw live data in each tile.
    pub tile_results: std::collections::BTreeMap<String, TileQueryResult>,
    /// Snapshot of the editor buffer captured the last time
    /// `adopt_dashboard` seeded it from the focused chart. Used by the
    /// background dashboard-refresh path to decide whether re-adopting
    /// the fresh resource would clobber user edits.
    pub last_adopted_seed: Option<String>,
    pub current_file: Option<std::path::PathBuf>,
    /// Snapshot of the buffer the last time it was loaded or written to disk;
    /// used to compute the dirty flag without relying on `tui-textarea` internals.
    pub saved_buffer: String,
    /// Focused tile's viz kind. In Solo / file mode this is the
    /// kind the editor's `// @viz` pragma selects; in Grid mode it
    /// tracks whichever chart the user last zoomed in on (since the
    /// editor + status bar live in solo terms). Kept in sync with
    /// the buffer's pragma by [`App::sync_dashboard_from_buffer`],
    /// which runs after every buffer-mutating or buffer-loading path
    /// via [`App::recompute_diagnostics`].
    pub viz_kind: VizKind,
    /// Focused tile's `// @viz:opts` map (e.g. `n=10` for top-list).
    /// Same lifecycle as [`Self::viz_kind`].
    pub viz_opts: std::collections::BTreeMap<String, String>,
    /// Active query time range, shared by every tile in the loaded
    /// dashboard and by the editor's `:r` runs. Seeded from the
    /// dashboard's `timeWindowStart` / `End` (or the legacy
    /// `now-1h` / `now` defaults on file-mode startup) and mutated
    /// in place by `:time` and the picker.
    pub time_range: TimeRange,
    /// Counter incremented on each query start; only matching responses are accepted.
    last_query_id: u64,
    runtime: Handle,
    events_tx: mpsc::Sender<AppEvent>,
    events_rx: mpsc::Receiver<AppEvent>,
    client: Option<AxiomClient>,
}

impl App {
    pub fn new(runtime: Handle) -> Self {
        Self::with_cache(runtime, default_cache())
    }

    pub fn with_cache(runtime: Handle, cache: Cache) -> Self {
        let (events_tx, events_rx) = mpsc::channel();
        let cached_count = cache.dataset_count();
        let saved_query = cache.load_query();
        let editor = match &saved_query {
            Some(text) => editor::editor_with_text(text),
            None => editor::new_editor(),
        };
        let cache = Arc::new(RwLock::new(cache));
        let status = match (cached_count, saved_query.is_some()) {
            (0, false) => "ready".to_string(),
            (0, true) => "restored previous query".to_string(),
            (n, false) => format!("loaded {n} dataset(s) from cache"),
            (n, true) => format!("loaded {n} dataset(s); restored previous query"),
        };
        let initial_text = saved_query
            .clone()
            .unwrap_or_else(|| editor.lines().join("\n"));
        // Seed `viz_kind` / `viz_opts` from the buffer's `// @viz`
        // pragma so the first frame renders the right chart kind
        // before any edit runs `sync_dashboard_from_buffer`.
        // Pragma errors fall through silently — they'll resurface
        // as soon as `sync_dashboard_from_buffer` runs on the
        // first edit.
        let (initial_viz_kind, initial_viz_opts) = match viz::parse_pragma(&initial_text) {
            Ok(Some(spec)) => (spec.kind, spec.opts),
            _ => (VizKind::default(), std::collections::BTreeMap::new()),
        };
        Self {
            mode: Mode::Normal,
            editor,
            cmdline: CmdLine::default(),
            cmdline_completions: CmdlineCompletionState::default(),
            system_params: params::default_system_params(),
            cli_params: std::collections::BTreeMap::new(),
            current_file: None,
            saved_buffer: initial_text.clone(),
            viz_kind: initial_viz_kind,
            viz_opts: initial_viz_opts,
            time_range: TimeRange::default(),
            dashboards: DashboardPicker::default(),
            last_picked_dashboard: None,
            loaded_dashboard: None,
            dashinfo_visible: false,
            time_picker: None,
            buffer_mode: BufferMode::Mpl,
            tile_inspect_json: None,
            view_mode: ViewMode::Solo,
            selected_chart_idx: 0,
            dashboard_scroll: 0,
            tile_submode: TileSubMode::Idle,
            dashboard_dirty: false,
            tile_results: std::collections::BTreeMap::new(),
            last_adopted_seed: None,
            last_error: None,
            series: demo_series(),
            status,
            should_quit: false,
            busy: false,
            cache,
            completions: CompletionState::default(),
            quickfix: QuickFixPicker::default(),
            diagnostics: Vec::new(),
            last_trace_id: None,
            help_visible: false,
            help_scroll: 0,
            hover: None,
            sig_help: None,
            cmd_parser: command::Parser::new(),
            yank: None,
            last_find: None,
            last_change: None,
            visual_anchor: None,
            focus: Pane::Editor,
            legend_selected: 0,
            legend_hidden: Vec::new(),
            legend_details_visible: false,
            details_cursor: 0,
            legend_label_tags: Vec::new(),
            last_query_context: None,
            params_selected: 0,
            cmdline_return_focus: None,
            pending_ctrl_w: false,
            last_query_id: 0,
            runtime,
            events_tx,
            events_rx,
            client: None,
        }
    }

    /// Current editor buffer as a single string. System-param references
    /// like `$__interval` are preserved verbatim — the Axiom MetricsDB
    /// server resolves them from the request's time window.
    pub fn query_text(&self) -> String {
        self.editor.lines().join("\n")
    }

    fn current_chart_id(&self) -> Option<String> {
        self.loaded_dashboard
            .as_ref()
            .and_then(|r| r.dashboard.charts.get(self.selected_chart_idx))
            .map(|c| c.base().id.clone())
    }

    /// Reload `legend_label_tags` from the cache for the current
    /// active context, so the picker buffer + render labels reflect
    /// the focused tile (or editor query) instead of the previous
    /// one's selection.
    ///
    /// Wiring: this is called whenever the active context changes
    /// — tile focus moves in Grid view, dashboard adoption, view
    /// mode flips, the focused tile's first data lands, and the
    /// editor finishes a query. The lookup is cheap (two HashMap
    /// hits), and the value silently becomes empty when nothing is
    /// cached for the new context, which clears any stale leftover
    /// from the previous tile.
    fn reload_legend_label_tags(&mut self) {
        let tags = if self.view_mode == ViewMode::Grid
            && let Some(resource) = self.loaded_dashboard.as_ref()
            && let Some(chart) = resource
                .dashboard
                .charts
                .get(self.selected_chart_idx)
            && let crate::dashboard::Query::Mpl(mpl) =
                crate::dashboard::extract_query(chart)
            && let Ok((ds, m)) = crate::mpl::extract_dataset_metric(&mpl)
        {
            // Tile context: ignore the editor's query-hash store
            // (the tile's hash isn't the editor's) and key purely
            // by `(dataset, metric)`. Empty hash misses the
            // by-hash store; `resolve_legend_tags` then falls
            // through to the per-metric one.
            self.cache.read().unwrap().resolve_legend_tags("", &ds, &m)
        } else if let Some(ctx) = self.last_query_context.clone() {
            self.cache
                .read()
                .unwrap()
                .resolve_legend_tags(&ctx.hash, &ctx.dataset, &ctx.metric)
        } else {
            Vec::new()
        };
        self.legend_label_tags = tags;
    }

    /// Series slice driving the legend pane right now: the focused
    /// tile's series when a dashboard is loaded in Grid view,
    /// otherwise the editor's last query result. Matches the source
    /// `chart::draw_legend` already uses for rendering so the `e`
    /// tag picker and friends reflect what the user is looking at.
    pub fn active_legend_series(&self) -> &[Series] {
        if self.view_mode == ViewMode::Grid
            && let Some(resource) = self.loaded_dashboard.as_ref()
            && let Some(chart) = resource
                .dashboard
                .charts
                .get(self.selected_chart_idx)
            && let Some(tr) = self.tile_results.get(&chart.base().id)
        {
            return &tr.series;
        }
        &self.series
    }

    /// `legend_selected` clamped into the active series slice.
    /// Returns `None` when there's nothing selectable.
    fn active_legend_index(&self) -> Option<usize> {
        let n = self.active_legend_series().len();
        if n == 0 { None } else { Some(self.legend_selected.min(n - 1)) }
    }

    /// Snapshot the selected tile's layout entry, synthesising a
    /// default one if missing so sub-modes always have something to
    /// revert to.
    fn snapshot_selected_layout(&mut self) -> Option<crate::axiom::LayoutItem> {
        let id = self.current_chart_id()?;
        let resource = self.loaded_dashboard.as_mut()?;
        if let Some(li) = resource.dashboard.layout.iter().find(|l| l.i == id) {
            return Some(li.clone());
        }
        // Synthesize and append so subsequent edits have something to
        // mutate.
        let li = crate::axiom::LayoutItem {
            i: id,
            x: 0,
            y: Some(0),
            w: 6,
            h: 6,
            extras: Default::default(),
        };
        resource.dashboard.layout.push(li.clone());
        Some(li)
    }

    fn revert_layout(&mut self, original: crate::axiom::LayoutItem) {
        if let Some(resource) = self.loaded_dashboard.as_mut()
            && let Some(li) = resource
                .dashboard
                .layout
                .iter_mut()
                .find(|l| l.i == original.i)
        {
            *li = original;
        }
        self.tile_submode = TileSubMode::Idle;
        self.status = "reverted".to_string();
    }

    /// Recompute the params pane's row list for the current buffer +
    /// `cli_params`. Cheap; mirrors the diagnostics-on-every-keystroke
    /// pattern.
    pub fn param_rows(&self) -> Vec<crate::params::ParamRow> {
        crate::mpl::param_rows(&self.query_text(), &self.system_params, &self.cli_params)
    }

    /// Write the current `legend_label_tags` to the cache and flush
    /// to disk. Two keying modes:
    ///
    ///   * **Grid view, dashboard tile focused** — key by the tile's
    ///     `(dataset, metric)` extracted from its MPL. The tile's
    ///     query hash isn't the editor's, so we deliberately skip
    ///     the by-hash store and rely on the per-metric one.
    ///   * **Solo / editor view** — key by `last_query_context`'s
    ///     hash + `(dataset, metric)`, same as before.
    ///
    /// Silent no-op when neither path yields a key.
    fn persist_legend_label_tags(&self) {
        if self.view_mode == ViewMode::Grid
            && let Some(resource) = self.loaded_dashboard.as_ref()
            && let Some(chart) = resource
                .dashboard
                .charts
                .get(self.selected_chart_idx)
            && let crate::dashboard::Query::Mpl(mpl) =
                crate::dashboard::extract_query(chart)
            && let Ok((ds, m)) = crate::mpl::extract_dataset_metric(&mpl)
        {
            let mut cache = self.cache.write().unwrap();
            cache.set_legend_tags_for_metric(&ds, &m, self.legend_label_tags.clone());
            if let Err(e) = cache.save() {
                eprintln!("metrics-tui: cache save failed: {e}");
            }
            return;
        }
        let Some(ctx) = &self.last_query_context else {
            return;
        };
        let mut cache = self.cache.write().unwrap();
        cache.set_legend_tags(
            &ctx.hash,
            &ctx.dataset,
            &ctx.metric,
            self.legend_label_tags.clone(),
        );
        if let Err(e) = cache.save() {
            eprintln!("metrics-tui: cache save failed: {e}");
        }
    }

    /// Show the help modal, resetting the scroll offset so the next
    /// open lands at the top instead of wherever the user left it.
    /// Single entry point so the reset can't be forgotten by ad-hoc
    /// callers.
    fn open_help(&mut self) {
        self.help_visible = true;
        self.help_scroll = 0;
    }

    /// Drive the cmdline completion popup on Tab / Shift-Tab. First
    /// Tab from a hidden state: compute candidates, splice in the
    /// longest common prefix, and — if there's still more than one
    /// candidate — show the popup with the first item selected.
    /// Subsequent Tabs cycle (Shift-Tab cycles backward) and splice
    /// the highlighted candidate over the current token in real time.
    pub fn handle_cmdline_tab(&mut self, backward: bool) {
        if !self.cmdline_completions.visible {
            // Fresh Tab: recompute the candidate set against the
            // current buffer + cursor.
            let ctx = crate::cmdline_complete::Context {
                dashboards: &self.dashboards.items,
            };
            let req = match crate::cmdline_complete::completions_for(
                &self.cmdline.buf,
                self.cmdline.cursor,
                &ctx,
            ) {
                Some(r) if !r.items.is_empty() => r,
                _ => return,
            };
            // Splice the longest common prefix immediately so single-
            // candidate paths are zero-friction.
            let prefix = req.common_prefix();
            self.splice_cmdline_token(req.range, &prefix);
            if req.items.len() == 1 {
                // Exact match: also append a trailing space so the
                // user can type the next arg without an extra
                // keystroke.
                self.cmdline.buf.push(' ');
                self.cmdline.cursor = self.cmdline.buf.chars().count();
                return;
            }
            // Multiple candidates: show the popup. Recompute the
            // splice range against the just-updated buffer so future
            // accepts overwrite the token we just typed in.
            let new_token_start = req.range.0;
            let new_token_end = new_token_start + prefix.len();
            self.cmdline_completions.items = req.items;
            self.cmdline_completions.selected = 0;
            self.cmdline_completions.replace_range = (new_token_start, new_token_end);
            self.cmdline_completions.visible = true;
            return;
        }
        // Popup already visible: cycle.
        let delta = if backward { -1 } else { 1 };
        self.move_cmdline_completion(delta);
    }

    fn move_cmdline_completion(&mut self, delta: isize) {
        let n = self.cmdline_completions.items.len();
        if n == 0 {
            return;
        }
        let i = self.cmdline_completions.selected as isize + delta;
        let wrapped = ((i % n as isize) + n as isize) % n as isize;
        self.cmdline_completions.selected = wrapped as usize;
        // Splice the new selection into the buffer so the user sees
        // each candidate as they cycle (vim wildmenu style).
        let item = self.cmdline_completions.items[self.cmdline_completions.selected].clone();
        let range = self.cmdline_completions.replace_range;
        self.splice_cmdline_token(range, &item);
        // Re-anchor the range so the next cycle replaces the just-
        // spliced text instead of an older slice.
        self.cmdline_completions.replace_range = (range.0, range.0 + item.len());
    }

    fn accept_cmdline_completion(&mut self) {
        // The current selection is already in the buffer (from the
        // last cycle); just hide the popup. Append a trailing space
        // to match the single-candidate path's affordance.
        if !self.cmdline.buf.ends_with(' ') {
            self.cmdline.buf.push(' ');
            self.cmdline.cursor = self.cmdline.buf.chars().count();
        }
        self.cmdline_completions.hide();
    }

    /// Replace `buf[range.0..range.1]` with `text` and reposition the
    /// char cursor at the end of the inserted text.
    fn splice_cmdline_token(&mut self, range: (usize, usize), text: &str) {
        let (start, end) = range;
        if start > self.cmdline.buf.len() || end > self.cmdline.buf.len() {
            return;
        }
        self.cmdline.buf.replace_range(start..end, text);
        let new_byte = start + text.len();
        // Convert byte position back to char count for `CmdLine.cursor`.
        self.cmdline.cursor = self.cmdline.buf[..new_byte].chars().count();
    }

    /// Active query time range, in the order the Axiom API wants it
    /// (`start`, `end`). Sourced from `self.time_range`, which is
    /// seeded from the loaded dashboard's `timeWindowStart`/`End`
    /// (or the legacy `now-1h`/`now` defaults) and mutated in place
    /// by `:time`. Both editor (`run_query`) and per-tile fetches
    /// (`run_tile_queries`, `run_focused_tile_query`) read this so
    /// the whole dashboard shares one consistent window.
    ///
    /// The returned strings go through [`normalize_time_expr`] so the
    /// `qr-` prefix Axiom's web UI stores in dashboards (e.g.
    /// `qr-now-7d`) is stripped before hitting the `_mpl` endpoint
    /// — that endpoint only understands the bare relative form
    /// (`now-7d`) and 400s otherwise.
    pub fn active_time_range(&self) -> (String, String) {
        (
            normalize_time_expr(&self.time_range.start),
            normalize_time_expr(&self.time_range.end),
        )
    }

    /// Common path for every time-range mutation: write the in-memory
    /// model, mirror onto the wire copy so `:dash save` persists, mark
    /// the dashboard dirty, status-line the change, and kick a refetch
    /// so the user sees the new window immediately.
    fn set_time_range(&mut self, start: String, end: String) {
        self.time_range = TimeRange {
            start: start.clone(),
            end: end.clone(),
        };
        if let Some(resource) = self.loaded_dashboard.as_mut() {
            resource.dashboard.time_window_start = Some(start.clone());
            resource.dashboard.time_window_end = Some(end.clone());
            self.dashboard_dirty = true;
        }
        self.status = format!("time: {start} → {end}");
        // Refetch so the dashboard reflects the new window without the
        // user having to remember `:r` (Solo) or `Ctrl-R` (Grid).
        if self.view_mode == ViewMode::Grid && self.loaded_dashboard.is_some() {
            self.run_tile_queries();
        } else if !self.query_text().trim().is_empty() {
            self.run_query();
        }
    }

    /// Serialise the current dashboard to pretty JSON. Errors when
    /// no dashboard is loaded. Pure helper exposed for tests of the
    /// round-trip; production code goes through `write_file`.
    #[cfg(test)]
    fn dashboard_to_json(&self) -> anyhow::Result<String> {
        use anyhow::anyhow;
        let resource = self
            .loaded_dashboard
            .as_ref()
            .ok_or_else(|| anyhow!("no dashboard loaded"))?;
        serde_json::to_string_pretty(resource).map_err(Into::into)
    }

    /// Switch into Grid view mode when the loaded dashboard has ≥2
    /// charts; otherwise stay in Solo. Called from `adopt_dashboard`
    /// and `open_file` so the user never has to manually flip into
    /// grid view to see a multi-tile dashboard.
    fn auto_switch_view_mode(&mut self) {
        let n = self
            .loaded_dashboard
            .as_ref()
            .map(|r| r.dashboard.charts.len())
            .unwrap_or(0);
        if n >= 2 {
            self.view_mode = ViewMode::Grid;
            self.focus = Pane::Dashboard;
        } else {
            self.view_mode = ViewMode::Solo;
        }
        self.selected_chart_idx = 0;
    }

    /// Build a pretty-printed JSON dump of the focused tile's `Chart`,
    /// or `None` if no dashboard / tile is selected. Used by
    /// `:tile json` to show the raw wire payload so we can debug
    /// query-classification questions.
    pub fn focused_chart_json(&self) -> Option<String> {
        let resource = self.loaded_dashboard.as_ref()?;
        let chart = resource.dashboard.charts.get(self.selected_chart_idx)?;
        serde_json::to_string_pretty(chart).ok()
    }

    /// Move the dashboard-pane selection by `delta`. Wraps within the
    /// chart list. No-op outside Grid mode.
    pub fn move_dashboard_selection(&mut self, delta: isize) {
        if self.view_mode != ViewMode::Grid {
            return;
        }
        let n = self
            .loaded_dashboard
            .as_ref()
            .map(|r| r.dashboard.charts.len())
            .unwrap_or(0);
        if n == 0 {
            return;
        }
        let i = self.selected_chart_idx as isize + delta;
        let wrapped = ((i % n as isize) + n as isize) % n as isize;
        self.selected_chart_idx = wrapped as usize;
        self.reload_legend_label_tags();
    }

    /// Spatial navigation in the dashboard grid: pick the chart whose
    /// `LayoutItem` centroid is nearest in the given direction.
    /// Falls back to row-major sequence cycling when no chart in the
    /// direction is closer than the current one (e.g. user is already
    /// on the edge).
    pub fn move_dashboard_selection_spatial(&mut self, dir: SpatialDir) {
        if self.view_mode != ViewMode::Grid {
            return;
        }
        let Some(resource) = self.loaded_dashboard.as_ref() else {
            return;
        };
        let charts = &resource.dashboard.charts;
        if charts.is_empty() {
            return;
        }
        if let Some(next) = pick_next_chart_in_direction(
            &resource.dashboard.layout,
            charts,
            self.selected_chart_idx,
            dir,
        ) {
            self.selected_chart_idx = next;
            self.reload_legend_label_tags();
            return;
        }
        // No spatial match — fall back to row-major cycle.
        // `move_dashboard_selection` already reloads tags.
        let delta = match dir {
            SpatialDir::Right | SpatialDir::Down => 1,
            SpatialDir::Left | SpatialDir::Up => -1,
        };
        self.move_dashboard_selection(delta);
    }

    /// Zoom the highlighted grid tile back into the single-tile
    /// renderer by re-seeding the editor buffer with that chart's
    /// MPL/APL. Drops view mode back to Solo + focuses the editor.
    pub fn zoom_selected_chart(&mut self) {
        use crate::dashboard::Query;
        let Some(resource) = self.loaded_dashboard.as_ref() else {
            return;
        };
        let Some(chart) = resource
            .dashboard
            .charts
            .get(self.selected_chart_idx)
            .cloned()
        else {
            return;
        };
        let kind = VizKind::from_chart(&chart);
        let query = crate::dashboard::extract_query(&chart);
        // The focused tile is whichever chart the user just zoomed
        // in on; reset opts (the wire chart has none) so the buffer
        // pragma is the only source of viz options.
        self.viz_kind = kind;
        self.viz_opts.clear();
        let pragma_line = format!("// @viz {}\n", kind.as_str());
        match &query {
            Query::Mpl(mpl) => {
                let text = format!("{pragma_line}{mpl}");
                self.editor = editor::editor_with_text(&text);
                self.recompute_diagnostics();
                // Pin the editor-side query context to the tile's
                // (dataset, metric) so the upcoming legend-tag
                // reload finds the right per-metric cache slot
                // (and any toggle persists under the tile's keys).
                // We don't know the AST hash without running the
                // pipeline; pass empty so `resolve_legend_tags`
                // falls through to the by-metric store.
                if let Ok((ds, m)) = crate::mpl::extract_dataset_metric(mpl) {
                    self.last_query_context = Some(QueryContext {
                        hash: String::new(),
                        dataset: ds,
                        metric: m,
                    });
                }
            }
            Query::Apl(apl) => {
                let text = format!(
                    "{pragma_line}// APL query — execution lands in step 14b\n// {apl}\n",
                    apl = apl.replace('\n', "\n// ")
                );
                self.editor = editor::editor_with_text(&text);
                self.recompute_diagnostics();
            }
            Query::Empty => {}
        }
        // Adopt the tile's last-known series into the Solo-view
        // `app.series` so the chart pane shows the real data
        // immediately instead of the sin(x) demo placeholder. The
        // tile data is already in `tile_results` from the dashboard
        // background fetch — we just promote it. A subsequent `:r`
        // (or the editor's run-on-Enter) will refresh it if the
        // user wants a fresh point-in-time.
        let chart_id = chart.base().id.clone();
        if let Some(tile) = self.tile_results.get(&chart_id) {
            self.series = tile.series.clone();
            self.legend_hidden = vec![false; self.series.len()];
            if self.legend_selected >= self.series.len() {
                self.legend_selected = 0;
            }
            if let Some(tid) = tile.trace_id.clone() {
                self.last_trace_id = Some(tid);
            }
        } else {
            // No tile data yet (zoom raced the fetch, or the tile
            // has no MPL). Clear so the user doesn't see stale
            // demo data labelled with a different tile's title.
            self.series.clear();
            self.legend_hidden.clear();
            self.legend_selected = 0;
        }
        self.view_mode = ViewMode::Solo;
        self.focus = Pane::Editor;
        // Now that `last_query_context` is pinned to the tile and
        // view mode is Solo, pick up that metric's saved tag
        // selection (or clear if there's nothing cached).
        self.reload_legend_label_tags();
        let title = chart
            .base()
            .name
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| kind.as_str().to_string());
        self.status = format!("zoomed `{title}`");
    }

    fn adopt_dashboard(&mut self, uid: String, resource: crate::axiom::DashboardSummary) {
        use crate::dashboard::Query;
        let name = resource.name().to_string();
        let chart_count = resource.dashboard.charts.len();
        self.time_range = TimeRange::from_resource(&resource);
        // Focus snaps to the first chart — matches the grid's
        // initial selection and the prior `Dashboard::tiles[0]`
        // semantics. Empty dashboards fall through to defaults.
        let first_chart = resource.dashboard.charts.first().cloned();
        let (focused_kind, focused_query) = match first_chart.as_ref() {
            Some(c) => (VizKind::from_chart(c), crate::dashboard::extract_query(c)),
            None => (VizKind::default(), Query::Empty),
        };
        self.viz_kind = focused_kind;
        self.viz_opts.clear();
        self.last_picked_dashboard = Some(uid);
        self.loaded_dashboard = Some(resource);

        let pragma_line = format!("// @viz {}\n", focused_kind.as_str());
        let mut seeded: Option<String> = None;
        match &focused_query {
            Query::Mpl(mpl) => {
                let text = format!("{pragma_line}{mpl}");
                self.editor = editor::editor_with_text(&text);
                self.recompute_diagnostics();
                seeded = Some(text);
            }
            Query::Apl(apl) => {
                let text = format!(
                    "{pragma_line}// APL query — execution lands in step 14b\n// {apl}\n",
                    apl = apl.replace('\n', "\n// ")
                );
                self.editor = editor::editor_with_text(&text);
                self.recompute_diagnostics();
                seeded = Some(text);
            }
            Query::Empty => {
                // Leave the editor alone; tile renderer surfaces the
                // note body / placeholder directly.
            }
        }
        // Capture the seed *after* `recompute_diagnostics` so it
        // matches what `query_text()` will return for an untouched
        // buffer (line endings normalised by the editor).
        self.last_adopted_seed = seeded.map(|_| self.query_text());
        self.auto_switch_view_mode();
        // Adopted; pick up the initially focused tile's saved tags
        // (if any) so the legend renders the right labels from frame
        // zero, before any tile data lands.
        self.reload_legend_label_tags();
        // Kick off per-tile fetches so the grid renders live data.
        // Solo mode also benefits when the focused chart turns out to
        // have an MPL query — the existing single-tile flow runs on
        // `:r`, so this just primes things.
        self.run_tile_queries();
        self.status = format!("loaded `{name}` — {chart_count} chart(s); :dashinfo for details");
    }

    fn do_open(&mut self, path: std::path::PathBuf, force: bool) {
        if !force && self.is_dirty() {
            self.set_error("E37: No write since last change (add ! to override)".to_string());
            return;
        }
        match self.open_file(path) {
            Ok(p) => self.status = format!("opened {}", display_path(&p)),
            Err(e) => self.set_error(format!("open failed: {e}")),
        }
    }

    /// Read `path` into the App. The behaviour branches on the file's
    /// content:
    ///
    /// * If the path ends in `.axiom.json` *or* the JSON has a
    ///   top-level `dashboard` object key, it's treated as a saved
    ///   `DashboardResource` envelope: parse it, adopt as the loaded
    ///   dashboard, switch `buffer_mode` to `Dashboard`.
    /// * Otherwise it's a plain MPL buffer (existing behaviour);
    ///   buffer_mode stays `Mpl`.
    ///
    /// `current_file` is updated either way so `:w` writes to the same
    /// place.
    pub fn open_file(&mut self, path: std::path::PathBuf) -> anyhow::Result<std::path::PathBuf> {
        use anyhow::Context;
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", display_path(&path)))?;
        if Self::looks_like_dashboard_file(&path, &text) {
            // Dashboard JSON: parse + adopt.
            let resource: crate::axiom::DashboardSummary = serde_json::from_str(&text)
                .with_context(|| format!("parsing dashboard JSON {}", display_path(&path)))?;
            let uid = resource.uid.clone();
            self.adopt_dashboard(uid, resource);
            self.buffer_mode = BufferMode::Dashboard;
            self.current_file = Some(path.clone());
            self.saved_buffer = text;
            self.last_error = None;
            return Ok(path);
        }
        self.buffer_mode = BufferMode::Mpl;
        self.editor = editor::editor_with_text(&text);
        self.saved_buffer = text;
        self.current_file = Some(path.clone());
        self.last_error = None;
        self.recompute_diagnostics();
        Ok(path)
    }

    /// Sniff whether `path` + `body` smell like a saved Axiom
    /// dashboard. Extension is the fast path; the magic-key probe is
    /// the safety net for files with non-canonical extensions.
    fn looks_like_dashboard_file(path: &std::path::Path, body: &str) -> bool {
        if let Some(ext) = path.file_name().and_then(|n| n.to_str())
            && (ext.ends_with(".axiom.json") || ext.ends_with(".dashboard.json"))
        {
            return true;
        }
        // Magic-key sniff: a `DashboardResource` envelope always has a
        // nested `"dashboard"` object. Bound the probe to the first 1k
        // bytes so we don't scan megabytes of unrelated JSON.
        let head = &body[..body.len().min(1024)];
        head.contains("\"dashboard\"") && head.contains("\"uid\"")
    }

    /// Write the current artifact to `path` (or `current_file` if
    /// `None`). Routes on `buffer_mode`:
    ///
    /// * `Mpl` — writes the editor buffer (long-standing behaviour).
    /// * `Dashboard` — serialises `loaded_dashboard` to pretty JSON
    ///   and writes that. The buffer is **not** synced back into the
    ///   focused chart (that's a 17d/17e concern); the user explicitly
    ///   edits a dashboard's structure through `:dash`-prefixed
    ///   commands.
    ///
    /// Writes go through a `<path>.tmp` → rename dance so a crash
    /// mid-write doesn't truncate the previous good copy.
    pub fn write_file(
        &mut self,
        path: Option<std::path::PathBuf>,
    ) -> anyhow::Result<std::path::PathBuf> {
        use anyhow::{Context, anyhow};
        let target = match path {
            Some(p) => p,
            None => self
                .current_file
                .clone()
                .ok_or_else(|| anyhow!("E32: No file name"))?,
        };
        if let Some(parent) = target.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", display_path(parent)))?;
        }
        let text = match self.buffer_mode {
            BufferMode::Mpl => self.query_text(),
            BufferMode::Dashboard => {
                let resource = self
                    .loaded_dashboard
                    .as_ref()
                    .ok_or_else(|| anyhow!("no dashboard loaded"))?;
                serde_json::to_string_pretty(resource).context("serialising dashboard JSON")?
            }
        };
        // Atomic-ish write: temp file in same dir + rename.
        let mut tmp = target.clone();
        let mut filename = target
            .file_name()
            .ok_or_else(|| anyhow!("target has no file name"))?
            .to_os_string();
        filename.push(".tmp");
        tmp.set_file_name(filename);
        std::fs::write(&tmp, &text).with_context(|| format!("writing {}", display_path(&tmp)))?;
        std::fs::rename(&tmp, &target).with_context(|| {
            format!(
                "renaming {} → {}",
                display_path(&tmp),
                display_path(&target)
            )
        })?;
        self.saved_buffer = text;
        self.current_file = Some(target.clone());
        if self.buffer_mode == BufferMode::Dashboard {
            self.dashboard_dirty = false;
        }
        Ok(target)
    }

    /// `true` when the editor buffer has unsaved changes compared to the last
    /// load or write.
    pub fn is_dirty(&self) -> bool {
        self.query_text() != self.saved_buffer
    }

    /// Set both the status line summary and the dismissable error overlay.
    /// Keeps the status line in sync so the bar reads the same as the overlay
    /// header. Truncates very long errors for the status line only.
    pub fn set_error(&mut self, msg: String) {
        let summary: String = msg
            .lines()
            .next()
            .unwrap_or(&msg)
            .chars()
            .take(140)
            .collect();
        self.status = summary;
        self.last_error = Some(msg);
    }

    /// Dismiss the error overlay. Returns `true` when an overlay was visible.
    pub fn dismiss_error(&mut self) -> bool {
        self.last_error.take().is_some()
    }

    /// Write the current editor contents to the on-disk session cache.
    /// Skipped when a `current_file` is open — the user owns that file via
    /// `:w`, and we shouldn't double-shadow it. Failures are logged to stderr
    /// (visible after the alt-screen tears down) but never surfaced as a
    /// user-facing error — persistence is best-effort.
    pub fn persist_query(&self) {
        if self.current_file.is_some() {
            return;
        }
        let text = self.query_text();
        if let Err(e) = self.cache.read().unwrap().save_query(&text) {
            eprintln!("metrics-tui: query cache save failed: {e}");
        }
    }

    fn open_completions(&mut self) {
        let Some(payload) = self.compute_completion_payload() else {
            self.completions.hide();
            self.status = "no completions".to_string();
            return;
        };
        if payload.items.is_empty() {
            self.completions.hide();
            self.maybe_kick_off_discovery(&payload.kind);
            return;
        }
        self.completions = state_from(payload, 0);
    }

    fn refresh_completions(&mut self) {
        let previous_selected = self.completions.selected;
        let Some(payload) = self.compute_completion_payload() else {
            self.completions.hide();
            return;
        };
        if payload.items.is_empty() {
            self.completions.hide();
            return;
        }
        let selected = previous_selected.min(payload.items.len() - 1);
        self.completions = state_from(payload, selected);
    }

    fn compute_completion_payload(&self) -> Option<completions::CompletionPayload> {
        let query = self.query_text();
        let cursor_byte = editor_cursor_byte_offset(&self.editor);
        completions::compute(
            &query,
            cursor_byte,
            &self.system_params,
            &self.cache.read().unwrap(),
        )
    }

    /// When a cache-backed context has nothing to offer, transparently kick off the
    /// fetch the user would otherwise have to invoke manually (`D` / `M`).
    fn maybe_kick_off_discovery(&mut self, kind: &completions::CompletionKind) {
        if self.busy {
            self.status = "no completions".to_string();
            return;
        }
        match kind {
            completions::CompletionKind::Dataset
                if self.cache.read().unwrap().dataset_count() == 0 =>
            {
                self.status = "no datasets cached — fetching…".to_string();
                self.fetch_datasets();
            }
            completions::CompletionKind::Metric { dataset }
                if !dataset.is_empty()
                    && self.cache.read().unwrap().metric_names(dataset).is_empty() =>
            {
                self.status = format!("no metrics cached for `{dataset}` — fetching…");
                self.fetch_metrics_for_current_query();
            }
            _ => {
                self.status = "no completions".to_string();
            }
        }
    }

    /// Kick off background discovery once at startup if the cache is empty so the
    /// first completion attempt has something to show, and run the persisted
    /// query (if any) so the chart pane is populated on launch.
    pub fn bootstrap(&mut self) {
        self.bootstrap_inner(true);
    }

    /// Same as [`bootstrap`] but suppresses the auto-run of the
    /// restored saved query. Used when `-d <uid>` is going to seed
    /// the editor from a dashboard — running the stale saved query
    /// first would just push wrong results into `self.series`.
    pub fn bootstrap_skip_initial_query(&mut self) {
        self.bootstrap_inner(false);
    }

    fn bootstrap_inner(&mut self, run_initial_query: bool) {
        if !self.cli_params.is_empty() {
            let n = self.cli_params.len();
            let plural = if n == 1 { "param" } else { "params" };
            self.status = format!("{}; {n} CLI {plural}", self.status);
        }
        if self.cache.read().unwrap().dataset_count() == 0 {
            self.fetch_datasets();
        }
        self.recompute_diagnostics();
        if run_initial_query && !self.query_text().trim().is_empty() {
            self.run_query();
        }
    }

    /// Re-run the MPL engine over the current buffer and update
    /// `self.diagnostics`. Cheap enough (~ms range on our queries) to run on
    /// every buffer-mutating keystroke; debounce if it ever shows up in a
    /// profile.
    pub fn recompute_diagnostics(&mut self) {
        let text = self.query_text();
        self.diagnostics = mpl::analyze(&text, &self.system_params);
        self.sync_dashboard_from_buffer(&text);
        self.recompute_sig_help();
    }

    /// Reconcile the focused tile's `kind`, `opts`, and MPL query text
    /// with whatever's in the editor buffer. Called by
    /// [`recompute_diagnostics`] on every buffer change, so the dashboard
    /// model is always in sync without scheduling extra passes.
    ///
    /// Pragma parse errors are pushed onto `self.diagnostics` so they
    /// surface alongside MPL diagnostics in the status bar and pane chrome.
    /// On error we keep the previous kind/opts so the chart doesn't
    /// flicker between renders while the user is mid-edit.
    fn sync_dashboard_from_buffer(&mut self, text: &str) {
        match viz::parse_pragma(text) {
            Ok(Some(spec)) => {
                self.viz_kind = spec.kind;
                self.viz_opts = spec.opts;
            }
            Ok(None) => {
                self.viz_kind = VizKind::default();
                self.viz_opts.clear();
            }
            Err((line_idx, err)) => {
                self.diagnostics
                    .push(pragma_diagnostic(text, line_idx, &err));
            }
        }
    }

    /// Refresh the status-line signature help from the current cursor.
    /// Cheap (single backwards byte scan + one stdlib lookup); fine to call
    /// on every keystroke and cursor move.
    pub fn recompute_sig_help(&mut self) {
        let text = self.query_text();
        let cursor = editor_cursor_byte_offset(&self.editor);
        self.sig_help = hover::find_call_context(&text, cursor);
    }

    /// Open the quick-fix picker for whichever diagnostic the editor cursor
    /// is sitting in. Falls back to the first diagnostic with any actions
    /// when the cursor isn't on one. No-op when nothing is fixable.
    fn open_quickfix(&mut self) {
        let cursor_byte = editor_cursor_byte_offset(&self.editor);
        let target = self
            .diagnostics
            .iter()
            .find(|d| d.span_contains(cursor_byte) && !d.actions.is_empty())
            .or_else(|| self.diagnostics.iter().find(|d| !d.actions.is_empty()));
        let Some(diag) = target else {
            self.status = "no quick fix available".to_string();
            return;
        };
        self.quickfix = QuickFixPicker {
            visible: true,
            actions: diag.actions.clone(),
            selected: 0,
            title: diag.message.clone(),
        };
    }

    fn move_quickfix_selection(&mut self, delta: isize) {
        if self.quickfix.actions.is_empty() {
            return;
        }
        let len = self.quickfix.actions.len();
        let i = self.quickfix.selected as isize + delta;
        let wrapped = ((i % len as isize) + len as isize) % len as isize;
        self.quickfix.selected = wrapped as usize;
    }

    fn accept_quickfix(&mut self) {
        if !self.quickfix.visible {
            return;
        }
        let Some(action) = self.quickfix.actions.get(self.quickfix.selected).cloned() else {
            self.quickfix.hide();
            return;
        };
        let query = self.query_text();
        let start_byte = action.byte_offset;
        let end_byte = action.byte_offset + action.byte_length;
        let (row, start_char) = byte_offset_to_row_col(&query, start_byte);
        let (_, end_char) = byte_offset_to_row_col(&query, end_byte);
        let replace_chars = end_char.saturating_sub(start_char);

        self.editor
            .move_cursor(CursorMove::Jump(row as u16, start_char as u16));
        self.editor.delete_str(replace_chars);
        self.editor.insert_str(&action.insert);
        self.status = format!("applied: {}", action.name);
        self.quickfix.hide();
        self.recompute_diagnostics();
    }

    fn move_completion_selection(&mut self, delta: isize) {
        if self.completions.items.is_empty() {
            return;
        }
        let len = self.completions.items.len();
        let i = self.completions.selected as isize + delta;
        let wrapped = ((i % len as isize) + len as isize) % len as isize;
        self.completions.selected = wrapped as usize;
    }

    fn accept_completion(&mut self) {
        if !self.completions.visible {
            return;
        }
        let item = match self.completions.items.get(self.completions.selected) {
            Some(it) => it.clone(),
            None => {
                self.completions.hide();
                return;
            }
        };
        let Some(kind) = self.completions.kind.clone() else {
            self.completions.hide();
            return;
        };
        let (start_byte, end_byte) = self.completions.replace_range_bytes;
        let query = self.query_text();
        let (row, start_char) = byte_offset_to_row_col(&query, start_byte);
        let (_, end_char) = byte_offset_to_row_col(&query, end_byte);
        let replace_chars = end_char.saturating_sub(start_char);

        self.editor
            .move_cursor(CursorMove::Jump(row as u16, start_char as u16));
        self.editor.delete_str(replace_chars);
        self.editor.insert_str(&item.apply);
        self.completions.hide();
        self.recompute_diagnostics();

        // When the user just picked a metric, kick off a background tag fetch
        // for the `(dataset, metric)` pair so the next `where`-position
        // completion can offer tag names. Cached pairs are skipped inside
        // `fetch_tags`.
        if let completions::CompletionKind::Metric { dataset } = &kind
            && !dataset.is_empty()
        {
            self.fetch_tags(dataset.clone(), item.label.clone());
        }

        // When the user just picked a tag name, prefetch its values so the
        // value popup has data the moment they type the comparison operator.
        if let completions::CompletionKind::Tag { dataset, metric } = &kind
            && !dataset.is_empty()
            && !metric.is_empty()
        {
            self.fetch_tag_values(dataset.clone(), metric.clone(), item.label.clone());
        }
    }
}

#[cfg(test)]
mod tests;
