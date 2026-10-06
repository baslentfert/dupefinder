//! Scan configuration and the safety rules that constrain it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How files are enumerated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScanStrategy {
    /// Parallel directory walk. Works everywhere, needs no privileges.
    Walk,
    /// Read the NTFS Master File Table directly: whole volumes in seconds,
    /// but requires an elevated process and an NTFS volume.
    Mft,
    /// Use [`ScanStrategy::Mft`] when the process is elevated and every root
    /// sits on NTFS, otherwise fall back to [`ScanStrategy::Walk`].
    #[default]
    Auto,
}

/// Which detectors to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Detectors {
    /// Byte-identical files of any type.
    pub exact: bool,
    /// Visually similar images (perceptual hashing).
    pub similar_images: bool,
}

impl Default for Detectors {
    fn default() -> Self {
        Self {
            exact: true,
            similar_images: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ScanConfig {
    pub roots: Vec<PathBuf>,
    pub strategy: ScanStrategy,
    pub detectors: Detectors,

    /// Files smaller than this are ignored. Empty files are all "duplicates"
    /// of each other and are never interesting, so the default skips them.
    pub min_size: u64,
    pub max_size: Option<u64>,

    /// Substrings matched case-insensitively against the full path.
    pub exclude_patterns: Vec<String>,
    /// When set, only these extensions (lowercase, no dot) are considered.
    pub only_extensions: Option<HashSet<String>>,

    pub include_hidden: bool,
    pub follow_symlinks: bool,
    /// Skip OS and application directories. Strongly recommended: duplicates
    /// there are load-bearing, not waste.
    pub skip_system_paths: bool,

    /// After hashes match, compare the files byte for byte before reporting.
    /// Costs a second full read; BLAKE3 collisions are not a practical risk,
    /// so this is off by default.
    pub verify_bytes: bool,

    /// Skip files that OneDrive, Dropbox and friends keep in the cloud.
    ///
    /// Hashing needs the bytes, and reading a placeholder makes Windows
    /// download the whole file. Scanning a OneDrive folder with this off can
    /// silently pull down hundreds of gigabytes and fill the disk, so the
    /// default is to leave them alone and report them as skipped.
    pub skip_cloud_placeholders: bool,

    /// Hamming distance (in bits, out of 64) below which two images count as
    /// similar. 0 = visually identical, ~10 = same photo re-encoded/resized,
    /// ~18 = burst shots and light edits.
    pub similarity_threshold: u32,

    /// Images shorter than this on their short side are not compared.
    ///
    /// A perceptual hash reduces an image to an 8×8 grid, so below roughly
    /// this size it is describing the grid rather than the picture: every
    /// 16-pixel emoji and toolbar sprite ends up "similar" to every other one,
    /// burying the actual photos.
    pub min_image_side: u32,

    /// Worker threads. `None` uses one per logical core.
    pub threads: Option<usize>,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            strategy: ScanStrategy::default(),
            detectors: Detectors::default(),
            min_size: 1,
            max_size: None,
            exclude_patterns: Vec::new(),
            only_extensions: None,
            include_hidden: true,
            follow_symlinks: false,
            skip_system_paths: true,
            verify_bytes: false,
            skip_cloud_placeholders: true,
            similarity_threshold: 10,
            min_image_side: 64,
            threads: None,
        }
    }
}

impl ScanConfig {
    /// Whether a discovered path survives the configured filters.
    ///
    /// Size is checked separately by the caller, which already has the
    /// metadata in hand.
    pub fn accepts_path(&self, path: &Path) -> bool {
        if self.skip_system_paths && is_system_path(path) {
            return false;
        }

        let lossy = path.to_string_lossy();
        let lower = lossy.to_ascii_lowercase();

        if self
            .exclude_patterns
            .iter()
            .any(|p| lower.contains(&p.to_ascii_lowercase()))
        {
            return false;
        }

        if let Some(exts) = &self.only_extensions {
            let ok = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| exts.contains(&e.to_ascii_lowercase()))
                .unwrap_or(false);
            if !ok {
                return false;
            }
        }

        true
    }

    pub fn accepts_size(&self, size: u64) -> bool {
        size >= self.min_size && self.max_size.is_none_or(|max| size <= max)
    }

    /// Whether a file with these Windows attributes may be read.
    pub fn accepts_attributes(&self, attributes: u32) -> bool {
        !self.skip_cloud_placeholders || !is_cloud_placeholder(attributes)
    }
}

/// Windows file attributes that mean "the bytes are not here".
///
/// `RECALL_ON_OPEN` and `RECALL_ON_DATA_ACCESS` are what a cloud sync client
/// sets on a file it has dehydrated; `OFFLINE` is the older equivalent used by
/// hierarchical storage. Touching the contents of any of them makes the
/// filesystem fetch the file first.
const FILE_ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;
const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;

pub fn is_cloud_placeholder(attributes: u32) -> bool {
    attributes
        & (FILE_ATTRIBUTE_OFFLINE
            | FILE_ATTRIBUTE_RECALL_ON_OPEN
            | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS)
        != 0
}

/// Directory prefixes that hold OS or application files.
///
/// These legitimately contain many identical files (side-by-side assemblies,
/// bundled runtimes, package caches) and removing them breaks software, so we
/// neither report nor delete them by default.
const SYSTEM_PREFIXES: &[&str] = &[
    r"\windows\",
    r"\program files\",
    r"\program files (x86)\",
    r"\programdata\package cache\",
    r"\$recycle.bin\",
    r"\system volume information\",
    r"\appdata\local\packages\",
    r"\appdata\local\microsoft\windowsapps\",
    r"\node_modules\",
    r"\.git\",
    r"\onedrivetemp\",
];

/// Paths that must never be modified, whatever the user clicks.
///
/// This is the last line of defence for the destructive actions; it is
/// deliberately independent of [`ScanConfig::skip_system_paths`] so that
/// turning the filter off still cannot arm a delete of `C:\Windows`.
const NEVER_TOUCH_PREFIXES: &[&str] = &[
    r"\windows\",
    r"\program files\",
    r"\program files (x86)\",
    r"\system volume information\",
    r"\$recycle.bin\",
];

fn normalised(path: &Path) -> String {
    let mut s = path
        .to_string_lossy()
        .to_ascii_lowercase()
        .replace('/', "\\");
    if !s.ends_with('\\') {
        s.push('\\');
    }
    s
}

pub fn is_system_path(path: &Path) -> bool {
    let s = normalised(path);
    SYSTEM_PREFIXES.iter().any(|p| s.contains(p))
}

/// Whether a destructive action may be performed on `path`.
pub fn is_protected(path: &Path) -> bool {
    let s = normalised(path);
    if NEVER_TOUCH_PREFIXES.iter().any(|p| s.contains(p)) {
        return true;
    }
    // A drive root itself, e.g. `C:\`.
    s.len() == 3 && s.as_bytes()[1] == b':'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_paths_are_recognised() {
        assert!(is_system_path(Path::new(r"C:\Windows\System32\x.dll")));
        assert!(is_system_path(Path::new(r"D:\proj\node_modules\a\b.js")));
        assert!(!is_system_path(Path::new(r"D:\Photos\2024\a.jpg")));
    }

    #[test]
    fn protected_paths_reject_destructive_actions() {
        assert!(is_protected(Path::new(r"C:\Windows\notepad.exe")));
        assert!(is_protected(Path::new(r"C:\")));
        assert!(!is_protected(Path::new(r"C:\Users\me\a.jpg")));
        // A project folder that merely *looks* like a system one is fine.
        assert!(!is_protected(Path::new(r"D:\dev\node_modules\a.js")));
    }

    #[test]
    fn cloud_placeholders_are_left_alone_by_default() {
        let cfg = ScanConfig::default();

        // A dehydrated OneDrive file: reading it would trigger a download.
        assert!(!cfg.accepts_attributes(FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS));
        assert!(!cfg.accepts_attributes(FILE_ATTRIBUTE_RECALL_ON_OPEN));
        assert!(!cfg.accepts_attributes(FILE_ATTRIBUTE_OFFLINE));

        // A normal file, and a synced file that is present locally.
        const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x20;
        const FILE_ATTRIBUTE_PINNED: u32 = 0x0008_0000;
        assert!(cfg.accepts_attributes(FILE_ATTRIBUTE_ARCHIVE));
        assert!(cfg.accepts_attributes(FILE_ATTRIBUTE_ARCHIVE | FILE_ATTRIBUTE_PINNED));
    }

    #[test]
    fn cloud_placeholders_can_be_opted_into() {
        let cfg = ScanConfig {
            skip_cloud_placeholders: false,
            ..Default::default()
        };
        assert!(cfg.accepts_attributes(FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS));
    }

    #[test]
    fn extension_filter_is_case_insensitive() {
        let cfg = ScanConfig {
            only_extensions: Some(["jpg".to_string()].into_iter().collect()),
            ..Default::default()
        };
        assert!(cfg.accepts_path(Path::new(r"D:\a\B.JPG")));
        assert!(!cfg.accepts_path(Path::new(r"D:\a\b.png")));
    }
}
