//! Exercises the platform layer: a second window, a desktop notification,
//! file drops, the desktop theme, single instance handoff, and window state
//! persistence. Escape closes a window; closing both quits.
//!
//! Launch it twice: the second launch forwards its arguments to the first and
//! exits. `QUARK_DEMO_EXIT_AFTER_MS=3000` quits on its own, for smoke tests.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use quark::scene::{FontKind, FontWeight, RectPrimitive, Scene, TextPrimitive};
use quark::{Color, Rect};
use quark_app::platform::notification::{Notification, NotificationAction};
use quark_app::platform::single_instance::{self, Instance, PrimaryInstance};
use quark_app::winit::keyboard::NamedKey;
use quark_app::winit::window::Theme;
use quark_app::{
    App, AppEvent, EventContext, FrameContext, InputEvent, WindowHandle, WindowOptions,
};

struct Demo {
    primary: Option<PrimaryInstance>,
    inspector: Option<WindowHandle>,
    theme: Theme,
    log: Vec<String>,
    exit_after: Option<Duration>,
    timer_fired: Arc<AtomicBool>,
}

impl Demo {
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
        let scale = cx.scale_factor();
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
        scene.text(TextPrimitive {
            rect: Rect {
                x: 16.0 * scale,
                y: 16.0 * scale,
                width: width - 32.0 * scale,
                height: height - 32.0 * scale,
            },
            text: text.into(),
            color: foreground,
            font_size: 14.0 * scale,
            font_kind: FontKind::Ui,
            font_weight: FontWeight::Normal,
        });
        scene
    }

    fn event(&mut self, event: InputEvent, cx: &mut EventContext) {
        match event {
            InputEvent::KeyPress(chord) if chord.named() == Some(NamedKey::Escape) => {
                if let Some(window) = cx.window_handle() {
                    cx.close_window(window);
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
        },
        WindowOptions {
            title: "Quark platform demo".into(),
            size: (640.0, 400.0),
            persist_key: Some("platform-demo-main".into()),
            ..WindowOptions::default()
        },
    )
}
