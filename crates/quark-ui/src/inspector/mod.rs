//! Developer tooling, compiled with the `devtools` feature: a frame HUD, an
//! element inspector with layout debug outlines, and live style overrides.
//!
//! The host (quark-app's `UiAdapter`) owns one [`Devtools`]. Each frame it
//! calls [`Devtools::begin_frame`] on the frame's [`FrameProbe`] before
//! layout, [`Devtools::end_frame`] after paint, then
//! [`Devtools::paint_overlay`] on top of the app's scene. Input goes through
//! [`Devtools::handle`] before the app sees it.
//!
//! - `ctrl+shift+h` toggles the HUD: per window frame timings, primitive
//!   count, and text layout cache traffic.
//! - `ctrl+shift+i` toggles the inspector. Hover highlights the topmost
//!   element under the pointer; click pins it; Escape unpins. The side panel
//!   shows bounds, clip, z, semantics, and style, edits padding, gap, colors,
//!   and radius of a pinned element that has a key or id, and lists the
//!   semantic tree (click a row to pin it).
//! - `ctrl+shift+l` toggles layout debug: every element's bounds outlined,
//!   and hit regions cut by a clip shown with their clipped part.

mod overlay;
mod overrides;
mod probe;

#[cfg(test)]
mod tests;

use std::time::Instant;

use quark::{Color, Rect, UiKey};
use quark_text::LayoutCacheStats;

pub use overlay::OverlayContext;
pub use overrides::{StyleEdit, StyleOverrides};
pub use probe::{
    ElementRecord, FrameProbe, InspectFrame, InspectInfo, PhaseTimings, StyleSummary,
    apply_override, record_paint, record_prepaint,
};

use crate::Action;
use crate::element::InputRouter;
use crate::hud::{HudSample, HudState};

pub const TOGGLE_HUD: &str = "ctrl+shift+h";
pub const TOGGLE_INSPECTOR: &str = "ctrl+shift+i";
pub const TOGGLE_LAYOUT: &str = "ctrl+shift+l";

/// Background and border colors the inspector cycles through.
const SWATCHES: [Color; 6] = [
    Color::rgba(235, 87, 87, 255),
    Color::rgba(242, 153, 74, 255),
    Color::rgba(39, 174, 96, 255),
    Color::rgba(47, 128, 237, 255),
    Color::rgba(155, 81, 224, 255),
    Color::rgba(240, 240, 240, 255),
];

/// Input as devtools see it, in logical points.
#[derive(Debug, Clone, Copy)]
pub enum DevtoolsInput<'a> {
    PointerMoved {
        x: f32,
        y: f32,
    },
    PointerLeft,
    PointerDown,
    PointerUp,
    Wheel,
    /// A key binding in keymap format, such as `"ctrl+shift+i"`.
    Key(&'a str),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Response {
    /// Devtools handled the event; the app must not see it.
    pub consumed: bool,
    pub redraw: bool,
}

impl Response {
    const IGNORED: Self = Self {
        consumed: false,
        redraw: false,
    };
    const HANDLED: Self = Self {
        consumed: true,
        redraw: true,
    };
}

/// What the inspector panel's controls emit.
#[derive(Debug, Clone, PartialEq)]
pub enum DevtoolsMsg {
    /// Pin the element that pushed this semantic node.
    PinNode(usize),
    Unpin,
    Padding(f32),
    Gap(f32),
    Radius(f32),
    CycleBackground,
    CycleBorder,
    ClearPinned,
    ClearAll,
    ToggleLayout,
}

impl From<DevtoolsMsg> for Action {
    fn from(msg: DevtoolsMsg) -> Self {
        Action::new(msg)
    }
}

/// The element the inspector is pinned to, found again in every frame.
#[derive(Debug, Clone, PartialEq)]
enum Pin {
    Key(UiKey),
    Bounds(Rect),
}

#[derive(Default)]
pub struct Devtools {
    pub hud: bool,
    pub inspector: bool,
    pub layout: bool,
    pointer: Option<(f32, f32)>,
    pin: Option<Pin>,
    overrides: StyleOverrides,
    frame: InspectFrame,
    huds: Vec<(u64, HudState)>,
    /// Routes clicks on the overlay's own panels.
    panel: InputRouter,
    text_stats: LayoutCacheStats,
}

impl Devtools {
    /// Devtools with the panels named in `QUARK_DEVTOOLS` open: a comma
    /// list of `hud`, `inspector`, `layout`, or `all`.
    pub fn from_env() -> Self {
        let spec = std::env::var("QUARK_DEVTOOLS").unwrap_or_default();
        let mut devtools = Self::default();
        for part in spec.split(',').map(str::trim) {
            match part {
                "hud" => devtools.hud = true,
                "inspector" => devtools.inspector = true,
                "layout" => devtools.layout = true,
                "all" => (devtools.hud, devtools.inspector, devtools.layout) = (true, true, true),
                _ => {}
            }
        }
        devtools
    }

    pub fn is_active(&self) -> bool {
        self.hud || self.inspector || self.layout
    }

    pub fn overrides(&self) -> &StyleOverrides {
        &self.overrides
    }

    pub fn overrides_mut(&mut self) -> &mut StyleOverrides {
        &mut self.overrides
    }

    /// The last frame's recorded elements.
    pub fn frame(&self) -> &InspectFrame {
        &self.frame
    }

    /// Prepare `probe` before the frame lays out: record elements while an
    /// overlay needs them, and carry the session's overrides.
    pub fn begin_frame(&self, probe: &mut FrameProbe) {
        probe.recording = self.inspector || self.layout;
        probe.overrides.clone_from(&self.overrides);
    }

    /// Keep the frame's records for picking and the overlay; returns its
    /// phase timings.
    pub fn end_frame(&mut self, probe: &mut FrameProbe) -> PhaseTimings {
        self.frame = probe.finish();
        probe.phases
    }

    /// Record one frame of `window` in its HUD. `text` is the layout
    /// cache's lifetime counters; the HUD shows this frame's share.
    pub fn record_frame(
        &mut self,
        window: u64,
        now: Instant,
        mut sample: HudSample,
        text: LayoutCacheStats,
    ) {
        sample.text_hits = text.hits.saturating_sub(self.text_stats.hits);
        sample.text_misses = text.misses.saturating_sub(self.text_stats.misses);
        self.text_stats = text;
        let hud = self.hud_mut(window);
        sample.frame_interval_us = hud.frame_started(now);
        hud.record(sample);
    }

    /// Forget text cache traffic caused by the overlay itself, so the next
    /// frame's HUD counts only the app's.
    pub fn sync_text_stats(&mut self, text: LayoutCacheStats) {
        self.text_stats = text;
    }

    fn hud_mut(&mut self, window: u64) -> &mut HudState {
        let index = match self.huds.iter().position(|(w, _)| *w == window) {
            Some(index) => index,
            None => {
                self.huds.push((window, HudState::default()));
                self.huds.len() - 1
            }
        };
        &mut self.huds[index].1
    }

    pub fn handle(&mut self, input: DevtoolsInput) -> Response {
        match input {
            DevtoolsInput::Key(TOGGLE_HUD) => {
                self.hud = !self.hud;
                return Response::HANDLED;
            }
            DevtoolsInput::Key(TOGGLE_INSPECTOR) => {
                self.inspector = !self.inspector;
                self.pin = None;
                return Response::HANDLED;
            }
            DevtoolsInput::Key(TOGGLE_LAYOUT) => {
                self.layout = !self.layout;
                return Response::HANDLED;
            }
            DevtoolsInput::PointerMoved { x, y } => self.pointer = Some((x, y)),
            DevtoolsInput::PointerLeft => self.pointer = None,
            _ => {}
        }
        if !self.inspector {
            return Response::IGNORED;
        }
        match input {
            DevtoolsInput::Key("escape") => {
                if self.pin.take().is_none() {
                    self.inspector = false;
                }
                Response::HANDLED
            }
            DevtoolsInput::PointerMoved { .. } | DevtoolsInput::PointerLeft => Response {
                consumed: false,
                redraw: true,
            },
            DevtoolsInput::PointerDown => {
                self.pointer_down();
                Response::HANDLED
            }
            // The press went to devtools, so the release does too.
            DevtoolsInput::PointerUp => {
                self.panel.pointer_up();
                Response::HANDLED
            }
            DevtoolsInput::Wheel => Response {
                consumed: self.pointer_over_panel(),
                redraw: false,
            },
            DevtoolsInput::Key(_) => Response::IGNORED,
        }
    }

    fn pointer_over_panel(&self) -> bool {
        self.pointer
            .is_some_and(|(x, y)| self.panel.target_at(x, y).is_some())
    }

    fn pointer_down(&mut self) {
        let Some((x, y)) = self.pointer else {
            return;
        };
        if self.pointer_over_panel() {
            let delivery = self.panel.pointer_down(x, y, &mut None);
            for action in delivery.actions {
                if let Some(msg) = action.downcast_ref::<DevtoolsMsg>() {
                    self.apply(msg.clone());
                }
            }
        } else if let Some(index) = self.frame.pick(x, y) {
            self.pin = Some(self.pin_for(index));
        }
    }

    fn pin_for(&self, index: usize) -> Pin {
        let record = &self.frame.records()[index];
        match &record.key {
            Some(key) => Pin::Key(key.clone()),
            None => Pin::Bounds(record.bounds),
        }
    }

    /// The pinned element in the last frame.
    pub fn pinned(&self) -> Option<usize> {
        match self.pin.as_ref()? {
            Pin::Key(key) => self.frame.for_key(key),
            Pin::Bounds(bounds) => self.frame.for_bounds(*bounds),
        }
    }

    /// The element under the pointer in the last frame, unless the pointer
    /// is over a devtools panel.
    pub fn hovered(&self) -> Option<usize> {
        let (x, y) = self.pointer?;
        if self.pointer_over_panel() {
            return None;
        }
        self.frame.pick(x, y)
    }

    fn apply(&mut self, msg: DevtoolsMsg) {
        match msg {
            DevtoolsMsg::PinNode(node) => {
                self.pin = self.frame.for_semantic(node).map(|i| self.pin_for(i));
            }
            DevtoolsMsg::Unpin => self.pin = None,
            DevtoolsMsg::ToggleLayout => self.layout = !self.layout,
            DevtoolsMsg::ClearAll => self.overrides.clear_all(),
            msg => self.edit_pinned(msg),
        }
    }

    fn edit_pinned(&mut self, msg: DevtoolsMsg) {
        let Some(record) = self.pinned().map(|i| &self.frame.records()[i]) else {
            return;
        };
        let (Some(key), Some(style)) = (record.key.clone(), record.style) else {
            return;
        };
        // Records carry the style after this session's overrides, so steps
        // accumulate from what is on screen.
        let step = |current: f32, delta: f32| (current + delta).max(0.0);
        match msg {
            DevtoolsMsg::Padding(delta) => self.overrides.edit(key, |edit| {
                edit.padding = Some(step(style.padding[0], delta));
            }),
            DevtoolsMsg::Gap(delta) => self.overrides.edit(key, |edit| {
                edit.gap = Some(step(style.gap[0], delta));
            }),
            DevtoolsMsg::Radius(delta) => self.overrides.edit(key, |edit| {
                edit.radius = Some(step(style.corner_radii[0], delta));
            }),
            DevtoolsMsg::CycleBackground => self.overrides.edit(key, |edit| {
                edit.background = Some(next_swatch(style.background));
            }),
            DevtoolsMsg::CycleBorder => self.overrides.edit(key, |edit| {
                edit.border_color = Some(next_swatch(style.border_color));
            }),
            DevtoolsMsg::ClearPinned => self.overrides.clear(&key),
            _ => {}
        }
    }
}

fn next_swatch(current: Option<Color>) -> Color {
    let next = current
        .and_then(|color| SWATCHES.iter().position(|swatch| *swatch == color))
        .map_or(0, |i| (i + 1) % SWATCHES.len());
    SWATCHES[next]
}
