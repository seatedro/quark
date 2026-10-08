//! Images of markdown image blocks, loaded and decoded off the UI thread.
//!
//! The app hands [`ImageStore`] a loader that turns an image URL into
//! bytes (from disk, a cache, or the network). A worker thread calls it and
//! decodes the result: PNG and JPEG with the `images` feature, or pixels
//! the loader already decoded. The store hands each block its
//! [`ImageState`]; an image renders as a placeholder until its pixels
//! arrive, and the app calls [`ImageStore::poll`] each frame and rebuilds
//! the rows of the images that resolved, as it does for code
//! highlights.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

/// What a loader returns for an image URL.
pub enum LoadedImage {
    /// PNG or JPEG bytes, decoded on the worker with the `images` feature.
    /// Without it, encoded images fail and show their alt text.
    Encoded(Vec<u8>),
    /// Pixels the loader decoded itself: `width * height` RGBA8 values.
    Rgba {
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    },
}

/// Turns an image URL into its bytes. Runs on the image worker thread.
pub type ImageLoader = Arc<dyn Fn(&str) -> Option<LoadedImage> + Send + Sync>;

/// Decoded RGBA8 pixels, shared between frames.
#[derive(Clone, PartialEq)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
    /// Identifies the pixels to the renderer's texture cache.
    pub cache_key: u64,
    /// Pixels per point: the whole factor between the pixels and the
    /// image's size hint (2 for an `@2x` image hinted at half), else 1.
    pub density: u32,
}

impl std::fmt::Debug for DecodedImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DecodedImage({}x{})", self.width, self.height)
    }
}

/// Where an image block's pixels are.
#[derive(Debug, Clone, PartialEq)]
pub enum ImageState {
    /// Not decoded yet. `size` is the intrinsic size when the app knows it
    /// ahead of the pixels ([`ImageStore::hint_size`]), so the block
    /// reserves the image's height from the start.
    Pending {
        size: Option<(u32, u32)>,
    },
    Ready(DecodedImage),
    /// No loader, or the loader or decoder failed: the alt text shows.
    Failed,
}

impl ImageState {
    /// Intrinsic size in points, when known: a hinted size, or the pixels
    /// over their density.
    pub fn size(&self) -> Option<(u32, u32)> {
        match self {
            Self::Pending { size } => *size,
            Self::Ready(image) => {
                let density = image.density.max(1);
                Some((image.width / density, image.height / density))
            }
            Self::Failed => None,
        }
    }
}

struct Entry {
    state: ImageState,
    /// The size the app said the image has, kept for when it loads again.
    hint: Option<(u32, u32)>,
    /// Bumped whenever `state` changes, so converted blocks know to update.
    version: u64,
    /// A block has asked for it; a hint alone does not count.
    used: bool,
    /// Sent to the worker since the loader was set.
    requested: bool,
}

struct Worker {
    requests: Sender<Arc<str>>,
    results: Receiver<(Arc<str>, Option<DecodedImage>)>,
}

/// Image states by URL, and the worker that loads them. One per app,
/// shared by every markdown row.
#[derive(Default)]
pub struct ImageStore {
    loader: Option<ImageLoader>,
    entries: HashMap<Arc<str>, Entry>,
    worker: Option<Worker>,
    /// Requests sent to the worker and not yet taken back.
    in_flight: usize,
}

impl ImageStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the loader. Images that failed for want of one load again.
    pub fn set_loader(&mut self, loader: ImageLoader) {
        self.loader = Some(loader);
        self.worker = None;
        self.in_flight = 0;
        for entry in self.entries.values_mut() {
            if !matches!(entry.state, ImageState::Ready(_)) {
                entry.state = ImageState::Pending { size: entry.hint };
                entry.version += 1;
            }
        }
        let pending: Vec<Arc<str>> = self.entries.keys().cloned().collect();
        for src in pending {
            if matches!(self.entries[&src].state, ImageState::Pending { .. }) {
                self.request(src);
            }
        }
    }

    /// The intrinsic size of `src` in points, known before its pixels (from
    /// an API response or an HTML attribute), so its block reserves the
    /// right height and nothing moves when the pixels land. Pixels that
    /// are an exact whole multiple of the hint are a high density image:
    /// it shows at the hinted size, sharp on a screen of that scale.
    pub fn hint_size(&mut self, src: &str, width: u32, height: u32) {
        let entry = self.entry(src);
        entry.hint = Some((width, height));
        match &mut entry.state {
            ImageState::Pending { size } if *size != entry.hint => {
                *size = entry.hint;
                entry.version += 1;
            }
            ImageState::Ready(image) => {
                let density = density(image, (width, height));
                if image.density != density {
                    image.density = density;
                    entry.version += 1;
                }
            }
            _ => {}
        }
    }

    /// The state of `src` and its version, starting its load on first use.
    pub fn state(&mut self, src: &str) -> (ImageState, u64) {
        let entry = self.entry(src);
        // An entry a size hint created is not loading yet.
        let fresh = !entry.used && !entry.requested;
        entry.used = true;
        let out = (entry.state.clone(), entry.version);
        if fresh {
            let src: Arc<str> = Arc::from(src);
            if self.loader.is_some() {
                self.request(src);
            } else {
                let entry = self.entry(&src);
                entry.state = ImageState::Failed;
                entry.version += 1;
                return (ImageState::Failed, entry.version);
            }
        }
        out
    }

    fn entry(&mut self, src: &str) -> &mut Entry {
        if !self.entries.contains_key(src) {
            self.entries.insert(
                Arc::from(src),
                Entry {
                    state: ImageState::Pending { size: None },
                    hint: None,
                    version: 0,
                    used: false,
                    requested: false,
                },
            );
        }
        self.entries.get_mut(src).expect("inserted above")
    }

    fn request(&mut self, src: Arc<str>) {
        let Some(loader) = self.loader.clone() else {
            return;
        };
        let worker = self.worker.get_or_insert_with(|| spawn(loader));
        if worker.requests.send(src.clone()).is_ok() {
            self.in_flight += 1;
            self.entry(&src).requested = true;
        } else {
            // The worker died (a loader panicked); its images fail.
            self.worker = None;
            self.resolve(src, None);
        }
    }

    fn resolve(&mut self, src: Arc<str>, image: Option<DecodedImage>) {
        let entry = self.entry(&src);
        let hint = entry.hint;
        let image = image.map(|mut image| {
            image.density = hint.map_or(1, |hint| density(&image, hint));
            image
        });
        entry.state = image.map_or(ImageState::Failed, ImageState::Ready);
        entry.version += 1;
    }

    /// Takes the images decoded since the last call and returns their
    /// URLs; rebuild the rows that show them.
    pub fn poll(&mut self) -> Vec<Arc<str>> {
        let mut done = Vec::new();
        while let Some(result) = self.worker.as_ref().and_then(|w| w.results.try_recv().ok()) {
            self.in_flight -= 1;
            done.push(result);
        }
        self.take(done)
    }

    /// Blocks until every requested image has resolved. For tests and
    /// screenshots.
    pub fn finish_pending(&mut self) -> Vec<Arc<str>> {
        let mut done = Vec::new();
        while self.in_flight > 0 {
            let Some(result) = self.worker.as_ref().and_then(|w| w.results.recv().ok()) else {
                break;
            };
            self.in_flight -= 1;
            done.push(result);
        }
        self.take(done)
    }

    fn take(&mut self, done: Vec<(Arc<str>, Option<DecodedImage>)>) -> Vec<Arc<str>> {
        let mut srcs = Vec::with_capacity(done.len());
        for (src, image) in done {
            self.resolve(src.clone(), image);
            srcs.push(src);
        }
        srcs
    }

    /// Whether images are still loading; draw frames to pick them up.
    pub fn is_loading(&self) -> bool {
        self.in_flight > 0
    }
}

fn spawn(loader: ImageLoader) -> Worker {
    let (requests, jobs) = channel::<Arc<str>>();
    let (done, results) = channel();
    std::thread::Builder::new()
        .name("quark-images".into())
        .spawn(move || {
            for src in jobs {
                let image = loader(&src).and_then(|loaded| decode(&src, loaded));
                if done.send((src, image)).is_err() {
                    break;
                }
            }
        })
        .expect("spawn the image worker");
    Worker { requests, results }
}

fn decode(src: &str, loaded: LoadedImage) -> Option<DecodedImage> {
    let (width, height, pixels) = match loaded {
        LoadedImage::Rgba {
            width,
            height,
            pixels,
        } => (width, height, pixels),
        LoadedImage::Encoded(bytes) => decode_encoded(&bytes)?,
    };
    let expected = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)?;
    if width == 0 || height == 0 || pixels.len() != expected {
        return None;
    }
    Some(DecodedImage {
        width,
        height,
        cache_key: quark::stable_hash(&format!("quark.image:{src}:{width}x{height}")),
        rgba: pixels.into(),
        density: 1,
    })
}

/// How many pixels per point `image` has if `hint` is its size in points:
/// the whole factor both sides share, or 1 when they share none.
fn density(image: &DecodedImage, (width, height): (u32, u32)) -> u32 {
    if width == 0 || height == 0 || !image.width.is_multiple_of(width) {
        return 1;
    }
    let k = image.width / width;
    if k > 1 && image.height == height * k {
        k
    } else {
        1
    }
}

#[cfg(feature = "images")]
fn decode_encoded(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    Some((image.width(), image.height(), image.into_raw()))
}

#[cfg(not(feature = "images"))]
fn decode_encoded(_bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    None
}

#[cfg(all(test, feature = "images"))]
mod tests {
    use super::*;

    // Catches the worker handing PNG bytes over undecoded or with rows
    // swapped.
    #[test]
    fn png_bytes_decode_to_their_pixels() {
        let pixels: Vec<u8> = (0..3 * 2 * 4).map(|i| i as u8 * 10).collect();
        let png = image::RgbaImage::from_raw(3, 2, pixels.clone()).unwrap();
        let mut bytes = std::io::Cursor::new(Vec::new());
        png.write_to(&mut bytes, image::ImageFormat::Png).unwrap();

        let decoded = decode("a.png", LoadedImage::Encoded(bytes.into_inner())).unwrap();

        assert_eq!(
            (decoded.width, decoded.height, &*decoded.rgba),
            (3, 2, &pixels[..])
        );
    }
}
