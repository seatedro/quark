//! Pictures that follow the pointer during a drag, such as a tab being
//! moved. A preview is paint only: it lays nothing out in the window's
//! tree, takes no hits, and has no accessibility node, so everything under
//! it keeps working as if it were not there.
//!
//! A [`DragHandler`] offers one through [`DragHandler::preview`]. The
//! router shows it once the pointer has moved [`DRAG_PREVIEW_THRESHOLD`]
//! from the press, keeps its hotspot under the pointer, and drops it when
//! the drag ends (released or cancelled) or its source leaves the frame
//! ([`InputRouter::drag_preview`]). A drag handed off to a
//! [`DragSession`] takes a copy of its preview along: the session owns it
//! from then on and shows it over whichever window the pointer is in
//! ([`DragSession::preview_in`]), whether or not the source element is
//! still painted. Either way the host paints it with a
//! [`DragPreviewLayer`] after the root, outside every clip and above every
//! z-index. The layer records the preview's drawing once per content key,
//! size, theme, and scale, and moves that recording with the pointer.

use quark_render::scene::Primitive;

use super::*;

/// How far the pointer moves from the press before a preview shows, so a
/// click shows none.
pub const DRAG_PREVIEW_THRESHOLD: f32 = 4.0;

/// The z-index a preview paints at: above anything a window paints. Its
/// own z-indices stack on top of it.
const DRAG_PREVIEW_Z: i32 = i32::MAX - (1 << 16);

/// Builds a preview's picture for a theme and a size.
type PreviewContent = Rc<dyn Fn(&Theme, (f32, f32)) -> AnyElement>;

/// What a drag shows under the pointer.
#[derive(Clone)]
pub struct DragPreview {
    content: PreviewContent,
    key: u64,
    size: (f32, f32),
    pub(super) hotspot: (f32, f32),
}

impl DragPreview {
    /// `content` builds the picture for a theme and a size in points, the
    /// size it is laid out at. `key` names what it shows: pass a different
    /// one whenever `content` would build something else (another title),
    /// so the picture is drawn again.
    pub fn new(
        key: u64,
        size: (f32, f32),
        content: impl Fn(&Theme, (f32, f32)) -> AnyElement + 'static,
    ) -> Self {
        Self {
            content: Rc::new(content),
            key,
            size,
            hotspot: (0.0, 0.0),
        }
    }

    /// The point of the picture held under the pointer, from its top left.
    /// Defaults to the top left.
    pub fn hotspot(mut self, x: f32, y: f32) -> Self {
        self.hotspot = (x, y);
        self
    }

    pub fn set_hotspot(&mut self, x: f32, y: f32) {
        self.hotspot = (x, y);
    }

    pub fn set_size(&mut self, width: f32, height: f32) {
        self.size = (width, height);
    }

    /// Where the picture's top left goes with the pointer at `pointer`.
    pub(crate) fn origin_at(&self, (x, y): (f32, f32)) -> (f32, f32) {
        (x - self.hotspot.0, y - self.hotspot.1)
    }
}

impl std::fmt::Debug for DragPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DragPreview")
            .field("key", &self.key)
            .field("size", &self.size)
            .field("hotspot", &self.hotspot)
            .finish_non_exhaustive()
    }
}

/// Paints a window's drag preview. Hosts keep one per window and call
/// [`Self::paint`] after painting each frame's root.
#[derive(Default)]
pub struct DragPreviewLayer {
    recorded: Option<Recorded>,
}

/// A preview's drawing with its top left at the origin, and what it was
/// drawn for.
struct Recorded {
    key: u64,
    size: (f32, f32),
    theme: Theme,
    scale: f32,
    primitives: Vec<Primitive>,
}

impl DragPreviewLayer {
    /// Paint `preview` with its top left at the given window point on top
    /// of `scene`, drawing it first unless the drawing of the same key,
    /// size, theme, and scale is kept from an earlier frame. `None` paints
    /// nothing and lets the drawing go.
    pub fn paint(
        &mut self,
        preview: Option<(&DragPreview, (f32, f32))>,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        let Some((preview, (x, y))) = preview else {
            self.recorded = None;
            return;
        };
        let current = self.recorded.as_ref().is_some_and(|r| {
            r.key == preview.key
                && r.size == preview.size
                && r.scale == cx.scale_factor
                && r.theme == *cx.theme
        });
        if !current {
            self.recorded = Some(record(preview, cx));
        }
        let Some(recorded) = &self.recorded else {
            return;
        };
        scene.push_z_index(DRAG_PREVIEW_Z);
        for primitive in &recorded.primitives {
            let mut primitive = match primitive {
                Primitive::ZIndexPush(z) => {
                    Primitive::ZIndexPush(DRAG_PREVIEW_Z.saturating_add(*z))
                }
                other => other.clone(),
            };
            primitive.offset(x, y);
            scene.push(primitive);
        }
        scene.pop_z_index();
    }
}

/// Draw `preview` at the origin in a context of its own, so its hits,
/// semantic nodes, and accessibility nodes go nowhere.
fn record(preview: &DragPreview, cx: &mut ElementContext) -> Recorded {
    let mut scene = Scene::default();
    let mut own = ElementContext::new(
        cx.theme,
        cx.scale_factor,
        &mut *cx.text,
        &mut *cx.layouts,
        None,
        cx.signal_store,
    )
    .with_accessibility(false);
    let mut root = (preview.content)(cx.theme, preview.size);
    let (width, height) = preview.size;
    render_element(&mut root, &mut scene, &mut own, width, height);
    Recorded {
        key: preview.key,
        size: preview.size,
        theme: cx.theme.clone(),
        scale: cx.scale_factor,
        primitives: scene.primitives,
    }
}
