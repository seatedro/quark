# Docking across windows

A `Dock` can spread over several native windows: tabs and whole groups
drag between windows, tear off into windows of their own, and come back.
`quark_app::dock_windows::DockWindows` (feature `components`) does the
window side. The app keeps its `DockState`, passes it to each
`DockWindows` call, and forwards its `UiApp` hooks; the table of hooks is
in the module docs of
[crates/quark-app/src/dock_windows.rs](../../crates/quark-app/src/dock_windows.rs).
[panels_demo](../../crates/quark-app/examples/panels_demo.rs) is the
complete example.

## Wiring

Each window shows one host of the workspace: the main window the four
region layout, every other window one floating host. `DockWindows::host`
says which, and `Dock::host` builds it. This excerpt of
[panels_demo](../../crates/quark-app/examples/panels_demo.rs) builds the
dock for whichever window is painting:

```rust
let Some(host) = self.windows.host(window) else {
    return div().w(size.0).h(size.1).into_any();
};
let mut dock = Dock::new(&self.dock, size, |e| Msg::Dock(e).into()).group_grips(true);
if host == HostId::MAIN {
    self.size = size;
    dock = dock
        .toggle_key(DockRegion::Left, "mod+b")
        .toggle_key(DockRegion::Right, "mod+alt+b")
        .toggle_key(DockRegion::Bottom, "mod+j")
        .always_show_tabs(DockRegion::Left)
        .always_show_tabs(DockRegion::Center)
        .always_show_tabs(DockRegion::Right)
        .always_show_tabs(DockRegion::Bottom);
} else {
    dock = dock.host(host);
}
```

Dock events go to `DockWindows::apply` instead of `DockState::apply_event`.
It applies the event and carries out what the move asks for: it opens and
closes windows, focuses the moved tab in the window it landed in,
announces the move once, retitles floating windows by their tabs, and
saves. Input goes to `DockWindows::input` first; when it returns true the
event ended a drag, and the app consumes it.

By default every tab in a strip is `tab_width` wide. `fit_tabs` sizes
each tab to its title instead, up to `tab_width`, ending longer titles in
an ellipsis. The dock does not measure text: the app passes a function
returning each panel title's width in points at the theme's
`ui_small_font_size`. The Workbench measures its titles with the
window's text system once per font size and scale factor (`TabLabels` in
[dock/mod.rs](../../examples/workbench/src/dock/mod.rs)) and builds its
dock this way, from [app.rs](../../examples/workbench/src/app.rs):

```rust
let label_widths = &self.dock.tab_labels;
let label_width = |panel| label_widths.width(panel);
// Tabs sized to their titles (up to 160 points), a close button
// only on the hovered or active tab, and an accent bar under the
// active one.
let mut dock_el = Dock::new(&self.dock.layout, dock_size, |e| {
    Msg::Dock(dock::Action::Dock(e)).into()
})
.tab_width(dock::TAB_MAX_WIDTH)
.fit_tabs(&label_width)
.tab_close_on_hover(true)
.tab_indicator(true);
```

`tab_close_on_hover` keeps an inactive tab's close button in the
accessibility tree while hiding it until the pointer is over the tab.
`tab_indicator` draws a 2-point accent bar along the active tab's bottom
edge.

`Dock::group_grips` adds a grip at the end of every tab strip that drags
the whole group. Apps with more than one dock in a window give each a
handle and tell `DockWindows::drop_scope` which one it drives.

## What users can do

| Action | Result |
|---|---|
| Drag a tab or group grip onto a group in any window | It moves there, into the strip at the slot under the pointer, or into or beside the group by the body zone |
| Drag it off every window of the app | Where windows can follow the pointer, it tears off at once into an unfocused window that follows the pointer. Elsewhere nothing changes until the release |
| Release a torn-off window over a group | It docks there and its window closes |
| Release it anywhere else | It stays a window where it was dropped |
| Press Escape during the drag | The payload goes back exactly where it was |
| Release outside every window where nothing could follow | A window opens there with the payload (on Wayland, where the compositor places it) |
| "Move to new window", "Move group to new window" (`DockEvent::MoveToNewHost`) | A window opens with the payload, focused on its tab. Works on every platform |
| Close a floating window | Its tabs go back where they left from, else to the first main window region that takes them. When some tab has nowhere to go, the window stays open and `DockWindows::notice` says why |
| Close the main window | The app quits; floating windows close with it and are restored next time |

Every move goes through the dock's policies (`TabPolicy`, `confine`), so a
sealed region's tabs never tear off and a refused drop moves nothing. A
drag only becomes a cross-window drag when a native transport starts for
it; without one (no window system support) it stays inside its window as
before.

## Platform differences

| | Drags between windows | Live tear-off | Release outside the app | Restored position |
|---|---|---|---|---|
| X11 | Source window's grab, stacking order from the X server | Yes | Stays a window | Yes |
| Windows | `SetCapture`, `WindowFromPoint` | Yes | Stays a window | Yes |
| macOS | `mouseDragged` outside the view, `windowNumberAtPoint` | Yes | Stays a window | Yes |
| Wayland with `xdg_toplevel_drag_v1` | Compositor drag and drop | Yes, the compositor moves the window | Stays a window | No |
| Wayland without it (Hyprland) | Compositor drag and drop | No | Opens a window, placed by the compositor | No |

On Wayland the compositor owns the pointer during the drag, so its drop
target events name the window under it; `DockWindows::wake` reads them.
Without `xdg_toplevel_drag_v1` no window can follow the pointer, so
nothing tears off during the drag. A drag that ends away from the app's
windows, over nothing or over another app, opens the payload in a new
window that the compositor places; Escape pressed away from the app's
windows does the same, since the compositor reports both as a cancel.

Wayland reports no window positions. Restored floating windows get their
saved size and maximized state, and the compositor places them.

The transport behind these is `quark_app::platform::dock_drag`; its module
docs describe each platform's mechanism. Headless tests and the X11
end-to-end specs exercise X11-style desktop behavior. The Windows, macOS,
and Wayland paths have no automated coverage.

## Saving and restoring

`DockWindows::save_to` names one checkpoint file holding the
`WorkspaceSnapshot` and a `PlacementRecord` per host, the main window's
under `HostId::MAIN`. It is written atomically after every settled change,
and again before windows close on quit, so floating windows survive a
restart. Window moves and resizes update the placements in memory and reach
the file with the next save.

At startup, `DockWindows::restore` reads the file (or a bare saved
`StoredDock`, the format before checkpoints), `DockWindows::main_window_options` sizes the main
window, and `DockWindows::init` moves it to its saved place and reopens the
floating windows. Placements restore onto the monitors connected now: a
missing monitor falls back to the main window's, and windows are pulled
inside the usable area ([placement](../../crates/quark-app/src/platform/placement.rs)).

Windows outside a dock persist the same records through
`WindowOptions::persist_key`
([window_state](../../crates/quark-app/src/platform/window_state.rs)).

## Testing

`UiTestHarness` drives several windows. `desktop_move`, `desktop_press`, and
`desktop_release` move the pointer across the virtual desktop with the
pressed window's grab, as X11, Windows, and macOS deliver a drag.
`DockWindows::window_stack` locates drags with a `ScriptedStack`
(`quark_app::platform::dock_drag`, feature `test-support`) instead of the
window system's; `UiTestHarness::set_capabilities` simulates a desktop
without window positions, and `UiTestHarness::with_monitor` starts on a
display so placements restore.
[crates/quark-app/src/dock_windows/tests.rs](../../crates/quark-app/src/dock_windows/tests.rs)
has the tear-off, drop, cancel, close, and restore tests.

The end-to-end specs `panels_demo/new_window` and `panels_demo/tear_off` in
[e2e/specs](../../e2e/specs/panels_demo) run the demo under Xvfb and
openbox: the first moves a tab to a new window from the keyboard and closes
it again, the second tears a tab off with real pointer events from
`xdotool` and docks it back. `quark_e2e.app_tree(window=...)` finds a window
by its name, and `app_frames()` lists them all.
