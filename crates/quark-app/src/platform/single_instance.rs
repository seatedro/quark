//! One running instance per app. The first launch listens on a per-user local
//! socket; later launches connect, forward their arguments (file paths, deep
//! link URLs), and exit. The first instance receives them as
//! [`AppEvent::OpenUrls`] after [`EventContext::listen_for_instances`].
//!
//! On Unix the socket lives in `$XDG_RUNTIME_DIR`, falling back to a
//! `0700` directory under the temp dir. On Windows it is a named pipe.
//!
//! macOS delivers URL clicks for a running app as an Apple Event to that
//! process rather than launching a second one, and winit 0.30 does not expose
//! `application:openURLs:`. So on macOS this handles only command line
//! launches, not URL scheme or file associations.
//!
//! ```no_run
//! use quark_app::platform::single_instance::{self, Instance};
//!
//! let args: Vec<String> = std::env::args().skip(1).collect();
//! let primary = match single_instance::acquire("com.example.notes", &args) {
//!     Ok(Instance::Primary(primary)) => Some(primary),
//!     Ok(Instance::Secondary) => return, // the first instance has the args
//!     Err(_) => None,                    // run unguarded rather than not at all
//! };
//! // Later, in `App::init`: `if let Some(p) = primary { cx.listen_for_instances(p) }`.
//! ```
//!
//! [`AppEvent::OpenUrls`]: crate::AppEvent::OpenUrls
//! [`EventContext::listen_for_instances`]: crate::EventContext::listen_for_instances

use std::io::{self, Read, Write};

use crate::runner::{AppEvent, EventSink};

/// Bounds what a connecting process can make the first instance read.
const MAX_ARGS: usize = 4096;
const MAX_BYTES: usize = 1 << 20;
/// How long either side waits on a stalled peer.
#[cfg(unix)]
const IO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const ACK: u8 = 1;

pub enum Instance {
    /// This process is the first instance and owns the socket.
    Primary(PrimaryInstance),
    /// Another instance is running and has received this process's arguments.
    Secondary,
}

/// Owns the listening socket of the first instance.
pub struct PrimaryInstance {
    listener: imp::Listener,
}

/// Become the first instance of `app_id`, or forward `args` to it. `app_id`
/// should be stable and unique, such as a reverse domain name.
pub fn acquire(app_id: &str, args: &[String]) -> io::Result<Instance> {
    imp::acquire(&imp::endpoint(app_id)?, args)
}

impl PrimaryInstance {
    /// Wait for the next launch and return the arguments it forwarded.
    pub fn accept(&self) -> io::Result<Vec<String>> {
        let mut stream = self.listener.accept()?;
        let args = read_args(&mut stream)?;
        stream.write_all(&[ACK])?;
        Ok(args)
    }

    pub(crate) fn spawn(self, events: EventSink) {
        let spawned = std::thread::Builder::new()
            .name("quark-single-instance".to_owned())
            .spawn(move || {
                loop {
                    match self.accept() {
                        Ok(args) => {
                            if !events.send(AppEvent::OpenUrls(args)) {
                                return;
                            }
                        }
                        // One bad client must not stop later launches.
                        Err(error) => tracing::warn!("single instance handoff failed: {error}"),
                    }
                }
            });
        if let Err(error) = spawned {
            tracing::warn!("could not start the single instance listener: {error}");
        }
    }
}

fn forward(mut stream: impl Read + Write, args: &[String]) -> io::Result<()> {
    let mut message = Vec::new();
    message.extend_from_slice(&(args.len() as u32).to_le_bytes());
    for arg in args {
        message.extend_from_slice(&(arg.len() as u32).to_le_bytes());
        message.extend_from_slice(arg.as_bytes());
    }
    stream.write_all(&message)?;
    stream.flush()?;
    let mut ack = [0];
    stream.read_exact(&mut ack)?;
    if ack[0] == ACK {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad acknowledgement",
        ))
    }
}

fn read_args(stream: &mut impl Read) -> io::Result<Vec<String>> {
    let count = read_u32(stream)? as usize;
    if count > MAX_ARGS {
        return Err(invalid("too many arguments"));
    }
    let mut total = 0;
    let mut args = Vec::with_capacity(count);
    for _ in 0..count {
        let len = read_u32(stream)? as usize;
        total += len;
        if total > MAX_BYTES {
            return Err(invalid("arguments too long"));
        }
        let mut bytes = vec![0; len];
        stream.read_exact(&mut bytes)?;
        args.push(String::from_utf8(bytes).map_err(|_| invalid("argument is not UTF-8"))?);
    }
    Ok(args)
}

fn read_u32(stream: &mut impl Read) -> io::Result<u32> {
    let mut bytes = [0; 4];
    stream.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Keeps the endpoint name portable and short enough for `sun_path`.
fn sanitize(app_id: &str) -> String {
    let name: String = app_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .take(64)
        .collect();
    if name.is_empty() {
        "quark-app".to_owned()
    } else {
        name
    }
}

#[cfg(unix)]
mod imp {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};

    use super::*;

    pub(super) struct Listener(UnixListener);

    impl Listener {
        pub(super) fn accept(&self) -> io::Result<UnixStream> {
            let (stream, _) = self.0.accept()?;
            stream.set_read_timeout(Some(IO_TIMEOUT))?;
            stream.set_write_timeout(Some(IO_TIMEOUT))?;
            Ok(stream)
        }
    }

    const MAX_SOCKET_PATH: usize = 104;

    pub(super) fn endpoint(app_id: &str) -> io::Result<PathBuf> {
        endpoint_in(
            std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
            app_id,
        )
    }

    pub(super) fn endpoint_in(runtime_dir: Option<PathBuf>, app_id: &str) -> io::Result<PathBuf> {
        let file = format!("{}.sock", sanitize(app_id));
        if let Some(dir) = runtime_dir.filter(|dir| dir.is_absolute()) {
            let path = dir.join(&file);
            // sun_path holds about 104 bytes (108 on Linux, minus the NUL);
            // a deep runtime dir would make bind fail, so use the temp dir.
            if path.as_os_str().len() < MAX_SOCKET_PATH {
                return Ok(path);
            }
        }
        // A shared temp dir needs a private subdirectory, or another user
        // could create the socket first and receive our arguments.
        let user = std::env::var("USER").unwrap_or_default();
        let dir = std::env::temp_dir().join(format!("quark-{}", sanitize(&user)));
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        let mode = std::fs::symlink_metadata(&dir)?.permissions().mode();
        if mode & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} is accessible to other users", dir.display()),
            ));
        }
        Ok(dir.join(file))
    }

    pub(super) fn acquire(path: &Path, args: &[String]) -> io::Result<Instance> {
        match connect(path) {
            Ok(stream) => {
                forward(stream, args)?;
                return Ok(Instance::Secondary);
            }
            // Nothing listening: no socket yet, or one left by a crash.
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) => {}
            Err(error) => return Err(error),
        }
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        match UnixListener::bind(path) {
            Ok(listener) => Ok(Instance::Primary(PrimaryInstance {
                listener: Listener(listener),
            })),
            // Another launch bound it between our connect and bind.
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                forward(connect(path)?, args)?;
                Ok(Instance::Secondary)
            }
            Err(error) => Err(error),
        }
    }

    fn connect(path: &Path) -> io::Result<UnixStream> {
        let stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        Ok(stream)
    }
}

#[cfg(windows)]
mod imp {
    use interprocess::local_socket::prelude::*;
    use interprocess::local_socket::{GenericNamespaced, ListenerOptions, Stream};

    use super::*;

    pub(super) struct Listener(interprocess::local_socket::Listener);

    impl Listener {
        pub(super) fn accept(&self) -> io::Result<Stream> {
            self.0.accept()
        }
    }

    pub(super) fn endpoint(app_id: &str) -> io::Result<String> {
        // Named pipes are per session but not per user; the user name keeps
        // two users on one machine apart.
        let user = std::env::var("USERNAME").unwrap_or_default();
        Ok(format!("{}-{}.quark", sanitize(app_id), sanitize(&user)))
    }

    pub(super) fn acquire(name: &str, args: &[String]) -> io::Result<Instance> {
        let ns_name = || name.to_ns_name::<GenericNamespaced>();
        if let Ok(stream) = Stream::connect(ns_name()?) {
            forward(stream, args)?;
            return Ok(Instance::Secondary);
        }
        match ListenerOptions::new().name(ns_name()?).create_sync() {
            Ok(listener) => Ok(Instance::Primary(PrimaryInstance {
                listener: Listener(listener),
            })),
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                forward(Stream::connect(ns_name()?)?, args)?;
                Ok(Instance::Secondary)
            }
            Err(error) => Err(error),
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;

    use super::*;

    fn socket_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("quark-si-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create test dir");
        let path = dir.join(name);
        let _ = std::fs::remove_file(&path);
        path
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn second_launch_forwards_args_to_first_and_reports_secondary() {
        let path = socket_path("handoff.sock");
        let Instance::Primary(primary) = imp::acquire(&path, &strings(&["first"])).unwrap() else {
            panic!("first launch should be primary");
        };
        let receiver = std::thread::spawn(move || primary.accept().unwrap());

        let forwarded = strings(&["notes://open?id=7", "/tmp/a b.txt", ""]);
        let second = imp::acquire(&path, &forwarded).unwrap();

        assert!(matches!(second, Instance::Secondary));
        assert_eq!(receiver.join().unwrap(), forwarded);
    }

    #[test]
    fn socket_left_by_crashed_instance_does_not_block_next_launch() {
        let path = socket_path("stale.sock");
        // Dropping a std listener leaves its socket file, as a crash would.
        drop(UnixListener::bind(&path).unwrap());

        let launch = imp::acquire(&path, &[]).unwrap();

        assert!(matches!(launch, Instance::Primary(_)));
    }

    #[test]
    fn runtime_dir_too_deep_for_a_socket_falls_back_to_temp_dir() {
        let deep = PathBuf::from(format!("/{}", "d".repeat(120)));

        let path = imp::endpoint_in(Some(deep.clone()), "app").unwrap();

        assert!(!path.starts_with(&deep), "{}", path.display());
        let _ = std::fs::remove_file(&path);
        assert!(UnixListener::bind(&path).is_ok());
        let _ = std::fs::remove_file(&path);
    }
}
