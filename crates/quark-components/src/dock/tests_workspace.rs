//! Hosts, transfers between them, and workspace persistence.

use proptest::prelude::*;

use super::*;

const LEFT: PanelId = PanelId(1);
const CHAT: PanelId = PanelId(2);
const A: PanelId = PanelId(10);
const B: PanelId = PanelId(11);
const C: PanelId = PanelId(12);

/// Threads on the left, chat in the center, A B C on the right with B
/// active.
fn dock() -> DockState {
    let mut dock = DockState::new(DockLayout::default());
    dock.open(DockRegion::Left, LEFT);
    dock.open(DockRegion::Center, CHAT);
    for panel in [A, B, C] {
        dock.open(DockRegion::Right, panel);
    }
    dock.open(DockRegion::Right, B);
    dock
}

fn pane_of(dock: &DockState, panel: PanelId) -> PaneId {
    dock.location(panel).expect("panel is docked").0.pane
}

/// Lets its tabs go, takes no others.
const TAKES_NONE: TabPolicy = TabPolicy {
    can_leave: true,
    accepts: false,
};

/// Into the group holding `onto`, in `zone`.
fn onto(dock: &DockState, panel: PanelId, zone: DropZone) -> DockDestination {
    let (at, _) = dock.location(panel).expect("panel is docked");
    DockDestination {
        host: at.host,
        pane: at.pane,
        zone,
    }
}

/// Move `payload` to a new floating host, its window made at once.
fn float(dock: &mut DockState, payload: MovePayload) -> HostId {
    let host = dock.reserve_host(payload).expect("may float");
    dock.commit_host(host).expect("window made");
    host
}

#[test]
fn moves_between_hosts_respect_area_policies_and_confinement() {
    let keeps = TabPolicy {
        can_leave: false,
        accepts: true,
    };
    let refuse = |panel, boundary| Err(TransferRefusal::Policy { panel, boundary });
    // (main center policy, floating host policy, panel confined, move,
    // result). The host holds C; "new" floats the panel instead.
    type Case = (
        TabPolicy,
        TabPolicy,
        Option<PanelId>,
        (PanelId, Option<PanelId>),
        Result<(), TransferRefusal>,
    );
    use Boundary::*;
    let cases: &[Case] = &[
        (
            TabPolicy::OPEN,
            TabPolicy::OPEN,
            None,
            (CHAT, Some(C)),
            Ok(()),
        ),
        (
            TabPolicy::OPEN,
            TabPolicy::OPEN,
            None,
            (C, Some(CHAT)),
            Ok(()),
        ),
        // A sealed center keeps its tabs from every other window's
        // center, and takes none of theirs.
        (
            TabPolicy::SEALED,
            TabPolicy::OPEN,
            None,
            (CHAT, Some(C)),
            refuse(CHAT, CannotLeave),
        ),
        (
            TabPolicy::SEALED,
            TabPolicy::OPEN,
            None,
            (CHAT, None),
            refuse(CHAT, CannotLeave),
        ),
        (
            TabPolicy::SEALED,
            TabPolicy::OPEN,
            None,
            (C, Some(CHAT)),
            refuse(C, NotAccepted),
        ),
        (
            TabPolicy::OPEN,
            TabPolicy::SEALED,
            None,
            (C, Some(CHAT)),
            refuse(C, CannotLeave),
        ),
        (
            TabPolicy::OPEN,
            TabPolicy::SEALED,
            None,
            (CHAT, Some(C)),
            refuse(CHAT, NotAccepted),
        ),
        (TabPolicy::OPEN, keeps, None, (CHAT, Some(C)), Ok(())),
        // Confinement binds to the panel's host as well as its region.
        (
            TabPolicy::OPEN,
            TabPolicy::OPEN,
            Some(C),
            (C, Some(CHAT)),
            refuse(C, Confined),
        ),
        (
            TabPolicy::OPEN,
            TabPolicy::OPEN,
            Some(CHAT),
            (CHAT, None),
            refuse(CHAT, Confined),
        ),
    ];
    for (i, &(center, host_policy, confined, (panel, to), expected)) in cases.iter().enumerate() {
        let mut dock = dock();
        let host = float(&mut dock, MovePayload::Panel(C));
        dock.set_policy(DockRegion::Center, center);
        dock.set_host_policy(host, host_policy);
        if let Some(p) = confined {
            dock.confine(p, true);
        }
        let before = dock.dump();
        let result = match to {
            Some(to) => dock
                .transfer(MovePayload::Panel(panel), onto(&dock, to, DropZone::Center))
                .map(|_| ()),
            None => dock.reserve_host(MovePayload::Panel(panel)).map(|_| ()),
        };
        assert_eq!(result, expected, "case {i}");
        if result.is_err() {
            assert_eq!(dock.dump(), before, "case {i}");
        }
    }
}

#[test]
fn a_confined_panel_still_splits_inside_its_floating_host() {
    let mut dock = dock();
    let group = MovePayload::Group(pane_of(&dock, A));
    let host = float(&mut dock, group);
    dock.confine(B, true);
    let effects = dock
        .transfer(MovePayload::Panel(B), onto(&dock, A, DropZone::Bottom))
        .expect("inside its host");
    assert_eq!(
        dock.dump(),
        "left: [1*]\ncenter: [2*]\nhost 1: col([10 12*] | [11*])\n"
    );
    assert_eq!(effects.destination.map(|d| d.host), Some(host));
}

#[test]
fn a_group_moves_whole_keeping_its_order_and_active_tab() {
    // (zone onto CHAT's group, or a new host for None; dock after)
    let cases: &[(Option<DropZone>, &str)] = &[
        (
            Some(DropZone::Center),
            "left: [1*]\ncenter: [2 10 11* 12]\n",
        ),
        (
            Some(DropZone::Tabs(0)),
            "left: [1*]\ncenter: [10 11* 12 2]\n",
        ),
        (
            Some(DropZone::Left),
            "left: [1*]\ncenter: row([10 11* 12] | [2*])\n",
        ),
        (None, "left: [1*]\ncenter: [2*]\nhost 1: [10 11* 12]\n"),
    ];
    for &(zone, expected) in cases {
        let mut dock = dock();
        let group = MovePayload::Group(pane_of(&dock, A));
        let effects = match zone {
            Some(zone) => dock.transfer(group, onto(&dock, CHAT, zone)).unwrap(),
            None => {
                let host = dock.reserve_host(group).unwrap();
                dock.commit_host(host).unwrap()
            }
        };
        assert_eq!(dock.dump(), expected, "{zone:?}");
        assert_eq!(effects.moved, [A, B, C], "{zone:?}");
        assert_eq!(
            dock.group(effects.destination.unwrap().pane)
                .and_then(TabGroup::active_panel),
            Some(B),
            "{zone:?}"
        );
    }
}

#[test]
fn one_refused_member_refuses_the_whole_group() {
    let mut dock = dock();
    dock.confine(C, true);
    let before = dock.dump();
    let group = MovePayload::Group(pane_of(&dock, A));
    assert_eq!(
        dock.transfer(group, onto(&dock, CHAT, DropZone::Center)),
        Err(TransferRefusal::Policy {
            panel: C,
            boundary: Boundary::Confined
        })
    );
    assert_eq!(
        dock.reserve_host(group),
        Err(TransferRefusal::Policy {
            panel: C,
            boundary: Boundary::Confined
        })
    );
    assert_eq!(dock.dump(), before);
}

#[test]
fn an_aborted_reservation_leaves_the_panels_in_place() {
    let mut dock = dock();
    let before = dock.dump();
    let host = dock.reserve_host(MovePayload::Panel(A)).unwrap();
    assert_eq!(dock.dump(), before);
    assert!(dock.abort_host(host));
    assert_eq!(
        dock.commit_host(host),
        Err(TransferRefusal::MissingHost(host))
    );
    assert_eq!(dock.dump(), before);
    assert_eq!(dock.hosts(), []);
}

#[test]
fn a_reservation_whose_panel_moved_since_is_refused() {
    let mut dock = dock();
    let host = dock.reserve_host(MovePayload::Panel(A)).unwrap();
    dock.transfer(MovePayload::Panel(A), onto(&dock, CHAT, DropZone::Center))
        .unwrap();
    assert_eq!(dock.commit_host(host), Err(TransferRefusal::Stale));
    assert_eq!(
        dock.dump(),
        "left: [1*]\nright: [11* 12]\ncenter: [2 10*]\n"
    );
    assert_eq!(dock.hosts(), []);
}

#[test]
fn moving_a_floating_hosts_last_tab_out_removes_the_host() {
    let mut dock = dock();
    let host = float(&mut dock, MovePayload::Panel(A));
    let effects = dock
        .transfer(MovePayload::Panel(A), onto(&dock, CHAT, DropZone::Center))
        .unwrap();
    assert_eq!(effects.closed_hosts, [host]);
    assert_eq!(dock.hosts(), []);
    assert_eq!(
        dock.dump(),
        "left: [1*]\nright: [11* 12]\ncenter: [2 10*]\n"
    );
}

#[test]
fn closing_a_floating_host_puts_its_panels_back() {
    // (after floating B: what changes in the main host, dock after close)
    type Setup = fn(&mut DockState);
    let cases: &[(&str, Setup, &str)] = &[
        (
            "where it left",
            |_| {},
            "left: [1*]\nright: [10 11* 12]\ncenter: [2*]\n",
        ),
        (
            "its group gone, into its region",
            |dock| {
                let to = onto(dock, CHAT, DropZone::Bottom);
                dock.transfer(MovePayload::Group(pane_of(dock, A)), to)
                    .unwrap();
                dock.open(DockRegion::Right, PanelId(30));
            },
            "left: [1*]\nright: [30 11*]\ncenter: col([2*] | [10 12*])\n",
        ),
        (
            "its region refusing, into the center",
            |dock| dock.set_policy(DockRegion::Right, TAKES_NONE),
            "left: [1*]\nright: [10 12*]\ncenter: [2 11*]\n",
        ),
    ];
    for &(name, setup, expected) in cases {
        let mut dock = dock();
        let host = float(&mut dock, MovePayload::Panel(B));
        setup(&mut dock);
        let effects = dock.close_host(host).expect(name);
        assert_eq!(dock.dump(), expected, "{name}");
        assert_eq!(effects.closed_hosts, [host], "{name}");
        assert_eq!(
            effects.focus(),
            Some((HostId::MAIN, Dock::tab_focus(pane_of(&dock, B)))),
            "{name}"
        );
    }
}

#[test]
fn closing_a_floating_host_is_refused_whole_when_a_panel_cannot_leave() {
    // (what strands a panel of the host holding A and B, the panel, the
    // refusal)
    type Setup = fn(&mut DockState, HostId);
    let cases: &[(Setup, PanelId, Boundary)] = &[
        (|dock, _| dock.confine(B, true), B, Boundary::Confined),
        (
            |dock, host| dock.set_host_policy(host, TabPolicy::SEALED),
            A,
            Boundary::CannotLeave,
        ),
        (
            |dock, _| {
                for region in DockRegion::ALL {
                    dock.set_policy(region, TAKES_NONE);
                }
            },
            A,
            Boundary::NotAccepted,
        ),
    ];
    for &(setup, panel, boundary) in cases {
        let mut dock = dock();
        let host = float(&mut dock, MovePayload::Panel(A));
        dock.transfer(MovePayload::Panel(B), onto(&dock, A, DropZone::Center))
            .unwrap();
        setup(&mut dock, host);
        let before = dock.dump();
        let refusal = Err(TransferRefusal::Policy { panel, boundary });
        assert_eq!(dock.can_close_host(host), refusal, "{boundary:?}");
        assert_eq!(dock.close_host(host).map(|_| ()), refusal, "{boundary:?}");
        // Nothing moves, not even a panel that could go back.
        assert_eq!(dock.dump(), before, "{boundary:?}");
    }
}

#[test]
fn recovering_a_host_keeps_panels_with_no_legal_destination() {
    let mut dock = dock();
    let host = float(&mut dock, MovePayload::Panel(A));
    dock.confine(A, true);
    let effects = dock.recover_host(host).unwrap();
    assert_eq!(effects.closed_hosts, [host]);
    assert_eq!(
        dock.dump(),
        "left: [1*]\nright: [11* 12]\ncenter: [2 10*]\n"
    );
}

#[test]
fn move_options_list_groups_in_every_host_then_a_new_window() {
    let mut dock = dock();
    let host = float(&mut dock, MovePayload::Panel(C));
    let group = |panel| MoveTarget::Group {
        host: dock.location(panel).unwrap().0.host,
        pane: pane_of(&dock, panel),
    };
    let floating = group(C);
    let (left, center, right) = (group(LEFT), group(CHAT), group(A));
    // (payload, options)
    let cases = [
        (
            MovePayload::Panel(B),
            vec![left, center, floating, MoveTarget::NewHost],
        ),
        // Alone in its window: a new one would change nothing.
        (MovePayload::Panel(C), vec![left, center, right]),
        (
            MovePayload::Group(pane_of(&dock, A)),
            vec![left, center, floating, MoveTarget::NewHost],
        ),
        (
            MovePayload::Group(pane_of(&dock, C)),
            vec![left, center, right],
        ),
    ];
    for (payload, expected) in cases {
        assert_eq!(dock.move_options(payload), expected, "{payload:?}");
    }
    assert_eq!(dock.hosts(), [host]);
}

#[test]
fn a_move_to_new_window_event_reserves_a_host_without_moving() {
    let mut dock = dock();
    let before = dock.dump();
    let outcome = dock.apply_event(DockEvent::MoveToNewHost(MovePayload::Panel(A)), 0);
    let host = outcome.effects.create_host.expect("reserved");
    assert!(!outcome.settled);
    assert_eq!(dock.dump(), before);
    let effects = dock.commit_host(host).unwrap();
    assert_eq!(
        effects.focus(),
        Some((host, Dock::tab_focus(pane_of(&dock, A))))
    );
}

#[test]
fn a_workspace_snapshot_restores_hosts_and_where_panels_return() {
    let mut dock = dock();
    dock.transfer(MovePayload::Panel(A), onto(&dock, B, DropZone::Bottom))
        .unwrap();
    dock.transfer(MovePayload::Panel(C), onto(&dock, A, DropZone::Center))
        .unwrap();
    // C leaves the right region's second group, at index 1.
    let host = float(&mut dock, MovePayload::Panel(C));
    let json = serde_json::to_string(&dock.workspace_snapshot()).unwrap();

    let stored: StoredDock = serde_json::from_str(&json).unwrap();
    let mut restored = DockState::new(DockLayout::default());
    let hosts = restored.restore_workspace(&stored.into_workspace(), |_| true);
    assert_eq!(hosts, [host]);
    assert_eq!(restored.dump(), dock.dump());
    // Back into its group, which renumbering renamed.
    restored.close_host(host).unwrap();
    assert_eq!(
        restored.dump(),
        "left: [1*]\nright: col([11*] | [10 12*])\ncenter: [2*]\n"
    );
    // A host made after a restore takes a fresh id.
    assert_ne!(restored.reserve_host(MovePayload::Panel(B)), Ok(host));
}

#[test]
fn a_saved_main_layout_restores_as_a_workspace_without_hosts() {
    let mut dock = dock();
    dock.transfer(MovePayload::Panel(A), onto(&dock, CHAT, DropZone::Right))
        .unwrap();
    let json = serde_json::to_string(&dock.snapshot()).unwrap();

    let stored: StoredDock = serde_json::from_str(&json).unwrap();
    assert!(matches!(stored, StoredDock::Legacy(_)));
    let mut restored = DockState::new(DockLayout::default());
    restored.open(DockRegion::Left, PanelId(50));
    float(&mut restored, MovePayload::Panel(PanelId(50)));
    assert_eq!(
        restored.restore_workspace(&stored.into_workspace(), |_| true),
        []
    );
    assert_eq!(restored.dump(), dock.dump());
}

#[test]
fn restore_drops_repeats_unknown_panels_and_empty_hosts() {
    let tabs = |id, panels: &[PanelId]| {
        PaneNode::Tabs(TabGroup {
            id: PaneId(id),
            panels: panels.to_vec(),
            active: 0,
        })
    };
    let mut snapshot = dock().workspace_snapshot();
    snapshot.floating = vec![
        // A is in the main host already.
        FloatingSnapshot {
            host: HostId(3),
            root: tabs(0, &[A, PanelId(40)]),
        },
        FloatingSnapshot {
            host: HostId(4),
            root: tabs(0, &[PanelId(99)]),
        },
        FloatingSnapshot {
            host: HostId(3),
            root: tabs(0, &[PanelId(41)]),
        },
    ];
    let mut restored = DockState::new(DockLayout::default());
    let hosts = restored.restore_workspace(&snapshot, |p| p != PanelId(99));
    assert_eq!(hosts, [HostId(3)]);
    assert_eq!(
        restored.dump(),
        "left: [1*]\nright: [10 11* 12]\ncenter: [2*]\nhost 3: [40*]\n"
    );
}

#[test]
fn a_floating_host_builds_only_its_own_tabs() {
    use quark::reactive::SignalStore;
    use quark_render::Scene;
    use quark_ui::accessibility::{AccessibilityFrame, dump_accessibility};
    use quark_ui::element::{ElementContext, render_element};

    let mut dock = dock();
    let host = float(&mut dock, MovePayload::Panel(A));
    dock.set_host_label(host, "Preview window");
    let title = |p: PanelId| format!("Panel {}", p.0);
    let mut element = Dock::new(&dock, (400.0, 300.0), |_| Action::new(()))
        .host(host)
        .build(&Theme::default_dark(), title, |_, _| div().into_any());

    let mut text = quark_text::TextSystem::vendored_only(&Default::default());
    let mut layouts = quark_text::LayoutCache::default();
    let store = SignalStore::new();
    let theme = Theme::default_dark();
    let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &store);
    cx.accessibility = AccessibilityFrame::new(400.0, 300.0);
    render_element(&mut element, &mut Scene::default(), &mut cx, 400.0, 300.0);
    let tabs: Vec<String> = dump_accessibility(&cx.accessibility)
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(" | ").collect();
            matches!(fields[1], "TabList" | "Tab" | "TabPanel")
                .then(|| format!("{} {}", fields[1], fields[2]))
        })
        .collect();
    // One panel, and still a tab strip to drag it back by.
    assert_eq!(
        tabs,
        [
            "TabList Preview window",
            "Tab Panel 10",
            "TabPanel Panel 10"
        ]
    );
}

/// Tear `payload` off live, returning its new host.
fn tear_off(dock: &mut DockState, payload: MovePayload) -> HostId {
    let effects = dock.begin_live_detach(payload).expect("may tear off");
    effects.open_host.expect("a host to open")
}

/// A beside CHAT in the center, their divider moved off the middle.
fn split_center(dock: &mut DockState) {
    dock.transfer(MovePayload::Panel(A), onto(dock, CHAT, DropZone::Right))
        .unwrap();
    let PaneNode::Split(split) = dock.root(DockRegion::Center) else {
        panic!("center is split");
    };
    let event = DockEvent::PaneDivider {
        split: split.id,
        divider: 0,
        event: PaneDividerEvent::Nudge {
            delta: 70.0,
            extent: 800.0,
        },
    };
    dock.apply(event, 0);
}

#[test]
fn cancelling_a_live_tear_off_puts_back_the_layout_it_left() {
    // (setup, what tears off)
    type Case = (fn(&mut DockState), fn(&DockState) -> MovePayload);
    let cases: &[Case] = &[
        (|_| {}, |_| MovePayload::Panel(B)),
        // The right region empties and hides, and shows again.
        (|_| {}, |dock| MovePayload::Group(pane_of(dock, B))),
        // The split it leaves unwraps, and comes back with its divider
        // where it was.
        (split_center, |_| MovePayload::Panel(A)),
    ];
    for (i, &(setup, payload)) in cases.iter().enumerate() {
        let mut dock = dock();
        setup(&mut dock);
        let (before, shown) = (dock.snapshot(), dock.dump());
        let payload = payload(&dock);
        let host = tear_off(&mut dock, payload);
        assert_ne!(dock.snapshot(), before, "case {i}");
        let effects = dock.cancel_live_detach(host).unwrap();
        assert_eq!(dock.snapshot(), before, "case {i}");
        assert_eq!(dock.dump(), shown, "case {i}");
        assert_eq!(effects.closed_hosts, [host], "case {i}");
        assert_eq!(dock.hosts(), [], "case {i}");
    }
}

#[test]
fn cancelling_after_the_source_changed_returns_panels_to_their_place() {
    const D: PanelId = PanelId(30);
    // (setup, what tears off, dock after cancelling once D opened)
    type Case = (
        fn(&mut DockState),
        fn(&DockState) -> MovePayload,
        DockRegion,
        &'static str,
    );
    let cases: &[Case] = &[
        // Back into its group at its index, active as it was.
        (
            |_| {},
            |_| MovePayload::Panel(B),
            DockRegion::Right,
            "left: [1*]\nright: [10 11* 12 30]\ncenter: [2*]\n",
        ),
        // Its group gone: a new one beside where it was.
        (
            split_center,
            |dock| MovePayload::Group(pane_of(dock, A)),
            DockRegion::Center,
            "left: [1*]\nright: [11* 12]\ncenter: row([2 30*] | [10*])\n",
        ),
    ];
    for &(setup, payload, region, expected) in cases {
        let mut dock = dock();
        setup(&mut dock);
        let payload = payload(&dock);
        let host = tear_off(&mut dock, payload);
        dock.open(region, D);
        dock.cancel_live_detach(host).unwrap();
        assert_eq!(dock.dump(), expected, "{payload:?}");
    }
}

#[test]
fn a_live_tear_off_dropped_on_a_group_moves_there_and_closes_its_host() {
    let mut dock = dock();
    let group = MovePayload::Group(pane_of(&dock, A));
    let host = tear_off(&mut dock, group);
    let effects = dock
        .drop_live_detach(host, onto(&dock, CHAT, DropZone::Center))
        .unwrap();
    assert_eq!(dock.dump(), "left: [1*]\ncenter: [2 10 11* 12]\n");
    assert_eq!(effects.closed_hosts, [host]);
    assert!(!dock.is_live(host));
}

#[test]
fn a_live_tear_off_released_over_nothing_stays_a_window() {
    let mut dock = dock();
    let host = tear_off(&mut dock, MovePayload::Panel(B));
    let effects = dock.end_live_detach(host).unwrap();
    assert!(effects.persist && effects.announce);
    assert_eq!(
        dock.dump(),
        "left: [1*]\nright: [10 12*]\ncenter: [2*]\nhost 1: [11*]\n"
    );
    // Closing the window later still finds the way back.
    dock.close_host(host).unwrap();
    assert_eq!(
        dock.dump(),
        "left: [1*]\nright: [10 11* 12]\ncenter: [2*]\n"
    );
}

#[test]
fn sealed_and_confined_tabs_never_tear_off() {
    // (setup, what tears off, refusal)
    type Case = (fn(&mut DockState), PanelId, TransferRefusal);
    let cases: &[Case] = &[
        (
            |dock| dock.set_policy(DockRegion::Right, TabPolicy::SEALED),
            B,
            TransferRefusal::Policy {
                panel: B,
                boundary: Boundary::CannotLeave,
            },
        ),
        (
            |dock| dock.confine(B, true),
            B,
            TransferRefusal::Policy {
                panel: B,
                boundary: Boundary::Confined,
            },
        ),
        // Alone in a window already.
        (
            |dock| {
                float(dock, MovePayload::Panel(C));
            },
            C,
            TransferRefusal::NoMove,
        ),
    ];
    for &(setup, panel, refusal) in cases {
        let mut dock = dock();
        setup(&mut dock);
        let before = dock.dump();
        assert!(
            !dock.can_live_detach(MovePayload::Panel(panel)),
            "{refusal:?}"
        );
        assert_eq!(
            dock.begin_live_detach(MovePayload::Panel(panel)),
            Err(refusal)
        );
        assert_eq!(dock.dump(), before, "{refusal:?}");
    }
}

/// Random workspace edits for the properties below.
#[derive(Debug, Clone)]
enum Op {
    Open(u8, u8),
    Close(u8),
    Move(u8, u8, u8),
    MoveGroup(u8, u8, u8),
    NewHost {
        group: bool,
        pick: u8,
        made: bool,
    },
    CloseHost(u8),
    RecoverHost(u8),
    Policy(u8, u8),
    Confine(u8),
    Restore,
    /// Tear off live, cancelling at once with `undo`.
    LiveBegin {
        group: bool,
        pick: u8,
        undo: bool,
    },
    LiveCancel(u8),
    LiveDrop(u8, u8, u8),
    LiveEnd(u8),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0..8u8, 0..4u8).prop_map(|(p, r)| Op::Open(p, r)),
        (0..16u8).prop_map(Op::Close),
        (0..16u8, 0..16u8, 0..8u8).prop_map(|(p, g, z)| Op::Move(p, g, z)),
        (0..16u8, 0..16u8, 0..8u8).prop_map(|(s, g, z)| Op::MoveGroup(s, g, z)),
        (any::<bool>(), 0..16u8, any::<bool>()).prop_map(|(group, pick, made)| Op::NewHost {
            group,
            pick,
            made
        }),
        (0..4u8).prop_map(Op::CloseHost),
        (0..4u8).prop_map(Op::RecoverHost),
        (0..8u8, 0..4u8).prop_map(|(a, p)| Op::Policy(a, p)),
        (0..16u8).prop_map(Op::Confine),
        Just(Op::Restore),
        (any::<bool>(), 0..16u8, any::<bool>()).prop_map(|(group, pick, undo)| Op::LiveBegin {
            group,
            pick,
            undo
        }),
        (0..4u8).prop_map(Op::LiveCancel),
        (0..4u8, 0..16u8, 0..8u8).prop_map(|(h, g, z)| Op::LiveDrop(h, g, z)),
        (0..4u8).prop_map(Op::LiveEnd),
    ]
}

/// Every docked panel and its area, sorted by panel.
fn areas(dock: &DockState) -> Vec<(u64, (HostId, DockRegion))> {
    let mut out: Vec<_> = dock
        .index
        .iter()
        .map(|(p, (at, _))| (p.0, (at.host, at.region)))
        .collect();
    out.sort_by_key(|(p, _)| *p);
    out
}

fn groups(dock: &DockState) -> Vec<(HostId, PaneId)> {
    dock.areas()
        .flat_map(|(host, _, root)| root.groups().into_iter().map(move |g| (host, g.id)))
        .filter(|&(_, pane)| !dock.group(pane).unwrap().panels.is_empty())
        .collect()
}

const ZONES: [DropZone; 8] = [
    DropZone::Tabs(0),
    DropZone::Tabs(1),
    DropZone::Tabs(9),
    DropZone::Center,
    DropZone::Left,
    DropZone::Right,
    DropZone::Top,
    DropZone::Bottom,
];

const POLICIES: [TabPolicy; 4] = [
    TabPolicy::OPEN,
    TabPolicy::SEALED,
    TabPolicy {
        can_leave: false,
        accepts: true,
    },
    TabPolicy {
        can_leave: true,
        accepts: false,
    },
];

proptest! {
    // Any sequence of opens, closes, panel and group moves, new windows
    // made or failed, window closes, policy changes, and restores keeps
    // every tree canonical and every panel in exactly one tab (the
    // integrity check), conserves panels through moves and restores,
    // changes nothing when refused, and never carries a panel across an
    // area boundary its policies or confinement forbid.
    #[test]
    fn random_workspace_edits_keep_panels_and_policies(ops in prop::collection::vec(op(), 1..60)) {
        let mut dock = dock();
        for op in ops {
            let prior = dock.clone();
            let before = areas(&dock);
            let dump = dock.dump();
            let panels: Vec<PanelId> = before.iter().map(|(p, _)| PanelId(*p)).collect();
            let all_groups = groups(&dock);
            let pick_panel = |i: u8| panels.get(i as usize % panels.len().max(1)).copied();
            let pick_group = |i: u8| all_groups.get(i as usize % all_groups.len().max(1)).copied();
            let pick_host = |i: u8| {
                let hosts = dock.hosts();
                hosts.get(i as usize % hosts.len().max(1)).copied()
            };
            let pick_live = |i: u8| {
                let live: Vec<HostId> = dock.live.iter().map(|l| l.host).collect();
                live.get(i as usize % live.len().max(1)).copied()
            };
            let destination = |(host, pane), zone: u8| DockDestination { host, pane, zone: ZONES[zone as usize] };
            // Some(result) for a checked move, whose crossings must obey
            // the policies as they were before it.
            let mut checked: Option<bool> = None;
            let mut conserves = true;
            match op {
                Op::Open(p, r) => {
                    dock.open(DockRegion::ALL[r as usize], PanelId(20 + u64::from(p)));
                    conserves = false;
                }
                Op::Close(g) => {
                    if let Some((_, pane)) = pick_group(g) {
                        let removed = dock.close(pane, 0);
                        prop_assert!(removed.is_some());
                        let after: Vec<_> = areas(&dock).into_iter().map(|(p, _)| p).collect();
                        let expected: Vec<_> = panels.iter().map(|p| p.0).filter(|p| Some(PanelId(*p)) != removed).collect();
                        prop_assert_eq!(after, expected);
                    }
                    conserves = false;
                }
                Op::Move(p, g, z) => {
                    if let (Some(panel), Some(to)) = (pick_panel(p), pick_group(g)) {
                        checked = Some(dock.transfer(MovePayload::Panel(panel), destination(to, z)).is_ok());
                    }
                }
                Op::MoveGroup(s, g, z) => {
                    if let (Some((_, from)), Some(to)) = (pick_group(s), pick_group(g)) {
                        checked = Some(dock.transfer(MovePayload::Group(from), destination(to, z)).is_ok());
                    }
                }
                Op::NewHost { group, pick, made } => {
                    let payload = if group {
                        pick_group(pick).map(|(_, pane)| MovePayload::Group(pane))
                    } else {
                        pick_panel(pick).map(MovePayload::Panel)
                    };
                    if let Some(payload) = payload {
                        checked = Some(match dock.reserve_host(payload) {
                            Ok(host) if made => dock.commit_host(host).is_ok(),
                            Ok(host) => {
                                prop_assert!(dock.abort_host(host));
                                false
                            }
                            Err(_) => false,
                        });
                    }
                }
                Op::CloseHost(h) => {
                    if let Some(host) = pick_host(h) {
                        checked = Some(dock.close_host(host).is_ok());
                    }
                }
                Op::RecoverHost(h) => {
                    if let Some(host) = pick_host(h) {
                        dock.recover_host(host).unwrap();
                        prop_assert!(!dock.hosts().contains(&host));
                    }
                }
                Op::Policy(a, p) => {
                    let policy = POLICIES[p as usize];
                    match a {
                        0..4 => dock.set_policy(DockRegion::ALL[a as usize], policy),
                        _ => if let Some(host) = pick_host(a) {
                            dock.set_host_policy(host, policy);
                        },
                    }
                }
                Op::Confine(p) => {
                    if let Some(panel) = pick_panel(p) {
                        dock.confine(panel, !dock.confined.contains(&panel));
                    }
                }
                Op::LiveBegin { group, pick, undo } => {
                    let payload = if group {
                        pick_group(pick).map(|(_, pane)| MovePayload::Group(pane))
                    } else {
                        pick_panel(pick).map(MovePayload::Panel)
                    };
                    if let Some(payload) = payload {
                        let snapshot = dock.snapshot();
                        let allowed = dock.can_live_detach(payload);
                        let begun = dock.begin_live_detach(payload);
                        prop_assert_eq!(begun.is_ok(), allowed);
                        match begun {
                            Ok(effects) if undo => {
                                let host = effects.open_host.unwrap();
                                dock.cancel_live_detach(host).unwrap();
                                prop_assert_eq!(dock.snapshot(), snapshot);
                                checked = Some(false);
                            }
                            begun => checked = Some(begun.is_ok()),
                        }
                    }
                }
                Op::LiveCancel(h) => {
                    if let Some(host) = pick_live(h) {
                        // Back home past any policy since; nothing lost.
                        if dock.cancel_live_detach(host).is_err() {
                            prop_assert_eq!(dock.dump(), dump.clone());
                        } else {
                            prop_assert!(!dock.hosts().contains(&host));
                        }
                    }
                }
                Op::LiveDrop(h, g, z) => {
                    if let (Some(host), Some(to)) = (pick_live(h), pick_group(g)) {
                        checked = Some(dock.drop_live_detach(host, destination(to, z)).is_ok());
                    }
                }
                Op::LiveEnd(h) => {
                    if let Some(host) = pick_live(h) {
                        dock.end_live_detach(host).unwrap();
                        prop_assert_eq!(dock.dump(), dump.clone());
                    }
                }
                Op::Restore => {
                    let json = serde_json::to_string(&dock.workspace_snapshot()).unwrap();
                    let stored: StoredDock = serde_json::from_str(&json).unwrap();
                    let mut restored = DockState::new(DockLayout::default());
                    restored.restore_workspace(&stored.into_workspace(), |_| true);
                    prop_assert_eq!(restored.dump(), dock.dump());
                    dock = restored;
                }
            }
            prop_assert_eq!(dock.verify_integrity(), Ok(()));
            let after = areas(&dock);
            if conserves {
                let ids = |v: &[(u64, (HostId, DockRegion))]| v.iter().map(|(p, _)| *p).collect::<Vec<_>>();
                prop_assert_eq!(ids(&after), ids(&before));
            }
            match checked {
                Some(false) => prop_assert_eq!(dock.dump(), dump),
                Some(true) => {
                    for ((panel, from), (_, to)) in before.iter().zip(&after) {
                        if from == to {
                            continue;
                        }
                        prop_assert!(!prior.confined.contains(&PanelId(*panel)), "{} confined", panel);
                        prop_assert!(prior.area_policy(from.0, from.1).can_leave, "{} left {:?}", panel, from);
                        // A host made by the move starts open.
                        prop_assert!(prior.area_policy(to.0, to.1).accepts, "{} entered {:?}", panel, to);
                    }
                }
                None => {}
            }
        }
    }
}
