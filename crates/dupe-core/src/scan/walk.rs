//! Parallel directory walk.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use ignore::{WalkBuilder, WalkState};

use crate::config::{is_system_path, ScanConfig};
use crate::error::{Error, Result};
use crate::model::FileEntry;
use crate::progress::{Reporter, ScanEvent, Throttle};

pub fn enumerate(cfg: &ScanConfig, reporter: &Reporter) -> Result<Vec<FileEntry>> {
    let mut roots = cfg.roots.iter();
    let Some(first) = roots.next() else {
        return Ok(Vec::new());
    };

    let mut builder = WalkBuilder::new(first);
    for root in roots {
        builder.add(root);
    }

    builder
        // We want every file on disk, not a source tree view: no .gitignore,
        // no .ignore, no automatic hidden-file filtering.
        .standard_filters(false)
        .hidden(!cfg.include_hidden)
        .follow_links(cfg.follow_symlinks)
        .threads(cfg.threads.unwrap_or_else(num_threads))
        .max_depth(None);

    let (tx, rx) = crossbeam_channel::unbounded::<FileEntry>();
    let files = AtomicU64::new(0);
    let bytes = AtomicU64::new(0);
    let throttle = Throttle::new(Duration::from_millis(120));

    builder.build_parallel().run(|| {
        let tx = tx.clone();
        let files = &files;
        let bytes = &bytes;
        let throttle = &throttle;

        Box::new(move |result| {
            if reporter.is_cancelled() {
                return WalkState::Quit;
            }

            let entry = match result {
                Ok(entry) => entry,
                Err(err) => {
                    // Unreadable directories are normal on a full-disk scan
                    // (permissions, locked files); note them and move on.
                    // `ignore::Error` folds the path into its message.
                    reporter.skipped(Path::new("<walk>"), err.to_string());
                    return WalkState::Continue;
                }
            };

            let file_type = match entry.file_type() {
                Some(ft) => ft,
                // Only the root of a stdin walk has no file type.
                None => return WalkState::Continue,
            };

            if file_type.is_dir() {
                // Pruning here is what makes excluding `node_modules` cheap:
                // the subtree is never descended into at all.
                return if cfg.skip_system_paths && is_system_path(entry.path()) {
                    WalkState::Skip
                } else {
                    WalkState::Continue
                };
            }

            // Symlinks and junctions point at bytes counted elsewhere.
            if file_type.is_symlink() {
                return WalkState::Continue;
            }

            if !cfg.accepts_path(entry.path()) {
                return WalkState::Continue;
            }

            let Ok(meta) = entry.metadata() else {
                return WalkState::Continue;
            };

            let size = meta.len();
            if !cfg.accepts_size(size) {
                return WalkState::Continue;
            }

            // Reading a cloud placeholder makes Windows download it. Say so
            // rather than silently omitting the file — a user scanning their
            // OneDrive should know why nothing was found there.
            if !cfg.accepts_attributes(file_attributes(&meta)) {
                reporter.skipped(
                    entry.path(),
                    "stored in the cloud; reading it would download the file",
                );
                return WalkState::Continue;
            }

            let n = files.fetch_add(1, Ordering::Relaxed) + 1;
            let b = bytes.fetch_add(size, Ordering::Relaxed) + size;
            if throttle.ready() {
                reporter.send(ScanEvent::Enumerated { files: n, bytes: b });
            }

            let _ = tx.send(FileEntry {
                path: entry.into_path(),
                size,
                mtime: mtime_secs(&meta),
                // Resolved later, and only for files that survive size
                // grouping — opening a handle per file is far too expensive
                // to do during enumeration.
                file_id: None,
            });

            WalkState::Continue
        })
    });

    drop(tx);
    let entries: Vec<FileEntry> = rx.into_iter().collect();

    reporter.send(ScanEvent::Enumerated {
        files: files.load(Ordering::Relaxed),
        bytes: bytes.load(Ordering::Relaxed),
    });
    reporter.check_cancelled()?;

    Ok(entries)
}

/// Windows file attribute bits, or zero on platforms that have none.
#[cfg(windows)]
pub(crate) fn file_attributes(meta: &std::fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes()
}

#[cfg(not(windows))]
pub(crate) fn file_attributes(_meta: &std::fs::Metadata) -> u32 {
    0
}

pub(crate) fn mtime_secs(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub(crate) fn num_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

/// Validate roots up front so a typo fails immediately instead of returning
/// an empty result that looks like "no duplicates found".
pub fn validate_roots(cfg: &ScanConfig) -> Result<()> {
    for root in &cfg.roots {
        if !root.is_dir() {
            return Err(Error::BadRoot(root.clone()));
        }
    }
    Ok(())
}
