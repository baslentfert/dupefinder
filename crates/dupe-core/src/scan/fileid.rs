//! Resolving the on-disk identity of a file.
//!
//! Hard links make the same bytes reachable through several paths. Reporting
//! those as duplicates is actively harmful — "deleting the copy" frees nothing
//! and "deleting both" destroys the only copy — so candidates are de-duplicated
//! by [`FileId`] before anything is hashed.
//!
//! Resolution needs an open handle per file, which costs far more than a
//! `stat`, so it runs only on files that already share a size with another.

use std::path::Path;

use crate::model::FileId;

/// Wide, long-path-safe form of `path`, NUL-terminated.
#[cfg(windows)]
fn widen(path: &Path) -> Vec<u16> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    let os = path.as_os_str();
    // The `\\?\` prefix lifts the 260-character limit, but only for already
    // normalised absolute paths — which is what enumeration produces.
    let needs_prefix =
        os.len() > 240 && path.is_absolute() && !path.to_string_lossy().starts_with(r"\\?\");

    let mut wide: Vec<u16> = if needs_prefix {
        OsStr::new(r"\\?\").encode_wide().collect()
    } else {
        Vec::with_capacity(os.len() + 1)
    };
    wide.extend(os.encode_wide());
    wide.push(0);
    wide
}

#[cfg(windows)]
pub fn file_id(path: &Path) -> Option<FileId> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FileIdInfo, GetFileInformationByHandleEx, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        OPEN_EXISTING,
    };

    let wide = widen(path);

    unsafe {
        let handle = CreateFileW(
            PCWSTR(wide.as_ptr()),
            FILE_READ_ATTRIBUTES.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            // Needed to open directories, and harmless for files. Note this
            // asks only for attribute access, so files held open exclusively
            // by another process still resolve.
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
        .ok()?;

        let mut info = FILE_ID_INFO::default();
        let result = GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            (&raw mut info).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        );
        let _ = CloseHandle(handle);
        result.ok()?;

        Some(FileId {
            volume: info.VolumeSerialNumber,
            index: u128::from_le_bytes(info.FileId.Identifier),
        })
    }
}

#[cfg(not(windows))]
pub fn file_id(path: &Path) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt;

    let meta = std::fs::metadata(path).ok()?;
    Some(FileId {
        volume: meta.dev(),
        index: meta.ino() as u128,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_file_has_the_same_id_through_two_paths() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.bin");
        std::fs::write(&a, b"hello").unwrap();

        let b = dir.path().join("b.bin");
        // A hard link is the case this whole module exists to catch.
        if std::fs::hard_link(&a, &b).is_ok() {
            assert_eq!(file_id(&a), file_id(&b));
        }

        let c = dir.path().join("c.bin");
        std::fs::write(&c, b"hello").unwrap();
        assert_ne!(file_id(&a), file_id(&c));
    }

    #[test]
    fn a_missing_file_has_no_id() {
        assert_eq!(file_id(Path::new("does-not-exist-9f3a.bin")), None);
    }
}
