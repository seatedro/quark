//! Panic hook that records crashes through `tracing` and in a crash log file
//! before handing the panic to the previously installed hook.

use std::backtrace::Backtrace;
use std::fs::OpenOptions;
use std::io::Write;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::time::{SystemTime, UNIX_EPOCH};

static INSTALL: Once = Once::new();

/// Install the hook once per process. Later calls are no-ops, so running
/// several event loops does not stack hooks.
pub(crate) fn install(app_name: &str) {
    let app_name = sanitize(app_name);
    INSTALL.call_once(move || {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let report = crash_report(info);
            tracing::error!(target: "quark::panic", "{report}");
            match crash_log_path(&app_name) {
                Some(path) => {
                    if let Err(error) = append(&path, &report) {
                        eprintln!(
                            "quark: could not write crash log {}: {error}",
                            path.display()
                        );
                    }
                }
                None => eprintln!("quark: no state directory for a crash log"),
            }
            previous(info);
        }));
    });
}

fn crash_report(info: &PanicHookInfo<'_>) -> String {
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
        .unwrap_or("<non-string panic payload>");
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "<unknown>".to_owned());
    let thread = std::thread::current();
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!(
        "panic at unix time {seconds} on thread '{}': {message}\n  at {location}\nbacktrace:\n{}",
        thread.name().unwrap_or("<unnamed>"),
        Backtrace::force_capture(),
    )
}

fn append(path: &Path, report: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{report}")
}

/// `<state dir>/quark/<app>-crash.log`, where the state dir follows each
/// platform's convention for logs and runtime state.
fn crash_log_path(app_name: &str) -> Option<PathBuf> {
    Some(
        state_dir()?
            .join("quark")
            .join(format!("{app_name}-crash.log")),
    )
}

#[cfg(target_os = "macos")]
fn state_dir() -> Option<PathBuf> {
    home().map(|home| home.join("Library").join("Logs"))
}

#[cfg(target_os = "windows")]
fn state_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn state_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        // The XDG spec ignores relative paths.
        .filter(|dir| dir.is_absolute())
        .or_else(|| home().map(|home| home.join(".local").join("state")))
}

#[cfg(not(target_os = "windows"))]
fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
}

/// Keep the file name portable whatever the window title contains.
fn sanitize(name: &str) -> String {
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let name = name.trim_matches('-');
    if name.is_empty() {
        "app".to_owned()
    } else {
        name.to_owned()
    }
}
