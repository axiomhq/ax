//! Cross-platform atomic file write.
//!
//! Used by [`crate::cache`] and [`crate::history`] to commit small
//! JSON snapshots without leaving torn / half-written files on disk
//! if the process crashes mid-write or two instances race.
//!
//! The trick: write into a `NamedTempFile` in the *same directory* as
//! the target, `fsync`, then `rename` onto the final path. POSIX
//! guarantees same-filesystem rename is atomic, so any concurrent
//! reader sees either the old contents or the new — never a partial
//! file. Two concurrent writers each get a uniquely-named temp file
//! (no collision) and last-`rename` wins; the loser doesn't corrupt
//! the winner.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

/// Atomically write `contents` to `path`, creating parent dirs as
/// needed.
pub fn atomic_write_text(path: &Path, contents: &str) -> Result<()> {
    // A bare filename (`foo.json`) has `Some("")` as its parent;
    // treat that — and a true rootless path — as the current
    // directory so the temp file lands on the same filesystem.
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating temp file in {}", parent.display()))?;
    tmp.write_all(contents.as_bytes())
        .with_context(|| format!("writing {}", tmp.path().display()))?;
    tmp.as_file()
        .sync_all()
        .with_context(|| format!("flushing {}", tmp.path().display()))?;
    tmp.persist(path)
        .with_context(|| format!("renaming into {}", path.display()))?;
    // fsync the parent directory so the rename's directory entry is
    // durable across a crash too — POSIX doesn't guarantee the new
    // name is on disk until the directory itself is synced. Best
    // effort: platforms that can't open a dir as a file (Windows)
    // just skip it rather than failing the write.
    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}
