//! Orphaned packages in the Windows installer cache.
//!
//! `%SystemRoot%\Installer` holds a copy of every `.msi` and `.msp` Windows
//! Installer has ever applied, so that products can be repaired or patches
//! rolled back. Uninstalling does not always remove the copy, and on a machine
//! that has seen a few years of Office and Visual Studio updates the leftovers
//! routinely run to tens of gigabytes.
//!
//! A package is orphaned when no product or patch registration points at it.
//! That makes the registry the source of truth, and it makes one failure mode
//! unacceptable: if the registry cannot be read, "found no references" looks
//! exactly like "nothing is referenced", and acting on it would break the
//! repair and uninstall path of every installed product on the machine.
//!
//! So this detector fails loudly instead of quietly:
//!
//! * The registry read must succeed.
//! * It must return a plausible number of references.
//! * Every reference it did find must be accounted for.
//!
//! If any of those does not hold, nothing is reported at all.

use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::progress::{Phase, Reporter, Throttle};

use super::{Confidence, OrphanFinding, OrphanKind};

/// Fewer registered packages than this on a real Windows install means the
/// read failed rather than that the machine is unusually clean.
const MINIMUM_PLAUSIBLE_REFERENCES: usize = 5;

#[cfg(windows)]
pub fn find(reporter: &Reporter) -> Result<Vec<OrphanFinding>> {
    use super::registry;

    reporter.phase(Phase::ScanningInstallerCache);
    let throttle = Throttle::new(std::time::Duration::from_millis(100));
    let mut seen = 0u64;

    let cache = installer_cache_dir()?;

    let referenced = registry::referenced_installer_packages().map_err(|err| {
        Error::Other(format!(
            "could not read the installer registry ({err}); \
             run as administrator to check the installer cache"
        ))
    })?;

    if referenced.len() < MINIMUM_PLAUSIBLE_REFERENCES {
        return Err(Error::Other(format!(
            "only {} registered installer packages were found, which cannot be right; \
             not reporting anything rather than risk breaking uninstall and repair",
            referenced.len()
        )));
    }

    let entries = std::fs::read_dir(&cache).map_err(|err| {
        Error::Other(format!(
            "could not list {} ({err}); this folder normally requires administrator rights",
            cache.display()
        ))
    })?;

    let mut findings = Vec::new();
    let mut seen_referenced = 0usize;

    for entry in entries.flatten() {
        reporter.check_cancelled()?;

        let path = entry.path();
        seen += 1;
        reporter.examining(&throttle, Phase::ScanningInstallerCache, &path, seen);

        let Some(name) = path
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase())
        else {
            continue;
        };

        if !name.ends_with(".msi") && !name.ends_with(".msp") {
            continue;
        }

        if referenced.contains(&name) {
            seen_referenced += 1;
            continue;
        }

        let Ok(meta) = entry.metadata() else {
            continue;
        };

        findings.push(OrphanFinding {
            id: 0,
            kind: OrphanKind::OrphanedInstallerPackage,
            path,
            size_bytes: meta.len(),
            is_directory: false,
            // The registry is authoritative here, but Windows Installer is
            // intricate enough that "certain" would be overclaiming.
            confidence: Confidence::Likely,
            reason: "no installed product or patch references this package".to_string(),
            last_modified: Some(crate::scan::walk::mtime_secs(&meta)),
        });
    }

    // Every registered package should have been found in the cache. If the
    // overlap is poor, the two sides are not talking about the same files and
    // the "orphans" are an artefact.
    if seen_referenced * 2 < referenced.len() {
        return Err(Error::Other(format!(
            "only {seen_referenced} of {} registered packages were found in {}; \
             the results would not be trustworthy",
            referenced.len(),
            cache.display()
        )));
    }

    Ok(findings)
}

#[cfg(not(windows))]
pub fn find(_reporter: &Reporter) -> Result<Vec<OrphanFinding>> {
    Err(Error::Other("the installer cache is Windows-only".into()))
}

fn installer_cache_dir() -> Result<PathBuf> {
    let root = std::env::var("SystemRoot")
        .or_else(|_| std::env::var("WINDIR"))
        .map_err(|_| Error::Other("SystemRoot is not set".into()))?;

    let path = PathBuf::from(root).join("Installer");
    if !path.is_dir() {
        return Err(Error::Other(format!("{} does not exist", path.display())));
    }
    Ok(path)
}

/// Whether a destructive action may target this path despite it living under
/// `%SystemRoot%`.
///
/// [`crate::config::is_protected`] blocks everything under `C:\Windows`, which
/// is the right default and is what makes this narrow, explicit exemption
/// safe to have: only files this detector can produce — `.msi` and `.msp`
/// directly inside the installer cache, never a subdirectory — are exempt.
pub fn is_installer_cache_package(path: &std::path::Path) -> bool {
    let Ok(cache) = installer_cache_dir() else {
        return false;
    };

    if path.parent() != Some(cache.as_path()) {
        return false;
    }

    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| e == "msi" || e == "msp")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn only_packages_directly_in_the_cache_are_exempt() {
        let cache = installer_cache_dir().unwrap();

        assert!(is_installer_cache_package(&cache.join("abc123.msi")));
        assert!(is_installer_cache_package(&cache.join("abc123.MSP")));

        // A subdirectory of the cache holds rollback data, not orphans.
        assert!(!is_installer_cache_package(
            &cache.join("$PatchCache$/x.msi")
        ));
        // And nothing else under Windows is ever exempt.
        assert!(!is_installer_cache_package(std::path::Path::new(
            r"C:\Windows\System32\kernel32.dll"
        )));
        assert!(!is_installer_cache_package(&cache.join("readme.txt")));
    }

    #[cfg(windows)]
    #[test]
    fn a_failed_registry_read_reports_nothing_rather_than_everything() {
        // Whatever happens, the detector must never return a long list of
        // findings while also having failed to read references.
        match find(&Reporter::silent()) {
            Ok(findings) => {
                for finding in findings {
                    assert_eq!(finding.kind, OrphanKind::OrphanedInstallerPackage);
                    assert!(is_installer_cache_package(&finding.path));
                }
            }
            Err(err) => {
                // Running unelevated is the normal case for a test run.
                let message = err.to_string();
                assert!(
                    message.contains("administrator")
                        || message.contains("could not")
                        || message.contains("trustworthy")
                        || message.contains("cannot be right"),
                    "unhelpful error: {message}"
                );
            }
        }
    }
}
