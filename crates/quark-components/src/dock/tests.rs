use super::*;

const LEFT: PanelId = PanelId(1);
const CHAT: PanelId = PanelId(2);
const A: PanelId = PanelId(10);
const B: PanelId = PanelId(11);
const C: PanelId = PanelId(12);

/// Threads on the left, chat in the center, A B C on the right.
fn dock() -> DockState {
    let mut dock = DockState::new(DockLayout::default());
    dock.open(DockRegion::Left, LEFT);
    dock.open(DockRegion::Center, CHAT);
    for panel in [A, B, C] {
        dock.open(DockRegion::Right, panel);
    }
    dock
}

fn pane_of(dock: &DockState, panel: PanelId) -> PaneId {
    dock.locate(panel).expect("panel is docked").1
}

/// Drop `panel` on the group holding `onto`, in `zone`.
fn drop(dock: &mut DockState, panel: PanelId, onto: PanelId, zone: DropZone) -> bool {
    let pane = pane_of(dock, onto);
    dock.apply(
        DockEvent::TabDrop {
            panel,
            target: Some(PaneDrop { pane, zone }),
        },
        0,
    )
}

#[test]
fn dropping_a_tab_moves_reorders_and_splits() {
    // (panel, onto the group of, zone, dock after)
    let cases: &[(PanelId, PanelId, DropZone, &str)] = &[
        (
            A,
            B,
            DropZone::Tabs(2),
            "left: [1*]\nright: [11 12 10*]\ncenter: [2*]\n",
        ),
        (
            C,
            B,
            DropZone::Tabs(0),
            "left: [1*]\nright: [12* 10 11]\ncenter: [2*]\n",
        ),
        (
            A,
            CHAT,
            DropZone::Center,
            "left: [1*]\nright: [11 12*]\ncenter: [2 10*]\n",
        ),
        (
            A,
            CHAT,
            DropZone::Tabs(0),
            "left: [1*]\nright: [11 12*]\ncenter: [10* 2]\n",
        ),
        (
            A,
            B,
            DropZone::Bottom,
            "left: [1*]\nright: col([11 12*] | [10*])\ncenter: [2*]\n",
        ),
        (
            C,
            CHAT,
            DropZone::Left,
            "left: [1*]\nright: [10 11*]\ncenter: row([12*] | [2*])\n",
        ),
    ];
    for &(panel, onto, zone, expected) in cases {
        let mut dock = dock();
        assert!(drop(&mut dock, panel, onto, zone), "{panel:?} {zone:?}");
        assert_eq!(dock.dump(), expected, "{panel:?} onto {onto:?} {zone:?}");
    }
}

#[test]
fn splits_of_one_axis_share_a_parent() {
    let mut dock = dock();
    drop(&mut dock, A, CHAT, DropZone::Right);
    drop(&mut dock, B, A, DropZone::Right);
    drop(&mut dock, C, A, DropZone::Bottom);
    assert_eq!(
        dock.dump(),
        "left: [1*]\ncenter: row([2*] | col([10*] | [12*]) | [11*])\n"
    );
}

#[test]
fn emptied_groups_collapse_out_of_the_tree() {
    let mut dock = dock();
    drop(&mut dock, A, CHAT, DropZone::Right);
    drop(&mut dock, B, A, DropZone::Bottom);
    // Moving A out leaves B's column with one child, which unwraps.
    drop(&mut dock, A, CHAT, DropZone::Center);
    assert_eq!(
        dock.dump(),
        "left: [1*]\nright: [12*]\ncenter: row([2 10*] | [11*])\n"
    );
    // Closing B's only tab leaves the center a single group again.
    dock.close(pane_of(&dock, B), 0);
    assert_eq!(dock.dump(), "left: [1*]\nright: [12*]\ncenter: [2 10*]\n");
}

#[test]
fn moving_the_last_tab_out_of_a_side_region_hides_it() {
    let mut dock = dock();
    drop(&mut dock, LEFT, CHAT, DropZone::Center);
    assert!(!dock.is_visible(DockRegion::Left));
    assert_eq!(dock.dump(), "right: [10 11 12*]\ncenter: [2 1*]\n");
}

#[test]
fn policies_and_confinement_refuse_moves_across_regions() {
    // (policy for the right region, confined panel, drop, allowed)
    let sealed = TabPolicy::SEALED;
    let keeps = TabPolicy {
        can_leave: false,
        accepts: true,
    };
    type Case = (
        TabPolicy,
        Option<PanelId>,
        (PanelId, PanelId, DropZone),
        bool,
    );
    let cases: &[Case] = &[
        (keeps, None, (A, CHAT, DropZone::Center), false),
        (keeps, None, (LEFT, A, DropZone::Center), true),
        (sealed, None, (LEFT, A, DropZone::Center), false),
        (sealed, None, (LEFT, A, DropZone::Left), false),
        // Inside a sealed region, tabs still reorder and split.
        (sealed, None, (A, B, DropZone::Tabs(2)), true),
        (sealed, None, (A, B, DropZone::Bottom), true),
        (TabPolicy::OPEN, Some(A), (A, CHAT, DropZone::Center), false),
        (TabPolicy::OPEN, Some(A), (B, CHAT, DropZone::Center), true),
    ];
    for &(policy, confined, (panel, onto, zone), allowed) in cases {
        let mut dock = dock();
        dock.set_policy(DockRegion::Right, policy);
        if let Some(p) = confined {
            dock.confine(p, true);
        }
        let before = dock.dump();
        assert_eq!(
            drop(&mut dock, panel, onto, zone),
            allowed,
            "{policy:?} {panel:?} {zone:?}"
        );
        if !allowed {
            assert_eq!(dock.dump(), before);
        }
    }
}

#[test]
fn a_lone_tab_cannot_split_its_own_group() {
    let mut dock = dock();
    assert!(!drop(&mut dock, CHAT, CHAT, DropZone::Right));
    assert!(drop(&mut dock, A, A, DropZone::Right));
    assert_eq!(
        dock.dump(),
        "left: [1*]\nright: row([11 12*] | [10*])\ncenter: [2*]\n"
    );
}

// Each move of a pressed divider counts from the press, and a move after
// the release changes nothing.
#[test]
fn a_dragged_divider_follows_the_pointer_from_its_press() {
    let mut dock = dock();
    drop(&mut dock, A, CHAT, DropZone::Right);
    let PaneNode::Split(split) = dock.root(DockRegion::Center) else {
        panic!("center is split");
    };
    let split = split.id;
    let mut divider = |event| {
        dock.apply(
            DockEvent::PaneDivider {
                split,
                divider: 0,
                event,
            },
            0,
        )
    };
    // 1000 points for the two groups beside a 1 point divider.
    let drag = |delta| PaneDividerEvent::Drag {
        delta,
        extent: 1001.0,
    };

    divider(PaneDividerEvent::Press);
    divider(drag(30.0));
    divider(drag(100.0));
    divider(PaneDividerEvent::Release);
    divider(drag(300.0));

    let PaneNode::Split(split) = dock.root(DockRegion::Center) else {
        panic!("center is split");
    };
    assert_eq!(split.weights, [0.6, 0.4]);
}

/// Apply a [`DockEvent::MoveTab`] of `panel` to the end of the group
/// holding `onto`.
fn move_onto(dock: &mut DockState, panel: PanelId, onto: PanelId) -> DockOutcome {
    let destination = pane_of(dock, onto);
    dock.apply_event(
        DockEvent::MoveTab {
            panel,
            destination,
            index: None,
        },
        0,
    )
}

#[test]
fn move_targets_follow_tree_order_and_skip_hidden_regions() {
    let mut dock = dock();
    // The right region split in two: [A] beside [B C].
    drop(&mut dock, A, B, DropZone::Left);
    const D: PanelId = PanelId(30);
    dock.open(DockRegion::Bottom, D);
    // (panel, forward, target holding)
    let cases: &[(PanelId, bool, Option<PanelId>)] = &[
        (LEFT, false, None),
        (LEFT, true, Some(CHAT)),
        (CHAT, false, Some(LEFT)),
        (CHAT, true, Some(D)),
        (D, true, Some(A)),
        (A, true, Some(B)),
        (B, false, Some(A)),
        (B, true, None),
    ];
    for &(panel, forward, expected) in cases {
        assert_eq!(
            dock.move_target(panel, forward),
            expected.map(|p| pane_of(&dock, p)),
            "{panel:?} forward {forward}"
        );
    }
    dock.set_visible(DockRegion::Bottom, false);
    assert_eq!(dock.move_target(CHAT, true), Some(pane_of(&dock, A)));
}

// Catches an emptied region dropping out of the move order: after its
// last tab moved out, the sidebar was hidden and no key moved it back.
#[test]
fn a_tab_moves_back_into_the_region_it_emptied() {
    let mut dock = dock();
    assert!(dock.move_tab(LEFT, pane_of(&dock, CHAT), None));
    assert!(!dock.is_visible(DockRegion::Left));

    let back = dock.move_target(LEFT, false).expect("the empty sidebar");
    assert!(dock.move_tab(LEFT, back, None));
    assert_eq!(
        dock.dump(),
        "left: [1*]\nright: [10 11 12*]\ncenter: [2*]\n"
    );
    assert!(dock.is_visible(DockRegion::Left));
}

#[test]
fn move_targets_skip_groups_policy_refuses() {
    let mut dock = dock();
    dock.set_policy(DockRegion::Center, TabPolicy::SEALED);
    dock.set_policy(DockRegion::Bottom, TabPolicy::SEALED);
    // Past the sealed center and bottom, to the right region.
    assert_eq!(dock.move_target(LEFT, true), Some(pane_of(&dock, A)));
    dock.confine(LEFT, true);
    assert_eq!(dock.move_target(LEFT, true), None);
    assert_eq!(dock.move_targets(LEFT), []);
}

#[test]
fn a_refused_move_changes_nothing() {
    let mut dock = dock();
    dock.set_policy(DockRegion::Right, TabPolicy::SEALED);
    let before = dock.dump();
    assert_eq!(move_onto(&mut dock, A, CHAT), DockOutcome::default());
    // Its own group is no destination either.
    assert_eq!(move_onto(&mut dock, A, B), DockOutcome::default());
    assert_eq!(dock.dump(), before);
}

#[test]
fn a_moved_tab_lands_at_the_end_of_its_destination_and_is_active() {
    let mut dock = dock();
    let outcome = move_onto(&mut dock, B, CHAT);
    assert_eq!(
        dock.dump(),
        "left: [1*]\nright: [10 12*]\ncenter: [2 11*]\n"
    );
    assert_eq!(
        outcome.moved,
        Some(TabMove {
            panel: B,
            host: HostId::MAIN,
            region: DockRegion::Center,
            pane: pane_of(&dock, CHAT),
        })
    );
    assert!(outcome.settled);
}

#[test]
fn moving_a_groups_last_tab_prunes_the_group_and_keeps_the_destination() {
    let mut dock = dock();
    drop(&mut dock, A, CHAT, DropZone::Right);
    let destination = pane_of(&dock, CHAT);
    let outcome = move_onto(&mut dock, A, CHAT);
    assert_eq!(
        dock.dump(),
        "left: [1*]\nright: [11 12*]\ncenter: [2 10*]\n"
    );
    // The destination group keeps its identity, so focus can follow.
    assert_eq!(outcome.moved.map(|m| m.pane), Some(destination));
    assert_eq!(dock.focused_panel(outcome.focus()), Some(A));
}

#[test]
fn a_snapshot_restores_split_groups() {
    let mut dock = dock();
    drop(&mut dock, A, CHAT, DropZone::Bottom);
    drop(&mut dock, B, A, DropZone::Right);
    let mut restored = DockState::new(DockLayout::default());
    restored.restore(&dock.snapshot(), |p| p != C);
    assert_eq!(
        restored.dump(),
        "left: [1*]\ncenter: col([2*] | row([10*] | [11*]))\n"
    );
}
