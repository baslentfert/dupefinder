//! Application data left behind by software that is no longer installed.
//!
//! This detector is a guess and is presented as one. It compares the folder
//! names directly under `%APPDATA%`, `%LOCALAPPDATA%` and `%PROGRAMDATA%`
//! against every product, publisher and install directory the system knows
//! about. A folder that matches nothing, and that nothing has written to in
//! months, is *suspected* of being a leftover.
//!
//! Deliberate biases, all in the direction of not deleting something live:
//!
//! * If the installed-product list comes back implausibly short, the whole
//!   detector aborts rather than declaring everything orphaned.
//! * A folder touched recently is never reported, whatever its name.
//! * Folders that belong to Windows itself are on a hard allowlist.
//! * Findings are always [`Confidence::Suspected`], so the UI will not
//!   preselect them.

use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::progress::{Phase, Reporter, Throttle};

use super::{measure_directory, Confidence, OrphanFinding, OrphanKind};

/// Folders under the app-data roots that Windows or the shell owns. Their
/// names rarely match an installed product, so without this list they would
/// be reported every single scan.
const SYSTEM_OWNED: &[&str] = &[
    "microsoft",
    "windows",
    "windowsapps",
    "packages",
    "package cache",
    "packagestaging",
    "temp",
    "tempstate",
    "crashdumps",
    "iconcache",
    "d3dscache",
    "comms",
    "connecteddevicesplatform",
    "publishers",
    "virtualstore",
    "elevatedDiagnostics",
    "history",
    "inetcache",
    "caches",
    "cache",
    "programs",
    "application data",
    "desktop",
    "start menu",
    "ssh",
    "gnupg",
    "pip",
    "npm",
    "npm-cache",
    "yarn",
    "nuget",
    "usoshared",
    "usoprivate",
    "regid.1991-06.com.microsoft",
];

/// A product list shorter than this means the registry read went wrong.
const MINIMUM_PLAUSIBLE_PRODUCTS: usize = 10;

#[cfg(windows)]
pub fn find(cfg: &super::OrphanConfig, reporter: &Reporter) -> Result<Vec<OrphanFinding>> {
    use super::registry;

    let products = registry::installed_products();
    if products.len() < MINIMUM_PLAUSIBLE_PRODUCTS {
        return Err(Error::Other(format!(
            "only {} installed products could be read from the registry; \
             refusing to guess which application data is orphaned",
            products.len()
        )));
    }

    reporter.phase(Phase::ScanningAppData);
    let throttle = Throttle::new(std::time::Duration::from_millis(100));
    let mut seen = 0u64;

    let now = now_secs();
    let quiet_for = cfg.appdata_untouched_days as i64 * 86_400;
    let mut findings = Vec::new();

    for root in appdata_roots() {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };

        for entry in entries.flatten() {
            reporter.check_cancelled()?;

            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }

            let name = entry.file_name().to_string_lossy().to_string();
            if SYSTEM_OWNED.contains(&name.to_ascii_lowercase().as_str()) {
                continue;
            }
            if products.matches(&name) {
                continue;
            }

            let path = entry.path();
            seen += 1;
            // Emitted before the measurement, not after: walking a
            // multi-gigabyte folder is the slow part, and this is the
            // name the user should see while it happens.
            reporter.examining(&throttle, Phase::ScanningAppData, &path, seen);

            let (size, last_modified) = measure_directory(&path);

            // Something still writes here, so something still uses it.
            if let Some(modified) = last_modified {
                if now - modified < quiet_for {
                    continue;
                }
            }

            let age_days = last_modified.map(|m| (now - m) / 86_400);
            findings.push(OrphanFinding {
                id: 0,
                kind: OrphanKind::LeftoverAppData,
                path,
                size_bytes: size,
                is_directory: true,
                confidence: Confidence::Suspected,
                reason: match age_days {
                    Some(days) => format!(
                        "no installed program matches the name \"{name}\", \
                         and nothing has written here for {days} days"
                    ),
                    None => format!("no installed program matches the name \"{name}\""),
                },
                last_modified,
            });
        }
    }

    Ok(findings)
}

#[cfg(not(windows))]
pub fn find(_cfg: &super::OrphanConfig, _reporter: &Reporter) -> Result<Vec<OrphanFinding>> {
    Err(Error::Other(
        "leftover application data detection is Windows-only".into(),
    ))
}

fn appdata_roots() -> Vec<PathBuf> {
    ["APPDATA", "LOCALAPPDATA", "PROGRAMDATA"]
        .iter()
        .filter_map(|var| std::env::var(var).ok())
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .collect()
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_data_roots_exist_on_this_machine() {
        // If none of these resolve, the detector silently finds nothing and
        // the failure would be invisible.
        assert!(!appdata_roots().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn windows_owned_folders_are_never_reported() {
        let cfg = super::super::OrphanConfig::default();
        let findings = find(&cfg, &Reporter::silent()).unwrap_or_default();

        for finding in &findings {
            let name = finding
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_ascii_lowercase();
            assert!(
                !SYSTEM_OWNED.contains(&name.as_str()),
                "reported system folder {}",
                finding.path.display()
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn every_finding_is_only_a_suspicion() {
        let cfg = super::super::OrphanConfig::default();
        for finding in find(&cfg, &Reporter::silent()).unwrap_or_default() {
            assert_eq!(finding.confidence, Confidence::Suspected);
        }
    }
}
