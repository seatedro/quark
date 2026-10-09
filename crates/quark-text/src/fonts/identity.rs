//! Which exact font bytes, face, and variation instance a shaped glyph came
//! from, independent of any one font database.
//!
//! A cosmic-text face id (`fontdb::ID`) names a face only within the
//! database that issued it: another [`crate::TextSystem`] hands the same
//! ids out again for other fonts, and a family or PostScript name can
//! belong to several files. Rasterizers and their caches key glyphs by the
//! ids here instead. [`FontSourceId`] names one immutable byte content for
//! the life of the process, so the same bytes loaded by two systems (or
//! again after a font change) share it, and different bytes never do.
//! [`FontInstanceId`] adds the collection face, the full set of variation
//! axis values shaping used, and the synthesized styling.
//!
//! A [`FontRegistry`] maps the glyphs of one [`FontSnapshot`] to
//! [`PreparedFont`]s, the handle a rasterizer receives: the bytes, face
//! index, variations, and synthesis to draw exactly the glyph that was
//! shaped. It does no family matching; a glyph id always means the face it
//! was shaped from.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use cosmic_text::skrifa::raw::TableProvider;
use cosmic_text::skrifa::{FontRef, MetadataProvider};
use cosmic_text::{CacheKeyFlags, fontdb};

use super::{FontSnapshot, VENDORED_FONT_BYTES};
use crate::epoch::FontEpoch;

type FontBytes = Arc<dyn AsRef<[u8]> + Send + Sync>;

/// One immutable font file's bytes, for as long as the process runs. Two
/// sources with equal bytes share an id while either is alive, however
/// they were loaded; bytes that change (a font file replaced on disk)
/// get a new id, so the id also serves as the source's revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FontSourceId(u32);

/// One face of a source: a TrueType collection holds several, a plain font
/// file one at index 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FaceId {
    pub source: FontSourceId,
    pub index: u32,
}

/// A face at one set of variation axis values with one synthesized
/// styling: everything about a font that decides a glyph's outline at a
/// given size. Equal instances share an id for the life of the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FontInstanceId(u32);

/// An OpenType variation axis and the user-space value it is set to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Variation {
    pub tag: [u8; 4],
    pub value: f32,
}

/// Styling a rasterizer adds to outlines that lack it. Real bold and
/// italic faces are matched first; these only apply when the family has
/// no such face, or when text asks for thickening.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Synthesis {
    /// Slant outlines by 14 degrees, for italic text in a family with no
    /// italic face (cosmic-text's `FAKE_ITALIC`).
    pub italic: bool,
    /// Widen outlines by a fiftieth of the physical em on each side
    /// (cosmic-text's `THICKEN`, a terminal option). Not semantic bold.
    pub thicken: bool,
}

impl Synthesis {
    /// The synthesis a glyph's cosmic-text flags ask for. The other flags
    /// are rasterization options (hinting, pixel fonts) or paint (the
    /// background luminance of linear correction), not font identity.
    pub fn from_flags(flags: CacheKeyFlags) -> Self {
        Self {
            italic: flags.contains(CacheKeyFlags::FAKE_ITALIC),
            thicken: flags.contains(CacheKeyFlags::THICKEN),
        }
    }
}

/// Where a source's bytes came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FontProvenance {
    /// A font quark-text embeds.
    Bundled,
    /// Bytes the app loaded ([`crate::TextSystem::load_font_data`]).
    Application,
    /// An installed font file, mapped from `path`. A native rasterizer may
    /// open the file itself, but must check it holds these bytes' face.
    Installed { path: Arc<Path> },
}

/// One font file's bytes, shared with the font database it came from.
pub struct FontSource {
    id: FontSourceId,
    data: FontBytes,
    provenance: FontProvenance,
}

impl FontSource {
    pub fn id(&self) -> FontSourceId {
        self.id
    }

    pub fn data(&self) -> &[u8] {
        (*self.data).as_ref()
    }

    /// The bytes as shared, for a rasterizer that keeps them alive in a
    /// native face without copying.
    pub fn shared_data(&self) -> &Arc<dyn AsRef<[u8]> + Send + Sync> {
        &self.data
    }

    pub fn provenance(&self) -> &FontProvenance {
        &self.provenance
    }
}

impl std::fmt::Debug for FontSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontSource")
            .field("id", &self.id)
            .field("bytes", &self.data().len())
            .field("provenance", &self.provenance)
            .finish()
    }
}

/// Everything a rasterizer needs to draw glyphs of one shaped font
/// instance: the exact bytes and face, every variation axis at the value
/// shaping used, and the synthesis. Cheap to clone.
#[derive(Debug, Clone)]
pub struct PreparedFont(Arc<PreparedInner>);

#[derive(Debug)]
struct PreparedInner {
    instance: FontInstanceId,
    face: FaceId,
    source: Arc<FontSource>,
    variations: Arc<[Variation]>,
    synthesis: Synthesis,
    glyph_count: u32,
}

impl PreparedFont {
    pub fn instance(&self) -> FontInstanceId {
        self.0.instance
    }

    pub fn face(&self) -> FaceId {
        self.0.face
    }

    pub fn source(&self) -> &FontSource {
        &self.0.source
    }

    /// The face's index in its source's collection.
    pub fn index(&self) -> u32 {
        self.0.face.index
    }

    /// Every axis the face has, sorted by tag, at the value shaping used:
    /// `wght` at the text's weight clamped to the axis, every other axis
    /// (`opsz` included) at its default, as harfrust shapes it. Apply all
    /// of them; a rasterizer that picks an axis value itself (an automatic
    /// optical size, say) draws glyphs shaping never measured.
    pub fn variations(&self) -> &[Variation] {
        &self.0.variations
    }

    pub fn synthesis(&self) -> Synthesis {
        self.0.synthesis
    }

    /// How many glyphs the face has; glyph ids at or past it are invalid.
    pub fn glyph_count(&self) -> u32 {
        self.0.glyph_count
    }
}

/// Why a glyph's face cannot be prepared.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FontSourceError {
    /// The snapshot's database has no face with the id: the glyph was
    /// shaped by another text system.
    #[error("no face with this id in the font snapshot")]
    UnknownFace,
    /// The face's file could not be read.
    #[error("the face's file could not be read")]
    NotLoaded,
    /// The face's bytes do not parse at its collection index.
    #[error("the face's bytes do not parse")]
    Unparsable,
}

/// Maps the faces of one [`FontSnapshot`] to their sources, and shaped
/// glyphs' face, weight, and flags to [`PreparedFont`]s. Remembers each
/// answer, failures included, so preparing a face again costs a lookup;
/// the first preparation of a source reads its bytes once to identify
/// them. One per rasterizing thread.
#[derive(Debug)]
pub struct FontRegistry {
    snapshot: FontSnapshot,
    faces: HashMap<fontdb::ID, Result<Arc<FaceRecord>, FontSourceError>>,
    instances: HashMap<(fontdb::ID, u16, Synthesis), Result<PreparedFont, FontSourceError>>,
}

#[derive(Debug)]
struct FaceRecord {
    face: FaceId,
    source: Arc<FontSource>,
    axes: Vec<Axis>,
    glyph_count: u32,
}

#[derive(Debug, Clone, Copy)]
struct Axis {
    tag: [u8; 4],
    min: f32,
    default: f32,
    max: f32,
}

impl FontRegistry {
    pub fn new(snapshot: FontSnapshot) -> Self {
        Self {
            snapshot,
            faces: HashMap::new(),
            instances: HashMap::new(),
        }
    }

    pub fn epoch(&self) -> FontEpoch {
        self.snapshot.epoch()
    }

    pub fn snapshot(&self) -> &FontSnapshot {
        &self.snapshot
    }

    /// Prepares glyphs from `snapshot` from now on. Forgets the old
    /// snapshot's faces when its epoch differs, since its face ids may
    /// name other faces now; source and instance ids stay the same for
    /// the same bytes, so rasters cached under them stay valid.
    pub fn set_snapshot(&mut self, snapshot: FontSnapshot) {
        if snapshot.epoch() != self.snapshot.epoch() {
            self.faces.clear();
            self.instances.clear();
        }
        self.snapshot = snapshot;
    }

    /// The instance a glyph shaped from face `font_id` at `weight` with
    /// cosmic-text `flags` draws in (a [`crate::Glyph`]'s `font_id`,
    /// `font_weight`, and `flags`).
    pub fn prepare(
        &mut self,
        font_id: fontdb::ID,
        weight: fontdb::Weight,
        flags: CacheKeyFlags,
    ) -> Result<&PreparedFont, FontSourceError> {
        let synthesis = Synthesis::from_flags(flags);
        let key = (font_id, weight.0, synthesis);
        if !self.instances.contains_key(&key) {
            let prepared = self.face(font_id).map(|face| {
                let variations = face_variations(&face.axes, weight);
                PreparedFont(Arc::new(PreparedInner {
                    instance: intern_instance(face.face, &variations, synthesis),
                    face: face.face,
                    source: face.source.clone(),
                    variations,
                    synthesis,
                    glyph_count: face.glyph_count,
                }))
            });
            self.instances.insert(key, prepared);
        }
        self.instances[&key].as_ref().map_err(Clone::clone)
    }

    fn face(&mut self, font_id: fontdb::ID) -> Result<Arc<FaceRecord>, FontSourceError> {
        let snapshot = &self.snapshot;
        self.faces
            .entry(font_id)
            .or_insert_with(|| face_record(snapshot.database(), font_id).map(Arc::new))
            .clone()
    }
}

fn face_record(db: &fontdb::Database, font_id: fontdb::ID) -> Result<FaceRecord, FontSourceError> {
    let info = db.face(font_id).ok_or(FontSourceError::UnknownFace)?;
    let (data, path) = match &info.source {
        fontdb::Source::Binary(data) => (data.clone(), None),
        fontdb::Source::SharedFile(path, data) => (data.clone(), Some(path)),
        // A face the snapshot's database never loaded: shaping loads it
        // into its own database when it first needs it, after a snapshot
        // taken earlier was copied, so read the file here.
        fontdb::Source::File(path) => (read_font_file(path)?, Some(path)),
    };
    let font = FontRef::from_index((*data).as_ref(), info.index)
        .map_err(|_| FontSourceError::Unparsable)?;
    let mut axes: Vec<Axis> = font
        .axes()
        .iter()
        .map(|axis| Axis {
            tag: axis.tag().to_be_bytes(),
            min: axis.min_value(),
            default: axis.default_value(),
            max: axis.max_value(),
        })
        .collect();
    axes.sort_by_key(|axis| axis.tag);
    // A font with the same axis twice is malformed; keep the first, as
    // shaping's location lookup does.
    axes.dedup_by_key(|axis| axis.tag);
    let glyph_count = font.maxp().map_or(0, |maxp| u32::from(maxp.num_glyphs()));
    let bundled = VENDORED_FONT_BYTES
        .iter()
        .any(|bytes| same_memory(bytes, (*data).as_ref()));
    let provenance = match path {
        _ if bundled => FontProvenance::Bundled,
        Some(path) => FontProvenance::Installed {
            path: Arc::from(path.as_path()),
        },
        None => FontProvenance::Application,
    };
    let id = intern_source(&data, bundled);
    Ok(FaceRecord {
        face: FaceId {
            source: id,
            index: info.index,
        },
        source: Arc::new(FontSource {
            id,
            data,
            provenance,
        }),
        axes,
        glyph_count,
    })
}

/// Every axis at the value shaping uses: cosmic-text builds a face's
/// harfrust instance from the text's weight on `wght` alone, which skrifa
/// clamps to the axis, leaving the rest at their defaults.
fn face_variations(axes: &[Axis], weight: fontdb::Weight) -> Arc<[Variation]> {
    axes.iter()
        .map(|axis| Variation {
            tag: axis.tag,
            value: if &axis.tag == b"wght" {
                f32::from(weight.0).clamp(axis.min, axis.max)
            } else {
                axis.default
            },
        })
        .collect()
}

/// The bytes of the font file at `path`, read once per process while any
/// registry holds them.
fn read_font_file(path: &std::path::Path) -> Result<FontBytes, FontSourceError> {
    type Files = HashMap<std::path::PathBuf, Weak<dyn AsRef<[u8]> + Send + Sync>>;
    static FILES: OnceLock<Mutex<Files>> = OnceLock::new();
    let mut files = FILES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(data) = files.get(path).and_then(Weak::upgrade) {
        return Ok(data);
    }
    let data: FontBytes = Arc::new(std::fs::read(path).map_err(|_| FontSourceError::NotLoaded)?);
    files.retain(|_, data| data.strong_count() > 0);
    files.insert(path.to_path_buf(), Arc::downgrade(&data));
    Ok(data)
}

fn same_memory(a: &[u8], b: &[u8]) -> bool {
    a.as_ptr() == b.as_ptr() && a.len() == b.len()
}

/// Sources by content, process-wide. Entries hold their bytes weakly
/// (bundled ones strongly, as they are static anyway), so an id outlives
/// its bytes only as a number never handed out again.
struct SourceTable {
    next: u32,
    entries: Vec<SourceEntry>,
}

struct SourceEntry {
    id: FontSourceId,
    hash: u64,
    data: Weak<dyn AsRef<[u8]> + Send + Sync>,
    /// Keeps bundled bytes' entry alive, so they are never hashed twice.
    _bundled: Option<FontBytes>,
}

fn intern_source(data: &FontBytes, bundled: bool) -> FontSourceId {
    static TABLE: OnceLock<Mutex<SourceTable>> = OnceLock::new();
    let mut table = TABLE
        .get_or_init(|| {
            Mutex::new(SourceTable {
                next: 0,
                entries: Vec::new(),
            })
        })
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let bytes = (**data).as_ref();
    // The same memory, still alive, holds the same bytes: fonts are
    // immutable. Databases cloned into snapshots and twins built from a
    // recipe share their sources' memory, so this is the common case.
    for entry in &table.entries {
        if let Some(live) = entry.data.upgrade()
            && same_memory((*live).as_ref(), bytes)
        {
            return entry.id;
        }
    }
    let hash = {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut hasher);
        hasher.finish()
    };
    // Equal hashes are checked byte for byte, so a collision cannot merge
    // two fonts.
    let found = table.entries.iter().find_map(|entry| {
        let live = entry.data.upgrade()?;
        (entry.hash == hash && (*live).as_ref() == bytes).then_some(entry.id)
    });
    table.entries.retain(|entry| entry.data.strong_count() > 0);
    let id = found.unwrap_or_else(|| {
        let id = FontSourceId(table.next);
        table.next += 1;
        id
    });
    table.entries.push(SourceEntry {
        id,
        hash,
        data: Arc::downgrade(data),
        _bundled: bundled.then(|| data.clone()),
    });
    id
}

/// Instances by face, exact variation values, and synthesis, process-wide.
/// Instances are few (faces times the weights text uses), so the table is
/// never pruned.
fn intern_instance(face: FaceId, variations: &[Variation], synthesis: Synthesis) -> FontInstanceId {
    type Key = (FaceId, Vec<([u8; 4], u32)>, Synthesis);
    static TABLE: OnceLock<Mutex<HashMap<Key, FontInstanceId>>> = OnceLock::new();
    let mut table = TABLE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Bits, with negative zero folded into zero, so equal values are equal
    // keys.
    let values = variations
        .iter()
        .map(|v| (v.tag, (v.value + 0.0).to_bits()))
        .collect();
    let next = FontInstanceId(table.len() as u32);
    *table.entry((face, values, synthesis)).or_insert(next)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use quark::FontWeight;
    use quark::scene::FontStyle;

    use super::*;
    use crate::system::{RENAMED_INTER, renamed_inter};
    use crate::{FontSettings, Glyph, TextParams, TextSpan, TextStyle, TextSystem};

    /// The first glyph of `params` laid out by `system`, and the font a
    /// registry over the system's snapshot prepares for it.
    fn first_glyph(system: &mut TextSystem, params: &TextParams) -> (Glyph, PreparedFont) {
        let layout = system.layout(params).expect("layout");
        let glyph = layout.glyph(0).expect("glyph");
        let mut registry = FontRegistry::new(system.font_snapshot());
        let font = registry
            .prepare(glyph.font_id, glyph.font_weight, glyph.flags)
            .expect("prepared")
            .clone();
        (glyph, font)
    }

    /// A system whose UI family is [`RENAMED_INTER`], from `bytes`.
    fn system_with(bytes: Vec<u8>) -> TextSystem {
        let mut system = TextSystem::vendored_only(&FontSettings {
            ui_family: RENAMED_INTER.into(),
            ..FontSettings::default()
        });
        system.load_font_data(Arc::new(bytes));
        system
    }

    /// Geist Regular renamed to [`RENAMED_INTER`]: another font with the
    /// same family name.
    fn renamed_geist() -> Vec<u8> {
        let mut bytes = include_bytes!("../../assets/fonts/Geist-Regular.otf").to_vec();
        let utf16 =
            |name: &str| -> Vec<u8> { name.encode_utf16().flat_map(u16::to_be_bytes).collect() };
        for (from, to) in [
            (b"Geist".to_vec(), RENAMED_INTER.as_bytes().to_vec()),
            (utf16("Geist"), utf16(RENAMED_INTER)),
        ] {
            let mut at = 0;
            while let Some(i) = bytes[at..].windows(from.len()).position(|w| w == from) {
                bytes[at + i..at + i + from.len()].copy_from_slice(&to);
                at += i + from.len();
            }
        }
        bytes
    }

    // Regression: a face listed but not yet loaded when the snapshot was
    // copied (shaping loads it later, into its own database) failed to
    // prepare, so installed fonts never reached a native rasterizer.
    #[test]
    fn a_face_the_snapshot_never_loaded_prepares_from_its_file() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/fonts/IBMPlexMono-Regular.ttf"
        );
        let mut db = fontdb::Database::new();
        db.load_font_file(path).expect("font file");
        let id = db.faces().next().expect("a face").id;
        assert!(matches!(
            db.face(id).map(|f| &f.source),
            Some(fontdb::Source::File(_))
        ));
        let snapshot = crate::FontSnapshot {
            epoch: crate::FontEpoch {
                system: crate::TextSystemId::next(),
                generation: 0,
            },
            database: Arc::new(db),
        };
        let mut registry = FontRegistry::new(snapshot);
        let font = registry
            .prepare(id, fontdb::Weight::NORMAL, CacheKeyFlags::empty())
            .expect("prepared from the file");
        assert_eq!(font.source().data(), std::fs::read(path).expect("read"));
        assert!(matches!(
            font.source().provenance(),
            FontProvenance::Installed { .. }
        ));
    }

    // Catches keying rasters by cosmic-text face id or family name: two
    // systems that each loaded a different font last hand both out under
    // the same id and name, so a glyph rasterized for one would draw for
    // the other.
    #[test]
    fn same_face_id_and_family_from_different_bytes_are_different_sources() {
        let params = TextParams::new("Q", TextStyle::new(14.0));
        let (inter_glyph, inter) = first_glyph(&mut system_with(renamed_inter()), &params);
        let (geist_glyph, geist) = first_glyph(&mut system_with(renamed_geist()), &params);

        assert_eq!(inter_glyph.font_id, geist_glyph.font_id);
        assert_ne!(inter.face().source, geist.face().source);
        assert_ne!(inter.instance(), geist.instance());
    }

    // Catches identity tied to one load of a font: an app that loads the
    // same file into two systems (or again after a reload) must reuse the
    // rasters cached for it.
    #[test]
    fn same_bytes_loaded_twice_are_one_instance() {
        let params = TextParams::new("Q", TextStyle::new(14.0));
        let mut first = system_with(renamed_inter());
        let mut second = system_with(renamed_inter());
        let (_, a) = first_glyph(&mut first, &params);
        let (_, b) = first_glyph(&mut second, &params);

        assert_eq!(a.instance(), b.instance());
        assert_eq!(a.source().provenance(), &FontProvenance::Application);
    }

    // Catches a rasterizer drawing an instance shaping never measured: wght
    // is the text's weight clamped to the axis, as harfrust shapes it, and
    // every other axis (Inter's opsz) is pinned at the default shaping used
    // rather than left for a native rasterizer to choose by size.
    #[test]
    fn variations_are_the_values_shaping_used() {
        let mut system = crate::system::test_system();
        let cases = [
            (
                "Inter",
                FontWeight::Numeric(650),
                vec![(*b"opsz", 14.0), (*b"wght", 650.0)],
            ),
            (
                "Inter",
                FontWeight::Numeric(1000),
                vec![(*b"opsz", 14.0), (*b"wght", 900.0)],
            ),
            ("Geist", FontWeight::Bold, vec![]),
        ];
        for (family, weight, expected) in cases {
            let style = TextStyle::new(14.0).family(Some(family)).weight(weight);
            let (_, font) = first_glyph(&mut system, &TextParams::new("a", style));
            let got: Vec<_> = font.variations().iter().map(|v| (v.tag, v.value)).collect();
            assert_eq!(got, expected, "{family} {weight:?}");
        }
    }

    // Catches paint riding into font identity: linear correction stores the
    // background's luminance in the glyph flags, but changes no outline, so
    // text on two backgrounds shares rasters. Synthetic italic does change
    // the outline.
    #[test]
    fn only_synthesis_flags_change_the_instance() {
        let mut system = crate::system::test_system();
        let plain = TextStyle::new(14.0);
        let italic = TextParams::new("a", plain).spans(vec![TextSpan {
            range: 0..1,
            weight: None,
            style: Some(FontStyle::Italic),
            kind: None,
            size: None,
            letter_spacing: None,
        }]);
        let (_, a) = first_glyph(&mut system, &TextParams::new("a", plain));
        let (_, corrected) = first_glyph(
            &mut system,
            &TextParams::new("a", plain.linear_correction(Some(40))),
        );
        let (_, slanted) = first_glyph(&mut system, &italic);

        assert_eq!(corrected.instance(), a.instance());
        assert_ne!(slanted.instance(), a.instance());
        assert_eq!(
            slanted.synthesis(),
            Synthesis {
                italic: true,
                thicken: false
            }
        );
    }
}
