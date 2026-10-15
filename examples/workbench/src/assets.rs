//! Bundled images. Nothing here touches the network.

use std::sync::Arc;

use quark_app::quark_ui::document::{ImageLoader, LoadedImage};

/// A bundled snapshot of the fixture app, for the preview panel and the
/// transcript image. Its pixels are at 2x: both show it at
/// [`PREVIEW_SIZE`] points, sharp on a 2x screen and mipmapped on a 1x
/// one (the transcript reads the density from the size hint).
pub const PREVIEW_PNG: &[u8] = include_bytes!("../assets/preview@2x.png");
pub const PREVIEW_SIZE: (u32, u32) = (960, 600);
/// The fixture attachment the composer tests attach.
pub const ATTACHMENT_PNG: &[u8] = include_bytes!("../assets/attachment.png");
pub const ATTACHMENT_SIZE: (u32, u32) = (320, 200);

/// Bytes of a bundled image by the name markdown refers to it with.
pub fn image_bytes(src: &str) -> Option<&'static [u8]> {
    match src {
        "preview.png" => Some(PREVIEW_PNG),
        "attachment.png" => Some(ATTACHMENT_PNG),
        _ => None,
    }
}

/// The document image loader: bundled images by name, anything else
/// missing (the document shows its alt text and a retry state).
pub fn image_loader() -> ImageLoader {
    Arc::new(|src: &str| image_bytes(src).map(|b| LoadedImage::Encoded(b.to_vec())))
}
