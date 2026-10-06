//! Core value types shared by every stage of the pipeline.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Identity of a file *on disk*, independent of the path used to reach it.
///
/// Two paths with the same [`FileId`] are the same physical file (a hard link
/// or a junction/symlink target). Reporting them as duplicates would be wrong:
/// deleting one does not reclaim any space, and deleting both loses the data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileId {
    /// Volume serial number (Windows) / device id (unix).
    pub volume: u64,
    /// 128-bit file reference number (Windows) / inode (unix).
    pub index: u128,
}

/// A candidate file discovered during enumeration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: PathBuf,
    pub size: u64,
    /// Modification time in whole seconds since the unix epoch.
    pub mtime: i64,
    /// Not sent to the front end: it is an internal identity, and a 128-bit
    /// value would silently lose precision as a JSON number.
    #[serde(skip)]
    pub file_id: Option<FileId>,
}

impl FileEntry {
    /// Key under which this file's computed hashes are cached.
    ///
    /// Includes size and mtime so an edited file is re-hashed rather than
    /// silently reusing a stale digest.
    pub fn cache_key(&self) -> Option<CacheKey> {
        self.file_id.map(|id| CacheKey {
            id,
            size: self.size,
            mtime: self.mtime,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub id: FileId,
    pub size: u64,
    pub mtime: i64,
}

/// 256-bit BLAKE3 digest of a file's full contents.
///
/// Serialized as a hex string rather than an array of 32 numbers: the front
/// end shows it verbatim, and a 32-element JSON array is both larger and
/// useless to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentHash(pub [u8; 32]);

impl ContentHash {
    pub fn to_hex(self) -> String {
        use std::fmt::Write;

        let mut s = String::with_capacity(64);
        for b in self.0 {
            let _ = write!(s, "{b:02x}");
        }
        s
    }

    pub fn from_hex(text: &str) -> Option<Self> {
        if text.len() != 64 {
            return None;
        }

        let mut bytes = [0u8; 32];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()?;
        }
        Some(Self(bytes))
    }
}

impl Serialize for ContentHash {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for ContentHash {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::from_hex(&text).ok_or_else(|| serde::de::Error::custom("expected 64 hex characters"))
    }
}

impl std::fmt::Display for ContentHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// A set of files that are byte-identical to each other.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateGroup {
    /// Stable id, used by the UI to address the group across rescans.
    pub id: u64,
    pub hash: ContentHash,
    /// Size of a single member, in bytes.
    pub size: u64,
    pub files: Vec<FileEntry>,
}

impl DuplicateGroup {
    /// Bytes reclaimable by keeping exactly one copy.
    pub fn wasted_bytes(&self) -> u64 {
        self.size * (self.files.len().saturating_sub(1)) as u64
    }
}

/// A cluster of images that look alike without being byte-identical.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimilarGroup {
    pub id: u64,
    pub images: Vec<SimilarImage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimilarImage {
    pub file: FileEntry,
    pub width: u32,
    pub height: u32,
    /// Hamming distance to the cluster's reference image, in bits.
    pub distance: u32,
    /// EXIF capture time, seconds since epoch, when the file carries one.
    pub taken_at: Option<i64>,
}

impl SimilarImage {
    pub fn pixels(&self) -> u64 {
        self.width as u64 * self.height as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_content_hash_round_trips_through_json() {
        let hash = ContentHash([0xAB; 32]);
        let json = serde_json::to_string(&hash).unwrap();

        assert_eq!(json, format!("\"{}\"", "ab".repeat(32)));
        assert_eq!(serde_json::from_str::<ContentHash>(&json).unwrap(), hash);
    }

    #[test]
    fn a_malformed_hash_is_rejected() {
        assert!(ContentHash::from_hex("abc").is_none());
        assert!(ContentHash::from_hex(&"zz".repeat(32)).is_none());
    }

    #[test]
    fn the_file_id_never_reaches_the_front_end() {
        let entry = FileEntry {
            path: std::path::PathBuf::from(r"C:\a.txt"),
            size: 1,
            mtime: 2,
            file_id: Some(FileId {
                volume: 1,
                // Larger than 2^53: as a JSON number this would be rounded.
                index: 0x0001_0000_0000_0000_0000,
            }),
        };

        let json = serde_json::to_string(&entry).unwrap();
        assert!(!json.contains("fileId"), "{json}");
    }
}
