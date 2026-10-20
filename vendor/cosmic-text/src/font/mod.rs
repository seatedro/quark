// SPDX-License-Identifier: MIT OR Apache-2.0

use harfrust::Shaper;
use linebender_resource_handle::{Blob, FontData};
use skrifa::raw::{ReadError, TableProvider as _};
use skrifa::{metrics::Metrics, prelude::*};
// re-export skrifa
pub use skrifa;
// re-export peniko::Font;
#[cfg(feature = "peniko")]
pub use linebender_resource_handle::FontData as PenikoFont;

use core::fmt;

use alloc::sync::Arc;
#[cfg(not(feature = "std"))]
use alloc::{string::String, vec::Vec};

use self_cell::self_cell;

pub mod fallback;
pub use fallback::{Fallback, PlatformFallback};

pub use self::system::*;
mod system;

struct OwnedFaceData {
    data: Arc<dyn AsRef<[u8]> + Send + Sync>,
    shaper_data: harfrust::ShaperData,
    shaper_instance: harfrust::ShaperInstance,
    metrics: Metrics,
}

self_cell!(
    struct OwnedFace {
        owner: OwnedFaceData,

        #[covariant]
        dependent: Shaper,
    }
);

struct FontMonospaceFallback {
    monospace_em_width: Option<f32>,
    scripts: Vec<[u8; 4]>,
    unicode_codepoints: Vec<u32>,
}

/// A font
pub struct Font {
    #[cfg(feature = "swash")]
    swash: (u32, swash::CacheKey),
    harfrust: OwnedFace,
    data: FontData,
    id: fontdb::ID,
    monospace_fallback: Option<FontMonospaceFallback>,
    wght: Option<f32>,
}

impl fmt::Debug for Font {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Font")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Font {
    pub const fn id(&self) -> fontdb::ID {
        self.id
    }

    /// The `wght` coordinate the font was built at, if it has the axis.
    pub fn wght(&self) -> Option<f32> {
        self.wght
    }

    pub fn monospace_em_width(&self) -> Option<f32> {
        self.monospace_fallback
            .as_ref()
            .and_then(|x| x.monospace_em_width)
    }

    pub fn scripts(&self) -> &[[u8; 4]] {
        self.monospace_fallback.as_ref().map_or(&[], |x| &x.scripts)
    }

    pub fn unicode_codepoints(&self) -> &[u32] {
        self.monospace_fallback
            .as_ref()
            .map_or(&[], |x| &x.unicode_codepoints)
    }

    pub fn data(&self) -> &[u8] {
        self.data.data.data()
    }

    pub fn shaper(&self) -> &harfrust::Shaper<'_> {
        self.harfrust.borrow_dependent()
    }

    pub(crate) fn shaper_instance(&self) -> &harfrust::ShaperInstance {
        &self.harfrust.borrow_owner().shaper_instance
    }

    pub fn metrics(&self) -> &Metrics {
        &self.harfrust.borrow_owner().metrics
    }

    #[cfg(feature = "peniko")]
    pub fn as_peniko(&self) -> PenikoFont {
        self.data.clone()
    }

    #[cfg(feature = "swash")]
    pub fn as_swash(&self) -> swash::FontRef<'_> {
        let swash = &self.swash;
        swash::FontRef {
            data: self.data(),
            offset: swash.0,
            key: swash.1,
        }
    }
}

impl Font {
    pub fn new(db: &fontdb::Database, id: fontdb::ID, weight: fontdb::Weight) -> Option<Self> {
        let info = db.face(id)?;

        let data = match &info.source {
            fontdb::Source::Binary(data) => Arc::clone(data),
            #[cfg(feature = "std")]
            fontdb::Source::File(path) => {
                log::warn!("Unsupported fontdb Source::File('{}')", path.display());
                return None;
            }
            #[cfg(feature = "std")]
            fontdb::Source::SharedFile(_path, data) => Arc::clone(data),
        };

        // It's a bit unfortunate but we need to parse the data into a `FontRef`
        // twice--once to construct the HarfRust `ShaperInstance` and
        // `ShaperData`, and once to create the persistent `FontRef` tied to the
        // lifetime of the face data.
        let font_ref = FontRef::from_index((*data).as_ref(), info.index).ok()?;
        let wght = wght_coordinate(&font_ref, weight);
        let location = font_ref
            .axes()
            .location(wght.map(|value| (Tag::new(b"wght"), value)));
        let metrics = font_ref.metrics(Size::unscaled(), &location);

        let monospace_fallback = if cfg!(feature = "monospace_fallback") {
            (|| {
                let glyph_metrics = font_ref.glyph_metrics(Size::unscaled(), &location);
                let charmap = font_ref.charmap();
                let monospace_em_width = info
                    .monospaced
                    .then(|| {
                        let hor_advance = glyph_metrics.advance_width(charmap.map(' ')?)?;
                        let upem = metrics.units_per_em;
                        Some(hor_advance / f32::from(upem))
                    })
                    .flatten();

                if info.monospaced && monospace_em_width.is_none() {
                    None?;
                }

                let scripts = font_ref
                    .gpos()
                    .ok()?
                    .script_list()
                    .ok()?
                    .script_records()
                    .iter()
                    .chain(
                        font_ref
                            .gsub()
                            .ok()?
                            .script_list()
                            .ok()?
                            .script_records()
                            .iter(),
                    )
                    .map(|script| script.script_tag().into_bytes())
                    .collect();

                let mut unicode_codepoints = Vec::new();

                for (code_point, _) in charmap.mappings() {
                    unicode_codepoints.push(code_point);
                }

                unicode_codepoints.shrink_to_fit();

                Some(FontMonospaceFallback {
                    monospace_em_width,
                    scripts,
                    unicode_codepoints,
                })
            })()
        } else {
            None
        };

        let (shaper_instance, shaper_data) = {
            (
                harfrust::ShaperInstance::from_coords(&font_ref, location.coords().iter().copied()),
                harfrust::ShaperData::new(&font_ref),
            )
        };

        Some(Self {
            id: info.id,
            monospace_fallback,
            wght,
            #[cfg(feature = "swash")]
            swash: {
                let swash = swash::FontRef::from_index((*data).as_ref(), info.index as usize)?;
                (swash.offset, swash.key)
            },
            harfrust: OwnedFace::try_new(
                OwnedFaceData {
                    data: Arc::clone(&data),
                    shaper_data,
                    shaper_instance,
                    metrics,
                },
                |OwnedFaceData {
                     data,
                     shaper_data,
                     shaper_instance,
                     ..
                 }| {
                    let font_ref = FontRef::from_index((**data).as_ref(), info.index)?;
                    let shaper = shaper_data
                        .shaper(&font_ref)
                        .instance(Some(shaper_instance))
                        .build();
                    Ok::<_, ReadError>(shaper)
                },
            )
            .ok()?,
            data: FontData::new(Blob::new(data), info.index),
        })
    }
}

/// The `wght` coordinate that draws CSS `weight` in `font`, or `None` if
/// it has no `wght` axis. Most axes use the CSS scale, so the weight is
/// clamped to the axis. An axis on another scale (Berkeley Mono's 100 to
/// 150, which leaves its OS/2 weight class outside the axis) is mapped
/// through its named instances and its default at the weight class.
pub fn wght_coordinate(font: &FontRef<'_>, weight: fontdb::Weight) -> Option<f32> {
    let axes = font.axes();
    let axis = axes.iter().find(|axis| axis.tag() == Tag::new(b"wght"))?;
    let (min, max) = (axis.min_value(), axis.max_value());
    let requested = f32::from(weight.0);
    let class = font
        .os2()
        .map_or(0.0, |os2| f32::from(os2.us_weight_class()));
    if !(1.0..=1000.0).contains(&class) || (min..=max).contains(&class) {
        return Some(requested.clamp(min, max));
    }

    // (CSS weight, axis value) pairs, sorted by weight.
    let mut anchors: Vec<(f32, f32)> = Vec::from([(class, axis.default_value())]);
    for instance in font.named_instances().iter() {
        let name: String = font
            .localized_strings(instance.subfamily_name_id())
            .english_or_first()
            .map(|name| {
                name.chars()
                    .filter(char::is_ascii_alphanumeric)
                    .map(|c| c.to_ascii_lowercase())
                    .collect()
            })
            .unwrap_or_default();
        let name = name.trim_end_matches("italic").trim_end_matches("oblique");
        if let (Some(css), Some(value)) =
            (css_weight(name), instance.user_coords().nth(axis.index()))
        {
            anchors.push((css, value));
        }
    }
    anchors.sort_by(|a, b| a.0.total_cmp(&b.0));
    anchors.dedup_by_key(|anchor| anchor.0);
    // Drop instances that would make heavier weights draw lighter.
    let mut last = f32::NEG_INFINITY;
    anchors.retain(|&(_, value)| {
        let keep = value >= last;
        if keep {
            last = value;
        }
        keep
    });
    if anchors[0].0 > 1.0 {
        anchors.insert(0, (1.0, min));
    }
    if anchors[anchors.len() - 1].0 < 1000.0 {
        anchors.push((1000.0, max));
    }

    let value = anchors
        .windows(2)
        .find(|pair| requested <= pair[1].0)
        .map_or(anchors[anchors.len() - 1].1, |pair| {
            let ((w0, v0), (w1, v1)) = (pair[0], pair[1]);
            v0 + (v1 - v0) * ((requested - w0) / (w1 - w0)).clamp(0.0, 1.0)
        });
    Some(value.clamp(min, max))
}

/// The CSS weight of a standard weight name, lowercase without spaces.
fn css_weight(name: &str) -> Option<f32> {
    Some(match name {
        "thin" | "hairline" => 100.0,
        "extralight" | "ultralight" => 200.0,
        "light" => 300.0,
        "" | "regular" | "normal" | "book" => 400.0,
        "medium" => 500.0,
        "semibold" | "demibold" => 600.0,
        "bold" => 700.0,
        "extrabold" | "ultrabold" => 800.0,
        "black" | "heavy" => 900.0,
        _ => return None,
    })
}

#[cfg(test)]
mod test {
    #[test]
    fn test_fonts_load_time() {
        use crate::FontSystem;
        use sys_locale::get_locale;

        #[cfg(not(target_arch = "wasm32"))]
        let now = std::time::Instant::now();

        let mut db = fontdb::Database::new();
        let locale = get_locale().expect("Local available");
        db.load_system_fonts();
        FontSystem::new_with_locale_and_db(locale, db);

        #[cfg(not(target_arch = "wasm32"))]
        println!("Fonts load time {}ms.", now.elapsed().as_millis());
    }
}
