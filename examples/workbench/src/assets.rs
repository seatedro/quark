//! Local images and grammar-pack discovery. Nothing here touches the
//! network.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use quark_app::quark_ui::document::{ImageLoader, LoadedImage};

/// A bundled snapshot of the fixture app, for the preview panel and the
/// transcript image.
pub const PREVIEW_PNG: &[u8] = include_bytes!("../assets/preview.png");
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

/// Where grammar packs come from: `$QUARK_SYNTAX_PACKS`, else
/// `assets/syntax-packs` beside the executable, else the workspace's
/// `target/syntax-packs` (what `cargo run -p syntax-pack -- build` writes).
/// Each is a pack root as the store reads it, holding
/// `<target triple>/<language>/`, and counts only when it has packs for
/// this target. `None` when none does: code renders as plain text.
pub fn grammar_pack_root() -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("QUARK_SYNTAX_PACKS") {
        return Some(PathBuf::from(root));
    }
    use quark_app::quark_ui::quark_syntax::pack::TARGET;
    let has_packs = |root: &Path| root.join(TARGET).is_dir();
    let beside_exe = std::env::current_exe().ok().and_then(|exe| {
        let root = exe.parent()?.join("assets/syntax-packs");
        has_packs(&root).then_some(root)
    });
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/syntax-packs");
    beside_exe.or_else(|| has_packs(&workspace).then_some(workspace))
}

/// A grammar store over the local packs, with downloads off.
pub fn grammar_store() -> Option<quark_app::quark_ui::quark_syntax::GrammarStore> {
    use quark_app::quark_ui::quark_syntax::{GrammarStore, StoreConfig};
    let root = grammar_pack_root()?;
    Some(GrammarStore::new(StoreConfig::new().local_packs(root)))
}
