//! Seeing duplicated folders instead of thousands of duplicated files.
//!
//! A project downloaded six times produces one duplicate group per file in it.
//! The user is then asked to make two thousand decisions when there is really
//! only one: *I have this folder six times, keep one.* Grouping the folders
//! turns the former into the latter.
//!
//! # This is a view, not a new operation
//!
//! Nothing here deletes anything or introduces a second way to. A folder group
//! resolves to ordinary per-file [`crate::actions::GroupPlan`]s — keep this
//! copy of the file, remove that one — so the validation, the protected paths
//! and the recycle bin all still apply exactly as they do on the file list. If
//! a folder ends up empty afterwards, the empty-folder detector will offer it.
//!
//! # Why unique files are counted
//!
//! Two folders holding the same *duplicated* content are not necessarily the
//! same folder: one of them may also hold something that exists nowhere else.
//! Treating those as interchangeable would lose that file. So every folder
//! carries a count of files that are unique to it, and a group where any
//! member has one is reported as a partial overlap rather than a copy.
//!
//! Establishing that is cheap: a file eliminated by size grouping has no
//! counterpart anywhere in the scan by definition, so it never needed hashing
//! to be known as unique.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::{ContentHash, DuplicateGroup, FileEntry};

/// Folders sharing fewer files than this are not a "duplicated folder", just
/// two folders that happen to hold a file in common.
const MIN_SHARED_FILES: usize = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderGroup {
    pub id: u64,
    pub folders: Vec<FolderCopy>,
    /// Files every folder in the group holds a copy of.
    pub shared_files: u64,
    /// Size of one copy of that shared content.
    pub shared_bytes: u64,
    /// True when no folder in the group holds anything the others do not, so
    /// keeping one of them loses nothing at all.
    pub interchangeable: bool,
}

impl FolderGroup {
    /// Bytes freed by reducing the shared content to a single copy.
    pub fn wasted_bytes(&self) -> u64 {
        self.shared_bytes * (self.folders.len().saturating_sub(1)) as u64
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderCopy {
    pub path: PathBuf,
    /// Every file directly in this folder.
    pub total_files: u64,
    pub total_bytes: u64,
    /// Files whose content the other folders in the group also hold.
    pub shared_files: u64,
    /// Files that exist nowhere else in the scan. Removing this folder would
    /// lose them, which is why a group containing any is not interchangeable.
    pub unique_files: u64,
}

/// Group folders whose duplicated content is identical.
///
/// Only the files directly in a folder count; a parent is not merged with its
/// children, because "this folder is a copy" should mean what it says.
pub fn group_folders(entries: &[FileEntry], duplicates: &[DuplicateGroup]) -> Vec<FolderGroup> {
    // Which files are duplicated, and of what content.
    let mut content: HashMap<&Path, ContentHash> = HashMap::new();
    for group in duplicates {
        for file in &group.files {
            content.insert(file.path.as_path(), group.hash);
        }
    }

    if content.is_empty() {
        return Vec::new();
    }

    #[derive(Default)]
    struct Stats {
        total_files: u64,
        total_bytes: u64,
        shared_files: u64,
        shared_bytes: u64,
        unique_files: u64,
        hashes: HashSet<ContentHash>,
    }

    let mut folders: HashMap<&Path, Stats> = HashMap::new();

    for entry in entries {
        let Some(parent) = entry.path.parent() else {
            continue;
        };
        let stats = folders.entry(parent).or_default();

        stats.total_files += 1;
        stats.total_bytes += entry.size;

        match content.get(entry.path.as_path()) {
            Some(hash) => {
                stats.shared_files += 1;
                // Counted once per distinct content: two copies of the same
                // file inside one folder are not two copies of the folder.
                if stats.hashes.insert(*hash) {
                    stats.shared_bytes += entry.size;
                }
            }
            None => stats.unique_files += 1,
        }
    }

    // Folders holding exactly the same set of duplicated content.
    let mut by_content: HashMap<Vec<ContentHash>, Vec<&Path>> = HashMap::new();
    for (path, stats) in &folders {
        if stats.hashes.len() < MIN_SHARED_FILES {
            continue;
        }
        // Sorted so the key is stable regardless of walk order.
        let mut key: Vec<ContentHash> = stats.hashes.iter().copied().collect();
        key.sort_unstable_by_key(|h| h.0);
        by_content.entry(key).or_default().push(path);
    }

    let mut groups: Vec<FolderGroup> = by_content
        .into_iter()
        .filter(|(_, paths)| paths.len() > 1)
        .map(|(hashes, mut paths)| {
            paths.sort_unstable();

            let copies: Vec<FolderCopy> = paths
                .iter()
                .map(|path| {
                    let stats = &folders[*path];
                    FolderCopy {
                        path: path.to_path_buf(),
                        total_files: stats.total_files,
                        total_bytes: stats.total_bytes,
                        shared_files: stats.shared_files,
                        unique_files: stats.unique_files,
                    }
                })
                .collect();

            let shared_bytes = folders[paths[0]].shared_bytes;

            FolderGroup {
                id: 0,
                shared_files: hashes.len() as u64,
                shared_bytes,
                interchangeable: copies.iter().all(|c| c.unique_files == 0),
                folders: copies,
            }
        })
        .collect();

    // Most to gain first, and a copy you can take wholesale ahead of one that
    // needs thought.
    groups.sort_by(|a, b| {
        b.interchangeable
            .cmp(&a.interchangeable)
            .then_with(|| b.wasted_bytes().cmp(&a.wasted_bytes()))
            .then_with(|| a.folders[0].path.cmp(&b.folders[0].path))
    });
    for (i, group) in groups.iter_mut().enumerate() {
        group.id = i as u64 + 1;
    }

    groups
}

/// Turn "keep this copy of the folder" into ordinary per-file plans.
///
/// The folder view never deletes a folder. It resolves to the same keep-this-
/// one, remove-that-one decisions the file list produces, so every existing
/// guard applies unchanged — and a file that exists only in a folder being
/// cleared is never touched, because it is not shared content.
pub fn plan_for_group(
    group: &FolderGroup,
    duplicates: &[DuplicateGroup],
    keep: &Path,
) -> Vec<crate::actions::GroupPlan> {
    let clearing: HashSet<&Path> = group
        .folders
        .iter()
        .map(|f| f.path.as_path())
        .filter(|p| *p != keep)
        .collect();

    if clearing.len() + 1 != group.folders.len() {
        // `keep` is not one of this group's folders; refuse rather than guess.
        return Vec::new();
    }

    duplicates
        .iter()
        .filter_map(|duplicate| {
            let keeper = duplicate
                .files
                .iter()
                .find(|f| f.path.parent() == Some(keep))?;

            let remove: Vec<PathBuf> = duplicate
                .files
                .iter()
                .filter(|f| {
                    f.path
                        .parent()
                        .is_some_and(|parent| clearing.contains(parent))
                })
                .map(|f| f.path.clone())
                .collect();

            if remove.is_empty() {
                return None;
            }

            Some(crate::actions::GroupPlan {
                group_id: duplicate.id,
                keep: keeper.path.clone(),
                remove,
            })
        })
        .collect()
}

/// Which copy of a folder to keep.
pub fn choose_keeper(group: &FolderGroup, rule: crate::actions::KeepRule) -> Option<&FolderCopy> {
    use crate::actions::KeepRule;

    match rule {
        // Anything the others do not have makes a folder the one to keep,
        // whatever the rule says; losing it is not a trade-off the user asked
        // for. Only then does the rule decide.
        _ if group.folders.iter().any(|f| f.unique_files > 0) => group
            .folders
            .iter()
            .max_by_key(|f| (f.unique_files, f.total_bytes)),

        KeepRule::ShortestPath | KeepRule::HighestResolution => group
            .folders
            .iter()
            .min_by_key(|f| (f.path.as_os_str().len(), f.path.clone())),

        // Folders have no modification time of their own that means anything
        // here, so fall back to the shallowest path.
        KeepRule::Oldest | KeepRule::Newest => group
            .folders
            .iter()
            .min_by_key(|f| (f.path.components().count(), f.path.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, size: u64) -> FileEntry {
        FileEntry {
            path: PathBuf::from(path),
            size,
            mtime: 0,
            file_id: None,
        }
    }

    fn group(id: u64, hash: u8, size: u64, paths: &[&str]) -> DuplicateGroup {
        DuplicateGroup {
            id,
            hash: ContentHash([hash; 32]),
            size,
            files: paths.iter().map(|p| entry(p, size)).collect(),
        }
    }

    #[test]
    fn two_copies_of_a_folder_become_one_decision() {
        let entries = vec![
            entry(r"C:\a\one.psd", 100),
            entry(r"C:\a\two.psd", 200),
            entry(r"C:\b\one.psd", 100),
            entry(r"C:\b\two.psd", 200),
        ];
        let duplicates = vec![
            group(1, 1, 100, &[r"C:\a\one.psd", r"C:\b\one.psd"]),
            group(2, 2, 200, &[r"C:\a\two.psd", r"C:\b\two.psd"]),
        ];

        let groups = group_folders(&entries, &duplicates);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].folders.len(), 2);
        assert_eq!(groups[0].shared_files, 2);
        assert_eq!(groups[0].shared_bytes, 300);
        assert_eq!(groups[0].wasted_bytes(), 300);
        assert!(groups[0].interchangeable);
    }

    #[test]
    fn a_folder_holding_something_unique_is_not_interchangeable() {
        let entries = vec![
            entry(r"C:\a\one.psd", 100),
            entry(r"C:\a\two.psd", 200),
            // Only in `b`, and of a size nothing else shares — so it never
            // reached the hashing stage and is unique by construction.
            entry(r"C:\b\only-here.psd", 999),
            entry(r"C:\b\one.psd", 100),
            entry(r"C:\b\two.psd", 200),
        ];
        let duplicates = vec![
            group(1, 1, 100, &[r"C:\a\one.psd", r"C:\b\one.psd"]),
            group(2, 2, 200, &[r"C:\a\two.psd", r"C:\b\two.psd"]),
        ];

        let groups = group_folders(&entries, &duplicates);

        assert_eq!(
            groups.len(),
            1,
            "they still share content, so still a group"
        );
        assert!(
            !groups[0].interchangeable,
            "deleting b wholesale would lose only-here.psd"
        );

        let b = groups[0]
            .folders
            .iter()
            .find(|f| f.path.ends_with("b"))
            .unwrap();
        assert_eq!(b.unique_files, 1);
        assert_eq!(b.shared_files, 2);
    }

    #[test]
    fn folders_sharing_only_one_file_are_not_a_duplicated_folder() {
        let entries = vec![
            entry(r"C:\a\shared.txt", 10),
            entry(r"C:\a\own-a.txt", 11),
            entry(r"C:\b\shared.txt", 10),
            entry(r"C:\b\own-b.txt", 12),
        ];
        let duplicates = vec![group(1, 1, 10, &[r"C:\a\shared.txt", r"C:\b\shared.txt"])];

        assert!(group_folders(&entries, &duplicates).is_empty());
    }

    #[test]
    fn folders_with_different_content_are_not_grouped() {
        let entries = vec![
            entry(r"C:\a\one.psd", 100),
            entry(r"C:\a\two.psd", 200),
            entry(r"C:\b\one.psd", 100),
            entry(r"C:\b\three.psd", 300),
            entry(r"C:\c\two.psd", 200),
            entry(r"C:\c\three.psd", 300),
        ];
        let duplicates = vec![
            group(1, 1, 100, &[r"C:\a\one.psd", r"C:\b\one.psd"]),
            group(2, 2, 200, &[r"C:\a\two.psd", r"C:\c\two.psd"]),
            group(3, 3, 300, &[r"C:\b\three.psd", r"C:\c\three.psd"]),
        ];

        // Every folder shares two files, but no two share the *same* two.
        assert!(group_folders(&entries, &duplicates).is_empty());
    }

    #[test]
    fn the_same_file_twice_in_one_folder_counts_once() {
        let entries = vec![
            entry(r"C:\a\one.psd", 100),
            entry(r"C:\a\one copy.psd", 100),
            entry(r"C:\a\two.psd", 200),
            entry(r"C:\b\one.psd", 100),
            entry(r"C:\b\one copy.psd", 100),
            entry(r"C:\b\two.psd", 200),
        ];
        let duplicates = vec![
            group(
                1,
                1,
                100,
                &[
                    r"C:\a\one.psd",
                    r"C:\a\one copy.psd",
                    r"C:\b\one.psd",
                    r"C:\b\one copy.psd",
                ],
            ),
            group(2, 2, 200, &[r"C:\a\two.psd", r"C:\b\two.psd"]),
        ];

        let groups = group_folders(&entries, &duplicates);

        assert_eq!(groups.len(), 1);
        // Two distinct contents, not three files.
        assert_eq!(groups[0].shared_files, 2);
        assert_eq!(groups[0].shared_bytes, 300);
        assert_eq!(groups[0].folders[0].shared_files, 3);
    }

    #[test]
    fn keeping_a_folder_produces_ordinary_file_plans() {
        let entries = vec![
            entry(r"C:\a\one.psd", 100),
            entry(r"C:\a\two.psd", 200),
            entry(r"C:\b\one.psd", 100),
            entry(r"C:\b\two.psd", 200),
        ];
        let duplicates = vec![
            group(1, 1, 100, &[r"C:\a\one.psd", r"C:\b\one.psd"]),
            group(2, 2, 200, &[r"C:\a\two.psd", r"C:\b\two.psd"]),
        ];

        let groups = group_folders(&entries, &duplicates);
        let plans = plan_for_group(&groups[0], &duplicates, Path::new(r"C:\a"));

        assert_eq!(plans.len(), 2);
        for plan in &plans {
            assert!(plan.keep.starts_with(r"C:\a"), "kept {:?}", plan.keep);
            assert_eq!(plan.remove.len(), 1);
            assert!(plan.remove[0].starts_with(r"C:\b"));
        }
    }

    #[test]
    fn a_file_only_in_the_cleared_folder_is_never_removed() {
        let entries = vec![
            entry(r"C:\a\one.psd", 100),
            entry(r"C:\a\two.psd", 200),
            entry(r"C:\b\one.psd", 100),
            entry(r"C:\b\two.psd", 200),
            entry(r"C:\b\only-here.psd", 999),
        ];
        let duplicates = vec![
            group(1, 1, 100, &[r"C:\a\one.psd", r"C:\b\one.psd"]),
            group(2, 2, 200, &[r"C:\a\two.psd", r"C:\b\two.psd"]),
        ];

        let groups = group_folders(&entries, &duplicates);
        let plans = plan_for_group(&groups[0], &duplicates, Path::new(r"C:\a"));

        let doomed: Vec<_> = plans.iter().flat_map(|p| &p.remove).collect();
        assert_eq!(doomed.len(), 2);
        assert!(
            !doomed.iter().any(|p| p.ends_with("only-here.psd")),
            "a file that exists nowhere else was scheduled for removal"
        );
    }

    #[test]
    fn a_keeper_outside_the_group_is_refused() {
        let entries = vec![
            entry(r"C:\a\one.psd", 100),
            entry(r"C:\a\two.psd", 200),
            entry(r"C:\b\one.psd", 100),
            entry(r"C:\b\two.psd", 200),
        ];
        let duplicates = vec![
            group(1, 1, 100, &[r"C:\a\one.psd", r"C:\b\one.psd"]),
            group(2, 2, 200, &[r"C:\a\two.psd", r"C:\b\two.psd"]),
        ];

        let groups = group_folders(&entries, &duplicates);
        assert!(plan_for_group(&groups[0], &duplicates, Path::new(r"C:\elsewhere")).is_empty());
    }

    #[test]
    fn the_folder_holding_something_unique_is_the_one_kept() {
        use crate::actions::KeepRule;

        let entries = vec![
            // The shorter path would normally win.
            entry(r"C:\a\one.psd", 100),
            entry(r"C:\a\two.psd", 200),
            entry(r"C:\deep\nested\b\one.psd", 100),
            entry(r"C:\deep\nested\b\two.psd", 200),
            entry(r"C:\deep\nested\b\only-here.psd", 999),
        ];
        let duplicates = vec![
            group(1, 1, 100, &[r"C:\a\one.psd", r"C:\deep\nested\b\one.psd"]),
            group(2, 2, 200, &[r"C:\a\two.psd", r"C:\deep\nested\b\two.psd"]),
        ];

        let groups = group_folders(&entries, &duplicates);
        let keeper = choose_keeper(&groups[0], KeepRule::ShortestPath).unwrap();

        assert!(
            keeper.path.ends_with("b"),
            "kept {:?}, losing the only copy of only-here.psd",
            keeper.path
        );
    }

    #[test]
    fn nothing_duplicated_means_no_folder_groups() {
        let entries = vec![entry(r"C:\a\one.psd", 100)];
        assert!(group_folders(&entries, &[]).is_empty());
    }
}
