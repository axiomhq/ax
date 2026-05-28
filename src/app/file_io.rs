//! Buffer ↔ disk: `:e`, `:w`, dashboard-vs-MPL file detection.

use super::*;

impl App {
    pub(super) fn do_open(&mut self, path: std::path::PathBuf, force: bool) {
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
            // A freshly-loaded dashboard is clean; clear any dirty
            // flag left over from a previous dashboard session so
            // `is_dirty()` doesn't report stale unsaved state.
            self.dashboard_dirty = false;
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
        // Char-safe truncation: `body.get(..1024)` returns `None`
        // (→ fall back to the whole body) instead of panicking when
        // byte 1024 lands inside a multi-byte UTF-8 character.
        let head = body.get(..1024).unwrap_or(body);
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
    /// Writes go through a uniquely-named temp file in the target's
    /// directory followed by an atomic rename (see
    /// [`crate::util::atomic::atomic_write_text`]) so a crash
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
        // Atomic write via the shared helper (temp file in the target's
        // directory, fsync, rename — atomic on POSIX; survives
        // concurrent writers and partial-write crashes). It also
        // creates parent dirs as needed.
        crate::util::atomic::atomic_write_text(&target, &text)
            .with_context(|| format!("writing {}", display_path(&target)))?;
        self.saved_buffer = text;
        self.current_file = Some(target.clone());
        if self.buffer_mode == BufferMode::Dashboard {
            self.dashboard_dirty = false;
        }
        Ok(target)
    }
}
