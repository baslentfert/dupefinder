//! Turning an image file into pixels — a tiny grayscale grid for hashing, or
//! a modest colour preview for the UI.
//!
//! # Why the embedded thumbnail matters
//!
//! An iPhone HEIC is HEVC-coded: fully decoding a 12 MP frame costs tens of
//! milliseconds and, on Windows, needs the HEVC extension. But every photo a
//! phone writes already carries a small pre-rendered preview — the `thmb`
//! item in HEIF, the EXIF thumbnail in JPEG, the embedded preview in a raw
//! file. Reading that instead is one to two orders of magnitude cheaper, and
//! for a 32×32 perceptual hash it carries exactly the same information.
//!
//! So the decoder always asks for the thumbnail first and only falls back to
//! a full decode when there is none, when it is smaller than the caller needs,
//! or when it is cropped to a different aspect ratio (which would move the
//! hash). Hashing and previewing therefore ask for different minimum sizes
//! from the same code path.
//!
//! # Why WIC
//!
//! On Windows the Imaging Component exposes all of this behind one interface,
//! and it dispatches to the codecs already installed on the machine. That
//! keeps HEVC decoding inside the OS component the user is already licensed
//! for, rather than shipping a decoder — which is both a patent question and
//! an AGPL question for the pure-Rust alternatives. It also means camera raw
//! and AVIF come along for free wherever the user has those codecs.
//!
//! The portable `image` crate handles everything WIC declines.

use std::path::Path;

use crate::error::{Error, Result};
use crate::images::phash::{box_resize, Thumbnail, THUMB};

/// A hashing thumbnail is only trusted at this size or above. Below it,
/// upscaling into a 32×32 hash grid invents detail.
const MIN_THUMB_SIDE: u32 = 48;

/// Maximum relative difference in aspect ratio between a thumbnail and its
/// parent image before the thumbnail is assumed to be cropped.
const MAX_ASPECT_DRIFT: f32 = 0.02;

/// A colour preview for display, as tightly packed RGB8.
#[derive(Debug, Clone)]
pub struct Preview {
    pub rgb: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Decode `path` into a `THUMB × THUMB` grayscale thumbnail for hashing.
pub fn load(path: &Path) -> Result<Thumbnail> {
    #[cfg(all(windows, feature = "wic"))]
    {
        match wic::load_thumbnail(path) {
            Ok(thumb) => return Ok(thumb),
            Err(err) => {
                tracing::trace!(?path, %err, "WIC declined, trying the portable decoder");
            }
        }
    }

    portable::load_thumbnail(path)
}

/// Decode `path` into a colour preview no larger than `max_side` on either
/// axis, preserving the aspect ratio.
pub fn load_preview(path: &Path, max_side: u32) -> Result<Preview> {
    #[cfg(all(windows, feature = "wic"))]
    {
        match wic::load_preview(path, max_side) {
            Ok(preview) => return Ok(preview),
            Err(err) => {
                tracing::trace!(?path, %err, "WIC declined preview, trying the portable decoder");
            }
        }
    }

    portable::load_preview(path, max_side)
}

/// Whether a thumbnail of `(tw, th)` faithfully represents `(w, h)` and is big
/// enough for a caller that needs `min_side` pixels.
fn thumbnail_is_usable(tw: u32, th: u32, w: u32, h: u32, min_side: u32) -> bool {
    if tw.min(th) < min_side || w == 0 || h == 0 || th == 0 {
        return false;
    }
    // A thumbnail larger than its parent is nonsense metadata; don't trust it.
    if tw > w || th > h {
        return false;
    }

    let full = w as f32 / h as f32;
    let small = tw as f32 / th as f32;
    (full - small).abs() / full <= MAX_ASPECT_DRIFT
}

/// Fit `(w, h)` inside a `max_side` box without distorting it.
fn fit(w: u32, h: u32, max_side: u32) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (1, 1);
    }
    if w <= max_side && h <= max_side {
        return (w, h);
    }

    if w >= h {
        let height = ((h as u64 * max_side as u64) / w as u64).max(1) as u32;
        (max_side, height)
    } else {
        let width = ((w as u64 * max_side as u64) / h as u64).max(1) as u32;
        (width, max_side)
    }
}

/// File extensions worth handing to the image pipeline at all.
///
/// Deliberately includes formats only WIC can open (HEIC, raw, AVIF): the
/// portable fallback will simply fail on those, which is handled.
pub fn is_image_extension(path: &Path) -> bool {
    const EXTENSIONS: &[&str] = &[
        // Everyday formats.
        "jpg", "jpeg", "jpe", "jfif", "png", "gif", "bmp", "tif", "tiff", "webp", "ico",
        // Apple / modern containers.
        "heic", "heif", "hif", "avif", "jxl",
        // Camera raw, opened through whichever vendor codec is installed.
        "cr2", "cr3", "nef", "nrw", "arw", "srf", "sr2", "dng", "orf", "rw2", "raf", "pef", "srw",
        "x3f", "3fr", "erf", "kdc", "mrw",
    ];

    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| EXTENSIONS.contains(&e.as_str()))
}

/// Pure-Rust decoding for the formats the `image` crate understands.
mod portable {
    use super::*;

    fn open(path: &Path) -> Result<image::DynamicImage> {
        let fail = |e: image::ImageError| Error::Decode {
            path: path.to_path_buf(),
            reason: e.to_string(),
        };

        image::ImageReader::open(path)
            .map_err(|e| Error::Decode {
                path: path.to_path_buf(),
                reason: e.to_string(),
            })?
            .with_guessed_format()
            .map_err(|e| Error::Decode {
                path: path.to_path_buf(),
                reason: e.to_string(),
            })?
            .decode()
            .map_err(fail)
    }

    pub fn load_thumbnail(path: &Path) -> Result<Thumbnail> {
        let image = open(path)?;
        let width = image.width();
        let height = image.height();

        // Two steps: a fast nearest-neighbour reduction to a modest size,
        // then a proper box average down to the hash grid. Filtering a 12 MP
        // image straight to 32×32 with a good kernel costs far more than the
        // hash is worth.
        let intermediate = image
            .thumbnail(THUMB as u32 * 4, THUMB as u32 * 4)
            .to_luma8();
        let (iw, ih) = intermediate.dimensions();
        let pixels = box_resize(
            intermediate.as_raw(),
            iw as usize,
            ih as usize,
            THUMB,
            THUMB,
        );

        Ok(Thumbnail {
            pixels,
            width,
            height,
        })
    }

    pub fn load_preview(path: &Path, max_side: u32) -> Result<Preview> {
        let image = open(path)?;
        let (width, height) = fit(image.width(), image.height(), max_side);
        let scaled = image.thumbnail(width, height).to_rgb8();

        Ok(Preview {
            width: scaled.width(),
            height: scaled.height(),
            rgb: scaled.into_raw(),
        })
    }
}

#[cfg(all(windows, feature = "wic"))]
mod wic {
    use super::*;

    use std::cell::OnceCell;
    use std::os::windows::ffi::OsStrExt;

    use windows::core::{Interface, GUID, PCWSTR};
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat24bppRGB, GUID_WICPixelFormat8bppGray,
        IWICBitmapSource, IWICImagingFactory, WICBitmapDitherTypeNone,
        WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    };

    pub fn load_thumbnail(path: &Path) -> Result<Thumbnail> {
        with_factory(path, |factory| unsafe {
            let (source, width, height) = open_source(factory, path, MIN_THUMB_SIDE)?;
            let pixels = render(
                factory,
                &source,
                THUMB as u32,
                THUMB as u32,
                &GUID_WICPixelFormat8bppGray,
                1,
                path,
            )?;

            Ok(Thumbnail {
                pixels,
                width,
                height,
            })
        })
    }

    pub fn load_preview(path: &Path, max_side: u32) -> Result<Preview> {
        with_factory(path, |factory| unsafe {
            // A preview is only worth taking from the embedded thumbnail when
            // that thumbnail is at least as detailed as what we will display.
            let (source, width, height) = open_source(factory, path, max_side)?;
            let (target_width, target_height) = fit(width, height, max_side);

            let rgb = render(
                factory,
                &source,
                target_width,
                target_height,
                &GUID_WICPixelFormat24bppRGB,
                3,
                path,
            )?;

            Ok(Preview {
                rgb,
                width: target_width,
                height: target_height,
            })
        })
    }

    /// One COM apartment and one factory per worker thread, created on first
    /// use and reused for the rest of the scan.
    fn with_factory<R>(path: &Path, f: impl FnOnce(&IWICImagingFactory) -> Result<R>) -> Result<R> {
        thread_local! {
            static FACTORY: OnceCell<Option<IWICImagingFactory>> = const { OnceCell::new() };
        }

        FACTORY.with(|cell| {
            let factory = cell.get_or_init(|| unsafe {
                // A thread already in an apartment returns RPC_E_CHANGED_MODE,
                // which is fine — we only need *an* apartment.
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()
            });

            match factory {
                Some(factory) => f(factory),
                None => Err(Error::Decode {
                    path: path.to_path_buf(),
                    reason: "WIC is unavailable on this system".into(),
                }),
            }
        })
    }

    /// Open `path` and pick the cheapest source that is good enough: the
    /// embedded thumbnail when it is at least `min_side` and uncropped,
    /// otherwise the full frame.
    ///
    /// Also returns the dimensions of the *full* image, which is what the UI
    /// and the keeper heuristics care about — not the size of whatever we
    /// ended up decoding.
    unsafe fn open_source(
        factory: &IWICImagingFactory,
        path: &Path,
        min_side: u32,
    ) -> Result<(IWICBitmapSource, u32, u32)> {
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        let fail = |e: windows::core::Error| Error::Decode {
            path: path.to_path_buf(),
            reason: e.to_string(),
        };

        let decoder = factory
            .CreateDecoderFromFilename(
                PCWSTR(wide.as_ptr()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )
            .map_err(fail)?;

        let frame = decoder.GetFrame(0).map_err(fail)?;
        let mut width = 0u32;
        let mut height = 0u32;
        frame.GetSize(&mut width, &mut height).map_err(fail)?;

        // The whole point: for HEIC this avoids decoding the HEVC main image.
        let source: IWICBitmapSource = match frame.GetThumbnail() {
            Ok(thumbnail) => {
                let mut tw = 0u32;
                let mut th = 0u32;
                if thumbnail.GetSize(&mut tw, &mut th).is_ok()
                    && thumbnail_is_usable(tw, th, width, height, min_side)
                {
                    thumbnail
                } else {
                    frame.cast().map_err(fail)?
                }
            }
            Err(_) => frame.cast().map_err(fail)?,
        };

        Ok((source, width, height))
    }

    /// Scale `source` to `width × height` and convert it to `format`.
    unsafe fn render(
        factory: &IWICImagingFactory,
        source: &IWICBitmapSource,
        width: u32,
        height: u32,
        format: &GUID,
        bytes_per_pixel: usize,
        path: &Path,
    ) -> Result<Vec<u8>> {
        let fail = |e: windows::core::Error| Error::Decode {
            path: path.to_path_buf(),
            reason: e.to_string(),
        };

        let scaler = factory.CreateBitmapScaler().map_err(fail)?;
        scaler
            .Initialize(source, width, height, WICBitmapInterpolationModeFant)
            .map_err(fail)?;

        let converter = factory.CreateFormatConverter().map_err(fail)?;
        converter
            .Initialize(
                &scaler,
                format,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .map_err(fail)?;

        let stride = width as usize * bytes_per_pixel;
        let mut pixels = vec![0u8; stride * height as usize];
        converter
            .CopyPixels(std::ptr::null(), stride as u32, &mut pixels)
            .map_err(fail)?;

        Ok(pixels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proportional_thumbnail_is_accepted() {
        assert!(thumbnail_is_usable(320, 240, 4032, 3024, MIN_THUMB_SIDE));
    }

    #[test]
    fn a_cropped_thumbnail_is_rejected() {
        // Square preview of a 4:3 photo: hashing it would not match the
        // hash of the same photo decoded in full.
        assert!(!thumbnail_is_usable(240, 240, 4032, 3024, MIN_THUMB_SIDE));
    }

    #[test]
    fn a_tiny_thumbnail_is_rejected() {
        assert!(!thumbnail_is_usable(32, 24, 4032, 3024, MIN_THUMB_SIDE));
    }

    #[test]
    fn a_preview_needs_a_bigger_thumbnail_than_a_hash_does() {
        // 320×240 is plenty for a 32×32 hash but not for a 256px preview,
        // so the same file takes different paths depending on the caller.
        assert!(thumbnail_is_usable(320, 240, 4032, 3024, MIN_THUMB_SIDE));
        assert!(!thumbnail_is_usable(320, 240, 4032, 3024, 256));
    }

    #[test]
    fn fitting_preserves_the_aspect_ratio() {
        assert_eq!(fit(4000, 3000, 256), (256, 192));
        assert_eq!(fit(3000, 4000, 256), (192, 256));
        // Already small enough: left alone rather than upscaled.
        assert_eq!(fit(100, 80, 256), (100, 80));
        assert_eq!(fit(0, 0, 256), (1, 1));
    }

    #[test]
    fn extensions_are_recognised_case_insensitively() {
        assert!(is_image_extension(Path::new("IMG_1234.HEIC")));
        assert!(is_image_extension(Path::new("a.jpg")));
        assert!(!is_image_extension(Path::new("a.mp4")));
    }
}
