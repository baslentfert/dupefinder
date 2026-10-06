//! A custom URI scheme that serves image previews to the webview.
//!
//! The alternative — returning base64 data URLs over IPC — would put every
//! preview through JSON encoding, inflate it by a third, and defeat the
//! browser's image cache. A results view scrolling through a few thousand
//! photos makes that difference obvious, so previews get their own protocol:
//!
//! ```text
//! <img src="thumb://localhost/?path=C%3A%5Cphotos%5Ca.jpg&size=224">
//! ```
//!
//! Requests are served off the IPC thread, and a decode failure returns a
//! transparent pixel rather than an error, so a broken file shows an empty
//! frame instead of a browser error icon.

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use percent_encoding::percent_decode_str;
use tauri::http::{Request, Response, StatusCode};
use tauri::{AppHandle, UriSchemeResponder};

pub const SCHEME: &str = "thumb";

/// Upper bound on the requested size. Without it a hostile or buggy front end
/// could ask for an 8000px preview of every photo at once.
const MAX_SIZE: u32 = 1024;
const DEFAULT_SIZE: u32 = 224;

pub fn handle(_app: &AppHandle, request: Request<Vec<u8>>, responder: UriSchemeResponder) {
    tracing::debug!(uri = %request.uri(), "thumbnail request");

    // Decoding takes tens of milliseconds, so it cannot happen on the caller's
    // thread — but it cannot get a thread of its own either. Scrolling a photo
    // grid fires hundreds of requests at once, and one OS thread per request
    // (each with its own COM apartment and WIC factory) would bury the
    // machine. A small fixed pool serves them newest-first instead.
    pool().submit(move || {
        let response = match parse(request.uri().query().unwrap_or_default()) {
            Some((path, size)) => render(&path, size),
            None => {
                tracing::warn!(uri = %request.uri(), "unparseable thumbnail request");
                empty_png(StatusCode::BAD_REQUEST)
            }
        };

        tracing::debug!(
            status = response.status().as_u16(),
            bytes = response.body().len(),
            "thumbnail response"
        );
        responder.respond(response);
    });
}

type Job = Box<dyn FnOnce() + Send + 'static>;

/// Fixed pool of decoder threads, serving the newest request first.
///
/// Deliberately small: previews compete with a running scan for both cores and
/// disk, and the visible part of a grid is only ever a few dozen images.
///
/// The ordering is the part that matters for how fast the app *feels*. A queue
/// served first-in-first-out spends a fast scroll decoding the clusters the
/// user has already passed, while the ones now on screen wait behind them — so
/// the faster you scroll, the longer you stare at empty frames. Serving the
/// newest request first inverts that: what you are looking at is always next.
/// Nothing is dropped, so the stragglers still arrive, just last.
struct Pool {
    queue: Arc<Queue>,
    /// Zero when not a single worker could be started.
    workers: usize,
}

/// The shared stack the workers pull from.
struct Queue {
    jobs: Mutex<Vec<Job>>,
    /// Signals a waiting worker that a job arrived.
    ready: Condvar,
}

impl Queue {
    /// Block until a job is available, then take the newest one.
    fn take(&self) -> Job {
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(job) = jobs.pop() {
                return job;
            }
            jobs = self.ready.wait(jobs).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn push(&self, job: Job) {
        self.jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(job);
        self.ready.notify_one();
    }
}

impl Pool {
    fn new(requested: usize) -> Self {
        let queue = Arc::new(Queue {
            jobs: Mutex::new(Vec::new()),
            ready: Condvar::new(),
        });

        let mut workers = 0;
        for _ in 0..requested {
            let queue = Arc::clone(&queue);
            let spawned = std::thread::Builder::new()
                .name("thumbnail".into())
                // Each worker keeps its COM apartment and WIC factory for the
                // life of the process rather than rebuilding them per image.
                .spawn(move || loop {
                    queue.take()();
                });

            match spawned {
                Ok(_) => workers += 1,
                // Release builds abort on panic, so failing to start a worker
                // must not be fatal — previews are a convenience, and running
                // with fewer of them (or none) still leaves a working app.
                Err(err) => tracing::warn!(%err, "a thumbnail worker could not be started"),
            }
        }

        Self { queue, workers }
    }

    fn submit(&self, job: impl FnOnce() + Send + 'static) {
        if self.workers == 0 {
            // Nothing would ever pick the job up. Serve it here instead: slow,
            // but a preview that arrives late beats one that never arrives.
            job();
            return;
        }

        self.queue.push(Box::new(job));
    }
}

fn pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        Pool::new(cores.clamp(2, 6))
    })
}

/// Pull `path` and `size` out of the query string.
fn parse(query: &str) -> Option<(PathBuf, u32)> {
    let mut path = None;
    let mut size = DEFAULT_SIZE;

    for pair in query.split('&') {
        let (key, value) = pair.split_once('=')?;
        let value = percent_decode_str(value).decode_utf8().ok()?;

        match key {
            "path" => path = Some(PathBuf::from(value.as_ref())),
            "size" => size = value.parse().unwrap_or(DEFAULT_SIZE).clamp(16, MAX_SIZE),
            _ => {}
        }
    }

    Some((path?, size))
}

/// Quality for the JPEG previews. At 224 pixels the difference from lossless is
/// invisible, and the file is a fraction of the size.
const JPEG_QUALITY: u8 = 82;

fn render(path: &std::path::Path, size: u32) -> Response<Vec<u8>> {
    let Ok(preview) = dupe_core::images::decode::load_preview(path, size) else {
        return empty_png(StatusCode::NOT_FOUND);
    };

    let Some(buffer) = image::RgbImage::from_raw(preview.width, preview.height, preview.rgb) else {
        return empty_png(StatusCode::INTERNAL_SERVER_ERROR);
    };

    // JPEG rather than PNG, and the reason is the *webview*, not this process.
    // A preview is a photograph, so lossless compression buys nothing and costs
    // a lot: PNG runs about 70 KB per thumbnail and has to be inflated by the
    // renderer, on the same thread that handles scrolling and clicks. A grid
    // filling with dozens of those is exactly when the window feels stuck.
    // JPEG at this size is roughly a tenth the bytes and far cheaper to decode.
    let mut encoded = Cursor::new(Vec::new());
    let mut encoder =
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, JPEG_QUALITY);

    if encoder.encode_image(&buffer).is_err() {
        return empty_png(StatusCode::INTERNAL_SERVER_ERROR);
    }

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "image/jpeg")
        // Previews are derived from a file that would have to change on disk
        // for the image to change, and a scan is a fresh page load anyway.
        .header("Cache-Control", "private, max-age=3600")
        .body(encoded.into_inner())
        .unwrap_or_else(|_| empty_png(StatusCode::INTERNAL_SERVER_ERROR))
}

/// A 1×1 transparent PNG, so a failed preview renders as a blank frame.
fn empty_png(status: StatusCode) -> Response<Vec<u8>> {
    const PIXEL: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    Response::builder()
        .status(status)
        .header("Content-Type", "image/png")
        .body(PIXEL.to_vec())
        .expect("a static response always builds")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_windows_path_and_size() {
        let (path, size) = parse("path=C%3A%5Cphotos%5Ca.jpg&size=128").unwrap();
        assert_eq!(path, PathBuf::from(r"C:\photos\a.jpg"));
        assert_eq!(size, 128);
    }

    #[test]
    fn an_absurd_size_is_clamped() {
        let (_, size) = parse("path=a.jpg&size=99999").unwrap();
        assert_eq!(size, MAX_SIZE);
    }

    #[test]
    fn a_missing_path_is_rejected() {
        assert!(parse("size=128").is_none());
    }

    #[test]
    fn a_path_with_spaces_and_accents_survives() {
        let (path, _) = parse("path=D%3A%5CFoto%27s%202024%5Ccaf%C3%A9.jpg").unwrap();
        assert_eq!(path, PathBuf::from(r"D:\Foto's 2024\café.jpg"));
    }
}
