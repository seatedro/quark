# Platform services

Operating system features beyond drawing are calls on the window context
(`cx.window`, an `EventContext`) and are answered through
`UiApp::app_event`. Each lives in `quark_app::platform`; services that pull
in extra dependencies are behind cargo features of `quark-app`.

This doctest from [crates/quark-app/src/lib.rs](../../crates/quark-app/src/lib.rs)
sets a menu bar and a badge, and handles the menu pick:

```rust
use quark::view;
use quark_app::platform::menu::{Menu, MenuAction};
use quark_app::quark_ui::element::{AnyElement, IntoAnyElement, div};
use quark_app::{AppEvent, UiApp, UiContext, ViewContext};

struct Notes {
    unread: u32,
}

impl UiApp for Notes {
    type Action = ();
    type Message = ();

    fn init(&mut self, cx: &mut UiContext) {
        // The first menu is the macOS application menu. Linux shows no
        // native menu bar; the shortcut still reaches the app as a key.
        cx.window.set_menus(vec![
            Menu::app("Notes"),
            Menu::new("File", vec![MenuAction::new("new", "New Note").shortcut("mod+n").into()]),
            Menu::edit(),
        ]);
        cx.window.set_badge(Some(self.unread));
    }

    fn app_event(&mut self, event: AppEvent, cx: &mut UiContext) {
        if let AppEvent::Menu(id) = event {
            if id == "new" {
                self.unread = 0;
                cx.window.set_badge(None);
            }
        }
    }

    fn view(&mut self, _cx: &mut ViewContext) -> AnyElement {
        view! { <div /> }
    }

    fn update(&mut self, _: (), _cx: &mut UiContext) {}
}
```

[platform_demo.rs](../../crates/quark-app/examples/platform_demo.rs)
exercises every service below on a live desktop:
`cargo run -p quark-app --example platform_demo --features notifications`.

## Services by platform

| Service | Call, and event back | Feature | macOS | Windows | Linux |
|---|---|---|---|---|---|
| Menus | `set_menus`; `AppEvent::Menu(id)` | `ui` (default) | Global menu bar; the first menu is the app menu | Menu bar in every window | No native menu bar. `menus()` returns them for an app that draws its own; `native_menu_bar()` tells the cases apart |
| Badge | `set_badge(Some(n))`; `None` or `Some(0)` clears | | Dock tile label | Overlay icon on each taskbar button, `99+` past 99; not shown with small taskbar buttons | Unity `LauncherEntry` D-Bus signal (Ubuntu dock, KDE Plasma, Plank, Dash to Dock); `set_desktop_id` when the `.desktop` file is not named after the executable |
| Notifications | `notify`; `NotificationAction`, `NotificationDismissed` | `notifications` | `UNUserNotificationCenter`, only in an app bundle; asks permission on first use. Unbundled binaries show notifications but report no clicks | WinRT toasts, clicks reported while the app runs | freedesktop notification server |
| Tray icon | `set_tray`, `remove_tray`; `TrayClicked`, `TrayMenu(id)` | `tray` | Native | Native | StatusNotifierItem over D-Bus (KDE; GNOME with the AppIndicator extension). Desktops without a watcher show nothing |
| File dialogs | `file_dialog`; `FileDialogClosed` | `dialogs` | Native, without blocking the event loop | Native | XDG desktop portal; needs a running portal such as xdg-desktop-portal-gtk, -gnome, or -kde |
| Deep links | `AppEvent::OpenUrls` | | `kAEGetURL` Apple Event; declare the scheme in `Info.plist` | New process with the URL as argument: `register_url_scheme` plus single instance | `.desktop` file (`platform::desktop_entry`) plus single instance |
| Single instance | `single_instance::acquire`, then `listen_for_instances`; `OpenUrls` | | Unix socket and lock file; covers command line launches (URL clicks arrive as Apple Events) | Named pipe | Socket in `$XDG_RUNTIME_DIR`, else a `0700` directory in the temp dir, else `/tmp/quark-$USER` when the others make the socket path longer than about 104 bytes |
| Window state | `WindowOptions::persist_key` | | Size and position | Size and position | Size and position on X11; size only on Wayland, which reports no positions |
| Desktop theme | `AppEvent::ThemeChanged` | | System appearance | System setting | XDG settings portal `color-scheme`; bare window managers report nothing |
| Clipboard image | `clipboard_image`, `set_clipboard_image` | `clipboard-image` | yes | yes | yes, X11 and Wayland data control |

Window state is saved as JSON per key under `<state dir>/quark/`, in
logical points, and restored onto a connected monitor. The single instance
module docs have a startup snippet:
[platform/single_instance.rs](../../crates/quark-app/src/platform/single_instance.rs).

## Windows and input

- **Windows.** `open_window` and `close_window` manage extra windows;
  `AppEvent::WindowClosed` reports closes. `set_exit_when_last_window_closes(false)`
  keeps a tray app running.
- **Window verbs.** `minimize`, `toggle_maximized`, `toggle_fullscreen`,
  `set_always_on_top`, `focus_window`, `request_attention`, `set_title`.
- **Chrome.** `WindowChrome::Custom` lets the app draw its own title bar:
  on macOS the title bar turns transparent and the traffic lights stay
  (`WindowOptions::traffic_lights` moves them); elsewhere decorations are
  removed.
- **File drops.** `InputEvent::FileHovered`, `FileHoverCancelled`, and
  `FileDropped(path)` reach `UiApp::event`.
- **Crashes.** `WindowOptions::panic_hook` (on by default) logs a panic's
  message, location, and backtrace through `tracing` and to a crash log in
  the platform state directory, then runs the previous hook.

## Checks

`crates/quark-app/tests/platform_smoke.rs` opens a real window on macOS
and Windows CI runners and walks the native menu bar and accelerators, a
menu pick, the badge, always on top, edit roles, and (macOS) a `kAEGetURL`
deep link. Linux has no native menu bar and its CI test job no display, so
it skips there; the `forward_url` end-to-end spec covers single instance
handoff on Linux.
