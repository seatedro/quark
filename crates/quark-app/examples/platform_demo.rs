//! Exercises the platform layer: a second window, native menus, a desktop
//! notification, the dock or taskbar badge, window verbs, file drops, the
//! desktop theme, single instance handoff, and window state persistence.
//! Escape closes a window; closing both quits.
//!
//! The menu commands answer to the same shortcuts on Linux, where there is
//! no native menu bar: mod+n notifies, mod+b bumps the badge, mod+t toggles
//! always on top, mod+shift+a asks for attention after two seconds.
//!
//! Launch it twice: the second launch forwards its arguments to the first and
//! exits. `QUARK_DEMO_EXIT_AFTER_MS=3000` quits on its own, for smoke tests.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use quark::scene::{RectPrimitive, Scene, ShapedText, TextPrimitive};
use quark::{Color, Rect};
use quark_app::platform::menu::{Menu, MenuAction, MenuItem, MenuRole};
use quark_app::platform::notification::{Notification, NotificationAction};
use quark_app::platform::single_instance::{self, Instance, PrimaryInstance};
use quark_app::winit::keyboard::NamedKey;
use quark_app::winit::window::Theme;
use quark_app::{
    App, AppEvent, EventContext, FrameContext, InputEvent, WindowHandle, WindowOptions,
};
use quark_text::{TextParams, TextStyle};

struct Demo {
    primary: Option<PrimaryInstance>,
    inspector: Option<WindowHandle>,
    theme: Theme,
    log: Vec<String>,
    exit_after: Option<Duration>,
    timer_fired: Arc<AtomicBool>,
    badge: u32,
    on_top: bool,
    attention_due: Arc<AtomicBool>,
}

/// (menu item id, label, shortcut)
const COMMANDS: [(&str, &str, &str); 4] = [
    ("notify", "Notify", "mod+n"),
    ("badge", "Bump Badge", "mod+b"),
    ("on-top", "Always on Top", "mod+t"),
    ("attention", "Request Attention in 2 s", "mod+shift+a"),
];

impl Demo {
    fn menus(&self) -> Vec<Menu> {
        let mut file: Vec<MenuItem> = COMMANDS
            .iter()
            .map(|&(id, label, shortcut)| {
                let action = MenuAction::new(id, label).shortcut(shortcut);
                match id {
                    "on-top" => action.checked(self.on_top),
                    _ => action,
                }
                .into()
            })
            .collect();
        file.push(MenuItem::Separator);
        file.push(MenuRole::CloseWindow.into());
        vec![
            Menu::app("Quark platform demo"),
            Menu::new("File", file),
            Menu::edit(),
            Menu::window(),
        ]
    }

    fn command(&mut self, id: &str, cx: &mut EventContext) {
        match id {
            "notify" => cx.notify(Notification {
                title: "Quark platform demo".into(),
                body: "From the menu.".into(),
                id: 3,
                actions: Vec::new(),
            }),
            "badge" => {
                self.badge += 1;
                cx.set_badge(Some(self.badge));
            }
            "on-top" => {
                self.on_top = !self.on_top;
                cx.set_always_on_top(self.on_top);
                cx.set_menus(self.menus());
            }
            "attention" => {
                // Gives time to switch to another app first.
                let waker = cx.waker().clone();
                let due = Arc::clone(&self.attention_due);
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(2));
                    due.store(true, Ordering::Release);
                    waker.wake();
                });
            }
            _ => return,
        }
        self.log(format!("command {id}"), cx);
    }

    fn log(&mut self, line: String, cx: &mut EventContext) {
        println!("platform_demo: {line}");
        self.log.push(line);
        cx.request_redraw_all();
    }
}

impl App for Demo {
    fn init(&mut self, cx: &mut EventContext) {
        if let Some(primary) = self.primary.take() {
            cx.listen_for_instances(primary);
        }
        cx.set_menus(self.menus());
        self.inspector = Some(cx.open_window(WindowOptions {
            title: "Quark platform demo: inspector".into(),
            size: (360.0, 240.0),
            persist_key: Some("platform-demo-inspector".into()),
            ..WindowOptions::default()
        }));
        cx.notify(Notification {
            title: "Quark platform demo".into(),
            body: "Running. Drop files on a window.".into(),
            id: 1,
            actions: vec![NotificationAction {
                id: "again".into(),
                label: "Notify again".into(),
            }],
        });
        if let Some(after) = self.exit_after {
            let waker = cx.waker().clone();
            let fired = Arc::clone(&self.timer_fired);
            std::thread::spawn(move || {
                std::thread::sleep(after);
                fired.store(true, Ordering::Release);
                waker.wake();
            });
        }
        self.log("started".into(), cx);
    }

    fn frame(&mut self, cx: &mut FrameContext) -> Scene {
        let (width, height) = cx.size();
        let (background, foreground) = match self.theme {
            Theme::Dark => (
                Color::rgba(24, 24, 27, 255),
                Color::rgba(240, 240, 240, 255),
            ),
            Theme::Light => (
                Color::rgba(250, 250, 250, 255),
                Color::rgba(20, 20, 20, 255),
            ),
        };
        let title = if Some(cx.window_handle()) == self.inspector {
            "Inspector"
        } else {
            "Main window"
        };
        let mut text = format!("{title}\n");
        for line in self.log.iter().rev().take(12) {
            text.push_str(line);
            text.push('\n');
        }

        let mut scene = Scene::default();
        scene.rect(RectPrimitive {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width,
                height,
            },
            color: background,
        });
        let rect = Rect {
            x: 16.0,
            y: 16.0,
            width: width - 32.0,
            height: height - 32.0,
        };
        let params = TextParams::new(text, TextStyle::new(14.0)).wrap_width(Some(rect.width));
        if let Ok(layout) = cx.layout_text(&params) {
            scene.text(TextPrimitive {
                rect,
                layout: ShapedText::new(layout),
                color: foreground,
            });
        }
        scene
    }

    fn event(&mut self, event: InputEvent, cx: &mut EventContext) {
        match event {
            InputEvent::KeyPress(chord) if chord.named() == Some(NamedKey::Escape) => {
                if let Some(window) = cx.window_handle() {
                    cx.close_window(window);
                }
            }
            InputEvent::KeyPress(chord) => {
                let pressed = chord.binding_string().unwrap_or_default();
                let command = COMMANDS
                    .iter()
                    .find(|(_, _, shortcut)| quark_app::keymap::binding_eq(shortcut, &pressed));
                if let Some(&(id, _, _)) = command {
                    self.command(id, cx);
                }
            }
            InputEvent::FileDropped(path) => self.log(format!("dropped {}", path.display()), cx),
            InputEvent::FileHovered(path) => self.log(format!("hovering {}", path.display()), cx),
            _ => {}
        }
    }

    fn app_event(&mut self, event: AppEvent, cx: &mut EventContext) {
        match &event {
            AppEvent::ThemeChanged(theme) => self.theme = *theme,
            AppEvent::WindowClosed(window) if Some(*window) == self.inspector => {
                self.inspector = None;
            }
            AppEvent::Menu(id) => self.command(id, cx),
            AppEvent::NotificationAction { action, .. } if action == "again" => {
                cx.notify(Notification {
                    title: "Quark platform demo".into(),
                    body: "Again.".into(),
                    id: 2,
                    actions: Vec::new(),
                });
            }
            _ => {}
        }
        self.log(format!("{event:?}"), cx);
    }

    fn wake(&mut self, cx: &mut EventContext) {
        if self.attention_due.swap(false, Ordering::AcqRel) {
            cx.request_attention(false);
        }
        if self.timer_fired.load(Ordering::Acquire) {
            cx.exit();
        }
    }
}

fn main() -> Result<(), quark_app::RunError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let primary = match single_instance::acquire("dev.quark.platform-demo", &args) {
        Ok(Instance::Primary(primary)) => Some(primary),
        Ok(Instance::Secondary) => {
            println!("platform_demo: forwarded {args:?} to the running instance");
            return Ok(());
        }
        Err(error) => {
            eprintln!("platform_demo: single instance unavailable: {error}");
            None
        }
    };
    let exit_after = std::env::var("QUARK_DEMO_EXIT_AFTER_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map(Duration::from_millis);

    quark_app::run(
        Demo {
            primary,
            inspector: None,
            theme: Theme::Dark,
            log: Vec::new(),
            exit_after,
            timer_fired: Arc::default(),
            badge: 0,
            on_top: false,
            attention_due: Arc::default(),
        },
        WindowOptions {
            title: "Quark platform demo".into(),
            size: (640.0, 400.0),
            persist_key: Some("platform-demo-main".into()),
            ..WindowOptions::default()
        },
    )
}
