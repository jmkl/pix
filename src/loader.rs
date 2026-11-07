//! Background image decoding with a small LRU cache.
//!
//! The UI thread sends paths and polls for results; it never decodes or
//! resizes anything itself.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use image::metadata::Orientation;
use image::ImageDecoder;

/// Largest texture side handed to the GPU. 8192 is supported by essentially
/// every GPU; bigger images get a shrunken copy for display only.
pub const MAX_GPU_DIM: u32 = 8192;

/// A display-only copy of an image that was too large for the GPU.
pub struct GpuPixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub struct Decoded {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    /// Full-resolution RGBA8 (EXIF-rotated), shared so the clipboard worker
    /// can reuse it.
    pub rgba: Arc<Vec<u8>>,
    /// Present only when the image exceeds `MAX_GPU_DIM`.
    pub gpu: Option<GpuPixels>,
}

impl Decoded {
    /// The pixels to upload to the GPU: `(width, height, rgba)`.
    pub fn gpu_view(&self) -> (u32, u32, &[u8]) {
        match &self.gpu {
            Some(g) => (g.width, g.height, g.rgba.as_slice()),
            None => (self.width, self.height, self.rgba.as_slice()),
        }
    }
}

type Outcome = Result<Decoded, (PathBuf, anyhow::Error)>;

/// What `Loader::poll` found.
pub struct Polled {
    pub ready: Vec<Arc<Decoded>>,
    pub failed: Vec<PathBuf>,
}

pub struct Loader {
    tx: Sender<PathBuf>,
    rx: Receiver<Outcome>,
    cache: VecDeque<Arc<Decoded>>,
    capacity: usize,
}

impl Loader {
    /// `wake` is called from the decoder thread after each result is queued, so
    /// the (otherwise sleeping) event loop can come and collect it.
    pub fn new(capacity: usize, wake: impl Fn() + Send + 'static) -> Self {
        let (req_tx, req_rx) = mpsc::channel::<PathBuf>();
        let (res_tx, res_rx) = mpsc::channel::<Outcome>();

        thread::Builder::new()
            .name("decoder".into())
            .spawn(move || {
                while let Ok(path) = req_rx.recv() {
                    let outcome = match decode(&path) {
                        Ok(d) => Ok(d),
                        Err(e) => Err((path, e)),
                    };
                    let _ = res_tx.send(outcome);
                    wake();
                }
            })
            .expect("spawn decoder thread");

        Self { tx: req_tx, rx: res_rx, cache: VecDeque::new(), capacity }
    }

    /// Returns the cached image if present, otherwise queues a decode.
    pub fn request(&mut self, path: PathBuf) -> Option<Arc<Decoded>> {
        if let Some(pos) = self.cache.iter().position(|d| d.path == path) {
            // Move to the front (most recently used).
            let hit = self.cache.remove(pos).unwrap();
            self.cache.push_front(hit.clone());
            return Some(hit);
        }
        let _ = self.tx.send(path);
        None
    }

    /// Queue a decode only if not already cached (for preloading neighbors).
    pub fn preload(&mut self, path: PathBuf) {
        if !self.cache.iter().any(|d| d.path == path) {
            let _ = self.tx.send(path);
        }
    }

    /// Drain finished decodes into the cache.
    pub fn poll(&mut self) -> Polled {
        let mut out = Polled { ready: Vec::new(), failed: Vec::new() };
        while let Ok(result) = self.rx.try_recv() {
            match result {
                Ok(d) => {
                    let d = Arc::new(d);
                    // A file can be decoded twice (preload + request); keep one copy.
                    self.cache.retain(|c| c.path != d.path);
                    self.cache.push_front(d.clone());
                    self.cache.truncate(self.capacity);
                    out.ready.push(d);
                }
                Err((path, e)) => {
                    log::warn!("cannot decode {}: {e:#}", path.display());
                    out.failed.push(path);
                }
            }
        }
        out
    }
}

fn decode(path: &Path) -> anyhow::Result<Decoded> {
    use std::time::Instant;
    let t0 = Instant::now();
    // Open through a decoder so the EXIF orientation can be read, then bake
    // the rotation into the pixels.
    let mut decoder = image::ImageReader::open(path)?.with_guessed_format()?.into_decoder()?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut img = image::DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    let img = img.into_rgba8();
    let (width, height) = img.dimensions();

    let gpu = if width.max(height) > MAX_GPU_DIM {
        let scale = MAX_GPU_DIM as f32 / width.max(height) as f32;
        let w = ((width as f32 * scale) as u32).max(1);
        let h = ((height as f32 * scale) as u32).max(1);
        let small = image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle);
        Some(GpuPixels { width: w, height: h, rgba: small.into_raw() })
    } else {
        None
    };

    log::info!(
        "[{:>4} ms] decoded {} ({}x{}) in {:?}",
        crate::ms_since_start(),
        path.display(),
        width,
        height,
        t0.elapsed()
    );

    Ok(Decoded {
        path: path.to_path_buf(),
        width,
        height,
        rgba: Arc::new(img.into_raw()),
        gpu,
    })
}
