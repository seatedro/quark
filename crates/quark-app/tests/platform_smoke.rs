//! The platform layer on a real window: the native menu bar and its
//! accelerators, picking a menu item, the badge, always on top, edit roles,
//! and (macOS) a `kAEGetURL` deep link sent to this process. Each step waits
//! for the event the previous one should produce; any failure, or no
//! progress within a minute, exits non-zero.
//!
//! macOS and Windows only: Linux has no native menu bar, and its CI job has
//! no display.

#[cfg(not(any(target_os = "macos", windows)))]
fn main() {
    println!("platform_smoke: macOS and Windows only; skipped");
}

#[cfg(any(target_os = "macos", windows))]
fn main() {
    smoke::run();
}

#[cfg(any(target_os = "macos", windows))]
mod smoke {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use quark::scene::Scene;
    use quark_app::platform::menu::{Menu, MenuAction, MenuRole};
    use quark_app::{App, AppEvent, EventContext, FrameContext, InputEvent, WindowOptions};

    #[cfg(target_os = "macos")]
    const URL: &str = "quark-smoke://open/42";

    #[derive(Debug, PartialEq)]
    enum Step {
        /// Picked File > Save natively; waiting for `AppEvent::Menu`.
        Menu,
        /// Performed Edit > Copy; waiting for its key press.
        CopyKey,
        /// Sent a `kAEGetURL` event; waiting for `AppEvent::OpenUrls`.
        #[cfg(target_os = "macos")]
        Url,
        Done,
    }

    struct Smoke {
        step: Option<Step>,
        failures: Vec<String>,
    }

    impl Smoke {
        fn check<T: PartialEq + std::fmt::Debug>(&mut self, what: &str, got: T, want: T) {
            if got == want {
                println!("platform_smoke: ok: {what}: {got:?}");
            } else {
                self.failures
                    .push(format!("{what}: got {got:?}, want {want:?}"));
            }
        }

        fn finish(&mut self, cx: &mut EventContext) {
            self.step = Some(Step::Done);
            cx.exit();
        }
    }

    impl App for Smoke {
        fn init(&mut self, cx: &mut EventContext) {
            cx.set_menus(vec![
                Menu::app("Smoke"),
                Menu::new(
                    "File",
                    vec![
                        MenuAction::new("save", "Save").shortcut("mod+s").into(),
                        MenuAction::new("pin", "Pinned").checked(true).into(),
                    ],
                ),
                Menu::edit(),
            ]);
            cx.set_badge(Some(5));
            cx.set_always_on_top(true);

            let window = cx.window().expect("the first window is open in init");
            let titles = native::menu_titles(window);
            self.check("menu bar menus", titles.len(), 3);
            self.check(
                "menu titles",
                titles[1..].to_vec(),
                vec!["File".to_owned(), "Edit".to_owned()],
            );
            self.check(
                "save item",
                native::save_item(window),
                native::SAVE_ITEM.to_owned(),
            );
            self.check("always on top", native::on_top(window), true);
            #[cfg(target_os = "macos")]
            self.check("dock badge", native::badge(), Some("5".to_owned()));

            let picked = native::pick_save(window);
            self.check("picked File > Save", picked, true);
            self.step = Some(Step::Menu);
        }

        fn frame(&mut self, _cx: &mut FrameContext) -> Scene {
            Scene::default()
        }

        fn event(&mut self, event: InputEvent, cx: &mut EventContext) {
            let InputEvent::KeyPress(chord) = event else {
                return;
            };
            if self.step != Some(Step::CopyKey) {
                return;
            }
            let want = if cfg!(target_os = "macos") {
                "cmd+c"
            } else {
                "ctrl+c"
            };
            self.check(
                "Edit > Copy key",
                chord.binding_string(),
                Some(want.to_owned()),
            );
            #[cfg(target_os = "macos")]
            {
                let sent = native::send_get_url(URL);
                self.check("sent kAEGetURL", sent, true);
                self.step = Some(Step::Url);
            }
            #[cfg(not(target_os = "macos"))]
            self.finish(cx);
            let _ = &cx;
        }

        fn app_event(&mut self, event: AppEvent, cx: &mut EventContext) {
            match (&self.step, event) {
                (Some(Step::Menu), AppEvent::Menu(id)) => {
                    self.check("menu event", id.as_str(), "save");
                    self.step = Some(Step::CopyKey);
                    cx.perform_role(MenuRole::Copy);
                }
                #[cfg(target_os = "macos")]
                (Some(Step::Url), AppEvent::OpenUrls(urls)) => {
                    self.check("deep link", urls, vec![URL.to_owned()]);
                    self.finish(cx);
                }
                _ => {}
            }
        }
    }

    /// Set when the app is dropped at the end of the run with every check
    /// passed.
    static PASSED: AtomicBool = AtomicBool::new(false);

    impl Drop for Smoke {
        fn drop(&mut self) {
            if self.step != Some(Step::Done) {
                self.failures.push(format!("stopped at {:?}", self.step));
            }
            for failure in &self.failures {
                eprintln!("platform_smoke: FAIL: {failure}");
            }
            PASSED.store(self.failures.is_empty(), Ordering::Release);
        }
    }

    pub(super) fn run() {
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(60));
            eprintln!("platform_smoke: no progress within 60 s");
            std::process::exit(1);
        });
        let result = quark_app::run(
            Smoke {
                step: None,
                failures: Vec::new(),
            },
            WindowOptions {
                title: "platform smoke".into(),
                size: (320.0, 200.0),
                ..WindowOptions::default()
            },
        );
        if let Err(error) = result {
            eprintln!("platform_smoke: could not run: {error}");
            std::process::exit(1);
        }
        if !PASSED.load(Ordering::Acquire) {
            std::process::exit(1);
        }
        println!("platform_smoke: passed");
    }

    #[cfg(target_os = "macos")]
    mod native {
        use objc2::rc::Retained;
        use objc2::runtime::AnyObject;
        use objc2::{MainThreadMarker, class, msg_send};
        use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSEventType, NSMenu, NSView};
        use objc2_foundation::{NSPoint, NSString};
        use quark_app::winit::window::Window;
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};

        pub const SAVE_ITEM: &str = "Save Cmd+s";

        fn app() -> Retained<NSApplication> {
            NSApplication::sharedApplication(MainThreadMarker::new().expect("main thread"))
        }

        fn main_menu() -> Retained<NSMenu> {
            app().mainMenu().expect("a main menu")
        }

        pub fn menu_titles(_: &Window) -> Vec<String> {
            let menu = main_menu();
            (0..menu.numberOfItems())
                .map(|i| {
                    menu.itemAtIndex(i)
                        .and_then(|item| item.submenu())
                        .map(|submenu| submenu.title().to_string())
                        .unwrap_or_default()
                })
                .collect()
        }

        fn file_menu() -> Retained<NSMenu> {
            main_menu()
                .itemAtIndex(1)
                .and_then(|item| item.submenu())
                .expect("a File menu")
        }

        /// The Save item's title and key equivalent.
        pub fn save_item(_: &Window) -> String {
            let item = file_menu().itemAtIndex(0).expect("a Save item");
            let command = item
                .keyEquivalentModifierMask()
                .contains(NSEventModifierFlags::Command);
            format!(
                "{} {}{}",
                item.title(),
                if command { "Cmd+" } else { "" },
                item.keyEquivalent()
            )
        }

        pub fn badge() -> Option<String> {
            app().dockTile().badgeLabel().map(|label| label.to_string())
        }

        pub fn on_top(window: &Window) -> bool {
            let RawWindowHandle::AppKit(handle) = window.window_handle().unwrap().as_raw() else {
                return false;
            };
            // SAFETY: winit's handle points at its live content view.
            let view: &NSView = unsafe { handle.ns_view.cast().as_ref() };
            view.window().is_some_and(|window| window.level() > 0)
        }

        /// Press Cmd+S through the menu bar's key equivalent handling, as a
        /// key press in the app would.
        pub fn pick_save(_: &Window) -> bool {
            let s = NSString::from_str("s");
            // SAFETY: the arguments match +[NSEvent keyEventWithType:...];
            // keyCode 1 is kVK_ANSI_S.
            let event: Option<Retained<AnyObject>> = unsafe {
                msg_send![
                    class!(NSEvent),
                    keyEventWithType: NSEventType::KeyDown,
                    location: NSPoint::new(0.0, 0.0),
                    modifierFlags: NSEventModifierFlags::Command,
                    timestamp: 0.0f64,
                    windowNumber: 0isize,
                    context: std::ptr::null::<AnyObject>(),
                    characters: &*s,
                    charactersIgnoringModifiers: &*s,
                    isARepeat: false,
                    keyCode: 1u16
                ]
            };
            let Some(event) = event else { return false };
            unsafe { msg_send![&main_menu(), performKeyEquivalent: &*event] }
        }

        /// Send this process a `kAEGetURL` Apple Event for `url`, as Launch
        /// Services does when a link of the app's scheme is opened.
        pub fn send_get_url(url: &str) -> bool {
            let gurl = u32::from_be_bytes(*b"GURL");
            // SAFETY: the arguments match the NSAppleEventDescriptor methods;
            // AEReturnID -1 is kAutoGenerateReturnID, and send option 1 is
            // NSAppleEventSendNoReply.
            unsafe {
                let target: Retained<AnyObject> =
                    msg_send![class!(NSAppleEventDescriptor), currentProcessDescriptor];
                let event: Retained<AnyObject> = msg_send![
                    class!(NSAppleEventDescriptor),
                    appleEventWithEventClass: gurl,
                    eventID: gurl,
                    targetDescriptor: &*target,
                    returnID: -1i16,
                    transactionID: 0i32
                ];
                let url: Retained<AnyObject> = msg_send![
                    class!(NSAppleEventDescriptor),
                    descriptorWithString: &*NSString::from_str(url)
                ];
                let _: () = msg_send![
                    &event,
                    setParamDescriptor: &*url,
                    forKeyword: u32::from_be_bytes(*b"----")
                ];
                let mut error: *mut AnyObject = std::ptr::null_mut();
                let _reply: Option<Retained<AnyObject>> = msg_send![
                    &event,
                    sendEventWithOptions: 1usize,
                    timeout: 5.0f64,
                    error: &mut error
                ];
                error.is_null()
            }
        }
    }

    #[cfg(windows)]
    mod native {
        use quark_app::winit::window::Window;
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{
            GWL_EXSTYLE, GetMenu, GetMenuItemCount, GetMenuItemID, GetMenuStringW, GetSubMenu,
            GetWindowLongW, HMENU, MF_BYPOSITION, PostMessageW, WM_COMMAND, WS_EX_TOPMOST,
        };

        /// Win32 menus show the accelerator after a tab.
        pub const SAVE_ITEM: &str = "Save\tCtrl+S";

        fn hwnd(window: &Window) -> HWND {
            match window.window_handle().unwrap().as_raw() {
                RawWindowHandle::Win32(handle) => HWND(handle.hwnd.get() as _),
                _ => panic!("not a Win32 window"),
            }
        }

        fn label(menu: HMENU, position: u32) -> String {
            let mut buf = [0u16; 128];
            // SAFETY: the buffer outlives the call.
            let len = unsafe { GetMenuStringW(menu, position, Some(&mut buf), MF_BYPOSITION) };
            String::from_utf16_lossy(&buf[..len.max(0) as usize])
        }

        pub fn menu_titles(window: &Window) -> Vec<String> {
            // SAFETY: plain queries on a live window's menu.
            unsafe {
                let menu = GetMenu(hwnd(window));
                (0..GetMenuItemCount(Some(menu)).max(0) as u32)
                    .map(|i| label(menu, i))
                    .collect()
            }
        }

        pub fn save_item(window: &Window) -> String {
            // SAFETY: as above.
            unsafe { label(GetSubMenu(GetMenu(hwnd(window)), 1), 0) }
        }

        pub fn on_top(window: &Window) -> bool {
            // SAFETY: as above.
            let style = unsafe { GetWindowLongW(hwnd(window), GWL_EXSTYLE) } as u32;
            style & WS_EX_TOPMOST.0 != 0
        }

        /// Send the Save item's command, as clicking it does.
        pub fn pick_save(window: &Window) -> bool {
            let hwnd = hwnd(window);
            // SAFETY: as above.
            unsafe {
                let id = GetMenuItemID(GetSubMenu(GetMenu(hwnd), 1), 0);
                id != u32::MAX
                    && PostMessageW(Some(hwnd), WM_COMMAND, WPARAM(id as usize), LPARAM(0)).is_ok()
            }
        }
    }
}
