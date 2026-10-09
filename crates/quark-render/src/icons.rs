use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use quark::Color;
use quark_text::{FontEpoch, TextSystem};

/// Largest side an icon is rasterized at. Bigger requests (a NaN or huge
/// rect turned into a pixel size) render at this size and scale up.
pub const MAX_ICON_PX: u32 = 1024;

/// Rasterized bytes the CPU cache keeps before evicting the least recently
/// used icons.
const CACHE_BUDGET_BYTES: usize = 32 << 20;

static CACHE: Mutex<IconCache> = Mutex::new(IconCache {
    icons: None,
    bytes: 0,
    tick: 0,
});

struct IconCache {
    icons: Option<HashMap<u64, CachedIcon>>,
    bytes: usize,
    tick: u64,
}

struct CachedIcon {
    rgba: Arc<[u8]>,
    width: u32,
    height: u32,
    last_used: u64,
}

impl IconCache {
    fn get(&mut self, key: u64) -> Option<(Arc<[u8]>, u32, u32)> {
        self.tick += 1;
        let tick = self.tick;
        let icon = self.icons.as_mut()?.get_mut(&key)?;
        icon.last_used = tick;
        Some((icon.rgba.clone(), icon.width, icon.height))
    }

    fn insert(&mut self, key: u64, rgba: Arc<[u8]>, width: u32, height: u32) {
        let icons = self.icons.get_or_insert_with(HashMap::new);
        self.bytes += rgba.len();
        let replaced = icons.insert(
            key,
            CachedIcon {
                rgba,
                width,
                height,
                last_used: self.tick,
            },
        );
        if let Some(old) = replaced {
            self.bytes -= old.rgba.len();
        }
        if self.bytes > CACHE_BUDGET_BYTES {
            // Evict down to three quarters of the budget in one pass, so a
            // working set near the limit does not evict on every insert.
            let mut by_age: Vec<(u64, u64, usize)> = icons
                .iter()
                .map(|(key, icon)| (icon.last_used, *key, icon.rgba.len()))
                .collect();
            by_age.sort_unstable();
            for (_, key, len) in by_age {
                if self.bytes <= CACHE_BUDGET_BYTES / 4 * 3 {
                    break;
                }
                icons.remove(&key);
                self.bytes -= len;
            }
        }
    }
}

/// A panic while holding the cache can only lose cache entries, so a
/// poisoned lock is still usable.
fn lock_cache() -> MutexGuard<'static, IconCache> {
    CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The fonts SVG `<text>` resolves against: an immutable copy of one text
/// system's font database (its faces, generic families, and loaded data
/// shared, not reloaded) at one font epoch. Build one per epoch, not per
/// icon. Only these faces are reachable: SVG text loads no external or
/// network fonts.
#[derive(Clone)]
pub struct SvgFonts {
    epoch: FontEpoch,
    db: Arc<resvg::usvg::fontdb::Database>,
    /// Family of text with no `font-family`: the system's UI family.
    family: String,
}

impl std::fmt::Debug for SvgFonts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SvgFonts")
            .field("epoch", &self.epoch)
            .field("faces", &self.db.len())
            .field("family", &self.family)
            .finish()
    }
}

impl SvgFonts {
    /// A snapshot of `text`'s fonts as they are now.
    pub fn new(text: &TextSystem) -> Self {
        let db = text.font_system().db().clone();
        let family = db
            .family_name(&resvg::usvg::fontdb::Family::SansSerif)
            .to_owned();
        Self {
            epoch: text.font_epoch(),
            db: Arc::new(db),
            family,
        }
    }

    /// The epoch of the fonts the snapshot holds.
    pub fn epoch(&self) -> FontEpoch {
        self.epoch
    }

    fn options(&self) -> resvg::usvg::Options<'static> {
        resvg::usvg::Options {
            fontdb: self.db.clone(),
            font_family: self.family.clone(),
            ..Default::default()
        }
    }
}

/// Whether `svg` can draw text, which makes its pixels depend on fonts.
fn has_text(svg: &str) -> bool {
    svg.contains("<text")
}

pub fn cache_key(svg: &str, size: u32, color: Color) -> u64 {
    cache_key_with(svg, size, color, None)
}

/// [`cache_key`] for an SVG rasterized with `fonts`. Text-free SVGs key
/// alike whatever the fonts; ones with text key by the fonts' epoch, so
/// two text systems never share a raster.
pub fn cache_key_with(svg: &str, size: u32, color: Color, fonts: Option<&SvgFonts>) -> u64 {
    use std::hash::{Hash, Hasher};
    // Hash the FULL svg: every lucide icon shares the same `<svg xmlns=…>` prefix,
    // so hashing only a prefix + length collides any two same-length icons (e.g.
    // italic vs external-link), handing back the wrong cached bitmap.
    let mut h = std::collections::hash_map::DefaultHasher::new();
    svg.hash(&mut h);
    size.hash(&mut h);
    color.r.hash(&mut h);
    color.g.hash(&mut h);
    color.b.hash(&mut h);
    color.a.hash(&mut h);
    if has_text(svg) {
        fonts.map(|fonts| fonts.epoch).hash(&mut h);
    }
    h.finish()
}

/// Rasterize `svg` at `size` px (clamped to [`MAX_ICON_PX`]) tinted with
/// `color`. Returns RGBA bytes with `width * height * 4` length; an SVG that
/// does not parse or render yields no pixels and a 0x0 size. Results are
/// cached, and the shared bytes make repeat calls a refcount bump instead of
/// a copy.
pub fn rasterize_svg(svg: &str, size: u32, color: Color) -> (Arc<[u8]>, u32, u32) {
    rasterize_svg_with(svg, size, color, None)
}

/// [`rasterize_svg`] with `<text>` drawn in `fonts`; without fonts SVG text
/// draws nothing.
pub fn rasterize_svg_with(
    svg: &str,
    size: u32,
    color: Color,
    fonts: Option<&SvgFonts>,
) -> (Arc<[u8]>, u32, u32) {
    let key = cache_key_with(svg, size, color, fonts);
    if let Some(hit) = lock_cache().get(key) {
        return hit;
    }
    // Rasterize outside the lock so one large icon does not stall others.
    let (rgba, w, h) = render(svg, size.clamp(1, MAX_ICON_PX), color, fonts)
        .unwrap_or_else(|| (Arc::from([]), 0, 0));
    lock_cache().insert(key, rgba.clone(), w, h);
    (rgba, w, h)
}

fn render(
    svg: &str,
    size: u32,
    color: Color,
    fonts: Option<&SvgFonts>,
) -> Option<(Arc<[u8]>, u32, u32)> {
    let color_hex = format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b);
    let colored_svg = svg
        .replace("currentColor", &color_hex)
        .replace("stroke=\"#000\"", &format!("stroke=\"{color_hex}\""))
        .replace("fill=\"#000\"", &format!("fill=\"{color_hex}\""));

    let options = match fonts {
        Some(fonts) if has_text(svg) => fonts.options(),
        _ => resvg::usvg::Options::default(),
    };
    let tree = resvg::usvg::Tree::from_str(&colored_svg, &options).ok()?;
    let svg_size = tree.size();
    let scale = size as f32 / svg_size.width().max(svg_size.height());
    // The longer side scales to `size`, so both sides stay within it.
    let side = |length: f32| ((length * scale).ceil() as u32).clamp(1, size);
    let (w, h) = (side(svg_size.width()), side(svg_size.height()));

    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let mut rgba = pixmap.take();
    if color.a < 255 {
        let mult = color.a as u16;
        for px in rgba.as_chunks_mut::<4>().0 {
            for channel in px {
                *channel = ((*channel as u16 * mult) / 255) as u8;
            }
        }
    }
    Some((rgba.into(), w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQUARE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><rect width="24" height="24" fill="currentColor"/></svg>"#;

    // Regression: `size * size * 4` overflowed u32 and a huge size asked
    // tiny-skia for a pixmap it refused, which `unwrap` turned into a panic.
    // Every size must come back as a buffer the GPU upload accepts.
    #[test]
    fn rasterize_hostile_sizes_return_bounded_consistent_buffers() {
        let white = Color::rgba(255, 255, 255, 255);
        let cases = [
            (SQUARE, 0, 1),
            (SQUARE, 16, 16),
            (SQUARE, 65_536, MAX_ICON_PX),
            (SQUARE, u32::MAX, MAX_ICON_PX),
            ("not an svg", u32::MAX, 0),
        ];
        for (svg, size, side) in cases {
            let (rgba, w, h) = rasterize_svg(svg, size, white);
            assert_eq!((w, h), (side, side), "size {size}");
            assert_eq!(rgba.len(), w as usize * h as usize * 4, "size {size}");
        }
    }
}
