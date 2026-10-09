# Platform services

Menus, notifications, badges, tray, dialogs, deep links, single instance,
window state, and modal webviews.

- Calls go on the window context (`cx.window`, an `EventContext`); answers
  come back through `UiApp::app_event`.
- Everything lives in `quark_app::platform`.
- Services with extra dependencies sit behind `quark-app` cargo features.

From [crates/quark-app/src/lib.rs](../../crates/quark-app/src/lib.rs), a
menu bar and badge with the menu pick handled:

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

- Every service below on a live desktop:
  [platform_demo.rs](../../crates/quark-app/examples/platform_demo.rs), run
  with
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
| Webviews | `webviews().open`; `AppEvent::WebView` | `webview` | WKWebView | WebView2; Evergreen runtime required | WebKitGTK 2.40+ (API 4.1), GTK 3 window on X11 and Wayland |

- Window state: JSON per key under `<state dir>/quark/`, in logical points,
  restored onto a connected monitor.
- Single instance startup snippet: module docs of
  [platform/single_instance.rs](../../crates/quark-app/src/platform/single_instance.rs).

## Windows and input

| Area | API |
|---|---|
| Windows | `open_window`, `close_window`; `AppEvent::WindowClosed` reports closes. `set_exit_when_last_window_closes(false)` keeps a tray app running |
| Window verbs | `minimize`, `toggle_maximized`, `toggle_fullscreen`, `set_always_on_top`, `focus_window`, `request_attention`, `set_title` |
| Chrome | `WindowChrome::Custom`: the app draws its own title bar. macOS: transparent title bar, traffic lights stay (`WindowOptions::traffic_lights` moves them). Elsewhere: decorations removed |
| File drops | `InputEvent::FileHovered`, `FileHoverCancelled`, `FileDropped(path)` reach `UiApp::event` |
| Crashes | `WindowOptions::panic_hook` (on by default) logs message, location, and backtrace through `tracing` and to a crash log in the platform state directory, then runs the previous hook |

## Webviews

A modal browser window over a parent, for signing in to a website.
`quark_app::platform::webview`, feature `webview`.

| Call | Does |
|---|---|
| `cx.window.webviews().open(url, options)` | Modal over the context's window; `open_with_parent` for another |
| `evaluate_script(view, script, guard)` | Future of the result; dropping it cancels |
| `evaluate_script_event(view, script, guard)` | Result as `WebViewEvent::EvaluationFinished` |
| `cancel_evaluation(id)`, `close(view)` | Idempotent |
| `clear_profile(&profile)` | Future; fails while a view uses the profile |

- Events: `Opened`, `NavigationStarted`, `NavigationRedirected`,
  `NavigationBlocked`, `NavigationCommitted`, `NavigationFailed`,
  `PageLoadFinished`, `LocationChanged`, `TitleChanged`,
  `EvaluationFinished`, and one `Closed` per handle. The context is the
  parent window.
- `WebViewOptions::new(origins)`: required, nonempty list of exact `https`
  origins. Every top-level and frame navigation, redirect, and popup is
  checked against it.
- `evaluation_origins`: where scripts may run; empty by default.
- `OriginGuard::new(origin, document)`: the committed document from
  `NavigationCommitted`. Any later navigation fails the script with
  `NavigationChanged`, even back to the same URL.
- `AsyncScript`: a constant async function body, run in the page's world;
  arguments go in `args`, never in the source. Results must be JSON.
- Defaults: title "Sign in", 520 × 720 points (minimum 360 × 480), fresh
  ephemeral data store, popups denied, devtools off, 10 s timeout, 1 MiB
  result, depth 64, 8 evaluations in flight.
- `DataStore::Persistent(ProfileId::new(app, purpose))` keeps cookies and
  storage; one view at a time. macOS 14+.
- Parent lock: no pointer, key, IME, drop, menu, or accessibility input
  reaches the parent while its modal is open. One modal per parent.
- `Capabilities::parent`: `Native` where the window system knows the
  parent, else `AppEnforced` (Wayland without xdg-foreign).
  `require_native_parent(true)` fails instead.
- No webview engine built: `OpenError::Unsupported`.

| Platform | Build | Run |
|---|---|---|
| Linux | `pkg-config libgtk-3-dev libwebkit2gtk-4.1-dev` (Fedora: `gtk3-devel webkit2gtk4.1-devel`); `nix develop .#webview` | WebKitGTK 4.1 2.40+, libsoup 3, glib-networking, CA certificates, a session bus |
| macOS | SDK only | System WebKit; sandboxed apps need the outgoing network entitlement |
| Windows | MSVC toolchain | Evergreen WebView2 Runtime; the installer must provide it |

