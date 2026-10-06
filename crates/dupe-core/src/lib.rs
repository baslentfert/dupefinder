//! Duplicate and near-duplicate detection engine.
//!
//! The engine is deliberately free of any UI dependency so it can be driven
//! from Tauri, a CLI, or a benchmark harness.
//!
//! # Exact duplicates
//!
//! Four stages, each one only ever seeing what the previous could not rule
//! out. Most files are eliminated without their contents being read at all:
//!
//! 1. **Enumerate** — [`scan`], via a parallel walk or the NTFS MFT.
//! 2. **Group by size** — a file with a unique length has no duplicate, and
//!    this is decided from metadata alone.
//! 3. **Partial hash** — [`hash::partial`], head + tail + size.
//! 4. **Full hash** — [`hash::full`], BLAKE3 over everything that is left.
//!
//! # Similar images
//!
//! Perceptual hashes ([`images`]) placed in a BK-tree, then clustered by
//! Hamming distance, so re-encodes, resizes and light edits group together
//! even though no two files share a byte.

pub mod actions;
pub mod cache;
pub mod config;
pub mod error;
pub mod exact;
pub mod folders;
pub mod hash;
pub mod images;
pub mod model;
pub mod orphans;
pub mod platform;
pub mod progress;
pub mod report;
pub mod restore_point;
pub mod scan;

pub use config::{Detectors, ScanConfig, ScanStrategy};
pub use error::{Error, Result};
pub use folders::{FolderCopy, FolderGroup};
pub use model::{ContentHash, DuplicateGroup, FileEntry, FileId, SimilarGroup, SimilarImage};
pub use orphans::{OrphanConfig, OrphanFinding, OrphanKind, OrphanReport};
pub use progress::{CancelHandle, Phase, Reporter, ScanEvent, Summary};

use std::path::PathBuf;
use std::time::Instant;

use serde::{Deserialize, Serialize};

/// Everything a completed scan produced.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub duplicates: Vec<DuplicateGroup>,
    /// Folders whose duplicated content is identical — the same findings as
    /// `duplicates`, grouped the way a user actually decides about them.
    pub folders: Vec<FolderGroup>,
    pub similar: Vec<SimilarGroup>,
    pub summary: Summary,
}

/// Notified as parts of a scan complete.
///
/// The two detectors are independent and the image half is much the slower of
/// them, so a UI that waits for both leaves the user staring at a progress bar
/// with finished results sitting behind it.
pub trait ScanObserver: Sync {
    /// Exact duplicates are final; image comparison is about to start.
    fn duplicates_ready(&self, groups: &[DuplicateGroup], summary: &Summary);
}

/// Run a full scan.
///
/// Blocking; call it from a worker thread and consume `reporter`'s events for
/// progress. Returns [`Error::Cancelled`] if the scan was stopped part-way.
pub fn scan(
    cfg: &ScanConfig,
    reporter: &Reporter,
    cache_path: Option<PathBuf>,
) -> Result<ScanResult> {
    scan_observed(cfg, reporter, cache_path, None)
}

/// [`scan`], but told where to publish intermediate results.
pub fn scan_observed(
    cfg: &ScanConfig,
    reporter: &Reporter,
    cache_path: Option<PathBuf>,
    observer: Option<&dyn ScanObserver>,
) -> Result<ScanResult> {
    let started = Instant::now();
    scan::walk::validate_roots(cfg)?;

    if let Some(threads) = cfg.threads {
        // Best-effort: the pool is global and may already be built if a
        // previous scan ran in this process.
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build_global();
    }

    let cache = cache_path.as_deref().and_then(cache::HashCache::open);

    let mut entries = scan::enumerate(cfg, reporter)?;
    let mut summary = Summary {
        files_seen: entries.len() as u64,
        bytes_seen: entries.iter().map(|e| e.size).sum(),
        ..Default::default()
    };

    let duplicates = if cfg.detectors.exact {
        exact::find(cfg, &mut entries, reporter, cache.as_ref(), &mut summary)?
    } else {
        Vec::new()
    };

    // Hand the duplicates over before starting on images, which on a photo
    // library takes far longer than everything above it put together.
    if let Some(observer) = observer {
        let mut partial = summary.clone();
        partial.duplicate_groups = duplicates.len() as u64;
        partial.duplicate_files = duplicates.iter().map(|g| g.files.len() as u64).sum();
        partial.wasted_bytes = duplicates.iter().map(|g| g.wasted_bytes()).sum();
        partial.skipped = reporter.skip_count();
        partial.elapsed_ms = started.elapsed().as_millis() as u64;

        observer.duplicates_ready(&duplicates, &partial);
    }

    let similar = if cfg.detectors.similar_images {
        images::find(cfg, &mut entries, reporter, cache.as_ref(), &mut summary)?
    } else {
        Vec::new()
    };

    let folders = if cfg.detectors.exact {
        folders::group_folders(&entries, &duplicates)
    } else {
        Vec::new()
    };

    if let Some(cache) = &cache {
        cache.flush();
    }

    summary.duplicate_groups = duplicates.len() as u64;
    summary.duplicate_files = duplicates.iter().map(|g| g.files.len() as u64).sum();
    summary.wasted_bytes = duplicates.iter().map(|g| g.wasted_bytes()).sum();
    summary.similar_groups = similar.len() as u64;
    summary.similar_images = similar.iter().map(|g| g.images.len() as u64).sum();
    // Counted centrally: a file can fail in more than one stage, and
    // enumeration failures happen before any stage runs.
    summary.skipped = reporter.skip_count();
    summary.elapsed_ms = started.elapsed().as_millis() as u64;

    reporter.phase(Phase::Done);
    reporter.send(ScanEvent::Finished {
        summary: summary.clone(),
    });

    Ok(ScanResult {
        duplicates,
        folders,
        similar,
        summary,
    })
}
