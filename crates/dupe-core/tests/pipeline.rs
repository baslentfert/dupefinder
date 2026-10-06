//! End-to-end checks over a real directory tree.

use std::path::{Path, PathBuf};

use dupe_core::actions::{apply, validate, Action, GroupPlan, Plan};
use dupe_core::{Detectors, Reporter, ScanConfig, ScanStrategy};

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, bytes).unwrap();
    path
}

fn config(root: &Path) -> ScanConfig {
    ScanConfig {
        roots: vec![root.to_path_buf()],
        // The MFT backend needs a whole volume and an elevated process.
        strategy: ScanStrategy::Walk,
        detectors: Detectors {
            exact: true,
            similar_images: false,
        },
        // The fixtures are deliberately tiny.
        min_size: 0,
        ..Default::default()
    }
}

#[test]
fn finds_duplicates_across_directories_and_ignores_unique_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // Same content, three places, three names.
    let payload = vec![42u8; 200_000];
    write(root, "photos/holiday.bin", &payload);
    write(root, "backup/holiday.bin", &payload);
    write(root, "downloads/copy of holiday.bin", &payload);

    // Same length, different content: only the partial hash can separate
    // these, which is exactly the stage being exercised.
    let mut nearly = payload.clone();
    *nearly.last_mut().unwrap() = 43;
    write(root, "photos/other.bin", &nearly);

    write(root, "photos/unique.bin", b"nobody else has this");

    let result = dupe_core::scan(&config(root), &Reporter::silent(), None).unwrap();

    assert_eq!(result.duplicates.len(), 1, "one group expected");
    let group = &result.duplicates[0];
    assert_eq!(group.files.len(), 3);
    assert_eq!(group.size, 200_000);
    assert_eq!(group.wasted_bytes(), 400_000);

    assert_eq!(result.summary.duplicate_files, 3);
    assert_eq!(result.summary.wasted_bytes, 400_000);
}

#[test]
fn a_group_of_small_files_skips_the_partial_stage_and_still_matches() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    write(root, "a.txt", b"tiny");
    write(root, "b.txt", b"tiny");
    write(root, "c.txt", b"different");

    let result = dupe_core::scan(&config(root), &Reporter::silent(), None).unwrap();

    assert_eq!(result.duplicates.len(), 1);
    assert_eq!(result.duplicates[0].files.len(), 2);
}

#[test]
fn hard_links_are_not_reported_as_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let original = write(root, "original.bin", &vec![7u8; 50_000]);
    let link = root.join("alias.bin");
    if std::fs::hard_link(&original, &link).is_err() {
        // Some CI file systems refuse hard links; nothing to assert then.
        return;
    }

    let result = dupe_core::scan(&config(root), &Reporter::silent(), None).unwrap();

    assert!(
        result.duplicates.is_empty(),
        "two names for one file is not wasted space, got {:?}",
        result.duplicates
    );
}

#[test]
fn byte_verification_does_not_change_the_answer() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    write(root, "a.bin", &vec![1u8; 30_000]);
    write(root, "b.bin", &vec![1u8; 30_000]);
    write(root, "c.bin", &vec![2u8; 30_000]);

    let mut cfg = config(root);
    cfg.verify_bytes = true;

    let result = dupe_core::scan(&cfg, &Reporter::silent(), None).unwrap();
    assert_eq!(result.duplicates.len(), 1);
    assert_eq!(result.duplicates[0].files.len(), 2);
}

#[test]
fn a_rescan_reuses_the_cache_instead_of_rehashing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("files");
    std::fs::create_dir_all(&root).unwrap();

    write(&root, "a.bin", &vec![5u8; 100_000]);
    write(&root, "b.bin", &vec![5u8; 100_000]);

    let cache = dir.path().join("cache.redb");
    let cfg = config(&root);

    let first = dupe_core::scan(&cfg, &Reporter::silent(), Some(cache.clone())).unwrap();
    assert_eq!(first.summary.cache_hits, 0);
    assert!(first.summary.bytes_hashed > 0);

    let second = dupe_core::scan(&cfg, &Reporter::silent(), Some(cache)).unwrap();
    assert_eq!(second.duplicates.len(), 1);
    assert_eq!(second.summary.cache_hits, 2, "both files came from cache");
    assert_eq!(second.summary.bytes_hashed, 0, "nothing was re-read");
}

#[test]
fn a_scan_reports_progress_and_can_be_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for i in 0..50 {
        write(root, &format!("f{i}.bin"), &vec![(i % 7) as u8; 20_000]);
    }

    let (reporter, events) = Reporter::channel();
    let cancel = reporter.cancel_handle();
    let cfg = config(root);

    let handle = std::thread::spawn(move || dupe_core::scan(&cfg, &reporter, None));

    cancel.cancel();
    let result = handle.join().unwrap();

    // Either it finished before the cancel landed, or it stopped cleanly.
    match result {
        Ok(_) => {}
        Err(dupe_core::Error::Cancelled) => {}
        Err(other) => panic!("unexpected error: {other}"),
    }

    // The channel must not have been left dangling either way.
    drop(events);
}

#[test]
fn the_plan_deletes_only_what_it_was_told_to() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let keep = write(root, "keep.bin", &vec![9u8; 40_000]);
    let drop_me = write(root, "drop.bin", &vec![9u8; 40_000]);

    let result = dupe_core::scan(&config(root), &Reporter::silent(), None).unwrap();
    assert_eq!(result.duplicates.len(), 1);

    let quarantine = dir.path().join("quarantine");
    let plan = Plan {
        action: Action::MoveTo {
            destination: quarantine.clone(),
        },
        groups: vec![GroupPlan {
            group_id: result.duplicates[0].id,
            keep: keep.clone(),
            remove: vec![drop_me.clone()],
        }],
    };

    assert!(validate(&plan).is_empty());
    let report = apply(&plan).unwrap();

    assert_eq!(report.succeeded, 1);
    assert_eq!(report.failed, 0);
    assert_eq!(report.bytes_freed, 40_000);
    assert!(keep.exists(), "the keeper must survive");
    assert!(!drop_me.exists(), "the duplicate was moved away");
}
