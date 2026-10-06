//! Everything the front end can call.
//!
//! Two conventions run through this module:
//!
//! * Long work never blocks the IPC thread. Scans run on their own thread and
//!   report through the `scan-event` / `orphan-event` channels.
//! * Destructive commands validate first and return the violations rather
//!   than throwing, so the UI can explain exactly what it refused to do.

use std::path::PathBuf;

use dupe_core::actions::{self, ActionReport, GroupPlan, KeepRule, OrphanPlan, Plan, Violation};
use dupe_core::orphans::{OrphanConfig, OrphanFinding, OrphanReport};
use dupe_core::platform::{self, VolumeInfo};
use dupe_core::progress::ScanEvent;
use dupe_core::{
    DuplicateGroup, FolderGroup, Reporter, ScanConfig, ScanStrategy, SimilarGroup, Summary,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::state::{Activity, AppState};

/// Result type for commands: the front end sees a rejected promise carrying
/// this string.
type CommandResult<T> = Result<T, String>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfo {
    pub elevated: bool,
    /// Whether the MFT fast path is usable right now.
    pub mft_available: bool,
    pub logical_cores: usize,
    pub volumes: Vec<VolumeInfo>,
    pub suggested_roots: Vec<SuggestedRoot>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestedRoot {
    pub label: String,
    pub path: PathBuf,
}

/// Called once the UI has mounted, so the log records real time-to-interactive
/// instead of time-to-window.
#[tauri::command]
pub async fn frontend_ready() {
    tracing::info!(
        elapsed_ms = crate::elapsed_since_start().as_millis() as u64,
        "front end ready"
    );
}

#[tauri::command]
pub async fn system_info() -> SystemInfo {
    let elevated = platform::is_elevated();

    // Enumerating drives probes every mounted device, so it happens once and
    // everything else is derived from the result. Asking twice used to double
    // the startup wait on machines with a cloud or network drive.
    let volumes = platform::volumes();
    let has_ntfs = volumes.iter().any(|v| v.is_ntfs && v.is_fixed);

    SystemInfo {
        elevated,
        // An elevated process can read the MFT, but only of an NTFS volume.
        mft_available: elevated && has_ntfs,
        logical_cores: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1),
        volumes,
        suggested_roots: suggested_roots(),
    }
}

/// The folders worth offering first: where duplicates actually accumulate.
fn suggested_roots() -> Vec<SuggestedRoot> {
    let Ok(profile) = std::env::var("USERPROFILE") else {
        return Vec::new();
    };
    let profile = PathBuf::from(profile);

    [
        ("Pictures", "Pictures"),
        ("Downloads", "Downloads"),
        ("Documents", "Documents"),
        ("Desktop", "Desktop"),
        ("Videos", "Videos"),
        ("Music", "Music"),
        ("OneDrive", "OneDrive"),
    ]
    .iter()
    .map(|(label, folder)| SuggestedRoot {
        label: (*label).to_string(),
        path: profile.join(folder),
    })
    .filter(|root| root.path.is_dir())
    .collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicatePage {
    pub groups: Vec<DuplicateGroup>,
    pub total: usize,
    pub summary: Summary,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimilarPage {
    pub groups: Vec<SimilarGroup>,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderPage {
    pub groups: Vec<FolderGroup>,
    pub total: usize,
    pub reclaimable_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrphanPage {
    pub findings: Vec<OrphanFinding>,
    pub total: usize,
    pub reclaimable_bytes: u64,
    pub unavailable: Vec<dupe_core::orphans::DetectorUnavailable>,
}

#[tauri::command]
pub async fn start_scan(
    app: AppHandle,
    state: State<'_, AppState>,
    config: ScanConfig,
) -> CommandResult<()> {
    if config.roots.is_empty() {
        return Err("choose at least one folder or drive to scan".into());
    }

    let (reporter, events) = Reporter::channel();
    if !state.begin(Activity::Scanning, reporter.cancel_handle()) {
        return Err("a scan is already running".into());
    }

    // Forward engine events to the webview. Runs on its own thread so a slow
    // front end cannot back-pressure the scan.
    {
        let app = app.clone();
        std::thread::spawn(move || {
            for event in events {
                let _ = app.emit("scan-event", &event);
            }
        });
    }

    *state.last_roots.write() = config.roots.clone();

    let cache_path = state.cache_path.clone();
    std::thread::spawn(move || {
        // Publishes the duplicates the moment they are final, so the user can
        // start working on them while the photo half is still running.
        let observer = PartialResults { app: app.clone() };
        let result =
            dupe_core::scan_observed(&config, &reporter, Some(cache_path), Some(&observer));
        let state = app.state::<AppState>();

        match result {
            Ok(scan) => {
                let summary = scan.summary.clone();
                *state.duplicates.write() = Some(scan);
                let _ = app.emit("scan-event", ScanEvent::Finished { summary });
            }
            Err(dupe_core::Error::Cancelled) => {
                let _ = app.emit("scan-cancelled", ());
            }
            Err(err) => {
                let _ = app.emit("scan-failed", err.to_string());
            }
        }

        state.finish();
    });

    Ok(())
}

/// Stores each half of a scan as it becomes final and tells the UI to reload.
struct PartialResults {
    app: AppHandle,
}

impl dupe_core::ScanObserver for PartialResults {
    fn duplicates_ready(&self, groups: &[DuplicateGroup], summary: &Summary) {
        let state = self.app.state::<AppState>();

        *state.duplicates.write() = Some(dupe_core::ScanResult {
            duplicates: groups.to_vec(),
            // Neither of these is known yet; the finished scan replaces the
            // whole result. Folder grouping needs the file list, which only
            // the engine still holds at this point.
            folders: Vec::new(),
            similar: Vec::new(),
            summary: summary.clone(),
        });

        let _ = self.app.emit("duplicates-ready", summary.duplicate_groups);
    }
}

#[tauri::command]
pub async fn start_orphan_scan(
    app: AppHandle,
    state: State<'_, AppState>,
    config: OrphanConfig,
) -> CommandResult<()> {
    let (reporter, events) = Reporter::channel();
    if !state.begin(Activity::FindingOrphans, reporter.cancel_handle()) {
        return Err("a scan is already running".into());
    }

    {
        let app = app.clone();
        std::thread::spawn(move || {
            for event in events {
                let _ = app.emit("orphan-event", &event);
            }
        });
    }

    std::thread::spawn(move || {
        let result = dupe_core::orphans::find(&config, &reporter);
        let state = app.state::<AppState>();

        match result {
            Ok(report) => {
                let count = report.findings.len();
                let bytes = report.reclaimable_bytes;
                *state.orphans.write() = Some(report);
                let _ = app.emit(
                    "orphan-finished",
                    serde_json::json!({
                        "findings": count,
                        "reclaimableBytes": bytes,
                    }),
                );
            }
            Err(dupe_core::Error::Cancelled) => {
                let _ = app.emit("scan-cancelled", ());
            }
            Err(err) => {
                let _ = app.emit("scan-failed", err.to_string());
            }
        }

        state.finish();
    });

    Ok(())
}

#[tauri::command]
pub async fn cancel_scan(state: State<'_, AppState>) -> CommandResult<()> {
    state.cancel();
    Ok(())
}

#[tauri::command]
pub async fn activity(state: State<'_, AppState>) -> CommandResult<Activity> {
    Ok(state.activity())
}

#[tauri::command]
pub async fn duplicate_page(
    state: State<'_, AppState>,
    offset: usize,
    limit: usize,
) -> CommandResult<DuplicatePage> {
    let guard = state.duplicates.read();
    let result = guard.as_ref().ok_or("no scan has been run yet")?;

    Ok(DuplicatePage {
        groups: page(&result.duplicates, offset, limit),
        total: result.duplicates.len(),
        summary: result.summary.clone(),
    })
}

#[tauri::command]
pub async fn similar_page(
    state: State<'_, AppState>,
    offset: usize,
    limit: usize,
) -> CommandResult<SimilarPage> {
    let guard = state.duplicates.read();
    let result = guard.as_ref().ok_or("no scan has been run yet")?;

    Ok(SimilarPage {
        groups: page(&result.similar, offset, limit),
        total: result.similar.len(),
    })
}

#[tauri::command]
pub async fn folder_page(
    state: State<'_, AppState>,
    offset: usize,
    limit: usize,
) -> CommandResult<FolderPage> {
    let guard = state.duplicates.read();
    let result = guard.as_ref().ok_or("no scan has been run yet")?;

    Ok(FolderPage {
        groups: page(&result.folders, offset, limit),
        total: result.folders.len(),
        reclaimable_bytes: result.folders.iter().map(|g| g.wasted_bytes()).sum(),
    })
}

/// File-level plans for keeping one copy of every duplicated folder.
///
/// Returned rather than applied, and expressed as the same per-file plans the
/// duplicate list produces — the folder view adds a way to decide, not a way
/// to delete.
#[tauri::command]
pub async fn suggest_folder_plan(
    state: State<'_, AppState>,
    rule: KeepRule,
) -> CommandResult<Vec<GroupPlan>> {
    let guard = state.duplicates.read();
    let result = guard.as_ref().ok_or("no scan has been run yet")?;

    Ok(result
        .folders
        .iter()
        .filter_map(|group| {
            let keeper = dupe_core::folders::choose_keeper(group, rule)?;
            Some(dupe_core::folders::plan_for_group(
                group,
                &result.duplicates,
                &keeper.path,
            ))
        })
        .flatten()
        .collect())
}

/// Plans for keeping one named copy of one duplicated folder.
///
/// Built here rather than in the front end because the files involved may sit
/// outside the page the UI has loaded; the engine always has all of them.
#[tauri::command]
pub async fn folder_plan(
    state: State<'_, AppState>,
    group_id: u64,
    keep: PathBuf,
) -> CommandResult<Vec<GroupPlan>> {
    let guard = state.duplicates.read();
    let result = guard.as_ref().ok_or("no scan has been run yet")?;

    let group = result
        .folders
        .iter()
        .find(|g| g.id == group_id)
        .ok_or("that folder group is no longer in the results")?;

    Ok(dupe_core::folders::plan_for_group(
        group,
        &result.duplicates,
        &keep,
    ))
}

#[tauri::command]
pub async fn orphan_page(
    state: State<'_, AppState>,
    offset: usize,
    limit: usize,
) -> CommandResult<OrphanPage> {
    let guard = state.orphans.read();
    let report = guard.as_ref().ok_or("no orphan scan has been run yet")?;

    Ok(OrphanPage {
        findings: page(&report.findings, offset, limit),
        total: report.findings.len(),
        reclaimable_bytes: report.reclaimable_bytes,
        unavailable: report.unavailable.clone(),
    })
}

fn page<T: Clone>(items: &[T], offset: usize, limit: usize) -> Vec<T> {
    items
        .iter()
        .skip(offset)
        .take(limit.clamp(1, 500))
        .cloned()
        .collect()
}

/// Build a removal plan for every duplicate group, keeping one copy per group
/// according to `rule`.
///
/// Returned rather than applied: the user sees exactly what would happen and
/// confirms it. Groups whose members all sit in protected locations are left
/// out entirely.
#[tauri::command]
pub async fn suggest_duplicate_plan(
    state: State<'_, AppState>,
    rule: KeepRule,
) -> CommandResult<Vec<GroupPlan>> {
    let guard = state.duplicates.read();
    let result = guard.as_ref().ok_or("no scan has been run yet")?;

    Ok(result
        .duplicates
        .iter()
        .filter_map(|group| {
            let keeper = actions::choose_keeper(group, rule)?;
            let remove: Vec<PathBuf> = group
                .files
                .iter()
                .filter(|f| f.path != keeper.path)
                .map(|f| f.path.clone())
                .collect();

            if remove.is_empty() {
                return None;
            }

            Some(GroupPlan {
                group_id: group.id,
                keep: keeper.path.clone(),
                remove,
            })
        })
        .collect())
}

#[tauri::command]
pub async fn suggest_similar_plan(
    state: State<'_, AppState>,
    rule: KeepRule,
) -> CommandResult<Vec<GroupPlan>> {
    let guard = state.duplicates.read();
    let result = guard.as_ref().ok_or("no scan has been run yet")?;

    Ok(result
        .similar
        .iter()
        .filter_map(|group| {
            let keeper = actions::choose_image_keeper(group, rule)?;
            let remove: Vec<PathBuf> = group
                .images
                .iter()
                .filter(|i| i.file.path != keeper.file.path)
                .map(|i| i.file.path.clone())
                .collect();

            if remove.is_empty() {
                return None;
            }

            Some(GroupPlan {
                group_id: group.id,
                keep: keeper.file.path.clone(),
                remove,
            })
        })
        .collect())
}

/// Dry run: what would this plan do, and what would it refuse to do?
#[tauri::command]
pub async fn check_plan(plan: Plan) -> Vec<Violation> {
    actions::validate(&plan)
}

/// Whether a restore point can be made, so the UI can say so before the user
/// commits rather than failing at the moment of truth.
#[tauri::command]
pub async fn restore_point_availability() -> dupe_core::restore_point::Availability {
    dupe_core::restore_point::availability()
}

#[tauri::command]
pub async fn check_orphan_plan(plan: OrphanPlan) -> Vec<Violation> {
    actions::validate_orphans(&plan)
}

#[tauri::command]
pub async fn apply_plan(app: AppHandle, plan: Plan) -> Result<ActionReport, Vec<Violation>> {
    let report = blocking(move || actions::apply(&plan)).await?;
    prune_removed(&app, &report);
    Ok(report)
}

#[tauri::command]
pub async fn apply_orphan_plan(
    app: AppHandle,
    plan: OrphanPlan,
) -> Result<ActionReport, Vec<Violation>> {
    let report = blocking(move || actions::apply_orphans(&plan)).await?;
    prune_removed(&app, &report);
    Ok(report)
}

/// Run file-system work on the blocking pool and flatten the two ways it
/// can fail into the violation list the front end already handles.
async fn blocking<F>(work: F) -> Result<ActionReport, Vec<Violation>>
where
    F: FnOnce() -> Result<ActionReport, Vec<Violation>> + Send + 'static,
{
    match tauri::async_runtime::spawn_blocking(work).await {
        Ok(result) => result,
        Err(err) => Err(vec![Violation {
            group_id: 0,
            path: None,
            reason: format!("the operation could not be started: {err}"),
        }]),
    }
}

/// Drop everything that was just deleted from the in-memory results, so the
/// UI does not offer to delete it a second time.
fn prune_removed(app: &AppHandle, report: &ActionReport) {
    let removed: std::collections::HashSet<&PathBuf> = report
        .outcomes
        .iter()
        .filter(|o| o.ok)
        .map(|o| &o.path)
        .collect();

    if removed.is_empty() {
        return;
    }

    let state = app.state::<AppState>();
    let mut duplicates = state.duplicates.write();
    let mut orphans = state.orphans.write();

    if let Some(result) = duplicates.as_mut() {
        for group in &mut result.duplicates {
            group.files.retain(|f| !removed.contains(&f.path));
        }
        // A group with one member left is no longer a duplicate.
        result.duplicates.retain(|g| g.files.len() > 1);

        for group in &mut result.similar {
            group.images.retain(|i| !removed.contains(&i.file.path));
        }
        result.similar.retain(|g| g.images.len() > 1);

        result.summary.duplicate_groups = result.duplicates.len() as u64;
        result.summary.duplicate_files =
            result.duplicates.iter().map(|g| g.files.len() as u64).sum();
        result.summary.wasted_bytes = result.duplicates.iter().map(|g| g.wasted_bytes()).sum();
        result.summary.similar_groups = result.similar.len() as u64;
        result.summary.similar_images = result.similar.iter().map(|g| g.images.len() as u64).sum();
    }

    if let Some(orphan_report) = orphans.as_mut() {
        orphan_report
            .findings
            .retain(|f| !removed.contains(&f.path));
        orphan_report.reclaimable_bytes = orphan_report.findings.iter().map(|f| f.size_bytes).sum();
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevealRequest {
    pub path: PathBuf,
}

/// Open Explorer with the file selected, which is what "show me this one"
/// actually means to a user.
#[tauri::command]
pub async fn reveal_in_explorer(request: RevealRequest) -> CommandResult<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Explorer parses its own command line, so the path goes through
        // verbatim rather than being quoted by the shell.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        std::process::Command::new("explorer.exe")
            .raw_arg(format!("/select,\"{}\"", request.path.display()))
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    #[cfg(not(windows))]
    Err("only supported on Windows".to_string())
}

/// Which scan strategy `config` would actually use, so the UI can say
/// "reading the MFT" instead of leaving the user guessing.
#[tauri::command]
pub async fn resolved_strategy(config: ScanConfig) -> ScanStrategy {
    dupe_core::scan::resolve_strategy(&config)
}

#[tauri::command]
pub async fn clear_results(state: State<'_, AppState>) -> CommandResult<()> {
    *state.duplicates.write() = None;
    *state.orphans.write() = None;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportExport {
    pub duplicates: Vec<DuplicateGroup>,
    pub similar: Vec<SimilarGroup>,
    pub orphans: Vec<OrphanFinding>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReportFormat {
    /// A self-contained page, styled for reading and for printing.
    Html,
    /// One row per file, for sorting and pivoting in a spreadsheet.
    Csv,
}

/// Write the findings to a file the user can keep, print or send on.
///
/// Rendering happens on the blocking pool: an HTML report embeds a preview per
/// photo, so it decodes images and can take a second or two.
#[tauri::command]
pub async fn save_report(
    app: AppHandle,
    path: PathBuf,
    format: ReportFormat,
    include_thumbnails: bool,
) -> CommandResult<u64> {
    // Everything is copied out inside this block so no lock guard — and no
    // borrow of the app state — is still alive at the `.await` below. A future
    // holding either of those is not `Send`, and Tauri requires it to be.
    let (duplicates, similar, summary, orphans, roots) = {
        let state = app.state::<AppState>();

        let scan = state.duplicates.read();
        let (duplicates, similar, summary) = match scan.as_ref() {
            Some(result) => (
                result.duplicates.clone(),
                result.similar.clone(),
                result.summary.clone(),
            ),
            None => (Vec::new(), Vec::new(), Summary::default()),
        };
        drop(scan);

        let orphans = state
            .orphans
            .read()
            .as_ref()
            .map(|r| r.findings.clone())
            .unwrap_or_default();

        let roots = state.last_roots.read().clone();

        (duplicates, similar, summary, orphans, roots)
    };

    if duplicates.is_empty() && similar.is_empty() && orphans.is_empty() {
        return Err("there is nothing to report yet — run a scan first".into());
    }

    let written = tauri::async_runtime::spawn_blocking(move || {
        let input = dupe_core::report::ReportInput {
            generated_at: now_secs(),
            roots: &roots,
            summary: &summary,
            duplicates: &duplicates,
            similar: &similar,
            orphans: &orphans,
            options: dupe_core::report::ReportOptions {
                include_thumbnails,
                ..Default::default()
            },
        };

        let body = match format {
            ReportFormat::Html => dupe_core::report::to_html(&input),
            // The separator comes from the user's Windows settings: a
            // comma-separated file opens as one column in a Dutch Excel.
            ReportFormat::Csv => {
                dupe_core::report::to_csv(&input, dupe_core::platform::list_separator())
            }
        };

        std::fs::write(&path, body.as_bytes()).map(|()| body.len() as u64)
    })
    .await
    .map_err(|e| format!("the report could not be started: {e}"))?;

    written.map_err(|e| format!("could not write the report: {e}"))
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The whole result set, for "save a report before I delete anything".
#[tauri::command]
pub async fn export_report(state: State<'_, AppState>) -> CommandResult<ReportExport> {
    let duplicates = state.duplicates.read();
    let orphans = state.orphans.read();

    Ok(ReportExport {
        duplicates: duplicates
            .as_ref()
            .map(|r| r.duplicates.clone())
            .unwrap_or_default(),
        similar: duplicates
            .as_ref()
            .map(|r| r.similar.clone())
            .unwrap_or_default(),
        orphans: orphans
            .as_ref()
            .map(|r: &OrphanReport| r.findings.clone())
            .unwrap_or_default(),
    })
}
