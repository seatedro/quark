//! The preview panel (stream E): a bundled snapshot of the fixture app,
//! labelled "Snapshot preview" so it never passes for a browser, at 100%
//! (scrolling) or fitted to the panel. Its status line follows the file
//! store: once the proposed diff is applied it reads "Updated from
//! proposed changes". A snapshot that fails to decode shows why, with
//! Retry.

use quark::view;
use quark_app::ViewContext;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::icons::lucide;
use quark_app::quark_ui::style::Styled;
use quark_components::{Badge, Button, ButtonSize, SegmentedControl, SegmentedItem};

use crate::assets;
use crate::contracts::SurfaceCx;
use crate::design::tokens;

const TOOLBAR_H: f32 = 40.0;
const STATUS_H: f32 = 28.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Zoom {
    /// The whole snapshot inside the panel, aspect kept.
    #[default]
    Fit,
    /// One snapshot pixel per point, scrolling.
    Actual,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Zoom(Zoom),
    Retry,
}

pub struct State {
    image: Option<AnimatedImage>,
    /// The encoded snapshot, kept for Retry.
    bytes: &'static [u8],
    zoom: Zoom,
    scroll: ScrollHandle,
}

impl State {
    pub fn new() -> Self {
        Self::with_bytes(assets::PREVIEW_PNG)
    }

    /// A preview of `bytes`, decoded now: the snapshot is small, and a
    /// panel that pops in later would shift its controls.
    fn with_bytes(bytes: &'static [u8]) -> Self {
        let mut state = Self {
            image: None,
            bytes,
            zoom: Zoom::default(),
            scroll: ScrollHandle::new(),
        };
        state.load();
        state
    }

    fn load(&mut self) {
        let limits = AnimationLimits::default();
        self.image = decode_animation(self.bytes, limits).map(AnimatedImage::from_frames);
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

/// Snapshot size in points at `zoom` inside a `(w, h)` area.
fn image_size(zoom: Zoom, (w, h): (f32, f32)) -> (f32, f32) {
    let (iw, ih) = (assets::PREVIEW_SIZE.0 as f32, assets::PREVIEW_SIZE.1 as f32);
    match zoom {
        Zoom::Actual => (iw, ih),
        Zoom::Fit => {
            let k = (w / iw).min(h / ih).max(0.0);
            (iw * k, ih * k)
        }
    }
}

pub fn view(state: &mut State, scx: &SurfaceCx, _vcx: &mut ViewContext) -> AnyElement {
    let colors = &scx.theme.colors;
    let (width, height) = scx.size;
    let body = (width, (height - TOOLBAR_H - STATUS_H).max(0.0));
    let updated = scx.model.files.can_undo();
    let status = if updated {
        "Updated from proposed changes"
    } else {
        "Snapshot of the current build"
    };
    let zoom = SegmentedControl::new(
        [("Fit", Zoom::Fit), ("100%", Zoom::Actual)]
            .into_iter()
            .map(|(label, z)| {
                SegmentedItem::new(
                    label,
                    super::Action::Preview(Action::Zoom(z)),
                    state.zoom == z,
                )
            })
            .collect(),
    )
    .id("workbench.preview.zoom");
    let content = match &state.image {
        Some(image) => {
            let pad = tokens::SPACE_16;
            let (w, h) = image_size(state.zoom, (body.0 - 2.0 * pad, body.1 - 2.0 * pad));
            let picture = view! {
                <div w={w} h={h} class="shrink-0" bg={colors.surface} border={colors.border}
                     accessibility_role={accesskit::Role::Image}
                     aria-label="Snapshot of the Atlas app" test_id="preview.image">
                    {animated_image(image).size(w, h)}
                </div>
            };
            match state.zoom {
                Zoom::Fit => view! {
                    <div w={body.0} h={body.1} class="flex-col items-center justify-center">
                        {picture}
                    </div>
                }
                .into_any(),
                Zoom::Actual => view! {
                    <div w={body.0} h={body.1} class="overflow-scroll" track_scroll={&state.scroll}
                         aria-label="Snapshot at 100%">
                        <div class="flex-col" p={pad}>{picture}</div>
                    </div>
                }
                .into_any(),
            }
        }
        None => view! {
            <div w={body.0} h={body.1} class="flex-col items-center justify-center gap-[8]"
                 role="alert" aria-label="Could not load the snapshot">
                <icon svg={lucide::ALERT_CIRCLE} size={20.0} color={colors.status_error} />
                <text size={13.0} color={colors.text}>"Could not load the snapshot"</text>
                <Button on:click={super::Action::Preview(Action::Retry)} label="Retry"
                        icon={lucide::REFRESH} size={ButtonSize::Compact} />
            </div>
        }
        .into_any(),
    };
    view! {
        <div w={width} h={height} class="flex-col" bg={colors.canvas} test_id="dock.preview">
            <div w={width} h={TOOLBAR_H} class="flex-row items-center shrink-0"
                 px={tokens::SPACE_8} gap={tokens::SPACE_8} border_b={colors.border}
                 bg={colors.panel} accessibility_role={accesskit::Role::Toolbar}
                 aria-label="Preview controls">
                <Badge label="Snapshot preview" icon={lucide::EYE} />
                <div class="flex-1" />
                {zoom}
            </div>
            {content}
            <div w={width} h={STATUS_H} class="flex-row items-center shrink-0 gap-[6]"
                 px={tokens::SPACE_8} border_t={colors.border} role="status" aria-label={status}>
                <icon svg={if updated { lucide::CHECK } else { lucide::INFO }} size={12.0}
                      color={colors.text_muted} />
                <text size={11.0} color={colors.text_muted}>{status}</text>
            </div>
        </div>
    }
    .into_any()
}

pub fn update(state: &mut State, action: Action) {
    match action {
        Action::Zoom(zoom) => state.zoom = zoom,
        Action::Retry => state.load(),
    }
}
