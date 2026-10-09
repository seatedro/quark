//! Webviews against the local HTTPS fixture in `tests/webview/`. First the
//! fixture itself over real TLS: its CA verifies both hosts, the bad
//! certificate site does not, redirects carry their target, and a held
//! response waits for its release. Any failure exits non-zero.
//!
//! `cargo test -p quark-app --test webview_smoke -- --serve` instead runs
//! the fixture for another process; see `webview::fixture::serve_stdio` for
//! the line protocol.

#[path = "webview/mod.rs"]
mod webview;

use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Duration;

use webview::fixture::{Fixture, Site, client};

fn main() -> ExitCode {
    if std::env::args().any(|arg| arg == "--serve") {
        return match webview::fixture::serve_stdio() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("webview_smoke: fixture: {e}");
                ExitCode::FAILURE
            }
        };
    }
    let mut failures = Vec::new();
    fixture_checks(&mut failures);
    if failures.is_empty() {
        println!("webview_smoke: ok");
        ExitCode::SUCCESS
    } else {
        for failure in &failures {
            eprintln!("webview_smoke: FAILED: {failure}");
        }
        ExitCode::FAILURE
    }
}

fn check(failures: &mut Vec<String>, what: &str, ok: bool, detail: impl std::fmt::Debug) {
    if ok {
        println!("webview_smoke: ok: {what}");
    } else {
        failures.push(format!("{what}: {detail:?}"));
    }
}

fn fixture_checks(failures: &mut Vec<String>) {
    let fixture = Fixture::start().expect("fixture binds its sites");

    let got = client::get(&fixture, Site::App, "/getter?token=t");
    check(
        failures,
        "the fixture CA verifies the 127.0.0.1 leaf",
        matches!(got, Ok((200, _))),
        &got,
    );

    let got = client::get(&fixture, Site::Idp, "/getter");
    check(
        failures,
        "the fixture CA verifies the localhost leaf",
        matches!(got, Ok((200, _))),
        &got,
    );

    let got = client::get(&fixture, Site::BadCert, "/getter");
    let unknown_issuer = matches!(&got, Err(e) if e.to_string().contains("UnknownIssuer"));
    check(
        failures,
        "the bad certificate site fails verification",
        unknown_issuer,
        &got,
    );

    let to = fixture.url(Site::Foreign, "/getter", &[]);
    let got = client::get(
        &fixture,
        Site::App,
        &format!("/redirect?to={}", webview::pages::encode(&to)),
    );
    let location = format!("Location: {to}");
    let redirected = matches!(&got, Ok((302, head)) if head.lines().any(|line| line == location));
    check(failures, "a redirect names its target", redirected, &got);

    let (tx, rx) = mpsc::channel();
    let port = fixture.port(Site::App);
    std::thread::scope(|scope| {
        let fixture = &fixture;
        scope.spawn(move || {
            let _ = tx.send(client::get(fixture, Site::App, "/slow?key=k1"));
        });
        let logged = fixture.wait_request(Site::App, "/slow", Duration::from_secs(10));
        // The server logged the request; with k1 unreleased it must not have
        // answered.
        let early = rx.try_recv();
        check(
            failures,
            "a held response waits for its key",
            logged.is_some() && early.is_err(),
            (port, &logged, &early),
        );
        fixture.release("k1");
        let got = rx.recv_timeout(Duration::from_secs(10));
        check(
            failures,
            "releasing the key answers the held response",
            matches!(got, Ok(Ok((200, _)))),
            &got,
        );
    });
}
