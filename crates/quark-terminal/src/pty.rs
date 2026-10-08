//! A child process on a pseudo terminal: a Unix PTY, or ConPTY on Windows
//! (through `portable-pty`). A reader thread reads output into a few pooled
//! buffers and calls back when there is some; the UI thread takes it with
//! [`Pty::read`] and hands the buffers back. Input goes into a bounded
//! queue that a writer thread feeds to the program, so the UI never waits
//! on a program that is busy writing instead of reading. Resizes go
//! through [`Pty`] on the UI thread.
//!
//! Closing a [`Pty`] returns at once and its threads end soon after, even
//! when the program neither reads nor writes: on Unix they poll the master
//! together with a pipe the [`Pty`] closes, and a program that ignores
//! SIGHUP is killed shortly after; on Windows the program is
//! terminated, which closes the pseudoconsole and with it both pipes.

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

/// Input bytes queued for the writer thread; [`Pty::write`] takes no more
/// until it catches up.
pub const INPUT_QUEUE: usize = 1024 * 1024;
/// Bytes the writer thread takes from the queue per write.
const WRITE_CHUNK: usize = 64 * 1024;

/// How long a program has to exit after its [`Pty`] is dropped before it
/// is killed (Unix; Windows terminates it outright). About what
/// `portable-pty` allows between SIGHUP and SIGKILL in its own `kill`.
#[cfg(unix)]
const HANGUP_GRACE: std::time::Duration = std::time::Duration::from_millis(150);

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
            // Returns the buffer to the pool even if `f` panics, so the
            // reader is never left waiting for it.
            let lease = Lease {
                inbox: self,
                buf: Some(buf),
            };
            f(PtyEvent::Output(
                &lease.buf.as_ref().expect("leased")[..len],
            ));
        }
        self.wake();
    }

    fn close(&self) {
        self.lock().closed = true;
        self.returned.notify_all();
    }
}

/// An output buffer lent to [`Inbox::drain`]'s callback.
struct Lease<'a> {
    inbox: &'a Inbox,
    buf: Option<Box<[u8]>>,
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(buf) = self.buf.take() {
            self.inbox.lock().free.push(buf);
            self.inbox.returned.notify_one();
        }
    }
}

/// Input waiting for the writer thread.
struct Outbox {
    state: Mutex<OutboxState>,
    /// Signalled when input arrives or the PTY closes.
    queued: Condvar,
}

struct OutboxState {
    queue: VecDeque<u8>,
    /// [`Pty::write`] turned bytes away since the writer last made room.
    refused: bool,
    /// The program stopped taking input.
    failed: bool,
    closed: bool,
}

impl Outbox {
    fn new() -> Self {
        Self {
            state: Mutex::new(OutboxState {
                queue: VecDeque::new(),
                refused: false,
                failed: false,
                closed: false,
            }),
            queued: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, OutboxState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Queues what fits of `bytes` and returns how much did.
    fn push(&self, bytes: &[u8]) -> io::Result<usize> {
        let mut state = self.lock();
        if state.failed {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        let n = bytes.len().min(INPUT_QUEUE - state.queue.len());
        state.queue.extend(&bytes[..n]);
        state.refused |= n < bytes.len();
        drop(state);
        if n > 0 {
            self.queued.notify_one();
        }
        Ok(n)
    }

    /// The writer thread: feeds queued input to `writer` until the PTY
    /// closes or the program stops reading, and wakes the UI (`inbox`)
    /// when it makes room for input [`Pty::write`] turned away.
    fn run(&self, writer: &mut dyn Write, inbox: &Inbox) {
        let mut chunk = vec![0; WRITE_CHUNK];
        loop {
            let (n, refused) = {
                let mut state = self.lock();
                while state.queue.is_empty() && !state.closed {
                    state = self.queued.wait(state).unwrap_or_else(|e| e.into_inner());
                }
                if state.closed {
                    return;
                }
                let n = state.queue.len().min(chunk.len());
                for (to, from) in chunk.iter_mut().zip(state.queue.drain(..n)) {
                    *to = from;
                }
                (n, std::mem::take(&mut state.refused))
            };
            if refused {
                inbox.wake();
            }
            if writer
                .write_all(&chunk[..n])
                .and_then(|()| writer.flush())
                .is_err()
            {
                let mut state = self.lock();
                state.failed = true;
                state.queue = VecDeque::new();
                return;
            }
        }
    }

    fn close(&self) {
        self.lock().closed = true;
        self.queued.notify_all();
    }
}

/// The PTY master on Unix as one thread sees it: read and written without
/// blocking, waiting in `poll` on the master and on a pipe that the [`Pty`]
/// closes, so a read or write the program would hold up forever ends when
/// the [`Pty`] goes.
#[cfg(unix)]
struct Transport {
    file: std::fs::File,
    /// Readable (at end of file) once the [`Pty`] closed the other end.
    closed: Arc<io::PipeReader>,
}

#[cfg(unix)]
impl Transport {
    /// A copy of `master`, which the caller keeps open meanwhile.
    fn new(master: std::os::fd::RawFd, closed: Arc<io::PipeReader>) -> io::Result<Self> {
        // SAFETY: the caller's MasterPty owns `master` and keeps it open
        // through this call; the copy is ours.
        let fd = unsafe { std::os::fd::BorrowedFd::borrow_raw(master) }.try_clone_to_owned()?;
        Ok(Self {
            file: fd.into(),
            closed,
        })
    }

    /// Waits until the master is ready for `events`, or hung up. False once
    /// the [`Pty`] is closed.
    fn wait(&self, events: libc::c_short) -> io::Result<bool> {
        use std::os::fd::AsRawFd;
        let mut fds = [
            libc::pollfd {
                fd: self.file.as_raw_fd(),
                events,
                revents: 0,
            },
            libc::pollfd {
                fd: self.closed.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        loop {
            // SAFETY: `fds` holds two initialized pollfds.
            if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) } >= 0 {
                break;
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
        if fds[1].revents != 0 {
            return Ok(false);
        }
        if fds[0].revents & libc::POLLNVAL != 0 {
            // Would spin: the read or write after it would not block.
            return Err(io::Error::other("poll does not support the pty"));
        }
        Ok(true)
    }
}

#[cfg(unix)]
impl Read for Transport {
    /// End of file once the [`Pty`] is closed.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if !self.wait(libc::POLLIN)? {
                return Ok(0);
            }
            match (&self.file).read(buf) {
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                result => return result,
            }
        }
    }
}

#[cfg(unix)]
impl Write for Transport {
    /// Fails once the [`Pty`] is closed.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        loop {
            if !self.wait(libc::POLLOUT)? {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            match (&self.file).write(buf) {
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                result => return result,
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(unix)]
fn set_nonblocking(fd: std::os::fd::RawFd) -> io::Result<()> {
    // SAFETY: plain fcntl calls on an open descriptor.
    let ok = unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        flags >= 0 && libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) >= 0
    };
    if ok {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// The child's life on Unix, kept apart from the transport: the waiter
/// thread marks the exit before it reaps the child, and once the [`Pty`]
/// is dropped the reader thread gives a program that ignores SIGHUP
/// [`HANGUP_GRACE`] before it sends SIGKILL.
#[cfg(unix)]
struct Reap {
    pid: Option<u32>,
    state: Mutex<ReapState>,
    changed: Condvar,
}

#[cfg(unix)]
#[derive(Default)]
struct ReapState {
    exited: bool,
    dropped: bool,
}

#[cfg(unix)]
impl Reap {
    fn new(pid: Option<u32>) -> Self {
        Self {
            pid,
            state: Mutex::default(),
            changed: Condvar::new(),
        }
    }

    fn update(&self, f: impl FnOnce(&mut ReapState)) {
        f(&mut self.state.lock().unwrap_or_else(|e| e.into_inner()));
        self.changed.notify_all();
    }

    /// The waiter thread: waits for the exit, marks it, then reaps.
    fn wait(&self, child: &mut (dyn Child + Send + Sync)) -> Option<u32> {
        if let Some(pid) = self.pid {
            loop {
                // SAFETY: an all-zero siginfo_t is valid for waitid to fill.
                let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
                // WNOWAIT leaves the child unreaped, so its pid is not reused
                // before `exited` is set (see `kill_once_dropped`).
                // SAFETY: `info` is a valid out pointer.
                let result = unsafe {
                    libc::waitid(
                        libc::P_PID,
                        pid as libc::id_t,
                        &mut info,
                        libc::WEXITED | libc::WNOWAIT,
                    )
                };
                if result == 0 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                    break;
                }
            }
        }
        self.update(|state| state.exited = true);
        exit_code(child)
    }

    /// Returns once the child exited; if the [`Pty`] is dropped first,
    /// kills the child when it has not exited [`HANGUP_GRACE`] later.
    fn kill_once_dropped(&self) {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let state = self
            .changed
            .wait_while(state, |s| !s.exited && !s.dropped)
            .unwrap_or_else(|e| e.into_inner());
        let (state, _) = self
            .changed
            .wait_timeout_while(state, HANGUP_GRACE, |s| !s.exited)
            .unwrap_or_else(|e| e.into_inner());
        if !state.exited
            && let Some(pid) = self.pid
        {
            // Still unreaped while `exited` is false and the lock is held,
            // so the pid is the child's.
            // SAFETY: kill has no memory effects.
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
        }
    }
}

/// A running child on a PTY. Dropping it kills the child.
pub struct Pty {
    /// `None` once the child exited on Windows (see [`Pty::spawn`]).
    master: Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
    outbox: Arc<Outbox>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    geometry: PtyGeometry,
    inbox: Arc<Inbox>,
    #[cfg(unix)]
    reap: Arc<Reap>,
    /// Dropped with the Pty, which stops the reader and writer threads'
    /// reads and writes (see [`Transport`]).
    #[cfg(unix)]
    _close: io::PipeWriter,
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
    /// waits to be read; once, until [`Self::read`] empties the queue. It
    /// also runs, on the writer thread, when input [`Self::write`] turned
    /// away now fits. Point it at the app's waker and call [`Self::read`]
    /// when it fires.
    ///
    /// On Windows, ConPTY asks for the cursor position (`ESC [ 6 n`) and
    /// starts nothing until the reply comes back as input, so the output
    /// must reach a terminal that answers queries (as
    /// [`crate::TerminalState`] does) and its replies must be written back.
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
        #[cfg(unix)]
        let (mut reader, mut writer, close, reap) = {
            let fd = pair
                .master
                .as_raw_fd()
                .ok_or_else(|| io::Error::other("the pty has no file descriptor"))?;
            // On the open file, so for every copy of the master.
            set_nonblocking(fd)?;
            let (closed, close) = io::pipe()?;
            let closed = Arc::new(closed);
            (
                Transport::new(fd, Arc::clone(&closed))?,
                Transport::new(fd, closed)?,
                close,
                Arc::new(Reap::new(child.process_id())),
            )
        };
        #[cfg(windows)]
        let (mut reader, mut writer) = (
            pair.master.try_clone_reader().map_err(io::Error::other)?,
            pair.master.take_writer().map_err(io::Error::other)?,
        );
        let inbox = Arc::new(Inbox::new(on_ready));
        let shared = Arc::clone(&inbox);
        let outbox = Arc::new(Outbox::new());
        {
            let (outbox, inbox) = (Arc::clone(&outbox), Arc::clone(&inbox));
            // Blocks in a write while the program is not reading, until the
            // Pty closes (Unix) or its program is terminated (Windows).
            std::thread::Builder::new()
                .name("quark-terminal-write".into())
                .spawn(move || outbox.run(&mut writer, &inbox))?;
        }
        let master = Arc::new(Mutex::new(Some(pair.master)));
        #[cfg(windows)]
        let console = Arc::clone(&master);
        #[cfg(unix)]
        let waiting = Arc::clone(&reap);
        let waiter = std::thread::Builder::new()
            .name("quark-terminal-wait".into())
            .spawn(move || {
                #[cfg(unix)]
                let code = waiting.wait(child.as_mut());
                #[cfg(windows)]
                let code = exit_code(child.as_mut());
                // ConPTY keeps its output pipe open after the child exits,
                // until the pseudoconsole closes, so the reader would never
                // see end of file. Closing it flushes what the console has
                // not written yet, then ends the pipe.
                #[cfg(windows)]
                drop(console.lock().unwrap_or_else(|e| e.into_inner()).take());
                code
            })?;
        #[cfg(unix)]
        let reaping = Arc::clone(&reap);
        std::thread::Builder::new()
            .name("quark-terminal-pty".into())
            .spawn(move || {
                shared.pump(&mut reader);
                // Our copy of the master closes before any wait for the
                // child: once all are closed, the program's terminal hangs
                // up.
                drop(reader);
                #[cfg(unix)]
                reaping.kill_once_dropped();
                shared.finish(waiter.join().ok().flatten());
            })?;
        Ok(Self {
            master,
            outbox,
            killer,
            geometry,
            inbox,
            #[cfg(unix)]
            reap,
            #[cfg(unix)]
            _close: close,
        })
    }

    /// Hands the output read so far to `f` in order, then the exit once all
    /// output is read. Allocates nothing.
    pub fn read(&self, f: impl FnMut(PtyEvent<'_>)) {
        self.inbox.drain(f);
    }

    pub(crate) fn inbox(&self) -> Arc<Inbox> {
        Arc::clone(&self.inbox)
    }

    /// Queues input (keys, pastes, query replies) for the program and
    /// returns how many bytes fit; never waits for the program to read. At
    /// most [`INPUT_QUEUE`] bytes wait. Keep the rest and offer it again
    /// after the next `on_ready` wake (see [`Self::spawn`]), which comes
    /// once the queue has room. Fails once the program stopped reading
    /// input (it exited).
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        self.outbox.push(bytes)
    }

    /// Tells the program the grid changed size (SIGWINCH on Unix).
    pub fn resize(&mut self, geometry: PtyGeometry) -> io::Result<()> {
        if geometry == self.geometry {
            return Ok(());
        }
        self.geometry = geometry;
        match &*self.master.lock().unwrap_or_else(|e| e.into_inner()) {
            Some(master) => master.resize(geometry.size()).map_err(io::Error::other),
            None => Ok(()),
        }
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
        self.outbox.close();
        #[cfg(unix)]
        self.reap.update(|state| state.dropped = true);
        // SIGHUP on Unix (then `_close` drops, ending the transport);
        // TerminateProcess on Windows.
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

    /// A real program on the platform's PTY (ConPTY on Windows) sees a
    /// resize made while it runs, its output arrives, and its exit follows.
    #[test]
    fn a_program_sees_the_resize_and_its_exit_follows_its_output() {
        // Each waits for a line, then prints the size its terminal has.
        // On Windows the line arrives before ConPTY has started cmd, and
        // `set /p` reads input typed ahead like that.
        let command = if cfg!(windows) {
            PtyCommand::new("cmd")
                .arg("/d")
                .arg("/c")
                .arg("set /p x=& mode con")
        } else {
            PtyCommand::new("sh").arg("-c").arg("read x; stty size")
        };
        let size = |cols, rows| PtyGeometry {
            cols,
            rows,
            pixel_width: 0,
            pixel_height: 0,
        };
        let ui = thread::current();
        let mut pty = Pty::spawn(&command, size(80, 24), move || ui.unpark()).unwrap();
        pty.resize(size(100, 30)).unwrap();
        pty.write(b"\r").unwrap();

        let mut term = crate::TerminalState::headless(100, 30);
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let mut exit = None;
        while exit.is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "no exit before the deadline; screen:\n{}",
                term.refresh().text()
            );
            thread::park_timeout(Duration::from_millis(200));
            pty.read(|event| match event {
                PtyEvent::Output(bytes) => term.feed(bytes),
                PtyEvent::Exited(code) => exit = Some(code),
            });
            // Query replies go back to the program, as TerminalState sends
            // them with a PTY attached: ConPTY asks for the cursor position
            // and starts nothing until it is answered. Writing fails only
            // once the program has exited.
            let replies = term.take_input();
            if !replies.is_empty() {
                let _ = pty.write(&replies);
            }
        }
        let words = term
            .refresh()
            .text()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let expected = if cfg!(windows) {
            "Lines: 30 Columns: 100"
        } else {
            "30 100"
        };
        assert!(words.contains(expected), "{words}");
        assert_eq!(exit, Some(Some(0)));
    }

    /// Spawns `script` under `sh` on a raw PTY (no echo, no line editing
    /// or output translation), waking this thread.
    #[cfg(unix)]
    fn raw_sh(script: &str) -> Pty {
        let ui = thread::current();
        let command = PtyCommand::new("sh")
            .arg("-c")
            .arg(format!("stty raw -echo; {script}"));
        let size = PtyGeometry {
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
        };
        Pty::spawn(&command, size, move || ui.unpark()).unwrap()
    }

    /// Input of `len` bytes no line discipline treats specially.
    #[cfg(unix)]
    fn input(len: usize) -> Vec<u8> {
        (0..len).map(|i| b'a' + (i % 26) as u8).collect()
    }

    /// A program that writes a lot of output before it reads its input
    /// gets a large paste while the UI keeps draining its output: queueing
    /// the paste returns at once, and every byte arrives both ways.
    #[cfg(unix)]
    #[test]
    fn a_large_paste_to_a_program_busy_writing_does_not_block() {
        const LEN: usize = 1024 * 1024;
        let mut pty = raw_sh(&format!(
            "head -c {LEN} /dev/zero | tr '\\000' x; head -c {LEN}"
        ));
        let paste = input(LEN);
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        let mut out = Vec::new();
        let mut exit = None;
        let mut pasted = false;
        while exit.is_none() {
            // Paste once output shows the terminal is raw, while the
            // program still has most of its output to write.
            if !pasted && !out.is_empty() {
                assert!(out.len() < LEN / 2, "the program wrote too fast");
                let started = std::time::Instant::now();
                assert_eq!(pty.write(&paste).unwrap(), LEN);
                assert!(
                    started.elapsed() < Duration::from_secs(1),
                    "the write waited"
                );
                pasted = true;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "stuck at {} bytes",
                out.len()
            );
            thread::park_timeout(Duration::from_millis(100));
            pty.read(|event| match event {
                PtyEvent::Output(bytes) => out.extend_from_slice(bytes),
                PtyEvent::Exited(code) => exit = Some(code),
            });
        }
        assert_eq!(out.len(), 2 * LEN);
        assert!(out[..LEN].iter().all(|&b| b == b'x'));
        assert!(out[LEN..] == paste[..], "the paste arrived changed");
    }

    /// Input past the queue is turned away rather than waited on, and
    /// closing the PTY while its writer is stuck mid-write returns at once.
    #[cfg(unix)]
    #[test]
    fn a_full_queue_turns_input_away_and_closing_does_not_wait() {
        let mut pty = raw_sh("sleep 30");
        let paste = input(3 * INPUT_QUEUE);
        assert_eq!(pty.write(&paste).unwrap(), INPUT_QUEUE);
        let started = std::time::Instant::now();
        drop(pty);
        assert!(started.elapsed() < Duration::from_secs(2), "closing waited");
    }

    /// Polls `done` until it holds or `secs` pass.
    #[cfg(unix)]
    fn eventually(secs: u64, mut done: impl FnMut() -> bool) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(secs);
        while std::time::Instant::now() < deadline {
            if done() {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        done()
    }

    /// Dropping a PTY whose program has stopped reading, with input queued,
    /// returns at once and closes the master: a program that ignores
    /// SIGHUP still sees its terminal hang up (its read ends), and every
    /// worker thread lets go of the PTY.
    #[cfg(unix)]
    #[test]
    fn dropping_a_pty_hangs_up_a_program_that_stopped_reading() {
        let marker = std::env::temp_dir().join(format!("quark-pty-hangup-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        // The rest runs in the background so that killing the shell, which
        // ignores SIGHUP too, does not stop it. It reads nothing for a
        // while, so the queued input fills the terminal and the writer
        // blocks. An asynchronous list reads /dev/null unless given stdin.
        let mut pty = raw_sh(&format!(
            "trap '' HUP; exec 3<&0; \
             {{ sleep 0.2; cat <&3 >/dev/null; echo hung up >'{}'; }} & printf R; wait",
            marker.display()
        ));
        let mut ready = false;
        assert!(eventually(10, || {
            thread::park_timeout(Duration::from_millis(10));
            pty.read(|event| ready |= matches!(event, PtyEvent::Output(_)));
            ready
        }));
        let paste = input(INPUT_QUEUE);
        assert_eq!(pty.write(&paste).unwrap(), INPUT_QUEUE);
        let inbox = pty.inbox();
        let started = std::time::Instant::now();
        drop(pty);
        assert!(started.elapsed() < Duration::from_secs(1), "closing waited");
        assert!(
            eventually(10, || marker.exists()),
            "the program's terminal never hung up"
        );
        assert!(
            eventually(10, || Arc::strong_count(&inbox) == 1),
            "worker threads still hold the pty"
        );
        let _ = std::fs::remove_file(&marker);
    }

    /// Dropping a PTY kills a program that ignores SIGHUP, and reaps it.
    #[cfg(unix)]
    #[test]
    fn dropping_a_pty_kills_a_program_that_ignores_hangup() {
        let pty = raw_sh("trap '' HUP; printf '%s;' $$; exec sleep 30");
        let mut out = Vec::new();
        assert!(eventually(10, || {
            thread::park_timeout(Duration::from_millis(10));
            pty.read(|event| {
                if let PtyEvent::Output(bytes) = event {
                    out.extend_from_slice(bytes);
                }
            });
            out.ends_with(b";")
        }));
        let pid: libc::pid_t = std::str::from_utf8(&out[..out.len() - 1])
            .unwrap()
            .parse()
            .unwrap();
        let inbox = pty.inbox();
        let started = std::time::Instant::now();
        drop(pty);
        assert!(started.elapsed() < Duration::from_secs(1), "closing waited");
        // SAFETY: signal 0 only checks that the process exists; a zombie
        // still does, so this waits for it to be reaped too.
        assert!(
            eventually(10, || unsafe { libc::kill(pid, 0) } != 0),
            "the program outlived its pty"
        );
        assert!(
            eventually(10, || Arc::strong_count(&inbox) == 1),
            "worker threads still hold the pty"
        );
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
