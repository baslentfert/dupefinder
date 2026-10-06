//! What to actually do with the duplicates.
//!
//! Everything here is destructive, so the design is defensive by default:
//!
//! * A plan is validated as a whole before a single file is touched, and a
//!   group that would lose every copy is rejected outright.
//! * [`crate::config::is_protected`] is consulted again at execution time,
//!   independently of whatever the scan filters allowed.
//! * The default action is the recycle bin, not deletion.
//! * Replacing a copy with a hard link never deletes before the link exists.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::is_protected;
use crate::model::{DuplicateGroup, FileEntry, SimilarGroup};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Action {
    /// Recoverable from the recycle bin. The default, and the only action
    /// offered without an extra confirmation.
    Recycle,
    /// Unlink immediately. Not recoverable.
    DeletePermanently,
    /// Relocate into a quarantine folder, preserving the relative path so the
    /// move can be undone by hand.
    #[serde(rename_all = "camelCase")]
    MoveTo { destination: PathBuf },
    /// Free the space but keep every path working, by pointing the duplicate
    /// at the keeper's data. Same volume only.
    ReplaceWithHardLink,
}

impl Action {
    pub fn is_reversible(&self) -> bool {
        matches!(self, Action::Recycle | Action::MoveTo { .. })
    }
}

/// One group's decision: which copy stays, which copies go.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupPlan {
    pub group_id: u64,
    pub keep: PathBuf,
    pub remove: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub action: Action,
    pub groups: Vec<GroupPlan>,
}

impl Plan {
    pub fn files_affected(&self) -> usize {
        self.groups.iter().map(|g| g.remove.len()).sum()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub path: PathBuf,
    pub ok: bool,
    pub error: Option<String>,
    pub bytes_freed: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionReport {
    pub outcomes: Vec<Outcome>,
    pub succeeded: u64,
    pub failed: u64,
    pub bytes_freed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Violation {
    pub group_id: u64,
    pub path: Option<PathBuf>,
    pub reason: String,
}

/// Check a plan without touching anything.
///
/// Returns every problem found rather than the first, so the UI can show the
/// user the whole picture in one go.
pub fn validate(plan: &Plan) -> Vec<Violation> {
    let mut violations = Vec::new();

    for group in &plan.groups {
        if group.remove.is_empty() {
            continue;
        }

        if group.remove.iter().any(|p| p == &group.keep) {
            violations.push(Violation {
                group_id: group.group_id,
                path: Some(group.keep.clone()),
                reason: "the kept file is also marked for removal".into(),
            });
        }

        if !group.keep.exists() {
            violations.push(Violation {
                group_id: group.group_id,
                path: Some(group.keep.clone()),
                reason: "the file to keep no longer exists".into(),
            });
        }

        for path in &group.remove {
            if is_protected(path) {
                violations.push(Violation {
                    group_id: group.group_id,
                    path: Some(path.clone()),
                    reason: "system location, refusing to modify".into(),
                });
            }
        }

        if let Action::ReplaceWithHardLink = plan.action {
            for path in &group.remove {
                if !same_volume(&group.keep, path) {
                    violations.push(Violation {
                        group_id: group.group_id,
                        path: Some(path.clone()),
                        reason: "hard links cannot cross volumes".into(),
                    });
                }
            }
        }
    }

    violations
}

/// Execute a validated plan.
///
/// Refuses to start if validation fails: a partially applied plan is much
/// worse than one that never ran.
pub fn apply(plan: &Plan) -> std::result::Result<ActionReport, Vec<Violation>> {
    let violations = validate(plan);
    if !violations.is_empty() {
        return Err(violations);
    }

    let mut report = ActionReport::default();

    for group in &plan.groups {
        for path in &group.remove {
            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            let result = execute_one(&plan.action, &group.keep, path);

            match result {
                Ok(()) => {
                    report.succeeded += 1;
                    report.bytes_freed += size;
                    report.outcomes.push(Outcome {
                        path: path.clone(),
                        ok: true,
                        error: None,
                        bytes_freed: size,
                    });
                }
                Err(err) => {
                    report.failed += 1;
                    report.outcomes.push(Outcome {
                        path: path.clone(),
                        ok: false,
                        error: Some(err),
                        bytes_freed: 0,
                    });
                }
            }
        }
    }

    Ok(report)
}

fn execute_one(action: &Action, keep: &Path, path: &Path) -> std::result::Result<(), String> {
    match action {
        Action::Recycle => trash::delete(path).map_err(|e| e.to_string()),

        Action::DeletePermanently => std::fs::remove_file(path).map_err(|e| e.to_string()),

        Action::MoveTo { destination } => move_into(destination, path),

        Action::ReplaceWithHardLink => relink(keep, path),
    }
}

/// Move `path` under `destination`, keeping enough of the original path to
/// stay unambiguous and to allow a manual undo.
fn move_into(destination: &Path, path: &Path) -> std::result::Result<(), String> {
    let relative = flatten_path(path);
    let target = destination.join(relative);

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let target = unique_target(&target);

    // A rename across volumes fails; fall back to copy-then-delete.
    match std::fs::rename(path, &target) {
        Ok(()) => Ok(()),
        Err(rename_error) => {
            if path.is_dir() {
                // Copying a tree by hand risks a half-moved directory, which
                // is worse than not moving it. Say so instead.
                return Err(format!(
                    "cannot move {} to another volume ({rename_error}); \
                     choose a quarantine folder on the same drive",
                    path.display()
                ));
            }
            std::fs::copy(path, &target).map_err(|e| e.to_string())?;
            std::fs::remove_file(path).map_err(|e| e.to_string())
        }
    }
}

/// `C:\Users\me\a.jpg` becomes `C\Users\me\a.jpg`, so the quarantine folder
/// mirrors the original layout without an illegal drive-letter colon.
fn flatten_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy().replace(':', "");
    PathBuf::from(text.trim_start_matches(['\\', '/']))
}

fn unique_target(target: &Path) -> PathBuf {
    if !target.exists() {
        return target.to_path_buf();
    }

    let stem = target
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let extension = target.extension().map(|e| e.to_string_lossy().to_string());
    let parent = target.parent().unwrap_or(Path::new("."));

    for n in 1..10_000 {
        let name = match &extension {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }

    target.to_path_buf()
}

/// Replace `path` with a hard link to `keep`.
///
/// The original is renamed aside first and only removed once the link exists,
/// so an interruption at any point leaves the data reachable under one name
/// or the other — never neither.
fn relink(keep: &Path, path: &Path) -> std::result::Result<(), String> {
    let backup = path.with_extension(format!(
        "{}.dupefinder-old",
        path.extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_default()
    ));

    std::fs::rename(path, &backup).map_err(|e| e.to_string())?;

    match std::fs::hard_link(keep, path) {
        Ok(()) => {
            // The backup is now redundant: `path` and `keep` share the data.
            std::fs::remove_file(&backup).map_err(|e| e.to_string())
        }
        Err(err) => {
            let _ = std::fs::rename(&backup, path);
            Err(format!("could not create hard link: {err}"))
        }
    }
}

fn same_volume(a: &Path, b: &Path) -> bool {
    use crate::scan::fileid::file_id;

    match (file_id(a), file_id(b)) {
        (Some(a), Some(b)) => a.volume == b.volume,
        // Fall back to comparing the path prefix; better a false "different
        // volume" (which only blocks a hard link) than a false "same".
        _ => volume_prefix(a) == volume_prefix(b) && volume_prefix(a).is_some(),
    }
}

fn volume_prefix(path: &Path) -> Option<String> {
    let text = path.to_string_lossy();
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' {
        return Some(text[..1].to_ascii_uppercase());
    }
    None
}

/// Removal of orphan findings.
///
/// Kept separate from [`Plan`] because the invariants differ: there is no
/// "keeper" to protect, entries may be whole directories, and the installer
/// cache needs a narrow exemption from the system-path rule that duplicate
/// removal must never have.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrphanPlan {
    pub action: Action,
    pub paths: Vec<PathBuf>,
    /// Ask Windows for a system restore point before touching anything.
    ///
    /// Only meaningful here, and only for part of what this plan can remove:
    /// a restore point covers programs and the registry, which is what the
    /// installer cache and leftover application data belong to. It does not
    /// cover user data, which is why the duplicate and photo cleanup does not
    /// offer it.
    #[serde(default)]
    pub create_restore_point: bool,
}

pub fn validate_orphans(plan: &OrphanPlan) -> Vec<Violation> {
    let mut violations = Vec::new();

    if let Action::ReplaceWithHardLink = plan.action {
        violations.push(Violation {
            group_id: 0,
            path: None,
            reason: "orphans have nothing to link to".into(),
        });
        return violations;
    }

    for path in &plan.paths {
        if is_protected(path) && !crate::orphans::installer::is_installer_cache_package(path) {
            violations.push(Violation {
                group_id: 0,
                path: Some(path.clone()),
                reason: "system location, refusing to modify".into(),
            });
        }

        if !path.exists() {
            violations.push(Violation {
                group_id: 0,
                path: Some(path.clone()),
                reason: "no longer exists".into(),
            });
        }
    }

    violations
}

pub fn apply_orphans(plan: &OrphanPlan) -> std::result::Result<ActionReport, Vec<Violation>> {
    let violations = validate_orphans(plan);
    if !violations.is_empty() {
        return Err(violations);
    }

    // Held for the whole operation: dropping it is what tells Windows the
    // change is complete.
    let _restore_point = if plan.create_restore_point {
        let description = format!("Duplicate Finder: removing {} orphans", plan.paths.len());

        match crate::restore_point::begin(&description) {
            crate::restore_point::Outcome::Created(point) => Some(point),

            // Asked for a net and it could not be strung: do not jump. Going
            // ahead anyway would leave the user believing they are covered
            // when they are not, which is worse than refusing.
            crate::restore_point::Outcome::Unavailable(state) => {
                return Err(vec![Violation {
                    group_id: 0,
                    path: None,
                    reason: format!("no restore point was made, so nothing was removed. {state}"),
                }]);
            }
            crate::restore_point::Outcome::Failed { message, .. } => {
                return Err(vec![Violation {
                    group_id: 0,
                    path: None,
                    reason: format!("no restore point was made, so nothing was removed: {message}"),
                }]);
            }
        }
    } else {
        None
    };

    let mut report = ActionReport::default();

    for path in &plan.paths {
        let is_directory = path.is_dir();
        let size = if is_directory {
            crate::orphans::measure_directory(path).0
        } else {
            std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
        };

        let result = match &plan.action {
            Action::Recycle => trash::delete(path).map_err(|e| e.to_string()),
            Action::DeletePermanently => if is_directory {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_file(path)
            }
            .map_err(|e| e.to_string()),
            Action::MoveTo { destination } => move_into(destination, path),
            // Rejected during validation.
            Action::ReplaceWithHardLink => Err("not applicable to orphans".to_string()),
        };

        match result {
            Ok(()) => {
                report.succeeded += 1;
                report.bytes_freed += size;
                report.outcomes.push(Outcome {
                    path: path.clone(),
                    ok: true,
                    error: None,
                    bytes_freed: size,
                });
            }
            Err(error) => {
                report.failed += 1;
                report.outcomes.push(Outcome {
                    path: path.clone(),
                    ok: false,
                    error: Some(error),
                    bytes_freed: 0,
                });
            }
        }
    }

    Ok(report)
}

/// Rules for picking which copy to keep, applied by the UI's "select
/// automatically" button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeepRule {
    /// Oldest modification time — usually the original rather than a re-save.
    Oldest,
    Newest,
    /// Shortest path: files in `Pictures` beat files in
    /// `Downloads\zip\extracted\copy of`.
    ShortestPath,
    /// Highest resolution, for image clusters.
    HighestResolution,
}

/// Choose the file to keep from an exact-duplicate group.
pub fn choose_keeper(group: &DuplicateGroup, rule: KeepRule) -> Option<&FileEntry> {
    match rule {
        KeepRule::Oldest => group.files.iter().min_by_key(|f| f.mtime),
        KeepRule::Newest => group.files.iter().max_by_key(|f| f.mtime),
        // Identical bytes, so resolution cannot separate them either.
        KeepRule::ShortestPath | KeepRule::HighestResolution => group
            .files
            .iter()
            .min_by_key(|f| (f.path.as_os_str().len(), f.path.clone())),
    }
}

/// Choose the image to keep from a similarity cluster.
pub fn choose_image_keeper(
    group: &SimilarGroup,
    rule: KeepRule,
) -> Option<&crate::model::SimilarImage> {
    match rule {
        KeepRule::HighestResolution => group
            .images
            .iter()
            .max_by_key(|i| (i.pixels(), i.file.size)),
        // Prefer the EXIF capture time when present: a copy re-saved by a
        // messaging app keeps the capture date but gets a new mtime.
        KeepRule::Oldest => group
            .images
            .iter()
            .min_by_key(|i| i.taken_at.unwrap_or(i.file.mtime)),
        KeepRule::Newest => group
            .images
            .iter()
            .max_by_key(|i| i.taken_at.unwrap_or(i.file.mtime)),
        KeepRule::ShortestPath => group
            .images
            .iter()
            .min_by_key(|i| (i.file.path.as_os_str().len(), i.file.path.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ContentHash;

    fn entry(path: &str, mtime: i64) -> FileEntry {
        FileEntry {
            path: PathBuf::from(path),
            size: 100,
            mtime,
            file_id: None,
        }
    }

    #[test]
    fn a_plan_that_keeps_nothing_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.bin");
        std::fs::write(&a, b"x").unwrap();

        let plan = Plan {
            action: Action::DeletePermanently,
            groups: vec![GroupPlan {
                group_id: 1,
                keep: a.clone(),
                remove: vec![a.clone()],
            }],
        };

        let violations = validate(&plan);
        assert!(violations.iter().any(|v| v.reason.contains("also marked")));
        assert!(a.exists(), "nothing was touched");
    }

    #[test]
    fn system_paths_are_refused_even_when_explicitly_planned() {
        let plan = Plan {
            action: Action::DeletePermanently,
            groups: vec![GroupPlan {
                group_id: 1,
                keep: PathBuf::from(r"C:\Users\me\a.dll"),
                remove: vec![PathBuf::from(r"C:\Windows\System32\a.dll")],
            }],
        };
        assert!(validate(&plan)
            .iter()
            .any(|v| v.reason.contains("system location")));
    }

    #[test]
    fn moving_preserves_the_original_layout() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = dir.path().join("src");
        std::fs::create_dir_all(&source_dir).unwrap();
        let file = source_dir.join("a.bin");
        std::fs::write(&file, b"payload").unwrap();

        let quarantine = dir.path().join("quarantine");
        move_into(&quarantine, &file).unwrap();

        assert!(!file.exists());
        let moved = quarantine.join(flatten_path(&file));
        assert!(moved.exists(), "expected {moved:?}");
        assert_eq!(std::fs::read(&moved).unwrap(), b"payload");
    }

    #[test]
    fn a_second_move_does_not_overwrite_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let quarantine = dir.path().join("q");
        let target = quarantine.join("a.bin");
        std::fs::create_dir_all(&quarantine).unwrap();
        std::fs::write(&target, b"first").unwrap();

        let unique = unique_target(&target);
        assert_ne!(unique, target);
        assert!(unique.to_string_lossy().contains("(1)"));
    }

    #[test]
    fn hard_linking_leaves_both_paths_readable() {
        let dir = tempfile::tempdir().unwrap();
        let keep = dir.path().join("keep.bin");
        let dup = dir.path().join("dup.bin");
        std::fs::write(&keep, b"same").unwrap();
        std::fs::write(&dup, b"same").unwrap();

        if relink(&keep, &dup).is_ok() {
            assert_eq!(std::fs::read(&dup).unwrap(), b"same");
            assert_eq!(std::fs::read(&keep).unwrap(), b"same");
            assert!(!dir.path().join("dup.bin.dupefinder-old").exists());
        }
    }

    #[test]
    fn a_restore_point_that_cannot_be_made_stops_the_removal() {
        let dir = tempfile::tempdir().unwrap();
        let doomed = dir.path().join("leftover.bin");
        std::fs::write(&doomed, b"x").unwrap();

        let plan = OrphanPlan {
            action: Action::DeletePermanently,
            paths: vec![doomed.clone()],
            create_restore_point: true,
        };

        match apply_orphans(&plan) {
            // On an elevated machine with protection on, the point is made and
            // the file goes. Anywhere else the refusal must be total.
            Ok(_) => assert!(crate::restore_point::availability().is_ready()),
            Err(violations) => {
                assert!(
                    violations
                        .iter()
                        .any(|v| v.reason.contains("nothing was removed")),
                    "unclear refusal: {violations:?}"
                );
                assert!(doomed.exists(), "the file was removed despite the refusal");
            }
        }
    }

    #[test]
    fn without_a_restore_point_requested_nothing_blocks_the_removal() {
        let dir = tempfile::tempdir().unwrap();
        let doomed = dir.path().join("leftover.bin");
        std::fs::write(&doomed, b"x").unwrap();

        let report = apply_orphans(&OrphanPlan {
            action: Action::DeletePermanently,
            paths: vec![doomed.clone()],
            create_restore_point: false,
        })
        .expect("plan should be valid");

        assert_eq!(report.succeeded, 1);
        assert!(!doomed.exists());
    }

    #[test]
    fn the_oldest_copy_is_kept_by_default() {
        let group = DuplicateGroup {
            id: 1,
            hash: ContentHash([0; 32]),
            size: 100,
            files: vec![
                entry(r"D:\downloads\copy of a.jpg", 2000),
                entry(r"D:\photos\a.jpg", 1000),
            ],
        };

        let keeper = choose_keeper(&group, KeepRule::Oldest).unwrap();
        assert_eq!(keeper.path, PathBuf::from(r"D:\photos\a.jpg"));
    }
}
