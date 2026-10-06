use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use quark::Color;

static CACHE: Mutex<Option<HashMap<u64, CachedIcon>>> = Mutex::new(None);

struct CachedIcon {
    rgba: Arc<[u8]>,
    width: u32,
    height: u32,
}

pub fn cache_key(svg: &str, size: u32, color: Color) -> u64 {
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
    h.finish()
}

/// Rasterize `svg` at `size` px tinted with `color`. Results are cached, and
/// the shared bytes make repeat calls a refcount bump instead of a copy.
pub fn rasterize_svg(svg: &str, size: u32, color: Color) -> (Arc<[u8]>, u32, u32) {
    let key = cache_key(svg, size, color);

    let mut guard = CACHE.lock().unwrap();
    let cache = guard.get_or_insert_with(HashMap::new);

    if let Some(cached) = cache.get(&key) {
        return (cached.rgba.clone(), cached.width, cached.height);
    }

    let color_hex = format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b);
    let colored_svg = svg
        .replace("currentColor", &color_hex)
        .replace("stroke=\"#000\"", &format!("stroke=\"{color_hex}\""))
        .replace("fill=\"#000\"", &format!("fill=\"{color_hex}\""));

    let tree = resvg::usvg::Tree::from_str(&colored_svg, &resvg::usvg::Options::default());
    let tree = match tree {
        Ok(t) => t,
        Err(_) => {
            let empty = vec![0u8; (size * size * 4) as usize];
            return (empty.into(), size, size);
        }
    };

    let svg_size = tree.size();
    let scale = size as f32 / svg_size.width().max(svg_size.height());
    let w = (svg_size.width() * scale).ceil() as u32;
    let h = (svg_size.height() * scale).ceil() as u32;

    let mut pixmap = resvg::tiny_skia::Pixmap::new(w.max(1), h.max(1)).unwrap();
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let mut rgba = pixmap.data().to_vec();
    if color.a < 255 {
        let mult = color.a as u16;
        for px in rgba.chunks_exact_mut(4) {
            px[0] = ((px[0] as u16 * mult) / 255) as u8;
            px[1] = ((px[1] as u16 * mult) / 255) as u8;
            px[2] = ((px[2] as u16 * mult) / 255) as u8;
            px[3] = ((px[3] as u16 * mult) / 255) as u8;
        }
    }
    let rgba: Arc<[u8]> = rgba.into();
    cache.insert(
        key,
        CachedIcon {
            rgba: rgba.clone(),
            width: w,
            height: h,
        },
    );

    (rgba, w, h)
}
