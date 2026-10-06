//! Shared application state.
//!
//! Results live here rather than being pushed to the front end wholesale: a
//! full-disk scan can produce hundreds of thousands of entries, and shipping
//! that through IPC as one JSON blob would stall the webview for seconds. The
//! UI asks for pages instead.

use std::path::PathBuf;

use dupe_core::orphans::OrphanReport;
use dupe_core::{CancelHandle, ScanResult};
use parking_lot::{Mutex, RwLock};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Activity {
    Idle,
    Scanning,
    FindingOrphans,
}

pub struct AppState {
    activity: Mutex<ActivitySlot>,
    pub duplicates: RwLock<Option<ScanResult>>,
    /// Roots of the last scan, so a saved report can say what was looked at.
    pub last_roots: RwLock<Vec<PathBuf>>,
    pub orphans: RwLock<Option<OrphanReport>>,
    /// Where the persistent hash cache lives. Kept next to the app's other
    /// data so uninstalling takes it along.
    pub cache_path: PathBuf,
}

struct ActivitySlot {
    activity: Activity,
    cancel: Option<CancelHandle>,
}

impl AppState {
    pub fn new(cache_path: PathBuf) -> Self {
        Self {
            activity: Mutex::new(ActivitySlot {
                activity: Activity::Idle,
                cancel: None,
            }),
            duplicates: RwLock::new(None),
            last_roots: RwLock::new(Vec::new()),
            orphans: RwLock::new(None),
            cache_path,
        }
    }

    pub fn activity(&self) -> Activity {
        self.activity.lock().activity
    }

    /// Claim the engine for `activity`.
    ///
    /// Returns `false` when something is already running: two concurrent scans
    /// would fight over the cache and over the thread pool, and the UI has no
    /// way to show two progress bars anyway.
    pub fn begin(&self, activity: Activity, cancel: CancelHandle) -> bool {
        let mut slot = self.activity.lock();
        if slot.activity != Activity::Idle {
            return false;
        }
        slot.activity = activity;
        slot.cancel = Some(cancel);
        true
    }

    pub fn finish(&self) {
        let mut slot = self.activity.lock();
        slot.activity = Activity::Idle;
        slot.cancel = None;
    }

    /// Ask the running job to stop. No-op when nothing is running.
    pub fn cancel(&self) {
        if let Some(cancel) = &self.activity.lock().cancel {
            cancel.cancel();
        }
    }
}
