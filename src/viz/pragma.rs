//! `// @viz` pragma parsing.
//!
//! The pragma is a single comment line, anywhere in the buffer's leading
//! comment block:
//!
//! ```text
//! // @viz line
//! // @viz top_list n=10 by=host
//! // @viz statistic agg=last unit=ms
//! ```
//!
//! Format: `// @viz <kind> [k=v ...]`. Whitespace is ignored. Options
//! after the kind are stored verbatim in a `BTreeMap<String, String>`;
//! typed accessors live on the consumers that need them.

use std::collections::BTreeMap;

use crate::dashboard::VizKind;

/// Result of parsing the leading-pragma line of a buffer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VizSpec {
    pub kind: VizKind,
    pub opts: BTreeMap<String, String>,
}

/// What went wrong parsing a pragma line. Surfaced as a diagnostic by
/// the caller — never as a hard error, because a half-typed buffer is a
/// normal mid-edit state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PragmaError {
    /// The line started `// @viz` but had no kind token afterwards.
    MissingKind,
    /// The kind token was not a known [`VizKind`] identifier.
    UnknownKind { token: String },
    /// An option token didn't look like `k=v`.
    MalformedOption { token: String },
}

impl std::fmt::Display for PragmaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PragmaError::MissingKind => f.write_str("`@viz` pragma is missing the kind"),
            PragmaError::UnknownKind { token } => write!(f, "unknown viz kind: `{token}`"),
            PragmaError::MalformedOption { token } => {
                write!(f, "expected `key=value` in viz pragma, got `{token}`")
            }
        }
    }
}

/// Parse the first `// @viz` line of `src`. Returns:
///
/// - `Ok(Some(spec))` — pragma found and parsed.
/// - `Ok(None)` — no pragma line; caller should use [`VizSpec::default`].
/// - `Err((line_index, err))` — pragma line was malformed; caller can
///   surface this as a diagnostic anchored at `line_index` (zero-based).
pub fn parse_pragma(src: &str) -> Result<Option<VizSpec>, (usize, PragmaError)> {
    for (line_idx, line) in src.lines().enumerate() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("//") {
            // Stop scanning once we hit non-comment content; pragmas only
            // live in the leading comment block. Blank lines are allowed
            // between pragma and code, so we don't break on those.
            if trimmed.is_empty() {
                continue;
            }
            break;
        }
        let body = trimmed.trim_start_matches('/').trim_start();
        let Some(rest) = body.strip_prefix("@viz") else {
            continue;
        };
        // Require a space or end-of-line after `@viz` so `@vizfoo` is not
        // a match.
        if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
            continue;
        }
        let rest = rest.trim();
        let mut tokens = rest.split_whitespace();
        let kind_tok = tokens.next().ok_or((line_idx, PragmaError::MissingKind))?;
        let kind = VizKind::parse(kind_tok).ok_or_else(|| {
            (
                line_idx,
                PragmaError::UnknownKind {
                    token: kind_tok.to_string(),
                },
            )
        })?;
        let mut opts = BTreeMap::new();
        for tok in tokens {
            let Some((k, v)) = tok.split_once('=') else {
                return Err((
                    line_idx,
                    PragmaError::MalformedOption {
                        token: tok.to_string(),
                    },
                ));
            };
            opts.insert(k.to_string(), v.to_string());
        }
        return Ok(Some(VizSpec { kind, opts }));
    }
    Ok(None)
}

/// Format a [`VizSpec`] as a pragma line (without a trailing newline).
/// Used by `:viz` to insert/rewrite the line at the top of the buffer.
pub fn format_pragma(spec: &VizSpec) -> String {
    let mut out = format!("// @viz {}", spec.kind.as_str());
    // `BTreeMap` iteration is sorted, so option order is stable.
    for (k, v) in &spec.opts {
        out.push(' ');
        out.push_str(k);
        out.push('=');
        out.push_str(v);
    }
    out
}

/// Rewrite (or insert) the pragma line in `src`. Returns the new buffer
/// text. Idempotent: calling it twice with the same spec yields the same
/// output.
pub fn upsert_pragma(src: &str, spec: &VizSpec) -> String {
    let new_line = format_pragma(spec);
    let mut lines: Vec<String> = src.lines().map(str::to_string).collect();
    // Find the existing `// @viz` line in the leading comment block.
    let mut existing: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("//") {
            if trimmed.is_empty() {
                continue;
            }
            break;
        }
        let body = trimmed.trim_start_matches('/').trim_start();
        if let Some(rest) = body.strip_prefix("@viz")
            && (rest.is_empty() || rest.starts_with(char::is_whitespace))
        {
            existing = Some(i);
            break;
        }
    }
    match existing {
        Some(i) => lines[i] = new_line,
        None => lines.insert(0, new_line),
    }
    // Preserve the trailing newline if the original buffer had one. `str::lines`
    // drops it, so we add one back when needed.
    let mut out = lines.join("\n");
    if src.ends_with('\n') {
        out.push('\n');
    }
    out
}
