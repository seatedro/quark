//! Live theme files for the development loop.
//!
//! With `QUARK_WORKBENCH_THEME_DIR` set (`scripts/dev.sh` points it at
//! `assets/themes`), [`watch`] polls that directory on a thread and keeps
//! the newest valid `Workbench` family for [`super::themes_for`]. A file
//! that fails to parse is reported and changes nothing: the window keeps
//! the colors it had. The app re-applies [`super::themes_for`] when the
//! watcher calls back.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use quark_app::quark_ui::theme::{ThemeFamily, ThemeRegistry, ThemeWatcher};

use super::theme::FAMILY;

pub const THEME_DIR_ENV: &str = "QUARK_WORKBENCH_THEME_DIR";
/// How often the watcher looks at the theme files.
pub const POLL_MS: u64 = 100;

/// The newest valid family from the watched directory.
static LIVE: Mutex<Option<ThemeFamily>> = Mutex::new(None);
/// Why the last changed file was rejected, until taken.
static REJECTED: Mutex<Option<String>> = Mutex::new(None);

/// The watched family, once a valid one has loaded.
pub fn live_family() -> Option<ThemeFamily> {
    LIVE.lock().ok()?.clone()
}

/// Why the newest edit was rejected, once.
pub fn take_rejection() -> Option<String> {
    REJECTED.lock().ok()?.take()
}

/// What one poll found.
#[derive(Debug, Clone, PartialEq)]
pub struct Reload {
    /// The workbench family now in force: the new one, or the last valid
    /// one when the edit was rejected. `None` until a file defines it.
    pub family: Option<ThemeFamily>,
    /// Each rejected file with its issues, one line per issue.
    pub errors: Vec<String>,
}

/// Reloads the workbench family from a directory of theme files.
#[derive(Debug, Clone)]
pub struct ThemeReloader {
    watcher: ThemeWatcher,
    registry: ThemeRegistry,
}

impl ThemeReloader {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            watcher: ThemeWatcher::new(dir),
            registry: ThemeRegistry::default(),
        }
    }

    /// `Some` when a file changed since the last poll.
    pub fn poll(&mut self) -> Option<Reload> {
        let reload = self.watcher.poll(&mut self.registry)?;
        Some(Reload {
            family: self.registry.get(FAMILY).cloned(),
            errors: reload.errors.iter().map(ToString::to_string).collect(),
        })
    }
}

/// Watch the directory named by [`THEME_DIR_ENV`], if set, and call
/// `changed` on the watcher's thread after each change, valid or not.
/// Returns whether watching started.
pub fn watch(changed: impl Fn() + Send + 'static) -> bool {
    let Some(dir) = std::env::var_os(THEME_DIR_ENV) else {
        return false;
    };
    let mut reloader = ThemeReloader::new(PathBuf::from(dir));
    std::thread::Builder::new()
        .name("workbench-themes".into())
        .spawn(move || {
            loop {
                if let Some(reload) = reloader.poll() {
                    if let Ok(mut live) = LIVE.lock() {
                        live.clone_from(&reload.family);
                    }
                    if !reload.errors.is_empty() {
                        let message = reload.errors.join("\n");
                        eprintln!(
                            "workbench: theme file rejected, keeping the last theme:\n{message}"
                        );
                        if let Ok(mut rejected) = REJECTED.lock() {
                            *rejected = Some(message);
                        }
                    }
                    changed();
                }
                std::thread::sleep(Duration::from_millis(POLL_MS));
            }
        })
        .is_ok()
}
