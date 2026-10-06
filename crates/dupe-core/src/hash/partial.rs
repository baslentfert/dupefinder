use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use xxhash_rust::xxh3::Xxh3;

use crate::error::{IoContext, Result};

/// Bytes read from each end of the file.
///
/// 4 KiB is one NTFS cluster: reading less does not make the I/O cheaper, and
/// reading more starts to compete with just hashing the whole file.
pub const PARTIAL_WINDOW: usize = 4096;

/// Files at or below this size are read in full by the partial stage, so their
/// "partial" hash already covers every byte.
pub const SMALL_FILE_LIMIT: u64 = (PARTIAL_WINDOW * 2) as u64;

/// Which parts of a file the cheap pre-filter looks at.
///
/// Sampling more separates more files before the expensive full read, but the
/// sample is not free: every extra byte here is read from every candidate,
/// including the ones that would have been eliminated anyway. Whether a wider
/// sample pays for itself depends on the data, so it is a parameter rather
/// than a constant — see the `prefilter` mode of the `probe` example.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sampling {
    /// Bytes taken from the start, and again from the end.
    pub window: usize,
    /// Also take `window` bytes from the middle. Catches files that share a
    /// header and a trailer but differ in the body — container formats and
    /// re-encoded media, mostly.
    pub middle: bool,
}

/// What the pipeline uses unless told otherwise.
pub const DEFAULT_SAMPLING: Sampling = Sampling {
    window: PARTIAL_WINDOW,
    middle: false,
};

impl Sampling {
    /// Bytes this strategy reads from a file large enough to need all of them.
    pub fn cost(&self) -> u64 {
        let windows = if self.middle { 3 } else { 2 };
        (self.window * windows) as u64
    }

    /// Below this size the sample would cover the whole file, so the partial
    /// hash is the complete one and the full stage can be skipped.
    pub fn small_file_limit(&self) -> u64 {
        self.cost()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PartialHash {
    pub value: u64,
    /// True when the hash covered the entire file, letting the caller skip
    /// the full-hash stage entirely.
    pub complete: bool,
}

/// Hash the head and tail of `path`, mixing in `size`.
///
/// Including the size means files of different lengths can never share a
/// partial hash, so the caller does not have to re-check it.
pub fn partial_hash(path: &Path, size: u64) -> Result<PartialHash> {
    partial_hash_with(path, size, DEFAULT_SAMPLING)
}

/// [`partial_hash`] with an explicit sampling strategy.
pub fn partial_hash_with(path: &Path, size: u64, sampling: Sampling) -> Result<PartialHash> {
    let mut file = File::open(path).at(path)?;
    let mut hasher = Xxh3::new();
    hasher.update(&size.to_le_bytes());

    if size <= sampling.small_file_limit() {
        let mut buf = Vec::with_capacity(size as usize);
        file.read_to_end(&mut buf).at(path)?;
        hasher.update(&buf);
        return Ok(PartialHash {
            value: hasher.digest(),
            complete: true,
        });
    }

    let mut window = vec![0u8; sampling.window];

    read_exact_at(&mut file, &mut window, SeekFrom::Start(0), path)?;
    hasher.update(&window);

    if sampling.middle {
        // Aligned to the window so the same bytes are read every time, rather
        // than a position that drifts with rounding.
        let centre = (size / 2).saturating_sub(sampling.window as u64 / 2);
        read_exact_at(&mut file, &mut window, SeekFrom::Start(centre), path)?;
        hasher.update(&window);
    }

    read_exact_at(
        &mut file,
        &mut window,
        SeekFrom::End(-(sampling.window as i64)),
        path,
    )?;
    hasher.update(&window);

    Ok(PartialHash {
        value: hasher.digest(),
        complete: false,
    })
}

fn read_exact_at(file: &mut File, buf: &mut [u8], pos: SeekFrom, path: &Path) -> Result<()> {
    file.seek(pos).at(path)?;
    file.read_exact(buf).at(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = dir.join(name);
        let mut f = File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        p
    }

    #[test]
    fn small_files_are_hashed_completely() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "a.bin", b"hello world");
        let h = partial_hash(&p, 11).unwrap();
        assert!(h.complete);
    }

    #[test]
    fn identical_large_files_share_a_partial_hash() {
        let dir = tempfile::tempdir().unwrap();
        let data = vec![7u8; 64 * 1024];
        let a = write(dir.path(), "a.bin", &data);
        let b = write(dir.path(), "b.bin", &data);
        let size = data.len() as u64;
        assert_eq!(
            partial_hash(&a, size).unwrap().value,
            partial_hash(&b, size).unwrap().value
        );
    }

    #[test]
    fn a_middle_sample_catches_what_head_and_tail_miss() {
        let dir = tempfile::tempdir().unwrap();

        // Same header, same trailer, different body — a container format with
        // re-encoded contents looks exactly like this.
        let mut a_data = vec![7u8; 64 * 1024];
        let mut b_data = a_data.clone();
        a_data[32 * 1024] = 1;
        b_data[32 * 1024] = 2;

        let a = write(dir.path(), "a.bin", &a_data);
        let b = write(dir.path(), "b.bin", &b_data);
        let size = a_data.len() as u64;

        let edges = Sampling {
            window: 4096,
            middle: false,
        };
        assert_eq!(
            partial_hash_with(&a, size, edges).unwrap().value,
            partial_hash_with(&b, size, edges).unwrap().value,
            "head and tail alone cannot tell these apart"
        );

        let with_middle = Sampling {
            window: 4096,
            middle: true,
        };
        assert_ne!(
            partial_hash_with(&a, size, with_middle).unwrap().value,
            partial_hash_with(&b, size, with_middle).unwrap().value
        );
    }

    #[test]
    fn sampling_cost_counts_every_window() {
        assert_eq!(
            Sampling {
                window: 4096,
                middle: false
            }
            .cost(),
            8192
        );
        assert_eq!(
            Sampling {
                window: 4096,
                middle: true
            }
            .cost(),
            12288
        );
        assert_eq!(DEFAULT_SAMPLING.cost(), SMALL_FILE_LIMIT);
    }

    #[test]
    fn a_differing_tail_is_caught() {
        let dir = tempfile::tempdir().unwrap();
        let mut a_data = vec![7u8; 64 * 1024];
        let mut b_data = a_data.clone();
        // Same head, same length, different last byte: the case a head-only
        // hash would miss.
        *a_data.last_mut().unwrap() = 1;
        *b_data.last_mut().unwrap() = 2;

        let a = write(dir.path(), "a.bin", &a_data);
        let b = write(dir.path(), "b.bin", &b_data);
        let size = a_data.len() as u64;
        assert_ne!(
            partial_hash(&a, size).unwrap().value,
            partial_hash(&b, size).unwrap().value
        );
    }
}
