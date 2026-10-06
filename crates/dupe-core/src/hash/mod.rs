//! Content hashing.
//!
//! Two digests, used at two different stages:
//!
//! * [`partial`] — XXH3 over the first and last few kilobytes plus the file
//!   size. Cheap (two seeks, no streaming) and enough to separate same-sized
//!   files that merely collide on length, which is the common case for media
//!   libraries and installer caches.
//! * [`full`] — BLAKE3 over the whole file, SIMD-accelerated and, for large
//!   files, multi-threaded. Only files that survive the partial stage pay it.

pub mod full;
pub mod partial;

pub use full::full_hash;
pub use partial::{
    partial_hash, partial_hash_with, PartialHash, Sampling, DEFAULT_SAMPLING, PARTIAL_WINDOW,
};
