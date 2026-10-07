//! Resumable artifact downloads. Bytes land in `<dest>.part`; an interrupted
//! download leaves the part file behind and the next attempt asks for the
//! rest with an HTTP `Range` request. The file is renamed to `dest` only once
//! its SHA-256 matches the signed manifest.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use ring::digest::{Context, SHA256};

use crate::hex;
use crate::manifest::Artifact;

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("update download failed: {0}")]
    Http(#[from] ureq::Error),
    #[error("update download returned HTTP {0}")]
    Status(u16),
    #[error("update download stopped after {received} of {expected} bytes")]
    Incomplete { received: u64, expected: u64 },
    #[error("update download is corrupt: SHA-256 {actual}, expected {expected}")]
    ShaMismatch { expected: String, actual: String },
    #[error("update download I/O failed: {0}")]
    Io(#[from] io::Error),
}

/// Bytes on disk so far and the full size when known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub downloaded: u64,
    pub total: Option<u64>,
}

/// Download `artifact` to `dest`, resuming a part file from an earlier
/// attempt. Returns `dest` once the bytes match `artifact.sha256`. A
/// mismatch deletes the part file so the next attempt starts over.
pub fn download(
    agent: &ureq::Agent,
    artifact: &Artifact,
    dest: &Path,
    progress: &mut dyn FnMut(Progress),
) -> Result<PathBuf, DownloadError> {
    let expected = artifact.sha256.trim().to_ascii_lowercase();
    if dest.exists() {
        if sha256_file(dest)? == expected {
            return Ok(dest.to_path_buf());
        }
        fs::remove_file(dest)?;
    }
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    let part = part_path(dest);

    // One retry from zero when the server's range answer does not line up
    // with the part file.
    let mut restarted = false;
    loop {
        let mut offset = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        if artifact.size.is_some_and(|size| offset > size) {
            offset = 0;
        }
        let mut request = agent.get(&artifact.url);
        if offset > 0 {
            request = request.header("Range", format!("bytes={offset}-"));
        }
        let response = request.call()?;
        let status = response.status().as_u16();
        let start = match status {
            200 => 0,
            206 => response
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                .and_then(content_range_start)
                .unwrap_or(u64::MAX),
            // The part file already holds every byte the server has.
            416 if offset > 0 && artifact.size.is_none_or(|size| size == offset) => offset,
            416 if !restarted => {
                fs::remove_file(&part)?;
                restarted = true;
                continue;
            }
            status => return Err(DownloadError::Status(status)),
        };
        if status == 206 && start != offset {
            if restarted {
                return Err(DownloadError::Status(status));
            }
            fs::remove_file(&part)?;
            restarted = true;
            continue;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(start == 0)
            .append(start != 0)
            .open(&part)?;
        if status != 416 {
            let mut body = response.into_body();
            let mut reader = body.with_config().limit(u64::MAX).reader();
            copy_with_progress(&mut reader, &mut file, start, artifact.size, progress)?;
        }
        file.sync_all()?;
        break;
    }

    let received = fs::metadata(&part)?.len();
    if let Some(size) = artifact.size
        && received != size
    {
        return Err(DownloadError::Incomplete {
            received,
            expected: size,
        });
    }
    let actual = sha256_file(&part)?;
    if actual != expected {
        fs::remove_file(&part)?;
        return Err(DownloadError::ShaMismatch { expected, actual });
    }
    fs::rename(&part, dest)?;
    Ok(dest.to_path_buf())
}

pub(crate) fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_owned();
    name.push(".part");
    dest.with_file_name(name)
}

/// Writes go straight to the file, unbuffered, so the bytes of a body that
/// fails midway are on disk for the next attempt to resume from.
fn copy_with_progress(
    reader: &mut dyn Read,
    file: &mut File,
    start: u64,
    total: Option<u64>,
    progress: &mut dyn FnMut(Progress),
) -> io::Result<()> {
    let mut buf = vec![0; 64 * 1024];
    let mut downloaded = start;
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        file.write_all(&buf[..n])?;
        downloaded += n as u64;
        progress(Progress { downloaded, total });
    }
}

/// The first byte of `bytes <first>-<last>/<size>`.
fn content_range_start(value: &str) -> Option<u64> {
    value
        .trim()
        .strip_prefix("bytes ")?
        .split('-')
        .next()?
        .trim()
        .parse()
        .ok()
}

pub(crate) fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut context = Context::new(&SHA256);
    let mut buf = vec![0; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        context.update(&buf[..n]);
    }
    Ok(hex::encode(context.finish().as_ref()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::manifest::Format;
    use std::io::BufRead;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;

    /// How the test server answers each successive request.
    #[derive(Clone, Copy)]
    pub(crate) enum Reply {
        /// Honor `Range`, but close the connection after `n` body bytes.
        CutAfter(usize),
        /// Honor `Range` and send the rest.
        Full,
        /// Answer `200` with the whole file whatever the request asked.
        IgnoreRange,
    }

    /// A one request per connection HTTP/1.1 server on localhost serving
    /// `body`. Records each request's `Range` header (or `-`).
    pub(crate) struct Server {
        pub(crate) url: String,
        log: Arc<Mutex<Vec<String>>>,
        thread: Option<JoinHandle<()>>,
    }

    impl Server {
        pub(crate) fn start(body: Vec<u8>, replies: Vec<Reply>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/hello.AppImage", listener.local_addr().unwrap());
            let log = Arc::new(Mutex::new(Vec::new()));
            let thread_log = log.clone();
            let thread = std::thread::spawn(move || {
                for reply in replies {
                    let (mut stream, _) = listener.accept().unwrap();
                    let mut reader = io::BufReader::new(stream.try_clone().unwrap());
                    let mut range = None;
                    loop {
                        let mut line = String::new();
                        reader.read_line(&mut line).unwrap();
                        if line == "\r\n" || line.is_empty() {
                            break;
                        }
                        if let Some((name, value)) = line.split_once(':')
                            && name.eq_ignore_ascii_case("range")
                        {
                            range = Some(value.trim().to_owned());
                        }
                    }
                    thread_log
                        .lock()
                        .unwrap()
                        .push(range.clone().unwrap_or_else(|| "-".into()));
                    let start = match (reply, &range) {
                        (Reply::IgnoreRange, _) | (_, None) => 0,
                        (_, Some(range)) => range
                            .strip_prefix("bytes=")
                            .and_then(|r| r.trim_end_matches('-').parse().ok())
                            .unwrap(),
                    };
                    let rest = &body[start..];
                    let head = if start == 0 {
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            rest.len()
                        )
                    } else {
                        format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{}/{}\r\nConnection: close\r\n\r\n",
                            rest.len(),
                            body.len() - 1,
                            body.len()
                        )
                    };
                    stream.write_all(head.as_bytes()).unwrap();
                    let sent = match reply {
                        Reply::CutAfter(n) => &rest[..n.min(rest.len())],
                        _ => rest,
                    };
                    stream.write_all(sent).unwrap();
                }
            });
            Self {
                url,
                log,
                thread: Some(thread),
            }
        }

        /// The `Range` header of each request so far, once the server has
        /// answered every reply it was given.
        pub(crate) fn finish(mut self) -> Vec<String> {
            self.thread.take().unwrap().join().unwrap();
            self.log.lock().unwrap().clone()
        }
    }

    pub(crate) fn body() -> Vec<u8> {
        (0..200_000u32).map(|i| (i * 7 % 251) as u8).collect()
    }

    pub(crate) fn artifact(url: &str, body: &[u8]) -> Artifact {
        let mut context = Context::new(&SHA256);
        context.update(body);
        Artifact {
            format: Format::AppImage,
            url: url.to_owned(),
            sha256: hex::encode(context.finish().as_ref()),
            size: Some(body.len() as u64),
        }
    }

    fn agent() -> ureq::Agent {
        crate::updater::agent()
    }

    #[test]
    fn resumes_an_interrupted_download_from_the_part_file() {
        for (name, second, expected_log) in [
            ("server honors range", Reply::Full, ["-", "bytes=70000-"]),
            (
                "server ignores range",
                Reply::IgnoreRange,
                ["-", "bytes=70000-"],
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let dest = dir.path().join("hello.AppImage");
            let body = body();
            let server = Server::start(body.clone(), vec![Reply::CutAfter(70_000), second]);
            let artifact = artifact(&server.url, &body);

            let first = download(&agent(), &artifact, &dest, &mut |_| {});
            assert!(first.is_err(), "{name}: the cut transfer must fail");
            assert_eq!(
                fs::metadata(part_path(&dest)).unwrap().len(),
                70_000,
                "{name}"
            );

            let mut last = None;
            let path = download(&agent(), &artifact, &dest, &mut |p| last = Some(p)).unwrap();
            assert_eq!(fs::read(&path).unwrap(), body, "{name}");
            assert!(!part_path(&dest).exists(), "{name}");
            assert_eq!(
                last,
                Some(Progress {
                    downloaded: body.len() as u64,
                    total: Some(body.len() as u64)
                }),
                "{name}"
            );
            assert_eq!(server.finish(), expected_log, "{name}");
        }
    }

    #[test]
    fn sha_mismatch_discards_the_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("hello.AppImage");
        let body = body();
        let server = Server::start(body.clone(), vec![Reply::Full]);
        let mut artifact = artifact(&server.url, &body);
        artifact.sha256 = "00".repeat(32);

        let error = download(&agent(), &artifact, &dest, &mut |_| {}).unwrap_err();
        assert!(
            matches!(&error, DownloadError::ShaMismatch { expected, .. } if *expected == "00".repeat(32)),
            "{error}"
        );
        assert!(!dest.exists());
        assert!(!part_path(&dest).exists());
        server.finish();
    }
}
