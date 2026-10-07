//! A child process on a pseudo terminal: a Unix PTY, or ConPTY on Windows
//! (through `portable-pty`). A reader thread hands output to a callback,
//! which an app points at its `UiSender`; input and resizes go through
//! [`Pty`] on the UI thread.

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::PathBuf;

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// What to run.
#[derive(Debug, Clone, Default)]
pub struct PtyCommand {
    /// The program; `None` runs the user's shell (`$SHELL`, or the
    /// platform's default).
    pub program: Option<OsString>,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    /// Added to the parent's environment. `TERM=xterm-256color` and
    /// `COLORTERM=truecolor` are set unless given here.
    pub env: Vec<(OsString, OsString)>,
}

impl PtyCommand {
    /// The user's login shell.
    pub fn shell() -> Self {
        Self::default()
    }

    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: Some(program.into()),
            ..Self::default()
        }
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    fn builder(&self) -> CommandBuilder {
        let mut cmd = match &self.program {
            Some(program) => CommandBuilder::new(program),
            None => CommandBuilder::new_default_prog(),
        };
        cmd.args(&self.args);
        if let Some(dir) = &self.cwd {
            cmd.cwd(dir);
        }
        for (key, value) in [("TERM", "xterm-256color"), ("COLORTERM", "truecolor")] {
            if !self.env.iter().any(|(k, _)| k == key) {
                cmd.env(key, value);
            }
        }
        for (key, value) in &self.env {
            cmd.env(key, value);
        }
        cmd
    }
}

/// The grid size a PTY reports to its program, in cells and pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtyGeometry {
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

impl PtyGeometry {
    fn size(self) -> PtySize {
        PtySize {
            rows: self.rows.max(1),
            cols: self.cols.max(1),
            pixel_width: self.pixel_width,
            pixel_height: self.pixel_height,
        }
    }
}

/// Sent from the reader thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyEvent {
    /// Bytes the program wrote, in order.
    Output(Vec<u8>),
    /// The program exited (its code when the platform reports one) and its
    /// output is drained.
    Exited(Option<u32>),
}

/// A running child on a PTY. Dropping it kills the child.
pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    geometry: PtyGeometry,
}

impl std::fmt::Debug for Pty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pty")
            .field("geometry", &self.geometry)
            .finish_non_exhaustive()
    }
}

impl Pty {
    /// Starts `command` on a new PTY of `geometry`. `on_event` runs on the
    /// reader thread for each chunk of output and once at exit.
    pub fn spawn(
        command: &PtyCommand,
        geometry: PtyGeometry,
        on_event: impl Fn(PtyEvent) + Send + 'static,
    ) -> io::Result<Self> {
        let pair = native_pty_system()
            .openpty(geometry.size())
            .map_err(io::Error::other)?;
        let mut child = pair
            .slave
            .spawn_command(command.builder())
            .map_err(io::Error::other)?;
        // The child holds its own copy; keeping ours would hold the PTY open
        // after it exits, so the reader would never see end of file.
        drop(pair.slave);
        let killer = child.clone_killer();
        let mut reader = pair.master.try_clone_reader().map_err(io::Error::other)?;
        let writer = pair.master.take_writer().map_err(io::Error::other)?;
        std::thread::Builder::new()
            .name("quark-terminal-pty".into())
            .spawn(move || {
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => on_event(PtyEvent::Output(buf[..n].to_vec())),
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                        // Linux reports EIO once the child side closes.
                        Err(_) => break,
                    }
                }
                on_event(PtyEvent::Exited(exit_code(child.as_mut())));
            })?;
        Ok(Self {
            master: pair.master,
            writer,
            killer,
            geometry,
        })
    }

    /// Writes input (keys, pastes, query replies) to the program.
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    /// Tells the program the grid changed size (SIGWINCH on Unix).
    pub fn resize(&mut self, geometry: PtyGeometry) -> io::Result<()> {
        if geometry == self.geometry {
            return Ok(());
        }
        self.geometry = geometry;
        self.master
            .resize(geometry.size())
            .map_err(io::Error::other)
    }

    pub fn geometry(&self) -> PtyGeometry {
        self.geometry
    }

    /// Kills the child; the reader then reports [`PtyEvent::Exited`].
    pub fn kill(&mut self) -> io::Result<()> {
        self.killer.kill()
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.killer.kill();
    }
}

fn exit_code(child: &mut (dyn Child + Send + Sync)) -> Option<u32> {
    child.wait().ok().map(|status| status.exit_code())
}
