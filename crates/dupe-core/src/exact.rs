//! The exact-duplicate pipeline.
//!
//! Each stage is a filter: it receives the files the previous stage could not
//! separate and hands on only those still in contention. That ordering is the
//! whole performance story — on a typical disk over 95% of files are ruled out
//! by size alone, without a single byte being read.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use rayon::prelude::*;

use crate::cache::HashCache;
use crate::config::ScanConfig;
use crate::error::Result;
use crate::hash::partial::SMALL_FILE_LIMIT;
use crate::hash::{full, partial};
use crate::model::{ContentHash, DuplicateGroup, FileEntry, FileId};
use crate::progress::{Phase, Reporter, ScanEvent, Summary, Throttle};
use crate::scan::fileid;

pub fn find(
    cfg: &ScanConfig,
    entries: &mut [FileEntry],
    reporter: &Reporter,
    cache: Option<&HashCache>,
    summary: &mut Summary,
) -> Result<Vec<DuplicateGroup>> {
    // Stage 1 — size.
    reporter.phase(Phase::GroupingBySize);
    let mut by_size: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, entry) in entries.iter().enumerate() {
        by_size.entry(entry.size).or_default().push(i);
    }
    let mut candidates: Vec<usize> = by_size
        .into_values()
        .filter(|group| group.len() > 1)
        .flatten()
        .collect();
    candidates.sort_unstable();

    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    reporter.check_cancelled()?;

    // Stage 2 — identity. Hard links are the same bytes reached twice; they
    // must not be offered for deletion, and this is also where the cache keys
    // come from.
    resolve_ids(entries, &candidates);
    let candidates = drop_hardlink_aliases(entries, candidates);

    // Stage 3 — partial hash. Files small enough that the partial read would
    // cover them entirely skip straight to the full hash instead of being
    // read twice.
    let (small, large): (Vec<usize>, Vec<usize>) = candidates
        .into_iter()
        .partition(|&i| entries[i].size <= SMALL_FILE_LIMIT);

    let mut survivors = small;
    if !large.is_empty() {
        reporter.phase(Phase::HashingPartial);
        survivors.extend(partial_stage(entries, &large, reporter)?);
    }
    if survivors.is_empty() {
        return Ok(Vec::new());
    }
    reporter.check_cancelled()?;

    // Stage 4 — full hash.
    reporter.phase(Phase::HashingFull);
    let hashed = full_stage(entries, &survivors, reporter, cache, summary)?;

    let mut by_hash: HashMap<ContentHash, Vec<usize>> = HashMap::new();
    for (index, hash) in hashed {
        by_hash.entry(hash).or_default().push(index);
    }

    let mut groups: Vec<DuplicateGroup> = Vec::new();
    for (hash, mut members) in by_hash {
        if members.len() < 2 {
            continue;
        }
        members.sort_unstable_by(|&a, &b| entries[a].path.cmp(&entries[b].path));
        groups.push(DuplicateGroup {
            id: 0,
            hash,
            size: entries[members[0]].size,
            files: members.into_iter().map(|i| entries[i].clone()).collect(),
        });
    }

    if cfg.verify_bytes {
        reporter.phase(Phase::VerifyingBytes);
        groups = verify_stage(groups, reporter);
    }

    // Largest waste first: that is the order a user wants to work through.
    groups.sort_by(|a, b| {
        b.wasted_bytes()
            .cmp(&a.wasted_bytes())
            .then_with(|| a.files[0].path.cmp(&b.files[0].path))
    });
    for (i, group) in groups.iter_mut().enumerate() {
        group.id = i as u64 + 1;
    }

    Ok(groups)
}

/// Fill in [`FileEntry::file_id`] for the given indices, in parallel.
pub(crate) fn resolve_ids(entries: &mut [FileEntry], indices: &[usize]) {
    let ids: Vec<Option<FileId>> = {
        let snapshot: &[FileEntry] = entries;
        indices
            .par_iter()
            .map(|&i| fileid::file_id(&snapshot[i].path))
            .collect()
    };
    for (&i, id) in indices.iter().zip(ids) {
        entries[i].file_id = id;
    }
}

/// Keep one path per physical file.
fn drop_hardlink_aliases(entries: &[FileEntry], candidates: Vec<usize>) -> Vec<usize> {
    let mut seen: HashMap<FileId, usize> = HashMap::new();
    let mut out = Vec::with_capacity(candidates.len());

    for i in candidates {
        match entries[i].file_id {
            Some(id) => {
                if let Some(&kept) = seen.get(&id) {
                    // Deterministic winner, so repeated scans agree on which
                    // path represents the file.
                    if entries[i].path < entries[kept].path {
                        if let Some(slot) = out.iter_mut().find(|s| **s == kept) {
                            *slot = i;
                        }
                        seen.insert(id, i);
                    }
                } else {
                    seen.insert(id, i);
                    out.push(i);
                }
            }
            // No id available (unusual: a locked or exotic file system).
            // Keeping it risks a hard-link pair being reported; dropping it
            // risks missing a real duplicate. Keep it — the user reviews
            // before anything is deleted.
            None => out.push(i),
        }
    }

    out.sort_unstable();
    out
}

fn partial_stage(
    entries: &[FileEntry],
    indices: &[usize],
    reporter: &Reporter,
) -> Result<Vec<usize>> {
    let done = AtomicU64::new(0);
    let total = indices.len() as u64;
    let throttle = Throttle::new(Duration::from_millis(120));

    let hashed: Vec<(usize, u64)> = indices
        .par_iter()
        .filter_map(|&i| {
            if reporter.is_cancelled() {
                return None;
            }
            let entry = &entries[i];
            let result = partial::partial_hash(&entry.path, entry.size);

            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if throttle.ready() {
                reporter.send(ScanEvent::Progress {
                    phase: Phase::HashingPartial,
                    done: n,
                    total,
                    bytes_done: 0,
                    bytes_total: 0,
                });
            }

            match result {
                Ok(hash) => Some((i, hash.value)),
                Err(err) => {
                    reporter.skipped(entry.path.clone(), err.to_string());
                    None
                }
            }
        })
        .collect();

    reporter.check_cancelled()?;

    let mut by_partial: HashMap<u64, Vec<usize>> = HashMap::new();
    for (index, hash) in hashed {
        by_partial.entry(hash).or_default().push(index);
    }

    Ok(by_partial
        .into_values()
        .filter(|group| group.len() > 1)
        .flatten()
        .collect())
}

fn full_stage(
    entries: &[FileEntry],
    indices: &[usize],
    reporter: &Reporter,
    cache: Option<&HashCache>,
    summary: &mut Summary,
) -> Result<Vec<(usize, ContentHash)>> {
    let done = AtomicU64::new(0);
    let bytes_done = AtomicU64::new(0);
    let hits = AtomicU64::new(0);
    let hashed_bytes = AtomicU64::new(0);
    let total = indices.len() as u64;
    let bytes_total: u64 = indices.iter().map(|&i| entries[i].size).sum();
    let throttle = Throttle::new(Duration::from_millis(120));

    // Duplicates are known long before the stage ends: the moment a second
    // file lands on a hash, that pair is confirmed. Tallying as we go costs a
    // map insert that the grouping below would do anyway, and turns minutes of
    // blank progress bar into a number the user can watch climb.
    let confirmed: dashmap::DashMap<ContentHash, u64> = dashmap::DashMap::new();
    let groups_found = AtomicU64::new(0);
    let files_found = AtomicU64::new(0);
    let wasted_found = AtomicU64::new(0);
    let tally_throttle = Throttle::new(Duration::from_millis(250));

    let out: Vec<(usize, ContentHash)> = indices
        .par_iter()
        .filter_map(|&i| {
            if reporter.is_cancelled() {
                return None;
            }
            let entry = &entries[i];
            let key = entry.cache_key();

            let cached = key
                .as_ref()
                .and_then(|k| cache.and_then(|c| c.get_content(k)));

            let hash = match cached {
                Some(hash) => {
                    hits.fetch_add(1, Ordering::Relaxed);
                    Some(hash)
                }
                None => match full::full_hash(&entry.path, entry.size) {
                    Ok(hash) => {
                        hashed_bytes.fetch_add(entry.size, Ordering::Relaxed);
                        if let (Some(k), Some(c)) = (key.as_ref(), cache) {
                            c.put_content(k, hash);
                        }
                        Some(hash)
                    }
                    Err(err) => {
                        reporter.skipped(entry.path.clone(), err.to_string());
                        None
                    }
                },
            };

            if let Some(hash) = hash {
                // Held only for the increment; `entry` locks one shard, not
                // the map.
                let seen = {
                    let mut count = confirmed.entry(hash).or_insert(0);
                    *count += 1;
                    *count
                };

                match seen {
                    // The second copy makes it a group, and both members
                    // count towards the waste.
                    2 => {
                        groups_found.fetch_add(1, Ordering::Relaxed);
                        files_found.fetch_add(2, Ordering::Relaxed);
                        wasted_found.fetch_add(entry.size, Ordering::Relaxed);
                    }
                    n if n > 2 => {
                        files_found.fetch_add(1, Ordering::Relaxed);
                        wasted_found.fetch_add(entry.size, Ordering::Relaxed);
                    }
                    _ => {}
                }

                if tally_throttle.ready() {
                    reporter.send(ScanEvent::FoundSoFar {
                        duplicate_groups: groups_found.load(Ordering::Relaxed),
                        duplicate_files: files_found.load(Ordering::Relaxed),
                        wasted_bytes: wasted_found.load(Ordering::Relaxed),
                    });
                }
            }

            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            let b = bytes_done.fetch_add(entry.size, Ordering::Relaxed) + entry.size;
            if throttle.ready() {
                reporter.send(ScanEvent::Progress {
                    phase: Phase::HashingFull,
                    done: n,
                    total,
                    bytes_done: b,
                    bytes_total,
                });
            }

            hash.map(|h| (i, h))
        })
        .collect();

    reporter.send(ScanEvent::FoundSoFar {
        duplicate_groups: groups_found.load(Ordering::Relaxed),
        duplicate_files: files_found.load(Ordering::Relaxed),
        wasted_bytes: wasted_found.load(Ordering::Relaxed),
    });
    reporter.check_cancelled()?;

    summary.files_hashed += total - hits.load(Ordering::Relaxed);
    summary.bytes_hashed += hashed_bytes.load(Ordering::Relaxed);
    summary.cache_hits += hits.load(Ordering::Relaxed);

    Ok(out)
}

/// Re-read every group member and compare against the first, discarding any
/// member that is not literally identical.
fn verify_stage(groups: Vec<DuplicateGroup>, reporter: &Reporter) -> Vec<DuplicateGroup> {
    groups
        .into_par_iter()
        .filter_map(|mut group| {
            let reference: PathBuf = group.files[0].path.clone();
            let mut kept = vec![group.files[0].clone()];

            for file in group.files.iter().skip(1) {
                match full::bytes_equal(&reference, &file.path) {
                    Ok(true) => kept.push(file.clone()),
                    Ok(false) => reporter
                        .skipped(file.path.clone(), "hash matched but bytes differ; excluded"),
                    Err(err) => reporter.skipped(file.path.clone(), err.to_string()),
                }
            }

            if kept.len() < 2 {
                return None;
            }
            group.files = kept;
            Some(group)
        })
        .collect()
}
