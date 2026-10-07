//! The background updater: one thread that checks the feed on a
//! [`Schedule`], downloads on request (or automatically), and reports
//! [`UpdateEvent`]s to a callback. It never installs and never exits the
//! process; see [`PendingRestart`] for applying a staged update.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use semver::Version;

use crate::download::{self, DownloadError, Progress};
use crate::install::{self, InstallEnv, InstallPlan, Unsupported};
use crate::manifest::{self, Expected, ManifestError, PublicKey, Release, Verdict};
use crate::schedule::Schedule;

/// Manifests are small; anything larger is not one.
const MAX_MANIFEST_BYTES: u64 = 1 << 20;

#[derive(Debug, Clone)]
pub struct UpdateConfig {
    /// Must match the manifest's `app`, such as `dev.quark.hello`.
    pub app_id: String,
    pub current_version: Version,
    /// Such as `stable` or `beta`. Must match the manifest's `channel`.
    pub channel: String,
    /// The manifest URL. `{channel}` is replaced with [`Self::channel`], so
    /// one template serves every channel.
    pub feed_url: String,
    /// At least one key; a manifest must verify against one of them.
    pub trusted_keys: Vec<PublicKey>,
    /// Where downloads and helper scripts live. Survives restarts so an
    /// interrupted download resumes.
    pub staging_dir: PathBuf,
    pub schedule: Schedule,
    /// Check once right after [`Updater::start`].
    pub check_on_start: bool,
    /// Download as soon as a check finds an installable update.
    pub auto_download: bool,
}

impl UpdateConfig {
    /// A config with the default schedule, a check on start, no automatic
    /// download, and staging under the platform cache directory.
    pub fn new(
        app_id: impl Into<String>,
        current_version: Version,
        feed_url: impl Into<String>,
        trusted_keys: Vec<PublicKey>,
    ) -> Self {
        let app_id = app_id.into();
        Self {
            staging_dir: default_staging_dir(&app_id),
            app_id,
            current_version,
            channel: "stable".into(),
            feed_url: feed_url.into(),
            trusted_keys,
            schedule: Schedule::default(),
            check_on_start: true,
            auto_download: false,
        }
    }

    fn manifest_url(&self) -> String {
        self.feed_url.replace("{channel}", &self.channel)
    }
}

/// `<cache dir>/<app id>/updates`: `$XDG_CACHE_HOME` or `~/.cache` on
/// Linux, `~/Library/Caches` on macOS, `%LOCALAPPDATA%` on Windows, and the
/// temp dir when none is set.
pub fn default_staging_dir(app_id: &str) -> PathBuf {
    let var = |name: &str| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    let cache = if cfg!(target_os = "macos") {
        var("HOME").map(|home| home.join("Library").join("Caches"))
    } else if cfg!(windows) {
        var("LOCALAPPDATA")
    } else {
        var("XDG_CACHE_HOME").or_else(|| var("HOME").map(|home| home.join(".cache")))
    };
    cache
        .unwrap_or_else(std::env::temp_dir)
        .join(app_id)
        .join("updates")
}

/// A newer release this app can (or cannot) install in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableUpdate {
    pub release: Release,
    /// `Err` when this install cannot update itself (a deb install, an
    /// unbundled binary); show a download link instead.
    pub installable: Result<(), Unsupported>,
}

/// A verified download, ready to install when the app exits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedUpdate {
    pub release: Release,
    pub path: PathBuf,
    plan: InstallPlan,
    staging_dir: PathBuf,
}

impl StagedUpdate {
    /// Start the helper that waits for this process to exit and then
    /// installs and relaunches. Call it on the main thread after the event
    /// loop has returned, so window state and app data are already saved;
    /// [`PendingRestart`] does this.
    pub fn install_after_exit(&self) -> std::io::Result<()> {
        self.plan.spawn(&self.staging_dir)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateEvent {
    Checking,
    UpToDate,
    Available(AvailableUpdate),
    Downloading(Progress),
    Ready(StagedUpdate),
    /// A check or download failed. The updater retries on its own after
    /// `retry_in`.
    Failed {
        error: String,
        retry_in: Duration,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error(transparent)]
    Download(#[from] DownloadError),
    #[error(transparent)]
    Unsupported(#[from] Unsupported),
    #[error("update manifest request failed: {0}")]
    Http(#[from] ureq::Error),
    #[error("update manifest request returned HTTP {0}")]
    Status(u16),
    #[error("update I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

enum Command {
    Check,
    Download,
    SetChannel(String),
}

/// Owns the updater thread. Dropping it stops the thread at its next wake.
pub struct Updater {
    commands: Sender<Command>,
}

impl Updater {
    /// Start the updater thread. `on_event` runs on that thread; forward
    /// events to the UI thread (for example push them to a queue and call
    /// `quark_app::Waker::wake`).
    pub fn start(config: UpdateConfig, on_event: impl Fn(UpdateEvent) + Send + 'static) -> Self {
        let (commands, receiver) = mpsc::channel();
        if config.check_on_start {
            let _ = commands.send(Command::Check);
        }
        std::thread::Builder::new()
            .name("quark-update".into())
            .spawn(move || {
                let mut worker = Worker {
                    agent: agent(),
                    config,
                    available: None,
                };
                let mut wait = worker.config.schedule.interval;
                loop {
                    let command = match receiver.recv_timeout(wait) {
                        Ok(command) => command,
                        Err(RecvTimeoutError::Timeout) => Command::Check,
                        Err(RecvTimeoutError::Disconnected) => return,
                    };
                    wait = worker.run(command, &on_event);
                }
            })
            .expect("spawn the quark-update thread");
        Self { commands }
    }

    pub fn check_now(&self) {
        let _ = self.commands.send(Command::Check);
    }

    /// Download the update from the last [`UpdateEvent::Available`].
    pub fn download(&self) {
        let _ = self.commands.send(Command::Download);
    }

    /// Switch channels and check the new feed.
    pub fn set_channel(&self, channel: impl Into<String>) {
        let _ = self.commands.send(Command::SetChannel(channel.into()));
        let _ = self.commands.send(Command::Check);
    }
}

struct Worker {
    agent: ureq::Agent,
    config: UpdateConfig,
    available: Option<Release>,
}

impl Worker {
    /// Run `command` and return how long to wait before the next check.
    fn run(&mut self, command: Command, emit: &dyn Fn(UpdateEvent)) -> Duration {
        let result = match command {
            Command::SetChannel(channel) => {
                self.config.channel = channel;
                self.available = None;
                return self.config.schedule.interval;
            }
            Command::Check => self.check(emit),
            Command::Download => self.download(emit),
        };
        match result {
            Ok(()) => self.config.schedule.succeeded(),
            Err(error) => {
                let retry_in = self.config.schedule.failed();
                tracing::warn!(target: "quark::update", "{error}");
                emit(UpdateEvent::Failed {
                    error: error.to_string(),
                    retry_in,
                });
                retry_in
            }
        }
    }

    fn check(&mut self, emit: &dyn Fn(UpdateEvent)) -> Result<(), UpdateError> {
        emit(UpdateEvent::Checking);
        let bytes = fetch(&self.agent, &self.config.manifest_url())?;
        let manifest = manifest::verify(&bytes, &self.config.trusted_keys)?;
        let platform = manifest::platform_key();
        let verdict = manifest::evaluate(
            manifest,
            Expected {
                app: &self.config.app_id,
                channel: &self.config.channel,
                current: &self.config.current_version,
                platform: &platform,
            },
        )?;
        let release = match verdict {
            Verdict::Update(release) => release,
            Verdict::UpToDate => {
                emit(UpdateEvent::UpToDate);
                return Ok(());
            }
            Verdict::Downgrade { offered } => {
                tracing::info!(target: "quark::update", "ignoring older manifest version {offered}");
                emit(UpdateEvent::UpToDate);
                return Ok(());
            }
        };
        let installable = InstallEnv::current()
            .map_err(UpdateError::Io)
            .map(|env| install::plan(release.artifact.format, Path::new(""), &env).map(drop))?;
        let auto = self.config.auto_download && installable.is_ok();
        self.available = Some(release.clone());
        emit(UpdateEvent::Available(AvailableUpdate {
            release,
            installable,
        }));
        if auto {
            self.download(emit)?;
        }
        Ok(())
    }

    fn download(&mut self, emit: &dyn Fn(UpdateEvent)) -> Result<(), UpdateError> {
        let Some(release) = self.available.clone() else {
            return self.check(emit);
        };
        let env = InstallEnv::current()?;
        // Fail before downloading anything this install cannot use.
        install::plan(release.artifact.format, Path::new(""), &env)?;
        let dir = self.config.staging_dir.join(release.version.to_string());
        let dest = dir.join(file_name(&release.artifact.url));
        let path = download::download(&self.agent, &release.artifact, &dest, &mut |p| {
            emit(UpdateEvent::Downloading(p))
        })?;
        let plan = install::plan(release.artifact.format, &path, &env)?;
        emit(UpdateEvent::Ready(StagedUpdate {
            release,
            path,
            plan,
            staging_dir: dir,
        }));
        Ok(())
    }
}

/// The last path segment of `url`, made safe to use as a file name.
fn file_name(url: &str) -> String {
    let last = url
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .rsplit('/')
        .next()
        .unwrap_or_default();
    let name: String = last
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.trim_matches('.').is_empty() {
        "update".into()
    } else {
        name
    }
}

pub(crate) fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .user_agent(concat!("quark-update/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn fetch(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>, UpdateError> {
    let response = agent.get(url).call()?;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(UpdateError::Status(status));
    }
    Ok(response
        .into_body()
        .with_config()
        .limit(MAX_MANIFEST_BYTES)
        .read_to_vec()?)
}

/// Hands a staged update from the UI to `main`, so the install starts after
/// the event loop has returned and the app has saved its state.
///
/// ```no_run
/// # fn run_app(_: quark_update::PendingRestart) {}
/// let pending = quark_update::PendingRestart::default();
/// // In the app, when the user picks "Restart to Update":
/// //     pending.request(staged); cx.exit();
/// run_app(pending.clone());
/// // `run` returned: windows are closed and their state persisted.
/// if let Err(error) = pending.apply() {
///     eprintln!("could not start the update: {error}");
/// }
/// ```
#[derive(Debug, Clone, Default)]
pub struct PendingRestart(Arc<Mutex<Option<StagedUpdate>>>);

impl PendingRestart {
    pub fn request(&self, staged: StagedUpdate) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(staged);
    }

    pub fn cancel(&self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take();
    }

    /// Start the install helper if an update was requested. Returns whether
    /// one was. The process should exit soon after: the helper waits for it.
    pub fn apply(&self) -> std::io::Result<bool> {
        let staged = self.0.lock().unwrap_or_else(|e| e.into_inner()).take();
        match staged {
            Some(staged) => staged.install_after_exit().map(|()| true),
            None => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_file_names_stay_inside_the_staging_dir() {
        for (url, expected) in [
            (
                "https://x.dev/r/hello_1.2.0_x86_64.AppImage",
                "hello_1.2.0_x86_64.AppImage",
            ),
            (
                "https://x.dev/r/Hello%20Setup.exe?sig=abc#frag",
                "Hello_20Setup.exe",
            ),
            ("https://x.dev/r/..", "update"),
            ("https://x.dev/r/", "update"),
            (
                "https://x.dev/..%2F..%2Fetc%2Fpasswd",
                ".._2F.._2Fetc_2Fpasswd",
            ),
        ] {
            assert_eq!(file_name(url), expected, "{url}");
        }
    }
}
