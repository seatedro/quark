//! Resizable split panes: a row or column of panes separated by draggable
//! dividers.
//!
//! The app owns a [`SplitState`] and builds a [`Split`] from it every frame.
//! Dividers emit [`SplitEvent`]s through a caller supplied mapping into the
//! app's action type; the app hands them back to [`SplitState::apply`].
//! [`SplitState::snapshot`] and [`SplitState::restore`] persist the user's
//! sizes with serde.
//!
//! Exactly one pane is the flex pane: it fills whatever the fixed panes
//! leave. Every other pane owns the one divider on its side facing the flex
//! pane. Dragging a divider resizes that pane out of the flex pane; once the
//! flex pane is at its minimum, the pane pushes the fixed panes beyond its
//! divider down to their minimums, nearest first, and they recover if the
//! same drag comes back. Sizes are in logical points.

use std::rc::Rc;

use accesskit::Role;
use quark::view;
use quark_ui::accessibility::{NumericActions, NumericValue, Orientation};
use quark_ui::element::{
    AnyElement, ClickEvent, CursorHint, DragHandler, DragReleaseResult, IntoAnyElement, div,
};
use quark_ui::style::Styled;
use quark_ui::theme::Theme;
use quark_ui::{Action, FocusId};
use serde::{Deserialize, Serialize};

/// Most panes a split holds. Layout resolves into a fixed array of this
/// length so a frame allocates nothing for sizes.
pub const MAX_PANES: usize = 8;

/// Laid out width (or height) of a divider.
pub const DIVIDER_THICKNESS: f32 = 1.0;

/// Width of the invisible strip around a divider that takes the pointer.
const DIVIDER_HIT: f32 = 8.0;

/// Presses on one divider closer together than this chain into a double
/// click, which resets its pane.
const DOUBLE_PRESS_MS: u64 = 500;

/// Arrow key step, and the step with Shift held.
pub(crate) const NUDGE_STEP: f32 = 10.0;
pub(crate) const NUDGE_STEP_LARGE: f32 = 50.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    /// Panes side by side; dividers are vertical lines.
    Horizontal,
    /// Panes stacked; dividers are horizontal lines.
    Vertical,
}

/// Constraints for one pane. Not persisted: they belong to the code.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pane {
    /// Accessible name; the divider is announced as "Resize {label}".
    pub label: &'static str,
    pub default: f32,
    pub min: f32,
    pub max: f32,
    /// Can shrink to zero: by a drag below half its minimum, Enter on its
    /// divider, or [`SplitState::toggle`].
    pub collapsible: bool,
    pub flex: bool,
}

impl Pane {
    /// A pane of `default` points that keeps its size when the split grows.
    pub const fn fixed(label: &'static str, default: f32) -> Self {
        Self {
            label,
            default,
            min: 0.0,
            max: f32::INFINITY,
            collapsible: false,
            flex: false,
        }
    }

    /// The pane that takes the remaining space.
    pub const fn flex(label: &'static str) -> Self {
        Self {
            label,
            default: 0.0,
            min: 0.0,
            max: f32::INFINITY,
            collapsible: false,
            flex: true,
        }
    }

    pub const fn min(mut self, min: f32) -> Self {
        self.min = min;
        self
    }

    pub const fn max(mut self, max: f32) -> Self {
        self.max = max;
        self
    }

    pub const fn collapsible(mut self) -> Self {
        self.collapsible = true;
        self
    }
}

/// What a divider reports. `extent` is the split's main axis length when the
/// divider was drawn, so the state can keep the flex pane at its minimum.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SplitEvent {
    /// Pointer pressed on the divider; starts a drag.
    Press { divider: usize },
    /// Pointer moved `delta` points along the axis since the press.
    Drag {
        divider: usize,
        delta: f32,
        extent: f32,
    },
    /// Pointer released; the drag is over.
    Release { divider: usize },
    /// Keyboard resize by `delta` points in screen direction (right or down
    /// is positive).
    Nudge {
        divider: usize,
        delta: f32,
        extent: f32,
    },
    /// Collapse or restore the pane that owns `divider`.
    Toggle { divider: usize },
}

/// The persisted part of a [`SplitState`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SplitSnapshot {
    pub sizes: Vec<f32>,
    pub collapsed: Vec<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitIntegrityError {
    /// `sizes`, `collapsed`, and `panes` differ in length.
    Lengths,
    /// Not exactly one flex pane, or more than [`MAX_PANES`] panes.
    Shape,
    /// A fixed pane's stored size is outside its `min..=max` or not finite.
    SizeOutOfRange { pane: usize },
    /// The flex pane is collapsed.
    FlexCollapsed,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct DragOrigin {
    divider: usize,
    size: f32,
    collapsed: bool,
    /// Every pane's size at the press, so panes the drag pushed recover.
    sizes: [f32; MAX_PANES],
}

/// Pane sizes resolved for one extent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaneSizes {
    sizes: [f32; MAX_PANES],
    len: usize,
}

impl PaneSizes {
    pub fn as_slice(&self) -> &[f32] {
        &self.sizes[..self.len]
    }
}

/// Sizes and collapse state of a split's panes, owned by the app.
#[derive(Debug, Clone, PartialEq)]
pub struct SplitState {
    axis: Axis,
    panes: Vec<Pane>,
    /// Expanded size of each fixed pane; the flex pane's entry is unused.
    sizes: Vec<f32>,
    collapsed: Vec<bool>,
    /// Per pane, collapsed from its own divider (Enter or a drag), which
    /// then stays in place to restore it even when collapsed dividers hide.
    collapsed_by_divider: Vec<bool>,
    flex: usize,
    hide_collapsed_dividers: bool,
    drag: Option<DragOrigin>,
    last_press: Option<(usize, u64)>,
}

impl SplitState {
    /// # Panics
    ///
    /// Unless exactly one pane is flex and there are at most
    /// [`MAX_PANES`].
    pub fn new(axis: Axis, panes: Vec<Pane>) -> Self {
        assert!(
            panes.len() <= MAX_PANES,
            "a split holds at most {MAX_PANES} panes"
        );
        assert_eq!(
            panes.iter().filter(|p| p.flex).count(),
            1,
            "a split needs one flex pane"
        );
        let flex = panes.iter().position(|p| p.flex).unwrap_or_default();
        let sizes = panes
            .iter()
            .map(|p| {
                if p.flex {
                    0.0
                } else {
                    p.default.clamp(p.min, p.max)
                }
            })
            .collect();
        let state = Self {
            axis,
            collapsed: vec![false; panes.len()],
            collapsed_by_divider: vec![false; panes.len()],
            panes,
            sizes,
            flex,
            hide_collapsed_dividers: false,
            drag: None,
            last_press: None,
        };
        state.debug_verify();
        state
    }

    /// Leave out the divider of a pane the app collapses by command
    /// ([`Self::set_collapsed`], [`Self::toggle`]). A pane collapsed from its
    /// divider keeps it, so focus stays there and Enter or a drag restores
    /// the pane.
    pub fn hide_collapsed_dividers(mut self, hide: bool) -> Self {
        self.hide_collapsed_dividers = hide;
        self
    }

    pub fn axis(&self) -> Axis {
        self.axis
    }

    pub fn panes(&self) -> &[Pane] {
        &self.panes
    }

    pub fn flex_pane(&self) -> usize {
        self.flex
    }

    /// The pane `divider` resizes: the one on its side away from the flex
    /// pane.
    pub fn divider_pane(&self, divider: usize) -> usize {
        if divider < self.flex {
            divider
        } else {
            divider + 1
        }
    }

    /// The divider pane `pane` owns; `None` for the flex pane.
    pub fn pane_divider(&self, pane: usize) -> Option<usize> {
        match pane.cmp(&self.flex) {
            std::cmp::Ordering::Less => Some(pane),
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Greater => Some(pane - 1),
        }
    }

    /// Expanded size of a fixed pane, kept while it is collapsed.
    pub fn size(&self, pane: usize) -> f32 {
        self.sizes[pane]
    }

    pub fn is_collapsed(&self, pane: usize) -> bool {
        self.collapsed[pane]
    }

    pub fn set_size(&mut self, pane: usize, size: f32) {
        if pane != self.flex {
            let p = self.panes[pane];
            self.sizes[pane] = size.clamp(p.min, p.max);
        }
        self.debug_verify();
    }

    pub fn set_collapsed(&mut self, pane: usize, collapsed: bool) {
        if pane != self.flex {
            self.collapsed[pane] = collapsed;
            self.collapsed_by_divider[pane] = false;
        }
        self.debug_verify();
    }

    pub fn toggle(&mut self, pane: usize) {
        if pane != self.flex && self.panes[pane].collapsible {
            self.collapsed[pane] = !self.collapsed[pane];
            self.collapsed_by_divider[pane] = false;
        }
        self.debug_verify();
    }

    /// Back to the pane's default size, expanded.
    pub fn reset(&mut self, pane: usize) {
        if pane != self.flex {
            let p = self.panes[pane];
            self.sizes[pane] = p.default.clamp(p.min, p.max);
            self.collapsed[pane] = false;
        }
        self.debug_verify();
    }

    /// Whether `divider` is laid out: false for a hidden collapsed pane's.
    pub(crate) fn divider_shown(&self, divider: usize) -> bool {
        self.divider_visible(divider)
    }

    fn divider_visible(&self, divider: usize) -> bool {
        let pane = self.divider_pane(divider);
        !(self.hide_collapsed_dividers && self.collapsed[pane] && !self.collapsed_by_divider[pane])
    }

    fn visible_dividers(&self) -> usize {
        (0..self.panes.len().saturating_sub(1))
            .filter(|&d| self.divider_visible(d))
            .count()
    }

    /// Size of each pane in a split `extent` points long. Fixed panes keep
    /// their sizes while the flex pane stays at or above its minimum; past
    /// that, fixed panes shrink toward their minimums, the last first.
    pub fn resolve(&self, extent: f32) -> PaneSizes {
        let mut out = PaneSizes {
            sizes: [0.0; MAX_PANES],
            len: self.panes.len(),
        };
        let avail = (extent - self.visible_dividers() as f32 * DIVIDER_THICKNESS).max(0.0);
        let mut fixed = 0.0;
        for (i, p) in self.panes.iter().enumerate() {
            if i != self.flex && !self.collapsed[i] {
                out.sizes[i] = self.sizes[i].clamp(p.min, p.max);
                fixed += out.sizes[i];
            }
        }
        let mut deficit = fixed - (avail - self.panes[self.flex].min);
        for i in (0..self.panes.len()).rev() {
            if deficit <= 0.0 {
                break;
            }
            if i == self.flex || self.collapsed[i] {
                continue;
            }
            let give = (out.sizes[i] - self.panes[i].min).clamp(0.0, deficit);
            out.sizes[i] -= give;
            fixed -= give;
            deficit -= give;
        }
        out.sizes[self.flex] = (avail - fixed).max(0.0);
        out
    }

    /// Fixed panes `pane` pushes once the flex pane is at its minimum: the
    /// expanded ones beyond its divider, nearest first.
    fn push_order(&self, pane: usize) -> impl Iterator<Item = usize> + '_ {
        // Index arithmetic rather than a boxed `Range` or `Rev<Range>`:
        // dynamic dispatch made the Kani proof of `resize_to` intractable.
        let forward = pane < self.flex;
        let beyond = if forward {
            pane + 1..self.panes.len()
        } else {
            0..pane
        };
        (0..beyond.len())
            .map(move |k| {
                if forward {
                    beyond.start + k
                } else {
                    beyond.end - 1 - k
                }
            })
            .filter(|&i| i != self.flex && !self.collapsed[i])
    }

    /// Give `pane` the size `target` (already in pane terms): collapse below
    /// half the minimum when collapsible, otherwise clamp to what fits. Space
    /// comes from the flex pane down to its minimum, then from the panes
    /// [`Self::push_order`] lists down to theirs.
    fn resize_to(&mut self, pane: usize, target: f32, extent: f32, may_collapse: bool) {
        let p = self.panes[pane];
        if may_collapse && p.collapsible && target < p.min / 2.0 {
            self.collapsed[pane] = true;
            self.collapsed_by_divider[pane] = true;
            return;
        }
        // Resolve with the pane at its minimum, so the others show their
        // own sizes rather than what this pane's current size squeezes
        // them to, and a push is written to their stored sizes.
        let current = self.sizes[pane];
        self.sizes[pane] = p.min;
        let resolved = self.resolve(extent);
        self.sizes[pane] = current;
        let mut dividers = self.visible_dividers();
        if let Some(d) = self.pane_divider(pane)
            && !self.divider_visible(d)
        {
            // Expanding the pane brings its divider back.
            dividers += 1;
        }
        let avail = extent - dividers as f32 * DIVIDER_THICKNESS;
        let others: f32 = (0..self.panes.len())
            .filter(|&i| i != pane && i != self.flex)
            .map(|i| resolved.sizes[i])
            .sum();
        // What the pane can take from the flex pane alone.
        let free = avail - others - self.panes[self.flex].min;
        let slack: f32 = self
            .push_order(pane)
            .map(|i| (resolved.sizes[i] - self.panes[i].min).max(0.0))
            .sum();
        let size = target.clamp(p.min, (free + slack).min(p.max).max(p.min));
        self.collapsed[pane] = false;
        self.sizes[pane] = size;
        let mut overflow = size - free;
        let donors: Vec<usize> = self.push_order(pane).collect();
        for i in donors {
            if overflow <= 0.0 {
                break;
            }
            let give = (resolved.sizes[i] - self.panes[i].min).clamp(0.0, overflow);
            if give > 0.0 {
                self.sizes[i] = resolved.sizes[i] - give;
                overflow -= give;
            }
        }
    }

    /// `+1` when moving the divider right or down grows its pane.
    fn grow_sign(&self, divider: usize) -> f32 {
        if divider < self.flex { 1.0 } else { -1.0 }
    }

    /// Apply a divider's event. `now_ms` dates presses so a second press
    /// within 500 ms resets the pane. Returns true when the change is
    /// settled and worth persisting (a release, key, toggle, or reset).
    pub fn apply(&mut self, event: SplitEvent, now_ms: u64) -> bool {
        let settled = match event {
            SplitEvent::Press { divider } => {
                let pane = self.divider_pane(divider);
                let double = self.last_press.is_some_and(|(d, at)| {
                    d == divider && now_ms.saturating_sub(at) <= DOUBLE_PRESS_MS
                });
                if double {
                    self.reset(pane);
                    self.last_press = None;
                } else {
                    self.last_press = Some((divider, now_ms));
                }
                let mut sizes = [0.0; MAX_PANES];
                sizes[..self.sizes.len()].copy_from_slice(&self.sizes);
                self.drag = Some(DragOrigin {
                    divider,
                    size: self.sizes[pane],
                    collapsed: self.collapsed[pane],
                    sizes,
                });
                double
            }
            SplitEvent::Drag {
                divider,
                delta,
                extent,
            } => {
                let Some(origin) = self.drag.filter(|o| o.divider == divider) else {
                    return false;
                };
                let pane = self.divider_pane(divider);
                // Start over from the press, so panes an earlier move pushed
                // grow back when the pointer returns.
                for i in 0..self.panes.len() {
                    if i != pane && i != self.flex {
                        self.sizes[i] = origin.sizes[i];
                    }
                }
                let start = if origin.collapsed { 0.0 } else { origin.size };
                if origin.collapsed {
                    // Expanding from collapsed starts from the stored size,
                    // not from wherever an earlier move left it.
                    self.sizes[pane] = origin.size;
                }
                self.resize_to(pane, start + delta * self.grow_sign(divider), extent, true);
                false
            }
            SplitEvent::Release { divider } => {
                self.drag.take().is_some_and(|o| o.divider == divider)
            }
            SplitEvent::Nudge {
                divider,
                delta,
                extent,
            } => {
                let pane = self.divider_pane(divider);
                let current = if self.collapsed[pane] {
                    0.0
                } else {
                    self.sizes[pane]
                };
                let target = current + delta * self.grow_sign(divider);
                if self.collapsed[pane] && target <= 0.0 {
                    return false;
                }
                self.resize_to(pane, target, extent, false);
                true
            }
            SplitEvent::Toggle { divider } => {
                let pane = self.divider_pane(divider);
                self.toggle(pane);
                self.collapsed_by_divider[pane] = self.collapsed[pane];
                true
            }
        };
        self.debug_verify();
        settled
    }

    pub fn snapshot(&self) -> SplitSnapshot {
        SplitSnapshot {
            sizes: self.sizes.clone(),
            collapsed: self.collapsed.clone(),
        }
    }

    /// Take sizes from a snapshot, clamped to the panes' constraints.
    /// Returns false, changing nothing, when it was saved for a different
    /// number of panes.
    pub fn restore(&mut self, snapshot: &SplitSnapshot) -> bool {
        let n = self.panes.len();
        if snapshot.sizes.len() != n || snapshot.collapsed.len() != n {
            return false;
        }
        for (i, p) in self.panes.iter().enumerate() {
            if i == self.flex {
                continue;
            }
            let size = snapshot.sizes[i];
            self.sizes[i] = if size.is_finite() {
                size.clamp(p.min, p.max)
            } else {
                p.default.clamp(p.min, p.max)
            };
            self.collapsed[i] = snapshot.collapsed[i];
            self.collapsed_by_divider[i] = false;
        }
        self.drag = None;
        self.debug_verify();
        true
    }

    pub fn verify_integrity(&self) -> Result<(), SplitIntegrityError> {
        let n = self.panes.len();
        if self.sizes.len() != n
            || self.collapsed.len() != n
            || self.collapsed_by_divider.len() != n
        {
            return Err(SplitIntegrityError::Lengths);
        }
        if n > MAX_PANES || self.panes.iter().filter(|p| p.flex).count() != 1 {
            return Err(SplitIntegrityError::Shape);
        }
        if !self.panes[self.flex].flex {
            return Err(SplitIntegrityError::Shape);
        }
        if self.collapsed[self.flex] {
            return Err(SplitIntegrityError::FlexCollapsed);
        }
        for (i, p) in self.panes.iter().enumerate() {
            let s = self.sizes[i];
            if i != self.flex && !(s.is_finite() && s >= p.min && s <= p.max) {
                return Err(SplitIntegrityError::SizeOutOfRange { pane: i });
            }
        }
        Ok(())
    }

    fn debug_verify(&self) {
        debug_assert_eq!(self.verify_integrity(), Ok(()));
    }
}

type EventMap = Rc<dyn Fn(SplitEvent) -> Action>;

/// Builds the element for a [`SplitState`].
pub struct Split<'a> {
    id: &'static str,
    state: &'a SplitState,
    extent: f32,
    map: EventMap,
    children: Vec<AnyElement>,
}

impl<'a> Split<'a> {
    /// `id` names the split's dividers for focus and accessibility; keep it
    /// unique in the window. `extent` is the split's length along its axis
    /// (it fills its parent across). `on_event` wraps divider events in the
    /// app's action type.
    pub fn new(
        id: &'static str,
        state: &'a SplitState,
        extent: f32,
        on_event: impl Fn(SplitEvent) -> Action + 'static,
    ) -> Self {
        Self {
            id,
            state,
            extent,
            map: Rc::new(on_event),
            children: Vec::with_capacity(state.panes.len()),
        }
    }

    /// The next pane's content. Collapsed panes drop theirs.
    pub fn child(mut self, child: impl IntoAnyElement) -> Self {
        self.children.push(child.into_any());
        self
    }

    /// Focus target of a divider, so apps can focus one by command.
    pub fn divider_focus(id: &'static str, divider: usize) -> FocusId {
        FocusId::new(FocusId::from_key(id).0.wrapping_add(divider as u64 + 1))
    }

    pub fn build(self, theme: &Theme) -> AnyElement {
        let Self {
            id,
            state,
            extent,
            map,
            children,
        } = self;
        let resolved = state.resolve(extent);
        let horizontal = state.axis == Axis::Horizontal;
        view! {
            <div class="relative"
                 @when {horizontal} { class="flex-row" w={extent} class="h-full" }
                 @when {!horizontal} { class="flex-col" h={extent} class="w-full" }>
                for (i, child) in children.into_iter().enumerate().take(state.panes.len()) {
                    if i > 0 && state.divider_visible(i - 1) {
                        {divider(id, state, i - 1, extent, &map, theme)}
                    }
                    <div class="flex-none overflow-clip relative"
                         @when {horizontal} { w={resolved.sizes[i]} class="h-full" }
                         @when {!horizontal} { h={resolved.sizes[i]} class="w-full" }>
                        if !state.collapsed[i] {
                            {child}
                        }
                    </div>
                }
            </div>
        }
    }
}

fn divider(
    id: &'static str,
    state: &SplitState,
    index: usize,
    extent: f32,
    map: &EventMap,
    theme: &Theme,
) -> AnyElement {
    let colors = &theme.colors;
    let horizontal = state.axis == Axis::Horizontal;
    let pane = state.divider_pane(index);
    let p = state.panes[pane];
    let collapsed = state.collapsed[pane];
    let size = if collapsed { 0.0 } else { state.sizes[pane] };
    let cursor = if horizontal {
        CursorHint::ResizeCol
    } else {
        CursorHint::ResizeRow
    };
    let (back, forward) = if horizontal {
        ("left", "right")
    } else {
        ("up", "down")
    };
    let nudge = |delta: f32| {
        map(SplitEvent::Nudge {
            divider: index,
            delta,
            extent,
        })
    };
    // Assistive tech sets and steps the pane's size; nudges move the
    // divider on screen, which grows the pane one way or the other.
    let grow = state.grow_sign(index);
    let set_map = map.clone();
    let numeric_actions = NumericActions::new(move |value| {
        set_map(SplitEvent::Nudge {
            divider: index,
            delta: (value as f32 - size) * grow,
            extent,
        })
    })
    .steps(nudge(-NUDGE_STEP * grow), nudge(NUDGE_STEP * grow));
    let drag_map = map.clone();
    let offset = -(DIVIDER_HIT - DIVIDER_THICKNESS) / 2.0;
    view! {
        <div class="flex-none relative" bg={colors.border_variant}
             @when {horizontal} { w={DIVIDER_THICKNESS} class="h-full" }
             @when {!horizontal} { h={DIVIDER_THICKNESS} class="w-full" }>
            <div class="absolute" z_index={1} accessibility_id={format!("{id}:divider:{index}")}
                 accessibility_role={Role::Splitter} role="separator"
                 aria-label={quark_ui::i18n::tr_args(
                     "quark-resize-named",
                     [("name", p.label.into())],
                 )}
                 aria-valuetext={format!("{size:.0}")}
                 accessibility_numeric={NumericValue {
                     value: f64::from(size),
                     // Collapsing is Enter's job; a size is never below min.
                     min: f64::from(p.min),
                     max: f64::from(p.max.min(extent)),
                     step: Some(f64::from(NUDGE_STEP)),
                 }}
                 accessibility_numeric_actions={numeric_actions}
                 accessibility_orientation={if horizontal {
                     Orientation::Vertical
                 } else {
                     Orientation::Horizontal
                 }}
                 test_id="split-divider" focus_ring={Split::divider_focus(id, index)}
                 cursor={cursor} hover_bg={colors.accent}
                 on_key={(back, nudge(-NUDGE_STEP))}
                 on_key={(forward, nudge(NUDGE_STEP))}
                 on_key={(format!("shift+{back}"), nudge(-NUDGE_STEP_LARGE))}
                 on_key={(format!("shift+{forward}"), nudge(NUDGE_STEP_LARGE))}
                 // Home and End give the pane its smallest and largest size
                 // (the window splitter pattern); the state clamps to what fits.
                 on_key={("home", nudge((p.min - size) * grow))}
                 on_key={("end", nudge((p.max.min(extent) - size) * grow))}
                 on:drag={move |press: ClickEvent| {
                     Box::new(DividerDrag {
                         map: drag_map.clone(),
                         divider: index,
                         horizontal,
                         origin: if horizontal { press.x } else { press.y },
                         extent,
                         cursor,
                     }) as Box<dyn DragHandler>
                 }}
                 @when {p.collapsible} {
                     aria-expanded={!collapsed}
                     on_key={("enter", map(SplitEvent::Toggle { divider: index }))}
                 }
                 @when {horizontal} {
                     class="top-0 bottom-0" left={offset} w={DIVIDER_HIT}
                 }
                 @when {!horizontal} {
                     class="left-0 right-0" top={offset} h={DIVIDER_HIT}
                 } />
        </div>
    }
}

struct DividerDrag {
    map: EventMap,
    divider: usize,
    horizontal: bool,
    origin: f32,
    extent: f32,
    cursor: CursorHint,
}

impl DragHandler for DividerDrag {
    fn on_press(&mut self) -> Vec<Action> {
        vec![(self.map)(SplitEvent::Press {
            divider: self.divider,
        })]
    }

    fn on_move(&mut self, x: f32, y: f32) -> Vec<Action> {
        let at = if self.horizontal { x } else { y };
        vec![(self.map)(SplitEvent::Drag {
            divider: self.divider,
            delta: at - self.origin,
            extent: self.extent,
        })]
    }

    fn on_release(&mut self) -> DragReleaseResult {
        DragReleaseResult {
            actions: vec![(self.map)(SplitEvent::Release {
                divider: self.divider,
            })],
        }
    }

    /// The panes follow the pointer as it moves, so cancelling keeps the
    /// sizes dragged to and settles them, like a release.
    fn on_cancel(&mut self) -> Vec<Action> {
        self.on_release().actions
    }

    fn cursor(&self) -> CursorHint {
        self.cursor
    }
}

#[cfg(kani)]
mod verification {
    use super::*;

    const N: usize = 3;

    /// Small whole numbers, so every sum the algorithm forms is exact in
    /// `f32` and the assertions can compare with `==`.
    fn any_points() -> f32 {
        f32::from(kani::any::<u8>() % 64)
    }

    /// Three panes with the flex pane at `flex`, arbitrary constraints and
    /// stored sizes, and any panes other than `pane` collapsed.
    fn any_state(pane: usize, flex: usize) -> SplitState {
        let mut panes = [Pane::fixed("", 0.0); N];
        let mut sizes = [0.0; N];
        let mut collapsed = [false; N];
        for i in 0..N {
            let min = any_points();
            let max = any_points();
            kani::assume(min <= max);
            panes[i] = Pane::fixed("", min).min(min).max(max);
            if i == flex {
                panes[i].flex = true;
                panes[i].max = f32::INFINITY;
            } else {
                sizes[i] = any_points();
                kani::assume(min <= sizes[i] && sizes[i] <= max);
                collapsed[i] = i != pane && kani::any();
            }
        }
        let mut state =
            SplitState::new(Axis::Horizontal, panes.to_vec()).hide_collapsed_dividers(kani::any());
        state.sizes = sizes.to_vec();
        state.collapsed = collapsed.to_vec();
        state
    }

    /// Resizing `pane` with the flex pane at `flex`.
    fn check(pane: usize, flex: usize) {
        let mut state = any_state(pane, flex);
        let extent = f32::from(kani::any::<u8>());
        let mins: f32 = state.panes.iter().map(|p| p.min).sum();
        // Room for every minimum and divider; below that no layout can
        // keep the minimums and resolve squeezes by design.
        kani::assume(extent >= mins + (N - 1) as f32 * DIVIDER_THICKNESS);
        let target = f32::from(kani::any::<u8>());
        let before = state.sizes.clone();
        // The panes `pane` may push, from the spec rather than from
        // `push_order`: the expanded fixed panes on the flex pane's side.
        let beyond = |i: usize| if pane < flex { i > pane } else { i < pane };
        let mut far = [false; N];
        for (i, far) in far.iter_mut().enumerate() {
            *far = i != pane && i != flex && !state.collapsed[i] && beyond(i);
        }

        state.resize_to(pane, target, extent, false);

        assert!(state.verify_integrity().is_ok());
        let resolved = state.resolve(extent);
        let r = resolved.as_slice();
        // Sums are left out: proving float sums equal under a different
        // association runs CBMC out of memory. The proptest checks them.
        assert!(r[flex] >= state.panes[flex].min);
        // The pane is laid out at the size it was given.
        assert!(r[pane] == state.sizes[pane]);
        for i in 0..N {
            if i == pane || i == flex {
                continue;
            }
            if far[i] {
                // A pushed pane only shrinks, and never below its minimum.
                assert!(state.sizes[i] <= before[i]);
            } else {
                // Panes on the near side are not touched.
                assert!(state.sizes[i] == before[i]);
            }
        }
        // Falling short of the target means nothing is left to push: the
        // flex pane and every far pane sit at their minimums.
        let p = state.panes[pane];
        if state.sizes[pane] < target.clamp(p.min, p.max) {
            assert!(r[flex] == state.panes[flex].min);
            for i in 0..N {
                if far[i] {
                    assert!(r[i] == state.panes[i].min);
                }
            }
        }
    }

    // One harness per placement of the resized and flex panes: with both
    // indices symbolic the proof ran past the 30 minute job under CaDiCaL
    // and under kissat.
    #[kani::proof]
    #[kani::unwind(5)]
    #[kani::solver(kissat)]
    fn split_resize_pane0_flex1() {
        check(0, 1);
    }

    #[kani::proof]
    #[kani::unwind(5)]
    #[kani::solver(kissat)]
    fn split_resize_pane0_flex2() {
        check(0, 2);
    }

    #[kani::proof]
    #[kani::unwind(5)]
    #[kani::solver(kissat)]
    fn split_resize_pane1_flex0() {
        check(1, 0);
    }

    #[kani::proof]
    #[kani::unwind(5)]
    #[kani::solver(kissat)]
    fn split_resize_pane1_flex2() {
        check(1, 2);
    }

    #[kani::proof]
    #[kani::unwind(5)]
    #[kani::solver(kissat)]
    fn split_resize_pane2_flex0() {
        check(2, 0);
    }

    #[kani::proof]
    #[kani::unwind(5)]
    #[kani::solver(kissat)]
    fn split_resize_pane2_flex1() {
        check(2, 1);
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn three_panes() -> SplitState {
        SplitState::new(
            Axis::Horizontal,
            vec![
                Pane::fixed("Left", 200.0)
                    .min(100.0)
                    .max(400.0)
                    .collapsible(),
                Pane::flex("Center").min(300.0),
                Pane::fixed("Right", 300.0)
                    .min(150.0)
                    .max(700.0)
                    .collapsible(),
            ],
        )
        .hide_collapsed_dividers(true)
    }

    fn event() -> impl Strategy<Value = SplitEvent> {
        let divider = 0..2usize;
        prop_oneof![
            divider
                .clone()
                .prop_map(|divider| SplitEvent::Press { divider }),
            (divider.clone(), -900.0f32..900.0, 600.0f32..1600.0).prop_map(
                |(divider, delta, extent)| SplitEvent::Drag {
                    divider,
                    delta,
                    extent
                }
            ),
            divider
                .clone()
                .prop_map(|divider| SplitEvent::Release { divider }),
            (divider.clone(), -60.0f32..60.0, 600.0f32..1600.0).prop_map(
                |(divider, delta, extent)| SplitEvent::Nudge {
                    divider,
                    delta,
                    extent
                }
            ),
            divider.prop_map(|divider| SplitEvent::Toggle { divider }),
        ]
    }

    // Regression: with the center at its minimum, the left divider stopped
    // dead even though the right pane had room to give.
    #[test]
    fn a_drag_past_the_flex_minimum_pushes_the_far_pane() {
        // Extent 850 leaves 848 after two dividers: 200 + 348 + 300.
        // (drag dx, resolved sizes), all moves of one press.
        let moves: &[(f32, [f32; 3])] = &[
            (100.0, [300.0, 300.0, 248.0]),
            (150.0, [350.0, 300.0, 198.0]),
            // Every pane on the far side is at its minimum.
            (400.0, [398.0, 300.0, 150.0]),
            (500.0, [398.0, 300.0, 150.0]),
            // The pushed pane recovers as the same drag comes back.
            (0.0, [200.0, 348.0, 300.0]),
        ];
        let mut state = three_panes();
        state.apply(SplitEvent::Press { divider: 0 }, 0);
        for &(delta, expected) in moves {
            state.apply(
                SplitEvent::Drag {
                    divider: 0,
                    delta,
                    extent: 850.0,
                },
                0,
            );
            assert_eq!(
                state.resolve(850.0).as_slice(),
                expected,
                "dragged by {delta}"
            );
            // The push is kept, not just laid out: the right pane announces
            // it and keeps it if the left pane later collapses.
            assert_eq!(state.size(2), expected[2], "dragged by {delta}");
        }
    }

    proptest! {
        // Whatever the user does, a frame lays panes out edge to edge across
        // the whole extent, keeps the center at its minimum when the
        // extent has room for every minimum, and keeps fixed panes in range.
        #[test]
        fn resolved_panes_fill_the_extent(
            events in prop::collection::vec((event(), 0u64..2_000), 0..40),
            extent in 600.0f32..1600.0,
        ) {
            let mut state = three_panes();
            let mut now = 0;
            for (event, gap) in events {
                now += gap;
                state.apply(event, now);
            }
            let sizes = state.resolve(extent);
            let dividers = state.visible_dividers() as f32 * DIVIDER_THICKNESS;
            let total: f32 = sizes.as_slice().iter().sum();
            prop_assert!((total + dividers - extent).abs() < 0.01, "{sizes:?} in {extent}");
            prop_assert!(sizes.as_slice()[1] >= 300.0 - 0.01, "{sizes:?} in {extent}");
            for pane in [0, 2] {
                let p = state.panes()[pane];
                let s = sizes.as_slice()[pane];
                prop_assert!(
                    if state.is_collapsed(pane) { s == 0.0 } else { s >= p.min && s <= p.max },
                    "pane {pane}: {sizes:?}"
                );
            }
        }
    }
}
