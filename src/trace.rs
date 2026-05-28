//! In-memory shape of a single OTel-style distributed trace.
//!
//! Owns the *data* side of the trace view (the renderer lands in
//! step 23). One trace can be ~1500 spans and ~5 MiB of raw JSON in
//! the wire response, so we eagerly decode the column-major APL
//! response into per-span structs once at fetch time and let the
//! renderer iterate over a pre-flattened DFS `Vec<TreeRow>` from
//! then on.
//!
//! ### Why eager decode
//!
//! Lazy access would force the renderer to (a) carry the raw
//! `serde_json::Value` around, (b) re-look up columns on every
//! frame, (c) re-parse the duration string on every frame. The
//! eager-decode pay-off — measured against the 40 sample traces
//! under `traces-research/traces/` — is < 200 KiB of typed structs
//! vs. ~4.6 MiB of raw JSON for a 1500-span trace, in exchange for
//! O(1) per-row access. Easy trade.
//!
//! ### Why a separate `tree: Vec<TreeRow>`
//!
//! The "natural" representation of a trace is a tree
//! (`children: Vec<Span>` per node), but the renderer wants a flat
//! window with random access by row index — for `j`/`k` navigation,
//! virtualization, and fold-aware scrolling. We DFS-flatten once at
//! decode time so the renderer never has to walk a tree to draw a
//! viewport.

use std::collections::{BTreeMap, HashSet};

use serde::Serialize;
use serde_json::Value as Json;

/// Pre-parsed OTel span kind. Carrying this as an enum (rather than
/// re-parsing the string on every render frame) means the renderer
/// can `match` on it for one-character icons (`S` / `C` / `I` …)
/// without a lookup table.
///
/// The wire representation is the lowercase OTLP form
/// (`"server"` / `"client"` / `"internal"` / `"producer"` /
/// `"consumer"`). Empty / missing / unrecognised values collapse to
/// [`SpanKind::Unknown`] so a server-side schema bump can't crash
/// the decode pipeline.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SpanKind {
    Server,
    Client,
    Internal,
    Producer,
    Consumer,
    Unknown,
}

impl SpanKind {
    /// Parse the OTLP `kind` column value. Case-insensitive on the
    /// canonical names; anything else \u2014 including empty string,
    /// `"SPAN_KIND_UNSPECIFIED"`, or future kinds we don't know
    /// about \u2014 falls through to [`SpanKind::Unknown`].
    pub fn from_str(s: &str) -> Self {
        // Match on the lowercased form so producers that emit
        // `"SERVER"` (some Java SDKs do) decode the same way as
        // `"server"`.
        match s.trim().to_ascii_lowercase().as_str() {
            "server" => Self::Server,
            "client" => Self::Client,
            "internal" => Self::Internal,
            "producer" => Self::Producer,
            "consumer" => Self::Consumer,
            _ => Self::Unknown,
        }
    }

    /// Canonical lower-snake string form. Used for serialising back
    /// to logs / inspect overlays and for round-trip tests.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::Client => "client",
            Self::Internal => "internal",
            Self::Producer => "producer",
            Self::Consumer => "consumer",
            Self::Unknown => "unknown",
        }
    }
}

/// One OTel span event \u2014 the `events[]` column in the APL
/// response. Times are unix nanoseconds, matching `start_ns` /
/// `end_ns` so the renderer can place events on the waterfall
/// without unit conversion.
#[derive(Debug, Clone, PartialEq)]
pub struct SpanEvent {
    pub time_ns: i64,
    pub name: String,
    /// Free-form event attributes. `BTreeMap` for deterministic
    /// rendering order in the detail pane.
    pub attributes: BTreeMap<String, Json>,
}

/// One span. Typed-core fields cover the columns the waterfall +
/// detail pane render directly; everything else (the ~100-column
/// long tail) lives in [`Span::attributes`] / [`Span::resource`]
/// so future columns surface without a code change.
#[derive(Debug, Clone)]
pub struct Span {
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub name: String,
    /// `service.name` resource attribute, hoisted out of
    /// [`Span::resource`] so the renderer can colour-key spans by
    /// service without a map lookup per frame. Empty string when
    /// missing.
    pub service: String,
    pub kind: SpanKind,
    /// `status.code` value, kept as `Option<String>` rather than an
    /// enum because we don't render-switch on it \u2014 we just colour
    /// errored rows red. Vendor-specific status codes (`"OK"` /
    /// `"ERROR"` / `"UNSET"`) pass through verbatim.
    pub status_code: Option<String>,
    /// Derived: `is_error` column when present, else
    /// `status_code == "ERROR"`. The renderer reads this directly
    /// for the red row tint.
    pub is_error: bool,
    pub start_ns: i64,
    /// `start_ns + duration_ns`. Stored to avoid recomputing on
    /// every render frame; clock skew (child ends after parent) is
    /// honoured by storing each span's own end independently
    /// instead of intersecting against the parent.
    pub end_ns: i64,
    pub duration_ns: i64,
    pub events: Vec<SpanEvent>,
    /// Every `attributes.*` column with a non-null value (the
    /// `attributes.` prefix stripped), plus the `attributes.custom`
    /// map's entries spliced in at the top level. `BTreeMap` for a
    /// deterministic detail-pane render order.
    pub attributes: BTreeMap<String, Json>,
    /// Same layout for the `resource.*` columns and
    /// `resource.custom` map.
    pub resource: BTreeMap<String, Json>,
}

impl Span {
    /// Convenience: a span whose `parent_span_id` is `None` is a
    /// canonical root. Step 21 records orphans (parent set but
    /// not found) as roots in the model too; the renderer
    /// reads [`TreeRow::is_orphan`] to flag those separately,
    /// so this method intentionally only checks the field-level
    /// state. `#[cfg(test)]` because every production caller
    /// uses [`TreeRow`] / [`TraceModel::roots`] instead.
    #[cfg(test)]
    pub fn is_root(&self) -> bool {
        self.parent_span_id.is_none()
    }
}

/// One row in the pre-flattened DFS view. `span_idx` indexes into
/// [`TraceModel::spans`]; `depth` drives indentation; `has_children`
/// tells the renderer whether to draw a fold caret. `is_orphan` is
/// true for roots whose `parent_span_id` was set but pointed
/// outside the loaded trace \u2014 surfaced in step 23 as a marker on
/// the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeRow {
    pub span_idx: usize,
    pub depth: u16,
    pub has_children: bool,
    pub is_orphan: bool,
}

/// The whole trace: spans + indices + flattened tree + time bounds.
/// Owned by `App` once `:trace <id>` lands a response (step 22).
#[derive(Debug, Clone)]
pub struct TraceModel {
    pub trace_id: String,
    pub dataset: String,
    pub spans: Vec<Span>,
    /// `span_id` -> index into [`Self::spans`]. Used by the parent
    /// linker and by `:trace`-internal commands (`/svc=…` filter,
    /// "jump to parent", etc.) added in step 24.
    pub by_id: BTreeMap<String, usize>,
    /// Indices into [`Self::spans`] of every span that has no
    /// `parent_span_id` *or* whose parent isn't in
    /// [`Self::by_id`]. Sorted by `start_ns` (same order they
    /// appear in [`Self::tree`]).
    pub roots: Vec<usize>,
    /// Earliest `start_ns` across every span. The renderer
    /// translates per-span times into pixels relative to this.
    pub t0_ns: i64,
    /// Latest `end_ns` across every span. Clock skew is normal:
    /// a child can end after its parent (the parent's
    /// `end_ns` won't reflect that), so we max over every span,
    /// not just the roots.
    pub t1_ns: i64,
    pub tree: Vec<TreeRow>,
}

impl TraceModel {
    /// `t1_ns - t0_ns`, the wall-clock width of the trace.
    /// Returns 0 if the trace is empty (the decoder rejects this
    /// case, but the accessor stays total to keep the renderer's
    /// math clean). Inverted bounds also collapse to 0 — the
    /// renderer divides by this for pixel layout and a negative
    /// width has no sane interpretation.
    pub fn duration_ns(&self) -> i64 {
        self.t1_ns.saturating_sub(self.t0_ns).max(0)
    }
}

// ============================================================
//          Step-24 helpers — fold / filter / visible_rows
// ============================================================
//
// Kept on the data layer (not the renderer) because every helper
// is pure structural manipulation — no pixels, no ratatui types.
// The renderer (`src/ui/trace.rs`) and the keymap
// (`src/app/keys/trace.rs`) both consume them, so the data layer
// is the only shared home.

/// Build the lower-cased "search blob" for one span. Pasted
/// together with `\t` separators so two adjacent fields can't
/// fuse into an accidental false positive (e.g. `foo` + `bar`
/// would otherwise hit `/oba`).
///
/// Contents in order: `name`, `service`, every attribute key +
/// JSON-rendered value, every resource key + value, every event
/// name and event attribute key/value. Lower-cased once at
/// build-time so the filter loop is a straight `str::contains`
/// against an already-lowered query.
///
/// The result is intentionally one allocation per span; the
/// `TraceView` cache stores it for the lifetime of the trace so
/// the cost is paid once per `:trace` invocation.
pub fn build_search_blob(span: &Span) -> String {
    let mut out = String::with_capacity(64 + span.attributes.len() * 16);
    push_lc(&mut out, &span.name);
    out.push('\t');
    push_lc(&mut out, &span.service);
    for (k, v) in &span.attributes {
        out.push('\t');
        push_lc(&mut out, k);
        out.push('=');
        push_json_lc(&mut out, v);
    }
    for (k, v) in &span.resource {
        out.push('\t');
        push_lc(&mut out, k);
        out.push('=');
        push_json_lc(&mut out, v);
    }
    for ev in &span.events {
        out.push('\t');
        push_lc(&mut out, &ev.name);
        for (k, v) in &ev.attributes {
            out.push('\t');
            push_lc(&mut out, k);
            out.push('=');
            push_json_lc(&mut out, v);
        }
    }
    out
}

fn push_lc(dst: &mut String, s: &str) {
    for c in s.chars() {
        for lc in c.to_lowercase() {
            dst.push(lc);
        }
    }
}

fn push_json_lc(dst: &mut String, v: &Json) {
    match v {
        Json::String(s) => push_lc(dst, s),
        Json::Null => dst.push_str("null"),
        Json::Bool(b) => dst.push_str(if *b { "true" } else { "false" }),
        Json::Number(n) => dst.push_str(&n.to_string()),
        Json::Array(_) | Json::Object(_) => {
            // Fall back to `serde_json::to_string` for nested
            // structures — keys + values both end up in the haystack
            // verbatim. Skip pretty-printing: the filter just needs
            // a flat character stream.
            if let Ok(s) = serde_json::to_string(v) {
                push_lc(dst, &s);
            }
        }
    }
}

/// Substring predicate. `query_lc` MUST be lower-cased by the
/// caller — the keymap layer does it once per keystroke so the
/// per-span hot path is a single `str::contains`.
pub fn span_matches_query(blob: &str, query_lc: &str) -> bool {
    query_lc.is_empty() || blob.contains(query_lc)
}

/// Walk every matched span's ancestor chain via `parent_span_id`,
/// returning the set of `span_idx` that must remain visible to
/// preserve the structural context around each match.
///
/// Orphans (parent set but not in `model.by_id`) terminate the
/// walk; the orphan itself stays in the set if it was matched.
/// Cycles can't happen in well-formed traces but the visited-set
/// guard makes the walk total in pathological input.
pub fn ancestor_closure(model: &TraceModel, matches: &[usize]) -> HashSet<usize> {
    let mut visible: HashSet<usize> = HashSet::with_capacity(matches.len() * 2);
    for &start in matches {
        if !visible.insert(start) {
            continue;
        }
        let mut cur = start;
        while let Some(parent_id) = model.spans[cur].parent_span_id.as_deref() {
            let Some(&parent_idx) = model.by_id.get(parent_id) else {
                break; // orphan boundary
            };
            if !visible.insert(parent_idx) {
                break; // already-walked ancestor cuts the chain
            }
            cur = parent_idx;
        }
    }
    visible
}

/// Indices into `model.tree` of the rows that should appear in
/// the current viewport.
///
/// Walk the flat DFS once, maintaining a "skip everything deeper
/// than this" watermark whenever we cross a collapsed parent.
/// When a filter set is supplied, rows whose `span_idx` isn't in
/// the set are dropped from the result — but collapsed-subtree
/// skipping still applies, so a hidden match inside a folded
/// subtree stays hidden (per spec: `zv` is the explicit opener).
///
/// `O(tree.len())` per render; the 1,498-span fixture clocks in
/// well under the per-frame budget on the developer machine.
pub fn visible_rows(
    tree: &[TreeRow],
    collapsed: &HashSet<usize>,
    filter: Option<&HashSet<usize>>,
) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::with_capacity(tree.len());
    // `Some(d)` ⇒ skip rows with depth > d until we step back up.
    let mut skip_below: Option<u16> = None;
    for (i, row) in tree.iter().enumerate() {
        if let Some(d) = skip_below {
            if row.depth > d {
                continue;
            }
            skip_below = None;
        }
        let pass_filter = match filter {
            Some(set) => set.contains(&row.span_idx),
            None => true,
        };
        if pass_filter {
            out.push(i);
        }
        if collapsed.contains(&row.span_idx) && row.has_children {
            // Suppress every deeper descendant until the DFS
            // walks back up to the collapsed row's depth.
            skip_below = Some(row.depth);
        }
    }
    out
}

/// Walk up the parent chain to find the deepest still-visible
/// ancestor of `span_idx` given the current `collapsed` set. Used
/// after `h` / `zM` collapses to snap the cursor into the
/// visible region without leaving it stranded inside a folded
/// subtree. Returns the input span_idx itself if no ancestor is
/// collapsed.
pub fn deepest_visible_ancestor(
    model: &TraceModel,
    collapsed: &HashSet<usize>,
    span_idx: usize,
) -> usize {
    let mut chain: Vec<usize> = vec![span_idx];
    let mut cur = span_idx;
    while let Some(parent_id) = model.spans[cur].parent_span_id.as_deref() {
        let Some(&parent_idx) = model.by_id.get(parent_id) else {
            break;
        };
        chain.push(parent_idx);
        cur = parent_idx;
    }
    // `chain` is leaf → root. The deepest visible ancestor is the
    // leaf-most entry whose every ancestor is *not* collapsed; or,
    // equivalently, the topmost collapsed entry (because once we
    // hit a collapsed parent, everything below it is invisible).
    // Walk root → leaf and stop at the first collapsed entry.
    let mut visible = span_idx;
    for &idx in chain.iter().rev() {
        if collapsed.contains(&idx) && idx != span_idx {
            return idx;
        }
        visible = idx;
    }
    visible
}

// ============================================================
//                   Span JSON projection
// ============================================================

/// `serde::Serialize` view over a [`Span`] used by `y` and
/// `:span json`. Mirrors the typed model the detail pane shows:
/// typed core fields + attributes / resource maps + events list,
/// not the raw 100-column APL response. Owned references are
/// fine — we serialise once per invocation.
#[derive(Serialize)]
pub struct SpanJson<'a> {
    pub trace_id: &'a str,
    pub span_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<&'a str>,
    pub name: &'a str,
    pub service: &'a str,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<&'a str>,
    pub is_error: bool,
    pub start_ns: i64,
    pub end_ns: i64,
    pub duration_ns: i64,
    pub attributes: &'a BTreeMap<String, Json>,
    pub resource: &'a BTreeMap<String, Json>,
    pub events: Vec<SpanEventJson<'a>>,
}

#[derive(Serialize)]
pub struct SpanEventJson<'a> {
    pub time_ns: i64,
    pub name: &'a str,
    pub attributes: &'a BTreeMap<String, Json>,
}

impl<'a> SpanJson<'a> {
    /// Borrow a [`Span`] into the projection. Caller owns the
    /// trace id (it lives on [`TraceModel`], not on the span).
    pub fn from_span(trace_id: &'a str, span: &'a Span) -> Self {
        Self {
            trace_id,
            span_id: &span.span_id,
            parent_span_id: span.parent_span_id.as_deref(),
            name: &span.name,
            service: &span.service,
            kind: span.kind.as_str(),
            status_code: span.status_code.as_deref(),
            is_error: span.is_error,
            start_ns: span.start_ns,
            end_ns: span.end_ns,
            duration_ns: span.duration_ns,
            attributes: &span.attributes,
            resource: &span.resource,
            events: span
                .events
                .iter()
                .map(|e| SpanEventJson {
                    time_ns: e.time_ns,
                    name: &e.name,
                    attributes: &e.attributes,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests;
