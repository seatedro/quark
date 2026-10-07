//! Structured crash reports for panics, and an opt-in hook to send them.
//!
//! The runner's panic hook (see [`crate::WindowOptions::panic_hook`]) writes
//! one JSON [`CrashReport`] per panic into the app's crash directory, next to
//! the plain text crash log it always kept. Nothing leaves the machine
//! unless the app asks: on a later launch it can pass its own upload function
//! to [`CrashStore::upload_pending`], which deletes each report it sends.
//!
//! ```no_run
//! use quark_app::platform::crash::{self, CrashStore};
//!
//! crash::set_app_info("Notes", env!("CARGO_PKG_VERSION"));
//! // Only if the user agreed to send crash reports:
//! if let Some(store) = CrashStore::for_app("Notes") {
//!     std::thread::spawn(move || {
//!         store.upload_pending(|report| my_upload("https://crash.example.com", &report.to_json()));
//!     });
//! }
//! # fn my_upload(_: &str, _: &str) -> Result<(), String> { Ok(()) }
//! ```
//!
//! Backtraces carry function names in release builds because the workspace
//! profile strips debug info but keeps the symbol table (`strip =
//! "debuginfo"`); file and line numbers need `debug = "line-tables-only"`.
//!
//! # Native crashes
//!
//! Segfaults, aborts, and stack overflows skip the panic hook, and quark does
//! not install a native crash handler. Capturing a minidump safely needs a
//! second process that outlives the crashed one (the `crash-handler` plus
//! `minidumper` crates), and a minidump is only useful with a symbol server
//! and `minidump-stackwalk` on the receiving end. Until an app needs that
//! pipeline, rely on the operating system's own reports:
//!
//! - macOS writes `.ips` reports to `~/Library/Logs/DiagnosticReports`.
//! - Windows Error Reporting writes dumps when
//!   `HKLM\SOFTWARE\Microsoft\Windows\Windows Error Reporting\LocalDumps\<exe>`
//!   exists (an installer can create it).
//! - Linux keeps cores through `systemd-coredump` where installed
//!   (`coredumpctl list <exe>`).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// Reports kept on disk; the oldest go first.
const MAX_REPORTS: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrashReport {
    pub app: String,
    /// Empty unless the app called [`set_app_info`].
    pub version: String,
    /// `std::env::consts::OS` and `ARCH`.
    pub os: String,
    pub arch: String,
    /// The OS release when known: `macOS 15.3`, `Ubuntu 24.04 LTS (6.8.0)`,
    /// `Windows build 26100`.
    pub os_version: Option<String>,
    pub unix_time: u64,
    pub thread: String,
    pub message: String,
    /// `file:line:column` of the panic.
    pub location: String,
    pub backtrace: String,
}

impl CrashReport {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a crash report serializes")
    }

    /// The text form written to the crash log and to `tracing`.
    pub fn render(&self) -> String {
        let version = if self.version.is_empty() {
            String::new()
        } else {
            format!(" {}", self.version)
        };
        let os_version = self
            .os_version
            .as_deref()
            .map(|v| format!(", {v}"))
            .unwrap_or_default();
        format!(
            "{app}{version} panicked at unix time {time} on thread '{thread}': {message}\n  \
             at {location}\n  \
             on {os}-{arch}{os_version}\nbacktrace:\n{backtrace}",
            app = self.app,
            time = self.unix_time,
            thread = self.thread,
            message = self.message,
            location = self.location,
            os = self.os,
            arch = self.arch,
            backtrace = self.backtrace,
        )
    }
}

struct AppInfo {
    name: String,
    version: String,
}

static APP_INFO: OnceLock<AppInfo> = OnceLock::new();

/// Name and version for crash reports. Call once, early in `main`; later
/// calls are ignored. Without it reports use the first window's title and
/// an empty version.
pub fn set_app_info(name: &str, version: &str) {
    let _ = APP_INFO.set(AppInfo {
        name: name.to_owned(),
        version: version.to_owned(),
    });
}

/// The configured name, else `fallback`, and the configured version.
pub(crate) fn app_info(fallback: &str) -> (String, String) {
    match APP_INFO.get() {
        Some(info) => (info.name.clone(), info.version.clone()),
        None => (fallback.to_owned(), String::new()),
    }
}

/// The crash report directory of one app.
#[derive(Debug, Clone)]
pub struct CrashStore {
    dir: PathBuf,
}

/// What [`CrashStore::upload_pending`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UploadSummary {
    pub sent: usize,
    /// Kept on disk for the next attempt.
    pub failed: usize,
}

impl CrashStore {
    /// `<state dir>/quark/crashes/<app>`, where the state dir is the one the
    /// crash log uses.
    pub fn for_app(app_name: &str) -> Option<Self> {
        Some(Self::at(
            crate::panic_hook::state_dir()?
                .join("quark")
                .join("crashes")
                .join(crate::panic_hook::sanitize(app_name)),
        ))
    }

    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Save `report` and drop the oldest reports beyond the most recent 20.
    pub fn write(&self, report: &CrashReport) -> io::Result<PathBuf> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.dir.join(format!(
            "{:020}-{}.json",
            report.unix_time,
            std::process::id()
        ));
        std::fs::write(&path, report.to_json())?;
        let reports = self.report_paths()?;
        for old in reports
            .iter()
            .take(reports.len().saturating_sub(MAX_REPORTS))
        {
            let _ = std::fs::remove_file(old);
        }
        Ok(path)
    }

    /// Saved reports, oldest first. Files that do not parse are skipped.
    pub fn pending(&self) -> Vec<(PathBuf, CrashReport)> {
        self.report_paths()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|path| {
                let text = std::fs::read_to_string(&path).ok()?;
                Some((path, serde_json::from_str(&text).ok()?))
            })
            .collect()
    }

    /// Pass each saved report to `upload`, oldest first, and delete the
    /// ones it accepts. `upload` is the app's own transport and endpoint;
    /// quark has none. Blocking, so run it off the UI thread.
    pub fn upload_pending<E: std::fmt::Display>(
        &self,
        mut upload: impl FnMut(&CrashReport) -> Result<(), E>,
    ) -> UploadSummary {
        let mut summary = UploadSummary::default();
        for (path, report) in self.pending() {
            match upload(&report) {
                Ok(()) => {
                    let _ = std::fs::remove_file(path);
                    summary.sent += 1;
                }
                Err(error) => {
                    tracing::warn!(target: "quark::crash", "crash report upload failed: {error}");
                    summary.failed += 1;
                }
            }
        }
        summary
    }

    /// File names sort by time: the timestamp is zero padded.
    fn report_paths(&self) -> io::Result<Vec<PathBuf>> {
        let mut paths: Vec<PathBuf> = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
                .collect(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e),
        };
        paths.sort();
        Ok(paths)
    }
}

/// A description of the OS release, read from files the OS keeps. `None`
/// where unknown. Runs inside the panic hook, so it spawns no processes.
pub(crate) fn os_version() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let pretty = std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|text| os_release_name(&text));
        let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .ok()
            .map(|k| k.trim().to_owned());
        match (pretty, kernel) {
            (Some(p), Some(k)) => Some(format!("{p} ({k})")),
            (p, k) => p.or(k),
        }
    }
    #[cfg(target_os = "macos")]
    {
        let plist =
            std::fs::read_to_string("/System/Library/CoreServices/SystemVersion.plist").ok()?;
        let after = plist.split("<key>ProductVersion</key>").nth(1)?;
        let value = after.split("<string>").nth(1)?.split("</string>").next()?;
        Some(format!("macOS {}", value.trim()))
    }
    #[cfg(windows)]
    {
        crate::platform::deep_link::registry::local_machine_string(
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "CurrentBuild",
        )
        .map(|build| format!("Windows build {build}"))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        None
    }
}

/// `PRETTY_NAME` from an os-release file, unquoted.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn os_release_name(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))
        .map(|v| v.trim().trim_matches('"').to_owned())
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(unix_time: u64, message: &str) -> CrashReport {
        CrashReport {
            app: "notes".into(),
            version: "1.2.0".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            os_version: Some("Ubuntu 24.04 LTS (6.8.0)".into()),
            unix_time,
            thread: "main".into(),
            message: message.into(),
            location: "src/main.rs:10:5".into(),
            backtrace: "   0: notes::main\n   1: std::rt::lang_start".into(),
        }
    }

    #[test]
    fn renders_text_report() {
        assert_eq!(
            report(1_700_000_000, "index out of bounds").render(),
            "notes 1.2.0 panicked at unix time 1700000000 on thread 'main': index out of bounds\n  \
             at src/main.rs:10:5\n  \
             on linux-x86_64, Ubuntu 24.04 LTS (6.8.0)\n\
             backtrace:\n   0: notes::main\n   1: std::rt::lang_start"
        );
        let mut bare = report(5, "boom");
        bare.version.clear();
        bare.os_version = None;
        assert!(bare.render().starts_with("notes panicked at unix time 5"));
        assert!(bare.render().contains("\n  on linux-x86_64\n"));
    }

    #[test]
    fn upload_deletes_sent_reports_and_keeps_failures() {
        let dir = std::env::temp_dir().join(format!("quark-crash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = CrashStore::at(&dir);
        store.write(&report(2, "second")).unwrap();
        store.write(&report(1, "first")).unwrap();
        store.write(&report(3, "offline")).unwrap();

        let mut seen = Vec::new();
        let summary = store.upload_pending(|r| {
            seen.push(r.message.clone());
            if r.message == "offline" {
                Err("no network")
            } else {
                Ok(())
            }
        });

        assert_eq!(seen, ["first", "second", "offline"]);
        assert_eq!(summary, UploadSummary { sent: 2, failed: 1 });
        let left: Vec<String> = store
            .pending()
            .into_iter()
            .map(|(_, r)| r.message)
            .collect();
        assert_eq!(left, ["offline"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn keeps_only_the_newest_reports() {
        let dir = std::env::temp_dir().join(format!("quark-crash-cap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = CrashStore::at(&dir);
        for time in 1..=25 {
            store.write(&report(time, &time.to_string())).unwrap();
        }
        let times: Vec<u64> = store
            .pending()
            .into_iter()
            .map(|(_, r)| r.unix_time)
            .collect();
        assert_eq!(times, (6..=25).collect::<Vec<_>>());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reads_pretty_name_from_os_release() {
        let text = "NAME=\"Fedora Linux\"\nPRETTY_NAME=\"Fedora Linux 41 (Workstation Edition)\"\nID=fedora\n";
        assert_eq!(
            os_release_name(text).as_deref(),
            Some("Fedora Linux 41 (Workstation Edition)")
        );
        assert_eq!(os_release_name("ID=arch\nPRETTY_NAME=\n"), None);
    }
}
