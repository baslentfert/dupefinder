//! Shortcuts whose target no longer exists.
//!
//! The `.lnk` format stores the target several times over — an item-ID list,
//! a link-info block, a relative path, and sometimes an environment-variable
//! block — and different creators fill in different subsets. Rather than
//! resolving through the shell (which can block on offline network paths and
//! silently "fix" the shortcut by searching for a new target), the file is
//! parsed and judged on what it actually says.
//!
//! The rule throughout is: only report a shortcut as broken when a concrete
//! local path was extracted *and* that path does not exist. Anything we could
//! not read is left alone, because a false "broken" costs the user a working
//! shortcut and a missed one costs nothing.

use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::config::is_system_path;
use crate::error::Result;
use crate::progress::{Phase, Reporter, Throttle};

use super::{Confidence, OrphanFinding, OrphanKind};

pub fn find(cfg: &super::OrphanConfig, reporter: &Reporter) -> Result<Vec<OrphanFinding>> {
    reporter.phase(Phase::ScanningShortcuts);
    let throttle = Throttle::new(std::time::Duration::from_millis(100));
    let mut seen = 0u64;
    let mut links = Vec::new();

    for root in &cfg.roots {
        for entry in ignore::WalkBuilder::new(root)
            .standard_filters(false)
            .follow_links(false)
            .build()
            .flatten()
        {
            if reporter.is_cancelled() {
                return Ok(Vec::new());
            }
            let path = entry.path();
            seen += 1;
            reporter.examining(&throttle, Phase::ScanningShortcuts, path, seen);

            if path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                == Some("lnk".to_string())
                && !is_system_path(path)
            {
                links.push(path.to_path_buf());
            }
        }
    }

    Ok(links.par_iter().filter_map(|path| inspect(path)).collect())
}

fn inspect(path: &Path) -> Option<OrphanFinding> {
    let target = read_target(path)?;

    // A shortcut to a control panel applet, a settings URI or a store app has
    // no file-system target and is not broken just because we cannot stat it.
    if !is_local_path(&target) {
        return None;
    }

    if target.exists() {
        return None;
    }

    // A missing drive is far more likely to be an unplugged USB stick or a
    // disconnected share than a dead shortcut.
    let drive_present = drive_of(&target)
        .map(|d| Path::new(&format!("{d}:\\")).exists())
        .unwrap_or(false);

    let meta = std::fs::metadata(path).ok();

    Some(OrphanFinding {
        id: 0,
        kind: OrphanKind::BrokenShortcut,
        path: path.to_path_buf(),
        size_bytes: meta.as_ref().map(|m| m.len()).unwrap_or(0),
        is_directory: false,
        confidence: if drive_present {
            Confidence::Certain
        } else {
            Confidence::Suspected
        },
        reason: if drive_present {
            format!("points at {}, which does not exist", target.display())
        } else {
            format!(
                "points at {}, on a drive that is not currently connected",
                target.display()
            )
        },
        last_modified: meta.as_ref().map(crate::scan::walk::mtime_secs),
    })
}

#[cfg(windows)]
fn read_target(path: &Path) -> Option<PathBuf> {
    // Windows-1252 is what the legacy (non-Unicode) string fields in a `.lnk`
    // use; the Unicode fields are read regardless of this setting.
    let link = lnk::ShellLink::open(path, lnk::encoding::WINDOWS_1252).ok()?;

    if let Some(target) = link.link_target() {
        let expanded = expand_environment(&target);
        if !expanded.trim().is_empty() {
            return Some(PathBuf::from(expanded));
        }
    }

    // Fall back to the link-info block, which older creators fill in when the
    // item-ID list is absent.
    let info = link.link_info().as_ref()?;
    let base = info.local_base_path()?;
    Some(PathBuf::from(expand_environment(base)))
}

#[cfg(not(windows))]
fn read_target(_path: &Path) -> Option<PathBuf> {
    None
}

/// Expand `%VAR%` references the way the shell would.
fn expand_environment(input: &str) -> String {
    if !input.contains('%') {
        return input.to_string();
    }

    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];

        match after.find('%') {
            Some(end) => {
                let name = &after[..end];
                match std::env::var(name) {
                    Ok(value) => out.push_str(&value),
                    // Leave an unknown variable in place: it is better to fail
                    // the "does it exist" check than to invent a path.
                    Err(_) => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push('%');
                out.push_str(after);
                rest = "";
            }
        }
    }

    out.push_str(rest);
    out
}

/// Whether this looks like a path on a local or mapped drive, as opposed to a
/// UNC share, a shell namespace GUID, or a URI.
fn is_local_path(path: &Path) -> bool {
    let text = path.to_string_lossy();
    let bytes = text.as_bytes();

    if text.starts_with("::{") || text.contains("://") {
        return false;
    }
    // UNC targets may simply be offline; not our call to make.
    if text.starts_with(r"\\") {
        return false;
    }

    bytes.len() >= 3 && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/')
}

fn drive_of(path: &Path) -> Option<char> {
    let text = path.to_string_lossy();
    text.chars().next().filter(|c| c.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_namespace_targets_are_not_paths() {
        assert!(!is_local_path(Path::new(
            "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"
        )));
        assert!(!is_local_path(Path::new("https://example.com")));
        assert!(!is_local_path(Path::new(r"\\server\share\file.txt")));
    }

    #[test]
    fn drive_paths_are_paths() {
        assert!(is_local_path(Path::new(r"C:\Windows\notepad.exe")));
        assert!(is_local_path(Path::new(r"D:/data/x.txt")));
    }

    #[test]
    fn environment_variables_are_expanded() {
        std::env::set_var("DUPEFINDER_TEST_DIR", r"C:\example");
        assert_eq!(
            expand_environment(r"%DUPEFINDER_TEST_DIR%\app.exe"),
            r"C:\example\app.exe"
        );
    }

    #[test]
    fn an_unknown_variable_is_left_alone() {
        let input = r"%NO_SUCH_VARIABLE_XYZ%\app.exe";
        assert_eq!(expand_environment(input), input);
    }

    #[test]
    fn a_stray_percent_does_not_eat_the_path() {
        assert_eq!(expand_environment(r"C:\100%\file.txt"), r"C:\100%\file.txt");
    }
}
