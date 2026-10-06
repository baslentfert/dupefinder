//! Persistent hash cache.
//!
//! Hashing is the expensive half of a scan, and almost nothing changes between
//! runs. Keying on (file id, size, mtime) means an edited or replaced file
//! misses the cache and is re-hashed, while an untouched 4 GiB video is never
//! read twice.
//!
//! Every operation fails soft: a corrupt or locked cache degrades the scan to
//! "slow but correct", never to "broken".

use std::path::Path;

use parking_lot::Mutex;
use redb::{Database, ReadableDatabase, TableDefinition};

use crate::model::{CacheKey, ContentHash};

const CONTENT: TableDefinition<&[u8], &[u8]> = TableDefinition::new("content_hash_v1");
const IMAGE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("image_hash_v1");

/// Perceptual hashes plus the dimensions needed to pick a cluster's best copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageRecord {
    pub dhash: u64,
    pub phash: u64,
    pub width: u32,
    pub height: u32,
}

pub struct HashCache {
    db: Database,
    /// Writes are batched: one commit per file would dominate the scan.
    pending_content: Mutex<Vec<([u8; KEY_LEN], [u8; 32])>>,
    pending_image: Mutex<Vec<([u8; KEY_LEN], [u8; 24])>>,
}

const KEY_LEN: usize = 8 + 16 + 8 + 8;
/// Flush threshold, in entries. Large enough that commits are rare, small
/// enough that a crash loses seconds of work rather than a whole scan.
const FLUSH_AT: usize = 4096;

impl HashCache {
    pub fn open(path: &Path) -> Option<Self> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let db = match Database::create(path) {
            Ok(db) => db,
            Err(err) => {
                tracing::warn!(%err, "hash cache unavailable, scanning without it");
                return None;
            }
        };

        Some(Self {
            db,
            pending_content: Mutex::new(Vec::new()),
            pending_image: Mutex::new(Vec::new()),
        })
    }

    pub fn get_content(&self, key: &CacheKey) -> Option<ContentHash> {
        let bytes = self.read(CONTENT, key)?;
        let arr: [u8; 32] = bytes.try_into().ok()?;
        Some(ContentHash(arr))
    }

    pub fn put_content(&self, key: &CacheKey, hash: ContentHash) {
        let mut pending = self.pending_content.lock();
        pending.push((encode_key(key), hash.0));
        if pending.len() >= FLUSH_AT {
            let batch = std::mem::take(&mut *pending);
            drop(pending);
            self.write_batch(CONTENT, batch.iter().map(|(k, v)| (k, &v[..])));
        }
    }

    pub fn get_image(&self, key: &CacheKey) -> Option<ImageRecord> {
        let bytes = self.read(IMAGE, key)?;
        if bytes.len() != 24 {
            return None;
        }
        Some(ImageRecord {
            dhash: u64::from_le_bytes(bytes[0..8].try_into().ok()?),
            phash: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
            width: u32::from_le_bytes(bytes[16..20].try_into().ok()?),
            height: u32::from_le_bytes(bytes[20..24].try_into().ok()?),
        })
    }

    pub fn put_image(&self, key: &CacheKey, record: ImageRecord) {
        let mut value = [0u8; 24];
        value[0..8].copy_from_slice(&record.dhash.to_le_bytes());
        value[8..16].copy_from_slice(&record.phash.to_le_bytes());
        value[16..20].copy_from_slice(&record.width.to_le_bytes());
        value[20..24].copy_from_slice(&record.height.to_le_bytes());

        let mut pending = self.pending_image.lock();
        pending.push((encode_key(key), value));
        if pending.len() >= FLUSH_AT {
            let batch = std::mem::take(&mut *pending);
            drop(pending);
            self.write_batch(IMAGE, batch.iter().map(|(k, v)| (k, &v[..])));
        }
    }

    /// Commit everything still buffered. Call once a scan finishes.
    pub fn flush(&self) {
        let content = std::mem::take(&mut *self.pending_content.lock());
        if !content.is_empty() {
            self.write_batch(CONTENT, content.iter().map(|(k, v)| (k, &v[..])));
        }
        let images = std::mem::take(&mut *self.pending_image.lock());
        if !images.is_empty() {
            self.write_batch(IMAGE, images.iter().map(|(k, v)| (k, &v[..])));
        }
    }

    fn read(&self, table: TableDefinition<&[u8], &[u8]>, key: &CacheKey) -> Option<Vec<u8>> {
        let txn = self.db.begin_read().ok()?;
        let table = txn.open_table(table).ok()?;
        let value = table.get_owned(&encode_key(key)[..]).ok()??;
        Some(value.value().to_vec())
    }

    fn write_batch<'a>(
        &self,
        table: TableDefinition<&[u8], &[u8]>,
        entries: impl Iterator<Item = (&'a [u8; KEY_LEN], &'a [u8])>,
    ) {
        let Ok(txn) = self.db.begin_write() else {
            return;
        };
        {
            let Ok(mut t) = txn.open_table(table) else {
                return;
            };
            for (key, value) in entries {
                let _ = t.insert(&key[..], value);
            }
        }
        let _ = txn.commit();
    }
}

impl Drop for HashCache {
    fn drop(&mut self) {
        self.flush();
    }
}

fn encode_key(key: &CacheKey) -> [u8; KEY_LEN] {
    let mut out = [0u8; KEY_LEN];
    out[0..8].copy_from_slice(&key.id.volume.to_le_bytes());
    out[8..24].copy_from_slice(&key.id.index.to_le_bytes());
    out[24..32].copy_from_slice(&key.size.to_le_bytes());
    out[32..40].copy_from_slice(&key.mtime.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FileId;

    fn key(mtime: i64) -> CacheKey {
        CacheKey {
            id: FileId {
                volume: 1,
                index: 2,
            },
            size: 100,
            mtime,
        }
    }

    #[test]
    fn round_trips_a_content_hash() {
        let dir = tempfile::tempdir().unwrap();
        let cache = HashCache::open(&dir.path().join("c.redb")).unwrap();

        let hash = ContentHash([9u8; 32]);
        cache.put_content(&key(10), hash);
        cache.flush();

        assert_eq!(cache.get_content(&key(10)), Some(hash));
    }

    #[test]
    fn a_changed_mtime_misses() {
        let dir = tempfile::tempdir().unwrap();
        let cache = HashCache::open(&dir.path().join("c.redb")).unwrap();

        cache.put_content(&key(10), ContentHash([9u8; 32]));
        cache.flush();

        // Same file, edited since: the stale digest must not come back.
        assert_eq!(cache.get_content(&key(11)), None);
    }
}
