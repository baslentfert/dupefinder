//! File enumeration.
//!
//! Two backends produce the same [`FileEntry`] list:
//!
//! * [`walk`] — a parallel directory walk. Portable, needs no privileges, and
//!   is the only option for scanning a subtree rather than a whole volume.
//! * [`mft`] — reads the NTFS Master File Table straight off the volume. It
//!   lists millions of files in seconds because it never touches the
//!   directory tree, but it needs an elevated process and an NTFS volume.

pub mod fileid;
pub mod walk;

#[cfg(all(windows, feature = "mft"))]
pub mod mft;

use crate::config::{ScanConfig, ScanStrategy};
use crate::error::Result;
use crate::model::FileEntry;
use crate::progress::{Phase, Reporter};

/// Enumerate every file matching `cfg`, using the configured strategy.
pub fn enumerate(cfg: &ScanConfig, reporter: &Reporter) -> Result<Vec<FileEntry>> {
    reporter.phase(Phase::Enumerating);

    match resolve_strategy(cfg) {
        ScanStrategy::Mft => {
            #[cfg(all(windows, feature = "mft"))]
            {
                match mft::enumerate(cfg, reporter) {
                    Ok(entries) => return Ok(entries),
                    Err(err) => {
                        // The MFT path is an optimisation, never a hard
                        // requirement: fall back rather than fail the scan.
                        tracing::warn!(%err, "MFT enumeration failed, falling back to walk");
                        reporter.skipped("<mft>", format!("{err}; using directory walk instead"));
                    }
                }
            }
            walk::enumerate(cfg, reporter)
        }
        _ => walk::enumerate(cfg, reporter),
    }
}

/// Turn [`ScanStrategy::Auto`] into a concrete choice.
pub fn resolve_strategy(cfg: &ScanConfig) -> ScanStrategy {
    match cfg.strategy {
        ScanStrategy::Walk => ScanStrategy::Walk,
        ScanStrategy::Mft => ScanStrategy::Mft,
        ScanStrategy::Auto => {
            if mft_available() && cfg.roots.iter().all(|r| is_volume_root(r)) {
                ScanStrategy::Mft
            } else {
                ScanStrategy::Walk
            }
        }
    }
}

/// Whether the MFT backend could run right now.
pub fn mft_available() -> bool {
    #[cfg(all(windows, feature = "mft"))]
    {
        crate::platform::is_elevated()
    }
    #[cfg(not(all(windows, feature = "mft")))]
    {
        false
    }
}

/// `C:\` and friends — the only roots the MFT backend can serve without
/// wasting work on paths the user did not ask for.
fn is_volume_root(path: &std::path::Path) -> bool {
    let s = path.to_string_lossy();
    matches!(s.len(), 2 | 3) && s.as_bytes().get(1) == Some(&b':')
}
