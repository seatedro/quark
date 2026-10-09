//! What the fixture serves at each path. Every origin serves every page, so
//! a test picks a page by path and its origin by [`super::fixture::Site`].
//!
//! | Path | Page |
//! |---|---|
//! | `/getter` (also `/`) | `fixture.getToken()` resolves to `token` after `delay_ms` (default 50), or once key `hold` is released |
//! | `/throws` | Its only script throws during load, so `window.fixture` is never defined |
//! | `/redirect` | `code` (default 302) to `to` |
//! | `/popup` | Calls `window.open(to)` on load; `fixture.openPopup()` and the `#popup` link (`target=_blank`) open it again |
//! | `/frame` | A getter page around an `<iframe src=src>`; `fixture.frameLoaded()` resolves once the frame loads |
//! | `/slow` | The getter page, but the response waits until key `key` is released |
//! | `/status` | The getter page with HTTP status `code` |
//! | `/hold` | `text/plain` once key `key` is released; pages await it through `fixture.hold(key)` |
//!
//! Every HTML page except `/throws` also takes `title` (set as
//! `document.title` after load) and `go` (navigate there with `location`
//! once key `go_after` is released, or right after load without it).
//!
//! `window.fixture` on those pages:
//!
//! | Member | Result |
//! |---|---|
//! | `getToken()` | The getter a sign-in page exposes: a Promise of the `token` string |
//! | `rejects()` | A Promise rejected with `Error("fixture rejection")` |
//! | `throws()` | Throws `TypeError("fixture throw")` synchronously |
//! | `never()` | A Promise that never settles, for deadlines |
//! | `whereami()` | `{ origin, main, title }`: `main` is false inside a frame |
//! | `hold(key)` | A Promise that resolves once the test releases `key` |
//! | `openPopup()` | `window.open(to)`; returns whether a window object came back |

use std::fmt::Write;

/// A parsed `application/x-www-form-urlencoded` query.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query(pub Vec<(String, String)>);

impl Query {
    pub fn parse(raw: &str) -> Query {
        Query(
            raw.split('&')
                .filter(|pair| !pair.is_empty())
                .map(|pair| {
                    let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                    (decode(key), decode(value))
                })
                .collect(),
        )
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// Percent-encodes `s` for a query value: everything except unreserved
/// characters.
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = |b: u8| (b as char).to_digit(16);
                match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                    (Some(hi), Some(lo)) => {
                        out.push((hi * 16 + lo) as u8);
                        i += 2;
                    }
                    _ => out.push(b'%'),
                }
            }
            byte => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// One HTTP response.
#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

impl Response {
    fn html(status: u16, body: String) -> Response {
        Response {
            status,
            headers: vec![("Content-Type", "text/html; charset=utf-8".into())],
            body: body.into_bytes(),
        }
    }

    pub fn text(status: u16, body: impl Into<String>) -> Response {
        Response {
            status,
            headers: vec![("Content-Type", "text/plain; charset=utf-8".into())],
            body: body.into().into_bytes(),
        }
    }
}

/// How the server answers one request.
#[derive(Debug)]
pub enum Route {
    Now(Response),
    /// Wait until the key is released, then answer.
    AfterRelease(String, Response),
}

pub fn route(path: &str, query: &Query) -> Route {
    match path {
        "/" | "/getter" => Route::Now(Response::html(200, page("getter", "", ""))),
        "/throws" => Route::Now(Response::html(
            200,
            "<!doctype html><title>throws</title>\
             <script>throw new Error('fixture page error');</script>\
             <p>This page's script threw.</p>"
                .into(),
        )),
        "/redirect" => {
            let code = query
                .get("code")
                .and_then(|c| c.parse().ok())
                .filter(|c| (300..400).contains(c))
                .unwrap_or(302);
            let mut response = Response::text(code, "redirecting");
            response
                .headers
                .push(("Location", query.get("to").unwrap_or("/").to_string()));
            Route::Now(response)
        }
        "/popup" => Route::Now(Response::html(
            200,
            page(
                "popup",
                "<p><a id=\"popup\" target=\"_blank\">open popup</a></p>",
                "const link = document.getElementById('popup');\
                 link.href = q.get('to') ?? '/getter';\
                 window.fixture.openPopup();",
            ),
        )),
        "/frame" => Route::Now(Response::html(
            200,
            page(
                "frame",
                "<iframe id=\"frame\" width=\"400\" height=\"200\"></iframe>",
                "const frame = document.getElementById('frame');\
                 const loaded = new Promise(resolve => frame.addEventListener('load', resolve));\
                 window.fixture.frameLoaded = () => loaded.then(() => true);\
                 frame.src = q.get('src') ?? 'about:blank';",
            ),
        )),
        "/slow" => match query.get("key") {
            Some(key) => Route::AfterRelease(key.into(), Response::html(200, page("slow", "", ""))),
            None => Route::Now(Response::text(400, "slow needs key")),
        },
        "/status" => {
            let code = query
                .get("code")
                .and_then(|c| c.parse().ok())
                .filter(|c| (200..600).contains(c))
                .unwrap_or(500);
            Route::Now(Response::html(code, page("status", "", "")))
        }
        "/hold" => match query.get("key") {
            Some(key) => Route::AfterRelease(key.into(), Response::text(200, "released")),
            None => Route::Now(Response::text(400, "hold needs key")),
        },
        _ => Route::Now(Response::text(404, "not found")),
    }
}

/// A fixture HTML page: the shared `window.fixture` helper, then `body`
/// markup and an `extra` script that runs after the helper.
fn page(name: &str, body: &str, extra: &str) -> String {
    format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>{name}</title></head>\n\
         <body><h1>{name}</h1>{body}\n<script>\n{HELPER}\n{{ const q = new URLSearchParams(location.search); {extra} }}\n</script></body></html>\n"
    )
}

/// The page-side half of the fixture. Query values reach page code only
/// through `URLSearchParams`, never through the HTML, so no value needs
/// escaping.
const HELPER: &str = r#"(() => {
  const q = new URLSearchParams(location.search);
  const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
  const hold = key => fetch('/hold?key=' + encodeURIComponent(key), { cache: 'no-store' })
    .then(r => r.text());
  const token = q.get('token') ?? 'fixture-token';
  window.fixture = {
    async getToken() {
      if (q.has('hold')) await hold(q.get('hold'));
      else await sleep(Number(q.get('delay_ms') ?? 50));
      return token;
    },
    async rejects() { await sleep(0); throw new Error('fixture rejection'); },
    throws() { throw new TypeError('fixture throw'); },
    never() { return new Promise(() => {}); },
    whereami() {
      return { origin: location.origin, main: window.top === window.self, title: document.title };
    },
    hold,
    openPopup() { return window.open(q.get('to') ?? '/getter') !== null; },
  };
  addEventListener('load', () => {
    if (q.has('title')) document.title = q.get('title');
    if (q.has('go')) {
      const go = () => { location.href = q.get('go'); };
      if (q.has('go_after')) hold(q.get('go_after')).then(go); else setTimeout(go, 0);
    }
  });
})();"#;
