//! Perceptual hashes.
//!
//! Two 64-bit hashes are computed per image, because each fails differently:
//!
//! * **dHash** (gradient) compares horizontally adjacent pixels. It is
//!   insensitive to brightness and gamma shifts — the usual damage from
//!   re-encoding or a messaging app — but it is easily fooled by flat images
//!   (a mostly-white scan, a screenshot of a document).
//! * **pHash** (DCT) keeps the low-frequency structure of the image. It is far
//!   more discriminating on flat images and survives moderate crops, but it
//!   reacts more strongly to a global tone curve.
//!
//! Requiring both to agree cuts the false-positive rate that either alone
//! produces on a real photo library. See [`super::cluster`].

/// Side length of the grayscale thumbnail every hash is derived from.
///
/// One decode feeds both hashes: 32×32 is what the DCT needs, and dHash's
/// 9×8 input is derived from it rather than from a second decode.
pub const THUMB: usize = 32;

/// A square grayscale thumbnail, row-major, `THUMB * THUMB` bytes.
#[derive(Debug, Clone)]
pub struct Thumbnail {
    pub pixels: Vec<u8>,
    /// Dimensions of the image this was made from, not of the thumbnail.
    pub width: u32,
    pub height: u32,
}

/// Gradient hash: 1 bit per horizontally adjacent pixel pair.
///
/// Built from a 9×8 reduction, giving exactly 64 comparisons.
pub fn dhash(thumb: &Thumbnail) -> u64 {
    let small = box_resize(&thumb.pixels, THUMB, THUMB, 9, 8);

    let mut bits = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            let left = small[y * 9 + x];
            let right = small[y * 9 + x + 1];
            if left > right {
                bits |= 1 << (y * 8 + x);
            }
        }
    }
    bits
}

/// DCT hash: the sign of the 64 lowest-frequency coefficients, relative to
/// their median.
///
/// The DC term is dropped — it encodes only overall brightness, which is
/// exactly the thing a re-encode is most likely to shift.
pub fn phash(thumb: &Thumbnail) -> u64 {
    let coeffs = dct2d(&thumb.pixels);

    // Top-left 8×8 block, minus the DC term at [0][0].
    let mut low = [0f32; 64];
    for y in 0..8 {
        for x in 0..8 {
            low[y * 8 + x] = coeffs[y * THUMB + x];
        }
    }
    low[0] = 0.0;

    let median = median_of(&low);

    let mut bits = 0u64;
    for (i, &value) in low.iter().enumerate() {
        if value > median {
            bits |= 1 << i;
        }
    }
    bits
}

pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// Separable 2-D DCT-II over a `THUMB × THUMB` block.
///
/// Done as rows then columns, which turns O(n⁴) into O(n³) — about 65k
/// multiplies per image, small enough next to the decode that a fast DCT
/// would not be measurable.
fn dct2d(pixels: &[u8]) -> Vec<f32> {
    let n = THUMB;
    let basis = dct_basis();

    let mut rows = vec![0f32; n * n];
    for y in 0..n {
        for u in 0..n {
            let mut sum = 0f32;
            for x in 0..n {
                sum += pixels[y * n + x] as f32 * basis[u * n + x];
            }
            rows[y * n + u] = sum;
        }
    }

    let mut out = vec![0f32; n * n];
    for x in 0..n {
        for v in 0..n {
            let mut sum = 0f32;
            for y in 0..n {
                sum += rows[y * n + x] * basis[v * n + y];
            }
            out[v * n + x] = sum;
        }
    }
    out
}

/// `cos((2x + 1) u π / 2n)` for every (u, x), computed once per process.
fn dct_basis() -> &'static [f32] {
    use std::sync::OnceLock;
    static BASIS: OnceLock<Vec<f32>> = OnceLock::new();

    BASIS.get_or_init(|| {
        let n = THUMB;
        let mut basis = vec![0f32; n * n];
        for u in 0..n {
            for x in 0..n {
                basis[u * n + x] = (((2 * x + 1) as f32) * (u as f32) * std::f32::consts::PI
                    / (2.0 * n as f32))
                    .cos();
            }
        }
        basis
    })
}

fn median_of(values: &[f32; 64]) -> f32 {
    let mut sorted = *values;
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    (sorted[31] + sorted[32]) / 2.0
}

/// Box-average downscale. Only ever shrinks, so averaging every source pixel
/// that falls in a destination cell is both correct and cheap.
pub fn box_resize(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<u8> {
    let mut out = vec![0u8; dw * dh];

    for dy in 0..dh {
        let y0 = dy * sh / dh;
        let y1 = (((dy + 1) * sh).div_ceil(dh)).max(y0 + 1).min(sh);

        for dx in 0..dw {
            let x0 = dx * sw / dw;
            let x1 = (((dx + 1) * sw).div_ceil(dw)).max(x0 + 1).min(sw);

            let mut sum = 0u32;
            let mut count = 0u32;
            for y in y0..y1 {
                for x in x0..x1 {
                    sum += src[y * sw + x] as u32;
                    count += 1;
                }
            }
            out[dy * dw + dx] = (sum / count.max(1)) as u8;
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thumb_from(f: impl Fn(usize, usize) -> u8) -> Thumbnail {
        let mut pixels = vec![0u8; THUMB * THUMB];
        for y in 0..THUMB {
            for x in 0..THUMB {
                pixels[y * THUMB + x] = f(x, y);
            }
        }
        Thumbnail {
            pixels,
            width: 4000,
            height: 3000,
        }
    }

    #[test]
    fn identical_images_hash_identically() {
        let a = thumb_from(|x, y| ((x * 7 + y * 3) % 256) as u8);
        let b = a.clone();
        assert_eq!(dhash(&a), dhash(&b));
        assert_eq!(phash(&a), phash(&b));
    }

    #[test]
    fn a_brightness_shift_barely_moves_the_hashes() {
        // What a re-encode or a "enhance" filter typically does.
        let a = thumb_from(|x, y| ((x * 7 + y * 3) % 200) as u8);
        let b = thumb_from(|x, y| (((x * 7 + y * 3) % 200) + 40) as u8);

        assert_eq!(dhash(&a), dhash(&b), "gradients are unchanged by an offset");
        assert!(
            hamming(phash(&a), phash(&b)) <= 4,
            "phash moved {} bits",
            hamming(phash(&a), phash(&b))
        );
    }

    /// Deterministic noise, so "two different photos" is reproducible.
    fn noise(seed: u32) -> impl Fn(usize, usize) -> u8 {
        move |x, y| {
            let mut state = seed
                .wrapping_add((x as u32).wrapping_mul(0x9E37_79B9))
                .wrapping_add((y as u32).wrapping_mul(0x85EB_CA6B));
            state ^= state >> 15;
            state = state.wrapping_mul(0x2545_F491);
            (state >> 24) as u8
        }
    }

    #[test]
    fn unrelated_images_are_far_apart() {
        let a = thumb_from(noise(1));
        let b = thumb_from(noise(999));

        assert!(
            hamming(dhash(&a), dhash(&b)) > 16,
            "dhash distance {}",
            hamming(dhash(&a), dhash(&b))
        );
        assert!(
            hamming(phash(&a), phash(&b)) > 16,
            "phash distance {}",
            hamming(phash(&a), phash(&b))
        );
    }

    #[test]
    fn phash_separates_what_dhash_cannot() {
        // A horizontal ramp and an image constant along each row both produce
        // the same gradient signature: dHash sees "never decreasing to the
        // right" in both. This is the blind spot that makes a second hash
        // necessary, and why flat scans and screenshots need pHash to avoid
        // collapsing into one bogus cluster.
        let horizontal = thumb_from(|x, _| (x * 8) as u8);
        let vertical = thumb_from(|_, y| (255 - y * 8) as u8);

        assert_eq!(
            hamming(dhash(&horizontal), dhash(&vertical)),
            0,
            "the documented dhash blind spot"
        );
        assert!(
            hamming(phash(&horizontal), phash(&vertical)) > 16,
            "phash distance {}",
            hamming(phash(&horizontal), phash(&vertical))
        );
    }

    #[test]
    fn resize_averages_without_reading_out_of_bounds() {
        let src: Vec<u8> = (0..(32 * 32)).map(|i| (i % 256) as u8).collect();
        let out = box_resize(&src, 32, 32, 9, 8);
        assert_eq!(out.len(), 72);
    }
}
