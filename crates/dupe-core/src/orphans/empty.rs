//! Directories with nothing in them.
//!
//! "Empty" is transitive: a folder containing only empty folders is itself
//! waste. The whole chain is therefore collapsed and only its topmost folder
//! is reported, so removing one finding cleans up the entire nest rather than
//! leaving the user to click through twenty nested entries.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::config::is_system_path;
use crate::error::Result;
use crate::progress::{Phase, Reporter, Throttle};

use super::{Confidence, OrphanFinding, OrphanKind};

/// Files Explorer creates on its own. A folder holding only these was never
/// filled by the user.
const SHELL_METADATA: &[&str] = &["desktop.ini", "thumbs.db", ".ds_store", "folder.jpg"];

pub fn find(cfg: &super::OrphanConfig, reporter: &Reporter) -> Result<Vec<OrphanFinding>> {
    reporter.phase(Phase::ScanningEmptyFolders);
    let throttle = Throttle::new(std::time::Duration::from_millis(100));
    let mut seen = 0u64;

    // `has_content` marks a directory and every ancestor of it as non-empty.
    let mut directories: HashSet<PathBuf> = HashSet::new();
    let mut has_content: HashSet<PathBuf> = HashSet::new();
    let mut mtimes: HashMap<PathBuf, i64> = HashMap::new();

    for root in &cfg.roots {
        for entry in ignore::WalkBuilder::new(root)
            .standard_filters(false)
            .follow_links(false)
            .build()
            .flatten()
        {
            if reporter.is_cancelled() {
                return Ok(Vec::new());
            }

            let path = entry.path();
            seen += 1;
            reporter.examining(&throttle, Phase::ScanningEmptyFolders, path, seen);

            let Ok(meta) = entry.metadata() else {
                // Unreadable: assume there is something in there.
                mark_ancestors(path, root, &mut has_content);
                continue;
            };

            if meta.is_dir() {
                if !is_system_path(path) {
                    directories.insert(path.to_path_buf());
                    mtimes.insert(path.to_path_buf(), crate::scan::walk::mtime_secs(&meta));
                }
                continue;
            }

            if cfg.ignore_shell_metadata && is_shell_metadata(path) {
                continue;
            }

            if let Some(parent) = path.parent() {
                mark_ancestors(parent, root, &mut has_content);
                has_content.insert(parent.to_path_buf());
            }
        }
    }

    // A scan root is a place the user chose; do not offer to delete it even
    // when it happens to be empty.
    let roots: HashSet<&Path> = cfg.roots.iter().map(|p| p.as_path()).collect();

    let empty: HashSet<&PathBuf> = directories
        .iter()
        .filter(|dir| !has_content.contains(*dir) && !roots.contains(dir.as_path()))
        .collect();

    // Report only the top of each empty chain.
    let mut findings: Vec<OrphanFinding> = empty
        .iter()
        .filter(|dir| {
            dir.parent()
                .map(|parent| !empty.contains(&parent.to_path_buf()))
                .unwrap_or(true)
        })
        .map(|dir| OrphanFinding {
            id: 0,
            kind: OrphanKind::EmptyFolder,
            path: (*dir).clone(),
            size_bytes: 0,
            is_directory: true,
            confidence: Confidence::Certain,
            reason: describe(dir, cfg.ignore_shell_metadata),
            last_modified: mtimes.get(*dir).copied(),
        })
        .collect();

    findings.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(findings)
}

fn describe(dir: &Path, ignore_metadata: bool) -> String {
    let nested = std::fs::read_dir(dir)
        .map(|entries| entries.flatten().count())
        .unwrap_or(0);

    if nested == 0 {
        "contains nothing".to_string()
    } else if ignore_metadata {
        "contains only empty folders and Explorer metadata".to_string()
    } else {
        "contains only empty folders".to_string()
    }
}

fn is_shell_metadata(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.to_ascii_lowercase())
        .is_some_and(|n| SHELL_METADATA.contains(&n.as_str()))
}

/// Mark every directory from `path` up to (and including) `root`.
fn mark_ancestors(path: &Path, root: &Path, out: &mut HashSet<PathBuf>) {
    let mut current = Some(path);
    while let Some(dir) = current {
        if !out.insert(dir.to_path_buf()) {
            // Already marked, so its ancestors are too.
            return;
        }
        if dir == root {
            return;
        }
        current = dir.parent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::Reporter;

    fn config(root: &Path) -> super::super::OrphanConfig {
        super::super::OrphanConfig {
            roots: vec![root.to_path_buf()],
            ..Default::default()
        }
    }

    #[test]
    fn reports_only_the_top_of_an_empty_chain() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b/c")).unwrap();

        let findings = find(&config(dir.path()), &Reporter::silent()).unwrap();

        assert_eq!(findings.len(), 1, "got {findings:?}");
        assert!(findings[0].path.ends_with("a"));
    }

    #[test]
    fn a_folder_with_a_file_is_not_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        std::fs::write(dir.path().join("a/b/keep.txt"), b"x").unwrap();

        let findings = find(&config(dir.path()), &Reporter::silent()).unwrap();
        assert!(findings.is_empty(), "got {findings:?}");
    }

    #[test]
    fn explorer_metadata_alone_still_counts_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("desktop.ini"), b"[.ShellClassInfo]").unwrap();

        let findings = find(&config(dir.path()), &Reporter::silent()).unwrap();
        assert_eq!(findings.len(), 1);

        let mut strict = config(dir.path());
        strict.ignore_shell_metadata = false;
        assert!(find(&strict, &Reporter::silent()).unwrap().is_empty());
    }

    #[test]
    fn the_scan_root_itself_is_never_offered() {
        let dir = tempfile::tempdir().unwrap();
        let findings = find(&config(dir.path()), &Reporter::silent()).unwrap();
        assert!(findings.is_empty());
    }

    #[test]
    fn a_sibling_with_content_does_not_protect_an_empty_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("full")).unwrap();
        std::fs::write(dir.path().join("full/a.txt"), b"x").unwrap();
        std::fs::create_dir_all(dir.path().join("hollow")).unwrap();

        let findings = find(&config(dir.path()), &Reporter::silent()).unwrap();
        assert_eq!(findings.len(), 1);
        assert!(findings[0].path.ends_with("hollow"));
    }
}
