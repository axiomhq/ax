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

mod completions_impl;
mod dashboard;
mod editing;
mod file_io;
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

    /// Refresh the status-line signature help from the current cursor.
    /// Cheap (single backwards byte scan + one stdlib lookup); fine to call
    /// on every keystroke and cursor move.
    pub fn recompute_sig_help(&mut self) {
        let text = self.query_text();
        let cursor = editor_cursor_byte_offset(&self.editor);
        self.sig_help = hover::find_call_context(&text, cursor);
    }
}

#[cfg(test)]
mod tests;
