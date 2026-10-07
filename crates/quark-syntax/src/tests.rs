use super::*;

/// `kind:text` for every span whose kind is in `kinds`, in source order.
fn dump(spans: &[HighlightSpan], source: &str, kinds: &[HighlightKind]) -> String {
    spans
        .iter()
        .filter(|span| kinds.contains(&span.kind))
        .map(|span| format!("{}:{}", span.kind.name(), &source[span.range()]))
        .collect::<Vec<_>>()
        .join(" ")
}

fn language(tag: &str) -> LanguageId {
    LanguageId::from_fence(tag).unwrap()
}

const RUST: &str = "fn main() {\n    let greeting = \"hi\";\n    if true { return; }\n}\n";
const RUST_DUMP: &str = "keyword:fn keyword:let string:\"hi\" keyword:if keyword:return";

// Catches a pack the tool builds failing to load, its query failing to
// compile against its grammar, or an alias not reaching it.
#[test]
fn rust_pack_built_by_the_tool_highlights_keywords_and_strings() {
    let Some(store) = testing::store_with("rust") else {
        return;
    };

    let dumped = dump(
        &highlight(&store, &language("rs"), RUST),
        RUST,
        &[HighlightKind::Keyword, HighlightKind::String],
    );

    assert_eq!(dumped, RUST_DUMP);
}

#[cfg(feature = "download")]
mod download {
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{Receiver, channel};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::Duration;

    use serde_json::{Value, json};

    use super::*;
    use crate::pack::{self, PackError, PackManifest, verify_index};

    const SEED: [u8; 32] = [7; 32];
    const OTHER_SEED: [u8; 32] = [9; 32];

    fn key(seed: &[u8; 32]) -> PublicKey {
        quark_update::manifest::public_key(seed).unwrap()
    }

    fn key_bytes(seed: &[u8; 32]) -> [u8; 32] {
        pack::unhex(&key(seed).to_hex())
            .unwrap()
            .try_into()
            .unwrap()
    }

    fn sign(payload: &Value) -> String {
        quark_update::manifest::sign(&SEED, payload).unwrap()
    }

    fn sha(bytes: &[u8]) -> String {
        pack::hex(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
    }

    fn file(path: &str, bytes: &[u8]) -> Value {
        json!({ "path": path, "sha256": sha(bytes), "size": bytes.len() })
    }

    /// A manifest for `language` whose files hold the given bytes.
    fn manifest(language: &str, library: &[u8], highlights: &[u8]) -> Value {
        json!({
            "schema": 1,
            "language": language,
            "aliases": ["rs"],
            "extensions": ["rs"],
            "version": "1.0.0",
            "target": pack::TARGET,
            "abi": 14,
            "symbol": "tree_sitter_rust",
            "library": file(&format!("libtree-sitter-{language}{}", std::env::consts::DLL_SUFFIX), library),
            "highlights": file("highlights.scm", highlights),
        })
    }

    fn index(packs: Vec<Value>) -> Value {
        json!({ "schema": 1, "target": pack::TARGET, "packs": packs })
    }

    /// The verified languages, or the error, as one line.
    fn outcome(document: &str, seed: &[u8; 32]) -> String {
        match verify_index(document.as_bytes(), &[key_bytes(seed)], pack::TARGET) {
            Ok(index) => {
                let mut parts: Vec<String> =
                    index.packs.iter().map(|p| p.language.clone()).collect();
                parts.extend(index.rejected.iter().map(|(language, error)| {
                    let kind = match error {
                        PackError::Abi { abi, .. } => format!("abi {abi}"),
                        PackError::Unsafe { field, .. } => format!("unsafe {field}"),
                        other => other.to_string(),
                    };
                    format!("rejected {language}: {kind}")
                }));
                parts.join(", ")
            }
            Err(PackError::BadSignature) => "bad signature".to_owned(),
            Err(PackError::WrongTarget { found, .. }) => format!("wrong target {found}"),
            Err(error) => error.to_string(),
        }
    }

    // Catches an index accepted without a valid signature from a trusted
    // key, for another platform, or with entries the loader must not act
    // on.
    #[test]
    fn index_verification_table() {
        let good = || manifest("rust", b"lib", b"(identifier) @variable");
        let with = |field: &str, value: Value| {
            let mut pack = good();
            pack[field] = value;
            sign(&index(vec![pack]))
        };
        let tampered = {
            let mut document: Value = serde_json::from_str(&sign(&index(vec![good()]))).unwrap();
            document["payload"]["packs"][0]["symbol"] = json!("tree_sitter_evil");
            document.to_string()
        };
        let mut other_target = index(vec![good()]);
        other_target["target"] = json!("riscv64gc-unknown-none-elf");

        let cases = [
            ("valid", sign(&index(vec![good()])), SEED, "rust"),
            ("tampered", tampered, SEED, "bad signature"),
            (
                "wrong key",
                sign(&index(vec![good()])),
                OTHER_SEED,
                "bad signature",
            ),
            (
                "wrong platform",
                sign(&other_target),
                SEED,
                "wrong target riscv64gc-unknown-none-elf",
            ),
            (
                "abi mismatch",
                with("abi", json!(99)),
                SEED,
                "rejected rust: abi 99",
            ),
            (
                "path traversal",
                with("library", file("../libevil.so", b"lib")),
                SEED,
                "rejected rust: unsafe library",
            ),
            (
                "symbol injection",
                with("symbol", json!("tree_sitter_rust; rm")),
                SEED,
                "rejected rust: unsafe symbol",
            ),
        ];
        for (name, document, seed, expected) in cases {
            assert_eq!(outcome(&document, &seed), expected, "{name}");
        }
    }

    /// Serves `files` by path over HTTP/1.1 on localhost, one request per
    /// connection, and logs each request's path. Responses wait while a
    /// test holds `hold`.
    struct Server {
        url: String,
        files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
        hold: Arc<Mutex<()>>,
        log: Arc<Mutex<Vec<String>>>,
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl Server {
        fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let files: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::default();
            let log: Arc<Mutex<Vec<String>>> = Arc::default();
            let hold: Arc<Mutex<()>> = Arc::default();
            let stop = Arc::new(AtomicBool::new(false));
            let (thread_files, thread_log, thread_hold, thread_stop) =
                (files.clone(), log.clone(), hold.clone(), stop.clone());
            let thread = std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if thread_stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let Ok(mut stream) = stream else { continue };
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut request = String::new();
                    reader.read_line(&mut request).unwrap();
                    loop {
                        let mut line = String::new();
                        reader.read_line(&mut line).unwrap();
                        if line == "\r\n" || line.is_empty() {
                            break;
                        }
                    }
                    let path = request.split(' ').nth(1).unwrap_or("").to_owned();
                    thread_log.lock().unwrap().push(path.clone());
                    drop(thread_hold.lock().unwrap());
                    let body = thread_files.lock().unwrap().get(&path).cloned();
                    let (status, body) = match body {
                        Some(body) => ("200 OK", body),
                        None => ("404 Not Found", Vec::new()),
                    };
                    let head = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&body);
                }
            });
            Self {
                url,
                files,
                hold,
                log,
                stop,
                thread: Some(thread),
            }
        }

        fn serve(&self, path: &str, bytes: Vec<u8>) {
            self.files.lock().unwrap().insert(path.to_owned(), bytes);
        }

        fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            let _ = TcpStream::connect(self.url.trim_start_matches("http://"));
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    /// A store downloading from `server`'s `/packs/{target}/index.json`
    /// into `cache`, and a channel that hears each resolution.
    fn downloading_store(server: &Server, cache: &Path) -> (GrammarStore, Receiver<()>) {
        let downloads = Downloads::new(
            "dev.quark.test",
            format!("{}/packs/{{target}}/index.json", server.url),
            &[key(&SEED)],
        )
        .cache_dir(cache);
        let store = GrammarStore::new(StoreConfig::new().downloads(downloads));
        let (sender, receiver) = channel();
        store.subscribe(move || sender.send(()).is_ok());
        (store, receiver)
    }

    fn index_path() -> String {
        format!("/packs/{}/index.json", pack::TARGET)
    }

    fn wait(resolved: &Receiver<()>) {
        resolved
            .recv_timeout(Duration::from_secs(60))
            .expect("the fetch thread resolves the language");
    }

    // Catches an unknown language costing a network round trip (or a new
    // index fetch) on every lookup instead of being remembered.
    #[test]
    fn unknown_languages_resolve_plain_with_one_index_fetch() {
        let server = Server::start();
        server.serve(&index_path(), sign(&index(vec![])).into_bytes());
        let cache = tempfile::tempdir().unwrap();
        let (store, resolved) = downloading_store(&server, cache.path());

        let first = store.status(&language("klingon"));
        wait(&resolved);
        let statuses = [
            first,
            store.status(&language("klingon")),
            store.status(&language("vulcan")),
        ];

        assert_eq!(
            (statuses, server.log()),
            (
                [
                    LanguageStatus::Pending,
                    LanguageStatus::Unavailable,
                    LanguageStatus::Unavailable
                ],
                vec![index_path()]
            )
        );
    }

    // Catches downloaded bytes reaching the cache (and dlopen) when they
    // do not match the signed index.
    #[test]
    fn pack_with_a_sha_mismatch_is_not_installed() {
        let server = Server::start();
        let pack = manifest("rust", b"the real library", b"(identifier) @variable");
        server.serve(&index_path(), sign(&index(vec![pack.clone()])).into_bytes());
        let library = pack["library"]["path"].as_str().unwrap().to_owned();
        let base = format!("/packs/{}/rust", pack::TARGET);
        server.serve(&format!("{base}/{library}"), b"a swapped library".to_vec());
        server.serve(
            &format!("{base}/highlights.scm"),
            b"(identifier) @variable".to_vec(),
        );
        let cache = tempfile::tempdir().unwrap();
        let (store, resolved) = downloading_store(&server, cache.path());

        store.status(&language("rust"));
        wait(&resolved);
        let installed = cache
            .path()
            .join(pack::TARGET)
            .join("rust/1.0.0")
            .join(&library)
            .exists();

        assert_eq!(
            (store.status(&language("rust")), installed),
            (LanguageStatus::Unavailable, false)
        );
    }

    // Catches a language looked up while the index is downloading being
    // remembered as unavailable for the session because the (still empty)
    // cache lacks it.
    #[test]
    fn language_looked_up_during_the_index_fetch_stays_pending() {
        let server = Server::start();
        let pack = manifest("go", b"a library", b"(identifier) @variable");
        server.serve(&index_path(), sign(&index(vec![pack])).into_bytes());
        let cache = tempfile::tempdir().unwrap();
        let (store, _) = downloading_store(&server, cache.path());

        let held = server.hold.lock().unwrap();
        store.status(&language("rust"));
        while server.log().is_empty() {
            std::thread::sleep(Duration::from_millis(5));
        }
        let during = store.status(&language("go"));
        drop(held);

        assert_eq!(during, LanguageStatus::Pending);
    }

    /// The tool-built pack in the test root, served under `/packs`.
    fn serve_pack(server: &Server, root: &Path) {
        let dir = root.join(pack::TARGET).join("rust");
        let manifest: PackManifest =
            serde_json::from_slice(&std::fs::read(dir.join(pack::MANIFEST)).unwrap()).unwrap();
        for file in manifest.files() {
            server.serve(
                &format!("/packs/{}/rust/{}", pack::TARGET, file.path),
                std::fs::read(dir.join(&file.path)).unwrap(),
            );
        }
        let payload = index(vec![serde_json::to_value(&manifest).unwrap()]);
        server.serve(&index_path(), sign(&payload).into_bytes());
    }

    // Catches a downloaded grammar never reaching blocks that asked for it
    // while it was on its way: the worker must answer plain at once, then
    // send the colored result for the same request.
    #[test]
    fn downloaded_pack_recolors_a_waiting_request() {
        if testing::store_with("rust").is_none() {
            return;
        }
        let server = Server::start();
        serve_pack(&server, &testing::pack_root());
        let cache = tempfile::tempdir().unwrap();
        let (store, _) = downloading_store(&server, cache.path());
        let worker = HighlightWorker::new(store);

        worker.request(1, 1, language("rust"), Arc::from(RUST));
        let first = worker.recv().unwrap();
        let second = worker.recv().unwrap();

        let kinds = [HighlightKind::Keyword, HighlightKind::String];
        assert_eq!(
            [
                (
                    first.generation,
                    first.pending,
                    dump(&first.spans, RUST, &kinds)
                ),
                (
                    second.generation,
                    second.pending,
                    dump(&second.spans, RUST, &kinds)
                ),
            ],
            [(1, true, String::new()), (1, false, RUST_DUMP.to_owned())]
        );
    }
}
