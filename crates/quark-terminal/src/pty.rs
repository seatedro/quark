//! A child process on a pseudo terminal: a Unix PTY, or ConPTY on Windows
//! (through `portable-pty`). A reader thread reads output into a few pooled
//! buffers and calls back when there is some; the UI thread takes it with
//! [`Pty::read`] and hands the buffers back. Input and resizes go through
//! [`Pty`] on the UI thread.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

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

/// What [`Pty::read`] hands over, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtyEvent<'a> {
    /// Bytes the program wrote, in order.
    Output(&'a [u8]),
    /// The program exited (its code when the platform reports one) and its
    /// output is drained.
    Exited(Option<u32>),
}

/// Read buffers per PTY. The reader waits for one to come back when all
/// are full, so a program writing faster than the UI feeds the terminal is
/// held back instead of queuing without bound. 256 KiB per terminal.
const BUFFERS: usize = 4;
const BUFFER_LEN: usize = 64 * 1024;

/// Output passed from the reader thread to the UI thread in a fixed set of
/// buffers, so reading allocates nothing after spawn.
pub(crate) struct Inbox {
    state: Mutex<InboxState>,
    /// Signalled when a buffer comes back or the PTY closes.
    returned: Condvar,
    /// Wakes the UI thread. In a mutex so it need not be `Sync`.
    on_ready: Mutex<Box<dyn Fn() + Send>>,
}

struct InboxState {
    /// Filled buffers and their lengths, oldest first.
    ready: VecDeque<(Box<[u8]>, usize)>,
    free: Vec<Box<[u8]>>,
    /// The exit, held until the output before it is read.
    exit: Option<Option<u32>>,
    /// `on_ready` ran and the UI thread has not found the inbox empty since.
    notified: bool,
    /// The [`Pty`] is gone: the reader stops.
    closed: bool,
}

impl Inbox {
    fn new(on_ready: impl Fn() + Send + 'static) -> Self {
        let inbox = Self {
            state: Mutex::new(InboxState {
                ready: VecDeque::with_capacity(BUFFERS),
                free: (0..BUFFERS)
                    .map(|_| vec![0; BUFFER_LEN].into_boxed_slice())
                    .collect(),
                exit: None,
                notified: false,
                closed: false,
            }),
            returned: Condvar::new(),
            on_ready: Mutex::new(Box::new(on_ready)),
        };
        // macOS builds std's mutexes and condvars on pthreads, which are
        // boxed on first use; touch them here so that happens at spawn and
        // not on the first read.
        drop(inbox.lock());
        drop(inbox.on_ready.lock());
        inbox.returned.notify_one();
        inbox
    }

    fn lock(&self) -> MutexGuard<'_, InboxState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn wake(&self) {
        (self.on_ready.lock().unwrap_or_else(|e| e.into_inner()))();
    }

    /// Reads `reader` to its end into pooled buffers, waiting for a free
    /// one when all are queued. Returns early once the PTY closes.
    fn pump(&self, reader: &mut dyn Read) {
        loop {
            let mut buf = {
                let mut state = self.lock();
                loop {
                    if state.closed {
                        return;
                    }
                    if let Some(buf) = state.free.pop() {
                        break buf;
                    }
                    state = self.returned.wait(state).unwrap_or_else(|e| e.into_inner());
                }
            };
            let n = loop {
                match reader.read(&mut buf) {
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    // Linux reports EIO once the child side closes.
                    result => break result.unwrap_or(0),
                }
            };
            let mut state = self.lock();
            if n == 0 {
                state.free.push(buf);
                return;
            }
            state.ready.push_back((buf, n));
            let wake = !std::mem::replace(&mut state.notified, true);
            drop(state);
            if wake {
                self.wake();
            }
        }
    }

    /// Queues the exit behind the output already read.
    fn finish(&self, code: Option<u32>) {
        let mut state = self.lock();
        if state.closed {
            return;
        }
        state.exit = Some(code);
        let wake = !std::mem::replace(&mut state.notified, true);
        drop(state);
        if wake {
            self.wake();
        }
    }

    /// Hands queued output to `f` in order, then the exit once no output is
    /// left. Takes at most [`BUFFERS`] chunks per call and wakes the UI
    /// again when more remain, so a flood of output still lets frames draw.
    pub(crate) fn drain(&self, mut f: impl FnMut(PtyEvent<'_>)) {
        for _ in 0..BUFFERS {
            let mut state = self.lock();
            let Some((buf, len)) = state.ready.pop_front() else {
                // Cleared under the lock with the queue empty, so the
                // reader's next chunk wakes the UI again.
                state.notified = false;
                let exit = state.exit.take();
                drop(state);
                if let Some(code) = exit {
                    f(PtyEvent::Exited(code));
                }
                return;
            };
            drop(state);
            f(PtyEvent::Output(&buf[..len]));
            self.lock().free.push(buf);
            self.returned.notify_one();
        }
        self.wake();
    }

    fn close(&self) {
        self.lock().closed = true;
        self.returned.notify_all();
    }
}

/// A running child on a PTY. Dropping it kills the child.
pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    geometry: PtyGeometry,
    inbox: Arc<Inbox>,
}

impl std::fmt::Debug for Pty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pty")
            .field("geometry", &self.geometry)
            .finish_non_exhaustive()
    }
}

impl Pty {
    /// Starts `command` on a new PTY of `geometry`. `on_ready` runs, on
    /// the reader thread or in [`Self::read`], when output or the exit
    /// waits to be read; once, until [`Self::read`] empties the queue.
    /// Point it at the app's waker and call [`Self::read`] when it fires.
    pub fn spawn(
        command: &PtyCommand,
        geometry: PtyGeometry,
        on_ready: impl Fn() + Send + 'static,
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
        let inbox = Arc::new(Inbox::new(on_ready));
        let shared = Arc::clone(&inbox);
        std::thread::Builder::new()
            .name("quark-terminal-pty".into())
            .spawn(move || {
                shared.pump(&mut reader);
                shared.finish(exit_code(child.as_mut()));
            })?;
        Ok(Self {
            master: pair.master,
            writer,
            killer,
            geometry,
            inbox,
        })
    }

    /// Hands the output read so far to `f` in order, then the exit once all
    /// output is read. Allocates nothing.
    pub fn read(&self, f: impl FnMut(PtyEvent<'_>)) {
        self.inbox.drain(f);
    }

    #[cfg_attr(not(ghostty_vt), allow(dead_code))]
    pub(crate) fn inbox(&self) -> Arc<Inbox> {
        Arc::clone(&self.inbox)
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
        self.inbox.close();
        let _ = self.killer.kill();
    }
}

fn exit_code(child: &mut (dyn Child + Send + Sync)) -> Option<u32> {
    child.wait().ok().map(|status| status.exit_code())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use quark_ui::test_alloc;

    use super::*;

    /// Reads `len` bytes of a known pattern in chunks of varying size,
    /// allocating nothing.
    struct Pattern {
        at: usize,
        len: usize,
    }

    fn byte(i: usize) -> u8 {
        (i * 7 % 251) as u8
    }

    impl Read for Pattern {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = (self.len - self.at)
                .min(buf.len())
                .min(1 + self.at % 70_000);
            for (i, b) in buf[..n].iter_mut().enumerate() {
                *b = byte(self.at + i);
            }
            self.at += n;
            Ok(n)
        }
    }

    /// Feeds `len` pattern bytes and then exit code 3 through an inbox from
    /// a reader thread, while this thread drains it into `on_event` on
    /// each wake. Returns the reader's allocations and this thread's
    /// allocations while draining.
    fn deliver(len: usize, mut on_event: impl FnMut(PtyEvent<'_>)) -> (u64, u64) {
        let ui = thread::current();
        let inbox = Arc::new(Inbox::new(move || ui.unpark()));
        let shared = Arc::clone(&inbox);
        let reader = thread::spawn(move || {
            let mut pattern = Pattern { at: 0, len };
            test_alloc::count(|| {
                shared.pump(&mut pattern);
                shared.finish(Some(3));
            })
            .1
        });
        let mut done = false;
        let mut drained = 0;
        while !done {
            thread::park();
            drained += test_alloc::count(|| {
                inbox.drain(|event| {
                    done |= matches!(event, PtyEvent::Exited(_));
                    on_event(event);
                });
            })
            .1;
        }
        (reader.join().unwrap(), drained)
    }

    /// More output than the pool holds arrives whole and in order, and the
    /// exit after all of it.
    #[test]
    fn output_arrives_in_order_and_the_exit_after_it() {
        let len = 20 * BUFFER_LEN + 123;
        let mut out = Vec::new();
        let mut exit = None;
        deliver(len, |event| match event {
            PtyEvent::Output(bytes) => {
                assert!(exit.is_none(), "output after the exit");
                out.extend_from_slice(bytes);
            }
            PtyEvent::Exited(code) => exit = Some(code),
        });
        assert_eq!(out.len(), len);
        assert!(out.iter().enumerate().all(|(i, &b)| b == byte(i)));
        assert_eq!(exit, Some(Some(3)));
    }

    /// Passing output from the reader to the UI thread allocates nothing on
    /// either side once the pool exists.
    #[test]
    fn delivering_output_allocates_nothing() {
        let len = 20 * BUFFER_LEN;
        let mut seen = 0;
        let (reader, drained) = deliver(len, |event| {
            if let PtyEvent::Output(bytes) = event {
                seen += bytes.len();
            }
        });
        assert_eq!(seen, len);
        assert_eq!((reader, drained), (0, 0));
    }

    /// A reader waiting for a buffer the UI never returns stops when the
    /// PTY closes.
    #[test]
    fn closing_stops_a_reader_waiting_for_a_buffer() {
        let inbox = Arc::new(Inbox::new(|| {}));
        let shared = Arc::clone(&inbox);
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            shared.pump(&mut Pattern {
                at: 0,
                len: usize::MAX,
            });
            let _ = tx.send(());
        });
        // Wait until every buffer is queued and the reader is stuck.
        while inbox.lock().ready.len() < BUFFERS {
            thread::yield_now();
        }
        inbox.close();
        rx.recv_timeout(Duration::from_secs(20))
            .expect("the reader still waits after close");
        assert_eq!(inbox.lock().ready.len(), BUFFERS);
    }
}
