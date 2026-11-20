//! A window's frame: the app's view, laid out and painted, then the
//! state input is routed through until the next one, and what the window
//! must hear about it (cursor, IME, material regions, the next frame).

use std::time::Duration;

use quark::scene::Scene;
use quark_ui::accessibility::AccessibilityFrame;
use quark_ui::element::{
    AnyElement, ElementContext, ImeTarget, InputFrame, TextInputHitArea, WindowFrameState,
    render_element,
};

use super::ime::ImeRequest;
use super::pointer::cursor_icon;
use super::{UiAdapter, UiApp, ViewContext, drag_window_id};
use crate::FrameContext;

/// One painted frame, with the scene still in logical points.
pub(super) struct Painted {
    pub(super) scene: Scene,
    pub(super) input: InputFrame,
    pub(super) accessibility: AccessibilityFrame,
    pub(super) next_frame_ms: Option<u64>,
    pub(super) text_areas: Vec<TextInputHitArea>,
    pub(super) ime_targets: Vec<ImeTarget>,
}

/// Lay out and paint `root` into a `width` x `height` point viewport, in
/// `scene`'s buffer.
pub(super) fn paint(
    root: &mut AnyElement,
    ecx: &mut ElementContext,
    mut scene: Scene,
    width: f32,
    height: f32,
) -> Painted {
    scene.primitives.clear();
    ecx.accessibility.reset(width, height);
    ecx.semantic.reset(width, height);
    render_element(root, &mut scene, ecx, width, height);
    ecx.finish_frame();
    Painted {
        scene,
        input: ecx.take_input_frame(),
        accessibility: std::mem::take(&mut ecx.accessibility),
        next_frame_ms: ecx.next_frame_ms(),
        text_areas: std::mem::take(&mut ecx.text_input_hit_areas),
        ime_targets: std::mem::take(&mut ecx.ime_targets),
    }
}

impl<U: UiApp> UiAdapter<U> {
    /// Build, lay out, and paint the frame of the window `cx` is for.
    pub(super) fn render(&mut self, cx: &mut FrameContext) -> Scene {
        let window = cx.window_handle();
        self.select(Some(window));
        let (width, height) = cx.size();
        let scale = cx.scale_factor();
        let clock_ms = cx.elapsed().as_millis() as u64;
        // A pointer held past a text field's edge keeps selecting (and so
        // scrolling) without moving; apply the step before this frame's view.
        if let Some((target, command)) = self
            .win
            .text_pointer
            .autoscroll(&self.win.text_areas, clock_ms)
        {
            self.app.edit_text_in(window, target, command);
        }
        self.win.animations.tick(clock_ms);
        // Before the view, so reactive code reads this frame's values.
        self.win.animations.publish_signals(&self.shared.signals);
        #[cfg(feature = "devtools")]
        let view_started = std::time::Instant::now();
        let win = &mut *self.win;
        let (focus, focus_visible) = (win.focus.current, win.focus.is_visible());
        let window_signals = win.element_cache.sync_signals(
            &self.shared.signals,
            WindowFrameState {
                focus,
                focus_visible,
                theme: &self.shared.theme,
            },
        );
        let mut root = self.app.view(&mut ViewContext {
            frame: cx,
            theme: &self.shared.theme,
            signals: &self.shared.signals,
            window_signals,
            focus,
            focus_visible,
            animations: &mut win.animations,
            geometry: &win.router.frame().geometry,
            handles: &mut win.element_handles,
            // Still the last frame's: this frame's paint takes them.
            tooltips: &win.spare.tooltip_regions,
            hovered_tooltip: win.pointer.tooltip.current.as_ref(),
        });
        #[cfg(feature = "devtools")]
        let build_us = view_started.elapsed().as_micros() as u64;

        let accessibility = cx.accessibility_active();
        let text = cx.text();
        let mut ecx = ElementContext::new(
            &self.shared.theme,
            scale,
            &mut text.system,
            &mut text.layouts,
            win.pointer.at,
            &self.shared.signals,
        )
        .with_focus(focus)
        .with_focus_visible(focus_visible)
        .with_clock(clock_ms)
        .with_animations(&mut win.animations)
        .with_presence(&mut win.presence)
        .with_element_cache(&mut win.element_cache)
        .with_accessibility(accessibility)
        .with_input_frame(std::mem::take(&mut win.spare.input));
        ecx.text_input_hit_areas = std::mem::take(&mut win.spare.text_areas);
        ecx.text_input_hit_areas.clear();
        ecx.ime_targets = std::mem::take(&mut win.spare.ime_targets);
        ecx.ime_targets.clear();
        ecx.accessibility = std::mem::take(&mut win.spare.accessibility);
        ecx.tooltip_regions = std::mem::take(&mut win.spare.tooltip_regions);
        ecx.tooltip_regions.clear();
        #[cfg(feature = "devtools")]
        win.devtools.begin_frame(&mut ecx.devtools);
        let scene = std::mem::take(&mut win.spare.scene);
        let mut painted = paint(&mut root, &mut ecx, scene, width, height);
        // After the root, outside its clips; checked against this frame so
        // a removed source shows no preview. A drag session's preview shows
        // over whichever window it is in, whatever its source paints.
        let session = self.shared.drag.as_ref().map(|active| &active.session);
        let preview = win
            .router
            .drag_preview(&painted.input)
            .or_else(|| session.and_then(|session| session.preview_in(drag_window_id(window))));
        win.drag_preview
            .paint(preview, &mut painted.scene, &mut ecx);
        win.jump.hints.refresh(&painted.input);
        win.jump.hints.paint(&mut painted.scene, &mut ecx);
        win.spare.tooltip_regions = std::mem::take(&mut ecx.tooltip_regions);
        #[cfg(feature = "devtools")]
        let phases = win.devtools.end_frame(&mut ecx.devtools);
        win.scale_factor = scale;
        if let Some(at_ms) = painted.next_frame_ms {
            cx.request_frame_in(Duration::from_millis(at_ms.saturating_sub(clock_ms)));
        }
        let (scene, ime) = self.finish_frame(painted);
        // The frame painted every write so far.
        self.shared.signals.clear_dirty();
        self.apply_post_frame(ime, clock_ms, cx);
        #[cfg(feature = "devtools")]
        let frame = crate::devtools::PaintedFrame {
            router: &self.win.router,
            theme: &self.shared.theme,
            signals: &self.shared.signals,
            build_us,
            phases,
        };
        #[cfg(feature = "devtools")]
        let scene = crate::devtools::finish_frame(&mut self.win.devtools, scene, cx, frame);
        scene
    }

    /// Hand the frame's `Div::material` regions to the window when they
    /// changed, so their native views follow the layout.
    fn forward_material_regions(&mut self, cx: &mut FrameContext) {
        use crate::platform::material::MaterialRect;

        let win = &mut *self.win;
        let requests = &win.router.frame().material_regions;
        let same = requests.len() == win.material_regions.len()
            && requests.iter().zip(&win.material_regions).all(|(r, m)| {
                (r.id, r.rect, r.corner_radius, r.kind) == (m.id, m.rect, m.corner_radius, m.kind)
            });
        if same {
            return;
        }
        win.material_regions.clear();
        win.material_regions
            .extend(requests.iter().map(|r| MaterialRect {
                id: r.id,
                rect: r.rect,
                corner_radius: r.corner_radius,
                kind: r.kind,
            }));
        cx.set_material_regions(win.material_regions.clone());
    }

    /// Sync cursor, tooltip, material regions, IME state, and autoscroll
    /// scheduling after a frame finishes painting.
    fn apply_post_frame(&mut self, ime: ImeRequest, clock_ms: u64, cx: &mut FrameContext) {
        // The frame may have shown or hidden an element under a still
        // pointer, such as an action revealed by the hover it painted; the
        // hovered set it leaves behind no longer differs on the next move.
        let pointer = &mut self.win.pointer;
        let cursor = cursor_icon(self.win.router.cursor_over(&pointer.hovered));
        if cursor != pointer.cursor {
            pointer.cursor = cursor;
            cx.set_cursor(cursor);
        }
        // The frame may have moved a region under a still pointer.
        let win = &mut *self.win;
        if win
            .pointer
            .tooltip
            .update(&win.spare.tooltip_regions, win.pointer.at, cx.elapsed())
        {
            cx.request_frame();
        }
        self.forward_material_regions(cx);
        if ime.reset {
            cx.reset_ime();
        }
        if let Some(allowed) = ime.allowed {
            cx.set_ime_allowed(allowed);
        }
        if let Some(caret) = ime.cursor_area {
            cx.set_ime_cursor_area(caret.x, caret.y, caret.width, caret.height);
        }
        if let Some(at_ms) = self
            .win
            .text_pointer
            .next_autoscroll_ms(&self.win.text_areas)
        {
            cx.request_frame_in(Duration::from_millis(at_ms.saturating_sub(clock_ms)));
        }
    }

    /// Keep `painted`'s input state for routing until the next frame, and
    /// return its scene with the IME changes for the caret it painted.
    pub(super) fn finish_frame(&mut self, painted: Painted) -> (Scene, ImeRequest) {
        let win = &mut *self.win;
        win.spare.input = win.router.replace_frame(painted.input);
        // This frame painted hover for the current pointer; later moves
        // compare against it. Into the kept buffer: a pointer resting over
        // the window must not cost an allocation per frame.
        let pointer = &mut win.pointer;
        match pointer.at {
            Some((x, y)) => (win.router.frame().hits).stack_at_into(x, y, &mut pointer.hovered),
            None => pointer.hovered.clear(),
        }
        win.spare.accessibility = std::mem::replace(&mut win.accessibility, painted.accessibility);
        let ime = win.ime.request(win.focus.current, &painted.ime_targets);
        win.spare.text_areas = std::mem::replace(&mut win.text_areas, painted.text_areas);
        win.spare.ime_targets = painted.ime_targets;
        (painted.scene, ime)
    }
}
