//! A local HTTPS server for webview tests: four loopback origins on their
//! own ports, the pages in [`super::pages`], a request log the tests read
//! as server-side markers, and keys a test releases to resume a held
//! response or Promise, so races never depend on sleeps.
//!
//! The certificates in `certs/` are synthetic. `ca.pem` signs the leaf
//! served by every site but [`Site::BadCert`], whose leaf comes from a CA
//! that is not kept anywhere. A platform test trusts the fixture by
//! accepting exactly [`leaf_der`] (or [`LEAF_PEM`]) for a loopback host; the
//! bad certificate must stay rejected. Regenerate with `certs/regen.sh`.

use std::collections::HashSet;
use std::io::{self, BufRead, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

use super::pages::{self, Query, Response, Route};

pub const CA_PEM: &str = include_str!("certs/ca.pem");
pub const LEAF_PEM: &str = include_str!("certs/leaf.pem");
const LEAF_KEY: &str = include_str!("certs/leaf.key");
pub const ROGUE_LEAF_PEM: &str = include_str!("certs/rogue-leaf.pem");
const ROGUE_LEAF_KEY: &str = include_str!("certs/rogue-leaf.key");

/// One of the fixture's origins. Each listens on its own port.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Site {
    /// `https://127.0.0.1:<port>`: the app's own origin, the one a test
    /// grants evaluation.
    App,
    /// `https://localhost:<port>`: a second allowed navigation origin, as an
    /// identity provider would be. A different host from `App`.
    Idp,
    /// `https://127.0.0.1:<port>`: never allowlisted; the cross-origin frame
    /// and the blocked redirect target.
    Foreign,
    /// `https://127.0.0.1:<port>` with a certificate from an unknown CA.
    BadCert,
}

impl Site {
    pub const ALL: [Site; 4] = [Site::App, Site::Idp, Site::Foreign, Site::BadCert];

    pub fn name(self) -> &'static str {
        match self {
            Site::App => "app",
            Site::Idp => "idp",
            Site::Foreign => "foreign",
            Site::BadCert => "bad_cert",
        }
    }

    pub fn from_name(name: &str) -> Option<Site> {
        Site::ALL.into_iter().find(|site| site.name() == name)
    }

    fn host(self) -> &'static str {
        match self {
            Site::Idp => "localhost",
            _ => "127.0.0.1",
        }
    }
}

/// A request the fixture received, after its TLS handshake succeeded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub site: Site,
    pub path: String,
    pub query: Query,
}

/// The running server. Dropping it stops accepting and wakes every held
/// response.
pub struct Fixture {
    ports: Vec<(Site, u16)>,
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    stopping: AtomicBool,
}

#[derive(Default)]
struct State {
    released: HashSet<String>,
    requests: Vec<Request>,
}

impl Fixture {
    /// Binds every site before returning, so a browser opened afterwards
    /// never races the listeners.
    pub fn start() -> io::Result<Fixture> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let good = server_config(&provider, LEAF_PEM, LEAF_KEY)?;
        let rogue = server_config(&provider, ROGUE_LEAF_PEM, ROGUE_LEAF_KEY)?;
        let shared = Arc::new(Shared::default());
        let mut ports = Vec::new();
        for site in Site::ALL {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
            let port = listener.local_addr()?.port();
            let config = if site == Site::BadCert { &rogue } else { &good };
            spawn_acceptor(site, listener, config.clone(), shared.clone());
            // Engines may try `localhost` over IPv6 first; answer there too
            // when the same port is free, rather than leave it to fallback.
            if site == Site::Idp
                && let Ok(v6) = TcpListener::bind((Ipv6Addr::LOCALHOST, port))
            {
                spawn_acceptor(site, v6, config.clone(), shared.clone());
            }
            ports.push((site, port));
        }
        Ok(Fixture { ports, shared })
    }

    pub fn port(&self, site: Site) -> u16 {
        self.ports
            .iter()
            .find(|(s, _)| *s == site)
            .map(|(_, port)| *port)
            .expect("every site is bound")
    }

    /// `https://<host>:<port>`, as a browser serializes the origin.
    pub fn origin(&self, site: Site) -> String {
        format!("https://{}:{}", site.host(), self.port(site))
    }

    /// The URL of `path` on `site` with `query` percent-encoded.
    pub fn url(&self, site: Site, path: &str, query: &[(&str, &str)]) -> String {
        let mut url = format!("{}{path}", self.origin(site));
        for (i, (key, value)) in query.iter().enumerate() {
            url.push(if i == 0 { '?' } else { '&' });
            url.push_str(&pages::encode(key));
            url.push('=');
            url.push_str(&pages::encode(value));
        }
        url
    }

    /// Resumes every response and page Promise waiting on `key`, now and
    /// later.
    pub fn release(&self, key: &str) {
        self.shared
            .state
            .lock()
            .unwrap()
            .released
            .insert(key.to_string());
        self.shared.changed.notify_all();
    }

    pub fn requests(&self) -> Vec<Request> {
        self.shared.state.lock().unwrap().requests.clone()
    }

    /// The first logged request for `path` on `site`, waiting up to
    /// `timeout` for it to arrive.
    pub fn wait_request(&self, site: Site, path: &str, timeout: Duration) -> Option<Request> {
        let deadline = Instant::now() + timeout;
        let mut state = self.shared.state.lock().unwrap();
        loop {
            if let Some(found) = state
                .requests
                .iter()
                .find(|r| r.site == site && r.path == path)
            {
                return Some(found.clone());
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            state = self.shared.changed.wait_timeout(state, left).unwrap().0;
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        self.shared.changed.notify_all();
        // Wake each blocked accept so its thread sees `stopping`.
        for (_, port) in &self.ports {
            let _ = TcpStream::connect((Ipv4Addr::LOCALHOST, *port));
        }
    }
}

/// The DER bytes of the trusted leaf, for adapters that compare
/// certificates.
pub fn leaf_der() -> Vec<u8> {
    CertificateDer::from_pem_slice(LEAF_PEM.as_bytes())
        .expect("fixture leaf parses")
        .to_vec()
}

fn server_config(
    provider: &Arc<rustls::crypto::CryptoProvider>,
    cert_pem: &str,
    key_pem: &str,
) -> io::Result<Arc<rustls::ServerConfig>> {
    let chain =
        vec![CertificateDer::from_pem_slice(cert_pem.as_bytes()).map_err(io::Error::other)?];
    let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes()).map_err(io::Error::other)?;
    let config = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(io::Error::other)?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map_err(io::Error::other)?;
    Ok(Arc::new(config))
}

fn spawn_acceptor(
    site: Site,
    listener: TcpListener,
    config: Arc<rustls::ServerConfig>,
    shared: Arc<Shared>,
) {
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            if shared.stopping.load(Ordering::SeqCst) {
                return;
            }
            let Ok(stream) = stream else { continue };
            let (config, shared) = (config.clone(), shared.clone());
            std::thread::spawn(move || {
                let _ = serve(site, stream, config, &shared);
            });
        }
    });
}

/// Answers one request on one connection, then closes it.
fn serve(
    site: Site,
    stream: TcpStream,
    config: Arc<rustls::ServerConfig>,
    shared: &Shared,
) -> io::Result<()> {
    // Bounds a client that connects and never finishes a request. Held
    // responses wait on the condvar, not on the socket, so this does not
    // cut them short.
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let connection = rustls::ServerConnection::new(config).map_err(io::Error::other)?;
    let mut tls = rustls::StreamOwned::new(connection, stream);
    let Some(target) = read_request_target(&mut tls)? else {
        return write_response(&mut tls, &Response::text(405, "GET only"));
    };
    let (path, raw_query) = target.split_once('?').unwrap_or((&target, ""));
    let query = Query::parse(raw_query);
    {
        let mut state = shared.state.lock().unwrap();
        state.requests.push(Request {
            site,
            path: path.to_string(),
            query: query.clone(),
        });
    }
    shared.changed.notify_all();
    let response = match pages::route(path, &query) {
        Route::Now(response) => response,
        Route::AfterRelease(key, response) => {
            let mut state = shared.state.lock().unwrap();
            while !state.released.contains(&key) {
                if shared.stopping.load(Ordering::SeqCst) {
                    return Ok(());
                }
                state = shared.changed.wait(state).unwrap();
            }
            response
        }
    };
    write_response(&mut tls, &response)?;
    tls.conn.send_close_notify();
    tls.flush()
}

/// The request target of a GET or HEAD request, or `None` for any other
/// method. Headers are read and dropped; the fixture needs none of them.
fn read_request_target(stream: &mut impl Read) -> io::Result<Option<String>> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 32 * 1024 || stream.read(&mut byte)? == 0 {
            return Err(io::ErrorKind::InvalidData.into());
        }
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head);
    let mut words = head.lines().next().unwrap_or("").split(' ');
    match (words.next(), words.next()) {
        (Some("GET" | "HEAD"), Some(target)) => Ok(Some(target.to_string())),
        _ => Ok(None),
    }
}

fn write_response(stream: &mut impl Write, response: &Response) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} Fixture\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n",
        response.status,
        response.body.len()
    );
    for (name, value) in &response.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(&response.body)?;
    stream.flush()
}

/// Runs the fixture for another process: prints one JSON line naming the
/// origins and certificate, then answers one JSON line per command line on
/// stdin until `quit` or end of input.
///
/// | Command | Reply |
/// |---|---|
/// | `release <key>` | `{"ok":true}` |
/// | `requests` | `{"requests":[{"site":"app","path":"/getter","query":{..}}, ..]}` |
/// | `wait <site> <path> <timeout_ms>` | `{"request":{..}}`, or `{"request":null}` on timeout |
/// | `quit` | `{"ok":true}`, then exit |
pub fn serve_stdio() -> io::Result<()> {
    let fixture = Fixture::start()?;
    let origins: serde_json::Map<_, _> = Site::ALL
        .into_iter()
        .map(|site| (site.name().to_string(), fixture.origin(site).into()))
        .collect();
    let mut out = io::stdout().lock();
    let ready = serde_json::json!({
        "ready": true,
        "origins": origins,
        "ca_pem": CA_PEM,
        "leaf_pem": LEAF_PEM,
    });
    writeln!(out, "{ready}")?;
    out.flush()?;
    for line in io::stdin().lock().lines() {
        let line = line?;
        let words: Vec<&str> = line.split_whitespace().collect();
        let reply = match words.as_slice() {
            ["release", key] => {
                fixture.release(key);
                serde_json::json!({ "ok": true })
            }
            ["requests"] => serde_json::json!({
                "requests": fixture.requests().iter().map(request_json).collect::<Vec<_>>(),
            }),
            ["wait", site, path, timeout_ms] => {
                match (Site::from_name(site), timeout_ms.parse::<u64>()) {
                    (Some(site), Ok(ms)) => serde_json::json!({
                        "request": fixture
                            .wait_request(site, path, Duration::from_millis(ms))
                            .as_ref()
                            .map(request_json),
                    }),
                    _ => serde_json::json!({ "error": "usage: wait <site> <path> <timeout_ms>" }),
                }
            }
            ["quit"] => {
                writeln!(out, "{}", serde_json::json!({ "ok": true }))?;
                return Ok(());
            }
            _ => serde_json::json!({ "error": format!("unknown command: {line}") }),
        };
        writeln!(out, "{reply}")?;
        out.flush()?;
    }
    Ok(())
}

fn request_json(request: &Request) -> serde_json::Value {
    let query: serde_json::Map<_, _> = request
        .query
        .0
        .iter()
        .map(|(k, v)| (k.clone(), v.clone().into()))
        .collect();
    serde_json::json!({ "site": request.site.name(), "path": request.path, "query": query })
}

/// Client-side helpers for checking the fixture itself over real TLS.
pub mod client {
    use super::*;

    /// The response head of `GET path` on `site`, trusting only the
    /// fixture CA. Fails with the TLS error when the certificate does not
    /// verify.
    pub fn get(fixture: &Fixture, site: Site, target: &str) -> io::Result<(u16, String)> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(CertificateDer::from_pem_slice(CA_PEM.as_bytes()).map_err(io::Error::other)?)
            .map_err(io::Error::other)?;
        let config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(io::Error::other)?
            .with_root_certificates(roots)
            .with_no_client_auth();
        let name = rustls::pki_types::ServerName::try_from(site.host().to_string())
            .map_err(io::Error::other)?;
        let connection =
            rustls::ClientConnection::new(Arc::new(config), name).map_err(io::Error::other)?;
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, fixture.port(site)));
        let socket = TcpStream::connect_timeout(&addr, Duration::from_secs(5))?;
        socket.set_read_timeout(Some(Duration::from_secs(10)))?;
        let mut tls = rustls::StreamOwned::new(connection, socket);
        write!(
            tls,
            "GET {target} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n\r\n",
            site.host(),
            fixture.port(site)
        )?;
        tls.flush()?;
        let mut reply = Vec::new();
        match tls.read_to_end(&mut reply) {
            Ok(_) => {}
            Err(e) => return Err(e),
        }
        let reply = String::from_utf8_lossy(&reply);
        let status = reply
            .split(' ')
            .nth(1)
            .and_then(|code| code.parse().ok())
            .ok_or_else(|| io::Error::other("no status line"))?;
        let head = reply.split("\r\n\r\n").next().unwrap_or("").to_string();
        Ok((status, head))
    }
}
