//! Near-duplicate image detection.

pub mod cluster;
pub mod decode;
pub mod phash;

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use rayon::prelude::*;

use crate::cache::{HashCache, ImageRecord};
use crate::config::ScanConfig;
use crate::error::Result;
use crate::model::{FileEntry, SimilarGroup, SimilarImage};
use crate::progress::{Phase, Reporter, ScanEvent, Summary, Throttle};

use cluster::{BkTree, DisjointSet};
use phash::hamming;

/// Extra slack allowed on the gradient hash when confirming a pHash match.
///
/// dHash and pHash disagree on different things, so demanding the same
/// tightness from both would throw away real matches; demanding nothing from
/// dHash lets flat images (scans, screenshots of text) collapse into one huge
/// bogus cluster.
const DHASH_SLACK: u32 = 6;

pub fn find(
    cfg: &ScanConfig,
    entries: &mut [FileEntry],
    reporter: &Reporter,
    cache: Option<&HashCache>,
    summary: &mut Summary,
) -> Result<Vec<SimilarGroup>> {
    let images: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| decode::is_image_extension(&entry.path))
        .map(|(i, _)| i)
        .collect();

    if images.len() < 2 {
        return Ok(Vec::new());
    }

    // Needed for cache keys; the exact pipeline has usually filled most of
    // these in already, so only the stragglers cost a handle.
    let unresolved: Vec<usize> = images
        .iter()
        .copied()
        .filter(|&i| entries[i].file_id.is_none())
        .collect();
    if !unresolved.is_empty() {
        crate::exact::resolve_ids(entries, &unresolved);
    }

    reporter.phase(Phase::HashingImages);
    let started = std::time::Instant::now();
    let mut hashed = hash_stage(entries, &images, reporter, cache, summary)?;
    tracing::info!(
        images = images.len(),
        ms = started.elapsed().as_millis() as u64,
        "hashed images"
    );

    // Sprites and emoji are smaller than the hash grid can describe, so they
    // all look alike to it. Dropping them here rather than before hashing
    // keeps the cache useful: the dimensions are only known after a decode.
    hashed.retain(|(_, record)| record.width.min(record.height) >= cfg.min_image_side);

    if hashed.len() < 2 {
        return Ok(Vec::new());
    }
    reporter.check_cancelled()?;

    reporter.phase(Phase::ClusteringImages);
    let started = std::time::Instant::now();
    let groups = build_clusters(entries, hashed, cfg.similarity_threshold);
    tracing::info!(
        clusters = groups.len(),
        ms = started.elapsed().as_millis() as u64,
        "clustered images"
    );
    Ok(groups)
}

fn hash_stage(
    entries: &[FileEntry],
    images: &[usize],
    reporter: &Reporter,
    cache: Option<&HashCache>,
    summary: &mut Summary,
) -> Result<Vec<(usize, ImageRecord)>> {
    let done = AtomicU64::new(0);
    let hits = AtomicU64::new(0);
    let total = images.len() as u64;
    let throttle = Throttle::new(Duration::from_millis(120));

    let out: Vec<(usize, ImageRecord)> = images
        .par_iter()
        .filter_map(|&i| {
            if reporter.is_cancelled() {
                return None;
            }
            let entry = &entries[i];
            let key = entry.cache_key();

            let cached = key
                .as_ref()
                .and_then(|k| cache.and_then(|c| c.get_image(k)));

            let record = match cached {
                Some(record) => {
                    hits.fetch_add(1, Ordering::Relaxed);
                    Some(record)
                }
                None => match decode::load(&entry.path) {
                    Ok(thumb) => {
                        let record = ImageRecord {
                            dhash: phash::dhash(&thumb),
                            phash: phash::phash(&thumb),
                            width: thumb.width,
                            height: thumb.height,
                        };
                        if let (Some(k), Some(c)) = (key.as_ref(), cache) {
                            c.put_image(k, record);
                        }
                        Some(record)
                    }
                    Err(err) => {
                        // A file that cannot be decoded is not a failure of
                        // the scan: it may be a raw format with no codec
                        // installed, or simply corrupt.
                        reporter.skipped(entry.path.clone(), err.to_string());
                        None
                    }
                },
            };

            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if throttle.ready() {
                reporter.send(ScanEvent::Progress {
                    phase: Phase::HashingImages,
                    done: n,
                    total,
                    bytes_done: 0,
                    bytes_total: 0,
                });
            }

            record.map(|r| (i, r))
        })
        .collect();

    reporter.check_cancelled()?;
    summary.cache_hits += hits.load(Ordering::Relaxed);

    Ok(out)
}

fn build_clusters(
    entries: &[FileEntry],
    hashed: Vec<(usize, ImageRecord)>,
    threshold: u32,
) -> Vec<SimilarGroup> {
    let mut tree = BkTree::new();
    for (slot, (_, record)) in hashed.iter().enumerate() {
        tree.insert(record.phash, slot);
    }

    // Querying the tree is read-only and independent per image, so it fans out
    // across cores; only the union-find that consumes the answers has to be
    // serial, and that part is cheap.
    let matches: Vec<Vec<usize>> = (0..hashed.len())
        .into_par_iter()
        .map(|slot| {
            let record = &hashed[slot].1;
            let mut neighbours = Vec::new();
            tree.find(record.phash, threshold, &mut neighbours);

            neighbours.retain(|&other| {
                other != slot
                    // Both hashes must agree before two images are joined.
                    && hamming(record.dhash, hashed[other].1.dhash) <= threshold + DHASH_SLACK
            });
            neighbours
        })
        .collect();

    let mut set = DisjointSet::new(hashed.len());
    for (slot, neighbours) in matches.into_iter().enumerate() {
        for other in neighbours {
            set.union(slot, other);
        }
    }

    let clusters = set.groups();

    // Every image that made it into a cluster needs its EXIF capture date, and
    // each one is a separate file open. Reading them in the serial loop below
    // made clustering wait on the disk one photo at a time; doing them all up
    // front lets rayon overlap the I/O.
    let taken_at: HashMap<usize, Option<i64>> = clusters
        .par_iter()
        .flatten()
        .map(|&slot| {
            let index = hashed[slot].0;
            (slot, exif_taken_at(&entries[index].path))
        })
        .collect();

    let mut groups: Vec<SimilarGroup> = clusters
        .into_iter()
        .map(|members| {
            // The reference is the copy most worth keeping: most pixels, then
            // most bytes. Distances are reported against it so the UI can say
            // "this one is a 92% match of the original".
            let best = *members
                .iter()
                .max_by_key(|&&slot| {
                    let (index, record) = &hashed[slot];
                    (
                        record.width as u64 * record.height as u64,
                        entries[*index].size,
                    )
                })
                .expect("groups() never yields an empty set");
            let reference = hashed[best].1;

            let mut images: Vec<SimilarImage> = members
                .iter()
                .map(|&slot| {
                    let (index, record) = &hashed[slot];
                    SimilarImage {
                        file: entries[*index].clone(),
                        width: record.width,
                        height: record.height,
                        distance: hamming(reference.phash, record.phash),
                        taken_at: taken_at.get(&slot).copied().flatten(),
                    }
                })
                .collect();

            images.sort_by(|a, b| {
                a.distance
                    .cmp(&b.distance)
                    .then_with(|| b.pixels().cmp(&a.pixels()))
                    .then_with(|| a.file.path.cmp(&b.file.path))
            });

            SimilarGroup { id: 0, images }
        })
        .collect();

    // Biggest clusters first — most to gain, least to think about.
    groups.sort_by(|a, b| {
        b.images
            .len()
            .cmp(&a.images.len())
            .then_with(|| a.images[0].file.path.cmp(&b.images[0].file.path))
    });
    for (i, group) in groups.iter_mut().enumerate() {
        group.id = i as u64 + 1;
    }

    groups
}

/// EXIF capture time, in seconds since the epoch.
///
/// Read only for images that made it into a cluster: it is the tiebreaker for
/// "which of these is the original", and paying for it on every image in the
/// library would not be worth it.
fn exif_taken_at(path: &Path) -> Option<i64> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(file);
    let exif = exif::Reader::new().read_from_container(&mut reader).ok()?;

    for tag in [exif::Tag::DateTimeOriginal, exif::Tag::DateTime] {
        let Some(field) = exif.get_field(tag, exif::In::PRIMARY) else {
            continue;
        };
        let exif::Value::Ascii(ref values) = field.value else {
            continue;
        };
        let Some(bytes) = values.first() else {
            continue;
        };
        if let Ok(datetime) = exif::DateTime::from_ascii(bytes) {
            return Some(naive_to_unix(&datetime));
        }
    }

    None
}

/// EXIF timestamps carry no timezone, so they are treated as UTC. That is
/// wrong by at most a few hours, which does not matter for "which of these
/// two files is older".
fn naive_to_unix(dt: &exif::DateTime) -> i64 {
    let year = dt.year as i64;
    let month = dt.month as i64;
    let day = dt.day as i64;

    // Days from the civil calendar, per Howard Hinnant's algorithm.
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;

    days * 86_400 + dt.hour as i64 * 3600 + dt.minute as i64 * 60 + dt.second as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_civil_calendar_conversion_matches_known_dates() {
        let epoch = exif::DateTime {
            year: 1970,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            nanosecond: None,
            offset: None,
        };
        assert_eq!(naive_to_unix(&epoch), 0);

        let y2k = exif::DateTime {
            year: 2000,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            nanosecond: None,
            offset: None,
        };
        assert_eq!(naive_to_unix(&y2k), 946_684_800);
    }
}
