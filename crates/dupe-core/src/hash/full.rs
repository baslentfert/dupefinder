use std::path::Path;

use crate::error::{Error, Result};
use crate::model::ContentHash;

/// Above this size, hashing one file across several threads beats relying on
/// file-level parallelism alone — a 4 GiB video in a group of two would
/// otherwise keep only two cores busy.
const RAYON_THRESHOLD: u64 = 64 * 1024 * 1024;

/// BLAKE3 digest of the whole file.
///
/// Uses a memory map, so the kernel streams pages in without a userspace copy
/// and without allocating for the file size.
pub fn full_hash(path: &Path, size: u64) -> Result<ContentHash> {
    let mut hasher = blake3::Hasher::new();

    let result = if size >= RAYON_THRESHOLD {
        hasher.update_mmap_rayon(path)
    } else {
        hasher.update_mmap(path)
    };

    result.map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;

    Ok(ContentHash(*hasher.finalize().as_bytes()))
}

/// Byte-for-byte comparison, for the paranoid verification pass.
///
/// Reads both files through memory maps and bails at the first difference, so
/// non-identical files cost far less than a full read.
pub fn bytes_equal(a: &Path, b: &Path) -> Result<bool> {
    use std::fs::File;

    let fa = File::open(a).map_err(|source| Error::Io {
        path: a.to_path_buf(),
        source,
    })?;
    let fb = File::open(b).map_err(|source| Error::Io {
        path: b.to_path_buf(),
        source,
    })?;

    if fa.metadata().map(|m| m.len()).unwrap_or(0) != fb.metadata().map(|m| m.len()).unwrap_or(1) {
        return Ok(false);
    }

    // SAFETY: the maps are read-only and dropped before this returns. Another
    // process truncating the file underneath us would fault; that is the same
    // risk every mmap-based hasher accepts, and the alternative (buffered
    // reads of two multi-gigabyte files) is materially slower.
    let ma = unsafe { memmap2::Mmap::map(&fa) }.map_err(|source| Error::Io {
        path: a.to_path_buf(),
        source,
    })?;
    let mb = unsafe { memmap2::Mmap::map(&fb) }.map_err(|source| Error::Io {
        path: b.to_path_buf(),
        source,
    })?;

    Ok(ma[..] == mb[..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_contents_hash_identically() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.bin");
        let b = dir.path().join("b.bin");
        std::fs::write(&a, vec![3u8; 100_000]).unwrap();
        std::fs::write(&b, vec![3u8; 100_000]).unwrap();

        assert_eq!(
            full_hash(&a, 100_000).unwrap(),
            full_hash(&b, 100_000).unwrap()
        );
        assert!(bytes_equal(&a, &b).unwrap());
    }

    #[test]
    fn differing_contents_do_not() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.bin");
        let b = dir.path().join("b.bin");
        std::fs::write(&a, vec![3u8; 100_000]).unwrap();
        std::fs::write(&b, vec![4u8; 100_000]).unwrap();

        assert_ne!(
            full_hash(&a, 100_000).unwrap(),
            full_hash(&b, 100_000).unwrap()
        );
        assert!(!bytes_equal(&a, &b).unwrap());
    }

    #[test]
    fn empty_files_are_handled() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.bin");
        std::fs::write(&a, b"").unwrap();
        assert!(full_hash(&a, 0).is_ok());
    }
}
