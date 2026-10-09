# Docking across windows

Spread a `Dock` over several native windows: tabs and groups drag between
windows, tear off, and come back.

- `quark_app::dock_windows::DockWindows` (feature `components`) does the
  window side.
- The app keeps its `DockState`, passes it to each `DockWindows` call, and
  forwards its `UiApp` hooks.
- Hook table: module docs of
  [dock_windows.rs](../../crates/quark-app/src/dock_windows.rs).
- Complete example:
  [panels_demo](../../crates/quark-app/examples/panels_demo.rs).

## Wiring

- Each window shows one host: the main window the four-region layout, every
  other window one floating host.
- `DockWindows::host` says which; `Dock::host` builds it.

From [panels_demo](../../crates/quark-app/examples/panels_demo.rs), building
the dock for whichever window is painting:

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

- Send dock events to `DockWindows::apply`, not `DockState::apply_event`. It
  applies the event, opens and closes windows, focuses the moved tab where
  it landed, announces the move once, retitles floating windows by their
  tabs, and saves.
- Send input to `DockWindows::input` first. `true` means the event ended a
  drag; consume it.
- `Dock::group_grips` adds a grip at the end of each tab strip that drags
  the whole group.
- More than one dock in a window: give each a handle and tell
  `DockWindows::drop_scope` which one it drives.

## Tab strip options

| Builder | Effect |
|---|---|
| `tab_width(w)` | Width of every tab (default sizing) |
| `fit_tabs(&width_fn)` | Each tab sized to its title, up to `tab_width`; longer titles end in an ellipsis |
| `tab_close_on_hover(true)` | Inactive tabs hide their close button until hovered; it stays in the accessibility tree |
| `tab_indicator(true)` | A 2-point accent bar along the active tab's bottom edge |

- `fit_tabs` takes a function returning each panel title's width in points
  at the theme's `ui_small_font_size`; the dock measures no text.
- The Workbench measures titles once per font size and scale factor
  (`TabLabels` in [dock/mod.rs](../../examples/workbench/src/dock/mod.rs)).

From [app.rs](../../examples/workbench/src/app.rs):

```rust
let label_widths = &self.dock.tab_labels;
let label_width = |panel| label_widths.width(panel);
let mut dock_el = Dock::new(&self.dock.layout, dock_size, |e| {
    Msg::Dock(dock::Action::Dock(e)).into()
})
.tab_width(dock::TAB_MAX_WIDTH)
.fit_tabs(&label_width)
.tab_close_on_hover(true)
.tab_indicator(true);
```

## What users can do

| Action | Result |
|---|---|
| Drag a tab or group grip onto a group in any window | Moves into the strip at the slot under the pointer, or into or beside the group by body zone |
| Drag it off every app window | Where windows can follow the pointer: tears off at once into an unfocused window that follows. Elsewhere: nothing until release |
| Release a torn-off window over a group | Docks there; its window closes |
| Release it anywhere else | Stays a window where dropped |
| Escape during the drag | Payload goes back exactly where it was |
| Release outside every window where nothing could follow | A window opens there with the payload (Wayland: where the compositor places it) |
| "Move to new window", "Move group to new window" (`DockEvent::MoveToNewHost`) | A window opens with the payload, focused on its tab. Every platform |
| Close a floating window | Tabs go back where they came from, else to the first main-window region that takes them. If a tab has nowhere to go, the window stays open and `DockWindows::notice` says why |
| Close the main window | The app quits; floating windows close and are restored next launch |

- Every move goes through the dock's policies (`TabPolicy`, `confine`): a
  sealed region's tabs never tear off, and a refused drop moves nothing.
- A drag crosses windows only when a native transport starts for it; without
  window system support it stays inside its window.

## Platform differences

| Platform | Live tear-off | Release outside the app | Restored position |
|---|---|---|---|
| X11 | Yes | Stays a window | Yes |
| Windows | Yes | Stays a window | Yes |
| macOS | Yes | Stays a window | Yes |
| Wayland with `xdg_toplevel_drag_v1` | Yes, the compositor moves the window | Stays a window | No |
| Wayland without it (Hyprland) | No | Opens a window placed by the compositor | No |

- Wayland: the compositor owns the pointer during the drag;
  `DockWindows::wake` reads its drop target events.
- Wayland without it: Escape pressed away from the app's windows also opens
  a new window, since the compositor reports a drop outside and Escape both
  as a cancel.
- Wayland reports no window positions: restored floating windows get their
  saved size and maximized state, and the compositor places them.
- Per-platform mechanism: module docs of `quark_app::platform::dock_drag`.

## Saving and restoring

- `DockWindows::save_to(path)` names one checkpoint file: the
  `WorkspaceSnapshot` plus a `PlacementRecord` per host (the main window's
  under `HostId::MAIN`).
- Written atomically after every settled change, and again before windows
  close on quit, so floating windows survive a restart.
- Window moves and resizes update placements in memory; they reach the file
  with the next save.

At startup:

1. `DockWindows::restore` reads the file (or a bare saved `StoredDock`).
2. `DockWindows::main_window_options` sizes the main window.
3. `DockWindows::init` moves it to its saved place and reopens floating
   windows.

- Placements restore onto monitors connected now: a missing monitor falls
  back to the main window's, and windows are pulled inside the usable area
  ([placement](../../crates/quark-app/src/platform/placement.rs)).
- Windows outside a dock persist the same records through
  `WindowOptions::persist_key`
  ([window_state](../../crates/quark-app/src/platform/window_state.rs)).
