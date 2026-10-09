//! Interaction groups: content that shows while a row is hovered or holds
//! focus, without the row changing size.
//!
//! A div marked [`Div::interaction_group`] is a group; a descendant marked
//! [`Div::show_when`] is a slot whose visibility follows the nearest
//! enclosing group with the same [`GroupId`]. Ids are names, scoped by
//! nesting: every row of a list can use the same id, and a slot refers to
//! its own row.
//!
//! A hidden slot keeps its layout space. It paints nothing, registers no
//! hits, handlers, semantic or accessibility nodes, so it is neither
//! clickable nor in the Tab order. A slot that holds the focused element
//! always shows, whatever its condition.

use super::*;

/// Name of an interaction group, scoped to the nearest enclosing group
/// with that name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GroupId(u64);

impl GroupId {
    pub const fn new(name: &str) -> Self {
        Self(quark::stable_hash(name))
    }

    /// An id the caller derived itself.
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

impl From<&str> for GroupId {
    fn from(name: &str) -> Self {
        Self::new(name)
    }
}

/// When a [`Div::show_when`] slot shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroupCondition {
    /// While the pointer is over the group (or a shown descendant inside
    /// it) or the focused element is inside the group: row actions.
    HoveredOrFocusWithin(GroupId),
    /// The inverse: while the group is neither hovered nor holds focus,
    /// e.g. a timestamp the actions replace.
    Idle(GroupId),
}

impl GroupCondition {
    pub fn group(self) -> GroupId {
        match self {
            Self::HoveredOrFocusWithin(id) | Self::Idle(id) => id,
        }
    }

    /// Whether the slot shows for a group that is `active` (hovered or
    /// holding focus).
    pub fn shows(self, active: bool) -> bool {
        match self {
            Self::HoveredOrFocusWithin(_) => active,
            Self::Idle(_) => !active,
        }
    }
}

/// One group prepainted this frame.
#[derive(Debug, Clone, Copy)]
struct GroupRow {
    id: GroupId,
    /// Hit rows of the group and its descendants.
    hits: (usize, usize),
    focus_within: bool,
    /// Resolved by [`InteractionFrame::resolve`].
    active: bool,
}

/// One slot prepainted this frame.
#[derive(Debug, Clone, Copy)]
struct SlotRow {
    condition: GroupCondition,
    /// The group it follows; `None` without a matching ancestor, which
    /// reads as a group never active.
    group: Option<u32>,
    /// Hit rows of the slot and its descendants.
    hits: (usize, usize),
    focus_within: bool,
    /// Resolved by [`InteractionFrame::resolve`].
    visible: bool,
}

/// The groups and slots of the frame being rendered. Groups and slots
/// register while they prepaint; [`ElementContext::run_hit_test`] resolves
/// them before paint, so a slot hidden there has its hit rows disabled
/// and paints nothing.
#[derive(Default)]
pub(super) struct InteractionFrame {
    groups: Vec<GroupRow>,
    slots: Vec<SlotRow>,
    /// Groups and slots whose prepaint is running, innermost last.
    open_groups: Vec<u32>,
    open_slots: Vec<u32>,
}

impl InteractionFrame {
    pub(super) fn clear(&mut self) {
        self.groups.clear();
        self.slots.clear();
        self.open_groups.clear();
        self.open_slots.clear();
    }

    fn is_open(&self) -> bool {
        !self.open_groups.is_empty() || !self.open_slots.is_empty()
    }

    /// Whether hit row `row` belongs to some slot, so its visibility is
    /// not yet known while groups resolve.
    fn in_slot(&self, row: usize) -> bool {
        self.slots
            .iter()
            .any(|slot| (slot.hits.0..slot.hits.1).contains(&row))
    }
}

impl ElementContext<'_> {
    /// Start a group whose own hit row is `hit`; see
    /// [`Div::interaction_group`]. Ends with [`Self::end_interaction_group`].
    pub(super) fn begin_interaction_group(&mut self, id: GroupId, hit: HitId) -> u32 {
        let start = self.hit_table.row(hit).unwrap_or(self.hit_table.len());
        let index = self.interaction.groups.len() as u32;
        self.interaction.groups.push(GroupRow {
            id,
            hits: (start, start),
            focus_within: false,
            active: false,
        });
        self.interaction.open_groups.push(index);
        index
    }

    pub(super) fn end_interaction_group(&mut self, index: u32) {
        let end = self.hit_table.len();
        if let Some(group) = self.interaction.groups.get_mut(index as usize) {
            group.hits.1 = end;
        }
        self.interaction.open_groups.pop();
    }

    /// Start a slot following `condition`, before the slot inserts any hit;
    /// see [`Div::show_when`]. Ends with [`Self::end_interaction_slot`].
    pub(super) fn begin_interaction_slot(&mut self, condition: GroupCondition) -> u32 {
        let frame = &self.interaction;
        let group = frame
            .open_groups
            .iter()
            .rev()
            .copied()
            .find(|&g| frame.groups[g as usize].id == condition.group());
        // A recorded subtree replays without resolving its groups again; a
        // slot inside one that follows a group outside it would replay the
        // visibility it was recorded with, so it is never replayed.
        let start = self.hit_table.len();
        let group_start = group.map(|g| frame.groups[g as usize].hits.0);
        if self
            .innermost_hit_recording()
            .is_some_and(|recording| group_start.is_none_or(|g| g < recording))
        {
            self.mark_volatile();
        }
        let index = self.interaction.slots.len() as u32;
        self.interaction.slots.push(SlotRow {
            condition,
            group,
            hits: (start, start),
            focus_within: false,
            visible: true,
        });
        self.interaction.open_slots.push(index);
        index
    }

    pub(super) fn end_interaction_slot(&mut self, index: u32) {
        let end = self.hit_table.len();
        if let Some(slot) = self.interaction.slots.get_mut(index as usize) {
            slot.hits.1 = end;
        }
        self.interaction.open_slots.pop();
    }

    /// An element prepainting now takes keyboard focus as `target`: when it
    /// holds focus, every enclosing group and slot holds it within. Reads
    /// focus (for cache boundaries) only inside a group or slot.
    pub fn note_focus_target(&mut self, target: FocusId) {
        if !self.interaction.is_open() || !self.is_focused(target) {
            return;
        }
        let frame = &mut self.interaction;
        for &g in &frame.open_groups {
            frame.groups[g as usize].focus_within = true;
        }
        for &s in &frame.open_slots {
            frame.slots[s as usize].focus_within = true;
        }
    }

    /// Whether slot `index` shows this frame. Valid after the hit test.
    pub(super) fn interaction_slot_visible(&self, index: u32) -> bool {
        self.interaction
            .slots
            .get(index as usize)
            .is_none_or(|slot| slot.visible)
    }

    /// Hover stack and group resolution for a frame with slots: groups are
    /// hovered through their own rows and rows outside any slot (so hidden
    /// content can neither reveal nor block), slots then show or hide, and
    /// hidden slots give up their rows before the final hover stack.
    pub(super) fn resolve_interaction(&mut self) {
        let point = self.mouse_position;
        let frame = &mut self.interaction;
        match point {
            Some((x, y)) => {
                let table = &self.hit_table;
                table.stack_at_where(x, y, &mut self.hovered, |id| {
                    table.row(id).is_some_and(|row| !frame.in_slot(row))
                });
            }
            None => self.hovered.clear(),
        }
        for group in &mut frame.groups {
            let (start, end) = group.hits;
            group.active = group.focus_within
                || self.hovered.iter().any(|&id| {
                    self.hit_table
                        .row(id)
                        .is_some_and(|row| (start..end).contains(&row))
                });
        }
        for slot in &mut frame.slots {
            let active = slot.group.is_some_and(|g| frame.groups[g as usize].active);
            slot.visible = slot.focus_within || slot.condition.shows(active);
            if !slot.visible {
                self.hit_table.disable(slot.hits.0..slot.hits.1);
            }
        }
        match point {
            Some((x, y)) => self.hit_table.stack_at_into(x, y, &mut self.hovered),
            None => self.hovered.clear(),
        }
    }

    pub(super) fn has_interaction_slots(&self) -> bool {
        !self.interaction.slots.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Styled;

    #[derive(Debug, Clone, PartialEq)]
    enum Msg {
        Open,
        Pin,
    }

    impl From<Msg> for Action {
        fn from(msg: Msg) -> Self {
            Action::new(msg)
        }
    }

    const ROW: GroupId = GroupId::new("row");
    const W: f32 = 400.0;
    const H: f32 = 200.0;
    /// Inside the row, away from its trailing slot.
    const OVER_ROW: (f32, f32) = (40.0, 16.0);
    /// Over the trailing slot, where Pin and the age overlap.
    const OVER_PIN: (f32, f32) = (330.0, 16.0);
    const AWAY: (f32, f32) = (40.0, 150.0);

    fn focus(id: &str) -> FocusId {
        FocusId::from_key(id)
    }

    /// A 32-point row that is clickable (so focusable), with a title and a
    /// trailing 80-point slot where the age and a Pin button overlap.
    fn row(pin: Div) -> Div {
        div()
            .w(W)
            .h(32.0)
            .flex_row()
            .id("row")
            .test_id("row")
            .on_click(Msg::Open)
            .interaction_group(ROW)
            .child(div().w(300.0).h(32.0).test_id("title"))
            .child(
                div()
                    .w(80.0)
                    .h(32.0)
                    .relative()
                    .child(
                        div()
                            .absolute()
                            .size_full()
                            .test_id("age")
                            .show_when(GroupCondition::Idle(ROW)),
                    )
                    .child(pin.show_when(GroupCondition::HoveredOrFocusWithin(ROW))),
            )
    }

    fn pin() -> Div {
        div()
            .absolute()
            .size_full()
            .id("pin")
            .test_id("pin")
            .on_click(Msg::Pin)
    }

    struct Harness {
        text: TextSystem,
        layouts: LayoutCache,
        theme: Theme,
        signals: SignalStore,
        cache: ElementCache,
    }

    impl Harness {
        fn new() -> Self {
            Self {
                text: TextSystem::vendored_only(&Default::default()),
                layouts: LayoutCache::default(),
                theme: Theme::default_dark(),
                signals: SignalStore::new(),
                cache: ElementCache::new(),
            }
        }

        /// Paint `root` with the pointer at `mouse` and `focus` focused,
        /// through the harness's element cache, and route input through
        /// the result.
        fn frame(
            &mut self,
            root: impl IntoAnyElement,
            mouse: Option<(f32, f32)>,
            focus: Option<FocusId>,
        ) -> InputRouter {
            let mut cx = ElementContext::new(
                &self.theme,
                1.0,
                &mut self.text,
                &mut self.layouts,
                mouse,
                &self.signals,
            )
            .with_focus(focus)
            .with_element_cache(&mut self.cache);
            cx.semantic = SemanticFrame::new(W, H);
            let mut root = root.into_any();
            render_element(&mut root, &mut Scene::default(), &mut cx, W, H);
            let mut router = InputRouter::default();
            router.set_frame(cx.take_input_frame());
            router
        }
    }

    /// Where the frame painted `test_id`, `None` when it painted nothing
    /// by that name.
    fn painted(router: &InputRouter, test_id: &str) -> Option<Rect> {
        router
            .frame()
            .geometry
            .by_test_id(test_id)
            .ok()
            .map(|g| g.bounds)
    }

    fn click(router: &mut InputRouter, (x, y): (f32, f32)) -> Vec<Action> {
        let delivery = router.pointer_down(x, y, &mut None);
        router.pointer_up();
        delivery.actions
    }

    #[test]
    fn hovering_the_row_swaps_age_for_actions_without_moving_anything() {
        let mut h = Harness::new();
        let idle = h.frame(row(pin()), Some(AWAY), None);
        let hovered = h.frame(row(pin()), Some(OVER_ROW), None);

        assert_eq!(painted(&idle, "pin"), None);
        assert_eq!(painted(&hovered, "age"), None);
        assert_eq!(painted(&hovered, "pin"), painted(&idle, "age"));
        for id in ["row", "title"] {
            assert_eq!(painted(&hovered, id), painted(&idle, id), "{id}");
        }
    }

    // Hidden content keeps no hit entry: its cursor does not show and a
    // click there reaches the row, though Pin is laid out there.
    #[test]
    fn hidden_action_takes_neither_cursor_nor_click() {
        let mut h = Harness::new();
        let mut router = h.frame(row(pin().cursor(CursorHint::Crosshair)), Some(AWAY), None);

        let (x, y) = OVER_PIN;
        assert_eq!(router.cursor_at(x, y), CursorHint::Pointer);
        let clicked = click(&mut router, OVER_PIN);
        assert_eq!(clicked.len(), 1);
        assert_eq!(clicked[0].downcast_ref(), Some(&Msg::Open));
    }

    // The Pin button blocks the mouse, so the row beneath it is not in the
    // hover stack over it; the pointer on the revealed child still keeps
    // the group active, and the click lands on Pin.
    #[test]
    fn pointer_on_revealed_action_keeps_it_shown_and_clicks_it() {
        let mut h = Harness::new();
        let mut router = h.frame(row(pin().block_mouse()), Some(OVER_PIN), None);

        assert!(painted(&router, "pin").is_some());
        let clicked = click(&mut router, OVER_PIN);
        assert_eq!(clicked.len(), 1);
        assert_eq!(clicked[0].downcast_ref(), Some(&Msg::Pin));
    }

    // Hidden content reaching outside its group cannot reveal itself: the
    // pointer over where it would be is not over the group.
    #[test]
    fn pointer_over_hidden_overflowing_action_does_not_reveal_it() {
        let mut h = Harness::new();
        let below = || pin().top(40.0).h(20.0);
        let router = h.frame(row(below()), Some((330.0, 50.0)), None);

        assert_eq!(painted(&router, "pin"), None);
    }

    #[test]
    fn focusing_the_row_reveals_its_actions_next_in_tab_order() {
        let mut h = Harness::new();
        let router = h.frame(row(pin()), None, Some(focus("row")));

        assert!(painted(&router, "pin").is_some());
        assert_eq!(
            router.traverse_focus(Some(focus("row")), false),
            Some(focus("pin"))
        );
    }

    #[test]
    fn focused_action_stays_shown_until_focus_leaves_the_group() {
        let mut h = Harness::new();
        let focused = h.frame(row(pin()), Some(AWAY), Some(focus("pin")));
        let left = h.frame(row(pin()), Some(AWAY), Some(focus("elsewhere")));

        assert!(painted(&focused, "pin").is_some());
        assert_eq!(painted(&left, "pin"), None);
    }

    // An Idle slot hides while its group holds focus, except when the
    // focus is inside the slot itself.
    #[test]
    fn slot_holding_focus_shows_whatever_its_condition() {
        let mut h = Harness::new();
        let root = div().w(W).h(32.0).interaction_group(ROW).child(
            div()
                .w(80.0)
                .h(32.0)
                .id("age")
                .test_id("age")
                .on_click(Msg::Open)
                .show_when(GroupCondition::Idle(ROW)),
        );
        let router = h.frame(root, None, Some(focus("age")));
        assert!(painted(&router, "age").is_some());
    }

    // A popover above the row that blocks the mouse keeps the row from
    // revealing its actions through it.
    #[test]
    fn occluded_row_does_not_reveal_its_actions() {
        let mut h = Harness::new();
        let root = || {
            div().w(W).h(H).relative().child(row(pin())).child(
                div()
                    .absolute()
                    .top(0.0)
                    .left(0.0)
                    .w(W)
                    .h(100.0)
                    .z_index(10)
                    .block_mouse(),
            )
        };
        let router = h.frame(root(), Some(OVER_ROW), None);
        assert_eq!(painted(&router, "pin"), None);
    }

    // A cached row replays while it stays idle; the replayed hits of its
    // hidden actions must stay inert.
    #[test]
    fn replayed_idle_row_keeps_hidden_action_inert() {
        let mut h = Harness::new();
        let root = || cached("row", 0, || row(pin().cursor(CursorHint::Crosshair)));
        h.frame(root(), Some(AWAY), None);
        let router = h.frame(root(), Some(AWAY), None);

        let (x, y) = OVER_PIN;
        assert_ne!(router.cursor_at(x, y), CursorHint::Crosshair);
    }

    // The group lies outside the cached slot content: hovering the group
    // away from the boundary must not record a visible slot that replays
    // once the pointer leaves.
    #[test]
    fn slot_cached_apart_from_its_group_follows_the_group() {
        let mut h = Harness::new();
        let root = || {
            div()
                .w(W)
                .h(32.0)
                .flex_row()
                .interaction_group(ROW)
                .child(div().w(300.0).h(32.0))
                .child(cached("slot", 0, || {
                    div()
                        .w(80.0)
                        .h(32.0)
                        .test_id("pin")
                        .show_when(GroupCondition::HoveredOrFocusWithin(ROW))
                }))
        };
        let hovered = h.frame(root(), Some(OVER_ROW), None);
        let left = h.frame(root(), Some(AWAY), None);

        assert!(painted(&hovered, "pin").is_some());
        assert_eq!(painted(&left, "pin"), None);
    }
}
