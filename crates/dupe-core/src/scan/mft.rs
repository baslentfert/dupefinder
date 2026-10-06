//! Whole-volume enumeration by reading the NTFS Master File Table.
//!
//! A directory walk of a full disk spends nearly all its time on per-directory
//! syscalls. The MFT is a flat table of every record on the volume, so reading
//! it sequentially lists millions of files in seconds. The costs: the process
//! must be elevated, the volume must be NTFS, and paths are reconstructed from
//! parent references rather than observed, so results are filtered afterwards
//! exactly like the walker's are.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use ntfs_reader::file_info::{FileInfo, HashMapCache};
use ntfs_reader::mft::Mft;
use ntfs_reader::volume::Volume;

use crate::config::ScanConfig;
use crate::error::{Error, Result};
use crate::model::FileEntry;
use crate::progress::{Reporter, ScanEvent, Throttle};

pub fn enumerate(cfg: &ScanConfig, reporter: &Reporter) -> Result<Vec<FileEntry>> {
    let mut entries = Vec::new();

    for letter in volume_letters(cfg) {
        reporter.check_cancelled()?;
        match enumerate_volume(letter, cfg, reporter, &mut entries) {
            Ok(()) => {}
            Err(err) => {
                reporter.skipped(format!("{letter}:\\"), err.to_string());
                return Err(err);
            }
        }
    }

    Ok(entries)
}

fn enumerate_volume(
    letter: char,
    cfg: &ScanConfig,
    reporter: &Reporter,
    out: &mut Vec<FileEntry>,
) -> Result<()> {
    let device = format!(r"\\.\{letter}:");
    let volume = Volume::new(&device).map_err(|e| Error::Mft {
        volume: device.clone(),
        reason: e.to_string(),
    })?;
    let mft = Mft::new(volume).map_err(|e| Error::Mft {
        volume: device.clone(),
        reason: e.to_string(),
    })?;

    // Resolving a record's full path walks its parent chain; caching those
    // parents is the difference between seconds and minutes on a big volume.
    let mut cache = HashMapCache::default();
    let seen = AtomicU64::new(0);
    let bytes = AtomicU64::new(0);
    let throttle = Throttle::new(Duration::from_millis(120));
    let mut cancelled = false;

    for file in mft.files() {
        if reporter.is_cancelled() {
            cancelled = true;
            break;
        }

        let info = FileInfo::with_cache(&mft, &file, &mut cache);
        if info.is_directory {
            continue;
        }

        // MFT paths are volume-relative; make them the absolute paths the
        // rest of the engine (and every file operation) expects.
        let path = absolute(letter, &info.path);

        if !cfg.accepts_size(info.size) || !cfg.accepts_path(&path) {
            continue;
        }

        // Same rule as the walker: never make the filesystem fetch a file that
        // is only stored in the cloud.
        if !cfg.accepts_attributes(info.file_attributes) {
            reporter.skipped(
                path,
                "stored in the cloud; reading it would download the file",
            );
            continue;
        }

        let n = seen.fetch_add(1, Ordering::Relaxed) + 1;
        let b = bytes.fetch_add(info.size, Ordering::Relaxed) + info.size;
        if throttle.ready() {
            reporter.send(ScanEvent::Enumerated { files: n, bytes: b });
        }

        out.push(FileEntry {
            path,
            size: info.size,
            mtime: info.modified.map(|t| t.unix_timestamp()).unwrap_or(0),
            file_id: None,
        });
    }

    if cancelled {
        return Err(Error::Cancelled);
    }

    reporter.send(ScanEvent::Enumerated {
        files: seen.load(Ordering::Relaxed),
        bytes: bytes.load(Ordering::Relaxed),
    });
    Ok(())
}

fn absolute(letter: char, path: &std::path::Path) -> PathBuf {
    if path.has_root() && path.to_string_lossy().as_bytes().get(1) == Some(&b':') {
        return path.to_path_buf();
    }
    let mut out = PathBuf::from(format!("{letter}:\\"));
    out.push(path.strip_prefix("\\").unwrap_or(path));
    out
}

/// Drive letters covered by the configured roots.
fn volume_letters(cfg: &ScanConfig) -> Vec<char> {
    let mut letters: Vec<char> = cfg
        .roots
        .iter()
        .filter_map(|r| r.to_string_lossy().chars().next())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    letters.sort_unstable();
    letters.dedup();
    letters
}
