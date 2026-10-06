//! Small Windows-specific helpers the rest of the engine asks about.

use serde::{Deserialize, Serialize};

/// A mounted drive the user can pick as a scan root.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeInfo {
    pub letter: char,
    /// Volume label, or an empty string when the drive has none.
    pub label: String,
    pub file_system: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub is_fixed: bool,
    /// Whether the MFT fast path could serve this drive.
    pub is_ntfs: bool,
}

/// Whether this process runs with an elevated token.
///
/// Reading the MFT means opening `\\.\C:` as a raw device, which a standard
/// user token cannot do. Knowing this up front lets the UI offer "restart as
/// administrator for a much faster scan" instead of failing halfway.
#[cfg(windows)]
pub fn is_elevated() -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token = Default::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }

        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some((&raw mut elevation).cast()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
        .is_ok();
        let _ = CloseHandle(token);

        ok && elevation.TokenIsElevated != 0
    }
}

#[cfg(not(windows))]
pub fn is_elevated() -> bool {
    false
}

/// How long the whole enumeration may take before slow drives are given up on.
///
/// `GetVolumeInformationW` on a disconnected share or a cloud file system can
/// block for tens of seconds. Nothing that happens after this call is worth
/// making the user wait that long, so stragglers are dropped rather than
/// waited for. The threads doing the blocking are left to finish on their own.
#[cfg(windows)]
const VOLUME_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Every mounted drive, with the information the UI needs to describe it.
///
/// Drives are probed concurrently: one unresponsive network drive should cost
/// its own latency, not everyone else's as well.
#[cfg(windows)]
pub fn volumes() -> Vec<VolumeInfo> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetDriveTypeW;

    // Not re-exported by the `windows` crate; the values are fixed by the
    // Win32 ABI.
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    const DRIVE_REMOTE: u32 = 4;

    // `GetDriveTypeW` reads the mount table and does not touch the device, so
    // this pass is cheap even for drives that are not answering.
    let candidates: Vec<(char, u32)> = ('A'..='Z')
        .filter_map(|letter| {
            let root: Vec<u16> = format!("{letter}:\\").encode_utf16().chain([0]).collect();
            let kind = unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) };
            matches!(kind, DRIVE_FIXED | DRIVE_REMOVABLE | DRIVE_REMOTE).then_some((letter, kind))
        })
        .collect();

    let expected = candidates.len();
    let (tx, rx) = crossbeam_channel::bounded(expected.max(1));

    for (letter, kind) in candidates {
        let tx = tx.clone();
        std::thread::spawn(move || {
            if let Some(volume) = probe(letter, kind) {
                // The receiver may already have given up, which is fine.
                let _ = tx.send(volume);
            }
        });
    }
    drop(tx);

    let deadline = std::time::Instant::now() + VOLUME_PROBE_TIMEOUT;
    let mut out = Vec::with_capacity(expected);
    while let Ok(volume) = rx.recv_deadline(deadline) {
        out.push(volume);
    }

    out.sort_by_key(|v| v.letter);
    out
}

/// Read one drive's label, file system and free space. Blocking.
#[cfg(windows)]
fn probe(letter: char, kind: u32) -> Option<VolumeInfo> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetVolumeInformationW};

    const DRIVE_FIXED: u32 = 3;

    let root: Vec<u16> = format!("{letter}:\\").encode_utf16().chain([0]).collect();
    let root_ptr = PCWSTR(root.as_ptr());

    let mut label = [0u16; 261];
    let mut file_system = [0u16; 32];

    unsafe {
        GetVolumeInformationW(
            root_ptr,
            Some(&mut label),
            None,
            None,
            None,
            Some(&mut file_system),
        )
        // An empty card reader or a disconnected share.
        .ok()?;
    }

    let mut free = 0u64;
    let mut total = 0u64;
    unsafe {
        let _ = GetDiskFreeSpaceExW(root_ptr, None, Some(&mut total), Some(&mut free));
    }

    let file_system = trim_wide(&file_system);
    Some(VolumeInfo {
        letter,
        label: trim_wide(&label),
        is_ntfs: file_system.eq_ignore_ascii_case("NTFS"),
        file_system,
        total_bytes: total,
        free_bytes: free,
        is_fixed: kind == DRIVE_FIXED,
    })
}

#[cfg(not(windows))]
pub fn volumes() -> Vec<VolumeInfo> {
    Vec::new()
}

/// Drive letters of the fixed NTFS volumes currently mounted.
pub fn ntfs_volumes() -> Vec<char> {
    volumes()
        .into_iter()
        .filter(|v| v.is_ntfs && v.is_fixed)
        .map(|v| v.letter)
        .collect()
}

/// The character this machine's spreadsheet software expects between CSV
/// fields.
///
/// Not a cosmetic choice: a comma-separated file opened in a Dutch or German
/// Excel lands entirely in column A, because those locales use the semicolon.
/// Windows knows which one the user has configured, so ask rather than guess.
#[cfg(windows)]
pub fn list_separator() -> char {
    use windows::Win32::Globalization::{GetLocaleInfoEx, LOCALE_SLIST};

    let mut buffer = [0u16; 8];
    let written = unsafe { GetLocaleInfoEx(None, LOCALE_SLIST, Some(&mut buffer)) };

    if written <= 0 {
        return ',';
    }

    trim_wide(&buffer)
        .chars()
        .next()
        // A tab or a space would make the file unreadable; only the two
        // separators anyone actually configures are honoured.
        .filter(|c| *c == ',' || *c == ';')
        .unwrap_or(',')
}

#[cfg(not(windows))]
pub fn list_separator() -> char {
    ','
}

#[cfg(windows)]
fn trim_wide(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn the_system_drive_is_listed() {
        let volumes = volumes();
        assert!(
            volumes.iter().any(|v| v.letter == 'C'),
            "no C: drive found in {volumes:?}"
        );

        let system = volumes.iter().find(|v| v.letter == 'C').unwrap();
        assert!(system.total_bytes > 0);
        assert!(system.is_fixed);
    }
}
