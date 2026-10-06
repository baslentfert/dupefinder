//! Progress reporting and cancellation.
//!
//! The engine never blocks on the consumer: events are sent on an unbounded
//! channel and dropped receivers are ignored, so a closed UI cannot stall a
//! running scan.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Enumerating,
    GroupingBySize,
    HashingPartial,
    HashingFull,
    VerifyingBytes,
    HashingImages,
    ClusteringImages,

    // The orphan detectors. They have no total to count towards — the work is
    // "look at everything and judge it" — so they report what they are looking
    // at rather than a percentage.
    ScanningShortcuts,
    ScanningEmptyFolders,
    ScanningAppData,
    ScanningInstallerCache,

    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ScanEvent {
    #[serde(rename_all = "camelCase")]
    PhaseStarted { phase: Phase },

    /// Emitted while walking, at most a few times per second.
    #[serde(rename_all = "camelCase")]
    Enumerated { files: u64, bytes: u64 },

    /// Work done inside the current phase.
    #[serde(rename_all = "camelCase")]
    Progress {
        phase: Phase,
        done: u64,
        total: u64,
        bytes_done: u64,
        bytes_total: u64,
    },

    /// The path currently under inspection.
    ///
    /// For work with no knowable total, a moving path is the honest progress
    /// indicator: it shows the scan is alive and where it has got to, without
    /// inventing a percentage.
    #[serde(rename_all = "camelCase")]
    Examining {
        phase: Phase,
        path: PathBuf,
        /// How many items this phase has looked at so far.
        count: u64,
    },

    /// Running tally of confirmed duplicates, while hashing is still going.
    ///
    /// A full-disk scan takes minutes, and until now every one of them was
    /// spent watching a bar with nothing to show for it — even though the
    /// answers were arriving all along. Counting is cheap enough to do on
    /// every match; only the reporting is throttled.
    #[serde(rename_all = "camelCase")]
    FoundSoFar {
        duplicate_groups: u64,
        duplicate_files: u64,
        wasted_bytes: u64,
    },

    /// A file could not be read. Scanning continues.
    #[serde(rename_all = "camelCase")]
    Skipped { path: PathBuf, reason: String },

    #[serde(rename_all = "camelCase")]
    Finished { summary: Summary },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub files_seen: u64,
    pub bytes_seen: u64,
    pub files_hashed: u64,
    pub bytes_hashed: u64,
    pub duplicate_groups: u64,
    pub duplicate_files: u64,
    pub wasted_bytes: u64,
    pub similar_groups: u64,
    pub similar_images: u64,
    pub skipped: u64,
    pub elapsed_ms: u64,
    /// True when hashes came from the cache for most files, i.e. a rescan.
    pub cache_hits: u64,
}

/// Individual skip notices sent before the reporter starts merely counting.
///
/// Scanning a whole disk produces one skip per unreadable file, and there can
/// be tens of thousands of those (permissions, locked files, reparse points).
/// Forwarding every one to a UI would cost more than the scan does, and nobody
/// reads the thirty-thousandth. The count in [`Summary::skipped`] stays exact.
const MAX_SKIP_DETAILS: u64 = 200;

/// Sink for progress events. Cloneable; every clone feeds the same consumer.
#[derive(Clone)]
pub struct Reporter {
    tx: Option<crossbeam_channel::Sender<ScanEvent>>,
    cancel: Arc<AtomicBool>,
    skips: Arc<std::sync::atomic::AtomicU64>,
}

impl Reporter {
    /// A reporter plus the receiver its events arrive on.
    pub fn channel() -> (Self, crossbeam_channel::Receiver<ScanEvent>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let reporter = Self {
            tx: Some(tx),
            cancel: Arc::new(AtomicBool::new(false)),
            skips: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        (reporter, rx)
    }

    /// A reporter that discards everything. Useful in tests and benchmarks.
    pub fn silent() -> Self {
        Self {
            tx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            skips: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    pub fn send(&self, event: ScanEvent) {
        if let Some(tx) = &self.tx {
            // A hung-up receiver means the caller stopped caring, which is not
            // an error for the scan itself.
            let _ = tx.send(event);
        }
    }

    pub fn phase(&self, phase: Phase) {
        self.send(ScanEvent::PhaseStarted { phase });
    }

    /// Report the item being inspected, at most once per `throttle` interval.
    pub(crate) fn examining(
        &self,
        throttle: &Throttle,
        phase: Phase,
        path: impl Into<PathBuf>,
        count: u64,
    ) {
        if throttle.ready() {
            self.send(ScanEvent::Examining {
                phase,
                path: path.into(),
                count,
            });
        }
    }

    /// Note a file that could not be processed.
    ///
    /// Only the first [`MAX_SKIP_DETAILS`] are described; the rest are counted.
    pub fn skipped(&self, path: impl Into<PathBuf>, reason: impl Into<String>) {
        let seen = self.skips.fetch_add(1, Ordering::Relaxed);
        if seen >= MAX_SKIP_DETAILS {
            return;
        }

        self.send(ScanEvent::Skipped {
            path: path.into(),
            reason: reason.into(),
        });
    }

    /// Total skips, including the ones that were not described.
    pub fn skip_count(&self) -> u64 {
        self.skips.load(Ordering::Relaxed)
    }

    /// Handle that other threads can use to stop the scan.
    pub fn cancel_handle(&self) -> CancelHandle {
        CancelHandle {
            flag: Arc::clone(&self.cancel),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// `Err(Cancelled)` once the scan has been asked to stop.
    pub fn check_cancelled(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone)]
pub struct CancelHandle {
    flag: Arc<AtomicBool>,
}

impl CancelHandle {
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }
}

/// Rate limiter so a scan of a million files does not emit a million events.
///
/// Lock-free: [`Throttle::ready`] is called once per file from every worker
/// thread, so a mutex here would be one of the hottest contended locks in the
/// program — for the sake of a progress bar.
pub(crate) struct Throttle {
    origin: std::time::Instant,
    /// Milliseconds since `origin` at the last emit, or [`NEVER`].
    last_ms: std::sync::atomic::AtomicU64,
    interval_ms: u64,
}

/// Sentinel for "has not fired yet", so the first call reports immediately
/// instead of leaving the UI blank for one interval.
const NEVER: u64 = u64::MAX;

impl Throttle {
    pub(crate) fn new(interval: std::time::Duration) -> Self {
        Self {
            origin: std::time::Instant::now(),
            last_ms: std::sync::atomic::AtomicU64::new(NEVER),
            interval_ms: interval.as_millis().max(1) as u64,
        }
    }

    /// True at most once per interval.
    ///
    /// Exactly one caller wins each interval: the compare-exchange makes sure
    /// two threads crossing the boundary together do not both report.
    pub(crate) fn ready(&self) -> bool {
        let now_ms = self.origin.elapsed().as_millis() as u64;
        let last = self.last_ms.load(Ordering::Relaxed);

        let due = last == NEVER || now_ms.saturating_sub(last) >= self.interval_ms;
        if !due {
            return false;
        }

        self.last_ms
            .compare_exchange(last, now_ms, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_first_skips_are_described() {
        let (reporter, events) = Reporter::channel();

        for i in 0..(MAX_SKIP_DETAILS + 500) {
            reporter.skipped(format!("C:/f{i}"), "denied");
        }
        drop(reporter);

        let described = events
            .into_iter()
            .filter(|e| matches!(e, ScanEvent::Skipped { .. }))
            .count() as u64;

        assert_eq!(described, MAX_SKIP_DETAILS);
    }

    #[test]
    fn the_skip_count_stays_exact() {
        let reporter = Reporter::silent();
        for i in 0..1000 {
            reporter.skipped(format!("C:/f{i}"), "denied");
        }
        assert_eq!(reporter.skip_count(), 1000);
    }

    #[test]
    fn the_throttle_lets_exactly_one_caller_through_per_interval() {
        let throttle = Throttle::new(std::time::Duration::from_millis(50));

        assert!(throttle.ready(), "the first call reports immediately");
        assert!(!throttle.ready(), "a second call in the same window");

        std::thread::sleep(std::time::Duration::from_millis(60));
        assert!(throttle.ready());
    }

    #[test]
    fn concurrent_callers_do_not_all_report() {
        let throttle = std::sync::Arc::new(Throttle::new(std::time::Duration::from_secs(60)));
        let allowed = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

        let threads: Vec<_> = (0..8)
            .map(|_| {
                let throttle = std::sync::Arc::clone(&throttle);
                let allowed = std::sync::Arc::clone(&allowed);
                std::thread::spawn(move || {
                    for _ in 0..1000 {
                        if throttle.ready() {
                            allowed.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                })
            })
            .collect();

        for thread in threads {
            thread.join().unwrap();
        }

        // A one-minute interval and a test that runs in milliseconds: exactly
        // one of the 8000 calls may win.
        assert_eq!(allowed.load(Ordering::Relaxed), 1);
    }
}
