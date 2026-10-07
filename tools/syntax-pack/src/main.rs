//! Builds quark-syntax grammar packs and their signed index.
//!
//! ```text
//! syntax-pack build [LANGUAGE...] [--target TRIPLE] [--out DIR] [--sources DIR] [--work DIR]
//!     Fetch each language's pinned source (grammars.toml), check its hash,
//!     compile the parser into a shared library for TRIPLE (default: this
//!     host), and write the pack to <out>/<triple>/<language>/. With no
//!     languages, builds them all. --sources DIR reads <DIR>/<language>/
//!     instead of fetching (offline and Nix builds); the hash still applies.
//!     Packs for this host are loaded and their query compiled as a check.
//! syntax-pack index [--target TRIPLE] [--out DIR]
//!     Sign <out>/<triple>/index.json listing every pack under
//!     <out>/<triple>/, with the Ed25519 seed in $QUARK_SYNTAX_SIGNING_KEY
//!     (64 hex digits; make one with
//!     `cargo run -p quark-update --example manifest_tool keygen`).
//! syntax-pack public-key
//!     Print the public key for $QUARK_SYNTAX_SIGNING_KEY, for the app.
//! syntax-pack list
//!     Print the pinned languages.
//! ```
//!
//! `--out` defaults to `<workspace>/target/syntax-packs`, which quark's
//! tests and examples read; `--work` (source checkouts and objects) to
//! `<workspace>/target/syntax-pack-work`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use quark_syntax::pack::{self, PackFile, PackManifest, PackSource};
use serde::Deserialize;
use serde_json::json;

const HOST: &str = env!("SYNTAX_PACK_HOST");
const KEY_VAR: &str = "QUARK_SYNTAX_SIGNING_KEY";
const LOCK: &str = include_str!("../grammars.toml");

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Grammar {
    repo: String,
    rev: String,
    version: String,
    #[serde(default = "default_src")]
    src: String,
    symbol: Option<String>,
    #[serde(default)]
    include: Vec<String>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    extensions: Vec<String>,
    highlights: Vec<String>,
    #[serde(default)]
    injections: Vec<String>,
    sha256: String,
}

fn default_src() -> String {
    "src".to_owned()
}

type Lock = BTreeMap<String, Grammar>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("syntax-pack: {error}");
            ExitCode::FAILURE
        }
    }
}

struct Options {
    languages: Vec<String>,
    target: String,
    out: PathBuf,
    work: PathBuf,
    sources: Option<PathBuf>,
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn parse_options(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        languages: Vec::new(),
        target: HOST.to_owned(),
        out: workspace().join("target/syntax-packs"),
        work: workspace().join("target/syntax-pack-work"),
        sources: None,
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--target" => options.target = value()?,
            "--out" => options.out = value()?.into(),
            "--work" => options.work = value()?.into(),
            "--sources" => options.sources = Some(value()?.into()),
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            language => options.languages.push(language.to_owned()),
        }
    }
    Ok(options)
}

fn run(args: &[String]) -> Result<(), String> {
    let lock: Lock = toml::from_str(LOCK).map_err(|e| format!("grammars.toml: {e}"))?;
    match args.first().map(String::as_str) {
        Some("build") => {
            let mut options = parse_options(&args[1..])?;
            if options.languages.is_empty() {
                options.languages = lock.keys().cloned().collect();
            }
            for language in &options.languages {
                build(&lock, language, &options)?;
            }
            Ok(())
        }
        Some("index") => index(&parse_options(&args[1..])?),
        Some("public-key") => {
            let key =
                quark_update::manifest::public_key(&signing_seed()?).map_err(|e| e.to_string())?;
            println!("{}", key.to_hex());
            Ok(())
        }
        Some("list") => {
            for (language, grammar) in &lock {
                println!(
                    "{language} {} {}@{}",
                    grammar.version, grammar.repo, grammar.rev
                );
            }
            Ok(())
        }
        _ => Err("usage: syntax-pack build|index|public-key|list (see src/main.rs)".to_owned()),
    }
}

// ---- Sources ------------------------------------------------------------

/// The checkout of `language`'s pinned source.
fn checkout(lock: &Lock, language: &str, options: &Options) -> Result<PathBuf, String> {
    let grammar = lock
        .get(language)
        .ok_or_else(|| format!("{language} is not in grammars.toml"))?;
    if let Some(sources) = &options.sources {
        return Ok(sources.join(language));
    }
    let dir = options
        .work
        .join("sources")
        .join(format!("{language}-{}", grammar.rev));
    if git(&dir, &["rev-parse", "HEAD"]).is_ok_and(|head| head.trim() == grammar.rev) {
        return Ok(dir);
    }
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    eprintln!("fetching {language} {} at {}", grammar.repo, grammar.rev);
    git(&dir, &["init", "--quiet"])?;
    git(
        &dir,
        &[
            "fetch",
            "--quiet",
            "--depth",
            "1",
            &grammar.repo,
            &grammar.rev,
        ],
    )?;
    git(&dir, &["checkout", "--quiet", "--detach", "FETCH_HEAD"])?;
    Ok(dir)
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A file the build reads: its hashed name and its path on disk.
/// `@<language>/<path>` names a file in another entry's checkout.
fn resolve_input(
    lock: &Lock,
    root: &Path,
    name: &str,
    options: &Options,
) -> Result<(String, PathBuf), String> {
    match name.strip_prefix('@') {
        Some(rest) => {
            let (language, path) = rest
                .split_once('/')
                .ok_or_else(|| format!("bad input {name}"))?;
            Ok((
                name.to_owned(),
                checkout(lock, language, options)?.join(path),
            ))
        }
        None => Ok((name.to_owned(), root.join(name))),
    }
}

/// Every C source and header under `src`, relative to the repository.
fn source_files(root: &Path, src: &str) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    let mut stack = vec![PathBuf::from(src)];
    while let Some(dir) = stack.pop() {
        let entries =
            fs::read_dir(root.join(&dir)).map_err(|e| format!("{}: {e}", dir.display()))?;
        for entry in entries.flatten() {
            let rel = dir.join(entry.file_name());
            if entry.path().is_dir() {
                stack.push(rel);
            } else if rel
                .extension()
                .is_some_and(|ext| ext == "c" || ext == "h" || ext == "cc")
            {
                files.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    Ok(files)
}

fn sha256(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// SHA-256 over `<name>\0<sha256>\n` for every input, sorted by name.
fn source_hash(inputs: &[(String, PathBuf)]) -> Result<String, String> {
    let mut lines = Vec::new();
    for (name, path) in inputs {
        let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        lines.push(format!("{name}\0{}\n", sha256(&bytes)));
    }
    lines.sort();
    Ok(sha256(lines.concat().as_bytes()))
}

// ---- Build --------------------------------------------------------------

fn build(lock: &Lock, language: &str, options: &Options) -> Result<(), String> {
    let grammar = lock
        .get(language)
        .ok_or_else(|| format!("{language} is not in grammars.toml"))?;
    let root = checkout(lock, language, options)?;

    let mut inputs = Vec::new();
    for name in source_files(&root, &grammar.src)?
        .iter()
        .chain(&grammar.include)
        .chain(&grammar.highlights)
        .chain(&grammar.injections)
    {
        inputs.push(resolve_input(lock, &root, name, options)?);
    }
    let hash = source_hash(&inputs)?;
    if hash != grammar.sha256 {
        return Err(format!(
            "{language}: sources hash to {hash}, grammars.toml pins {:?}",
            grammar.sha256
        ));
    }

    let src = root.join(&grammar.src);
    let parser_c = fs::read_to_string(src.join("parser.c"))
        .map_err(|e| format!("{language}: parser.c: {e}"))?;
    let abi = parser_c
        .lines()
        .find_map(|line| line.strip_prefix("#define LANGUAGE_VERSION "))
        .and_then(|v| v.trim().parse::<u32>().ok())
        .ok_or_else(|| format!("{language}: no LANGUAGE_VERSION in parser.c"))?;
    if src.join("scanner.cc").exists() {
        return Err(format!("{language}: C++ scanners are not supported"));
    }

    let target = &options.target;
    let out = options.out.join(target).join(language);
    let _ = fs::remove_dir_all(&out);
    fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let library_name = match pack::library_suffix(target) {
        ".dll" => format!("tree-sitter-{language}.dll"),
        suffix => format!("libtree-sitter-{language}{suffix}"),
    };
    let objects = options.work.join("objects").join(target).join(language);
    eprintln!("compiling {language} for {target}");
    compile(&src, target, &objects, &out.join(&library_name))?;

    let write_query = |name: &str, files: &[String]| -> Result<Option<PackFile>, String> {
        if files.is_empty() {
            return Ok(None);
        }
        let mut text = String::new();
        for file in files {
            let (_, path) = resolve_input(lock, &root, file, options)?;
            text.push_str(
                &fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?,
            );
            text.push('\n');
        }
        fs::write(out.join(name), &text).map_err(|e| e.to_string())?;
        Ok(Some(pack_file(&out, name)?))
    };
    let highlights = write_query("highlights.scm", &grammar.highlights)?
        .ok_or_else(|| format!("{language}: no highlight query"))?;
    let injections = write_query("injections.scm", &grammar.injections)?;

    let manifest = PackManifest {
        schema: pack::SCHEMA,
        language: language.to_owned(),
        aliases: grammar.aliases.clone(),
        extensions: grammar.extensions.clone(),
        version: grammar.version.clone(),
        target: target.clone(),
        abi,
        symbol: grammar
            .symbol
            .clone()
            .unwrap_or_else(|| format!("tree_sitter_{language}")),
        library: pack_file(&out, &library_name)?,
        highlights,
        injections,
        source: Some(PackSource {
            repo: grammar.repo.clone(),
            rev: grammar.rev.clone(),
            sha256: hash,
        }),
    };
    manifest
        .validate(target)
        .map_err(|e| format!("{language}: {e}"))?;
    let json = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    fs::write(out.join(pack::MANIFEST), json + "\n").map_err(|e| e.to_string())?;

    if target == HOST {
        pack::check_pack_dir(&out)
            .map_err(|e| format!("{language}: built pack does not load: {e}"))?;
    }
    println!(
        "{} ({} bytes)",
        out.join(&library_name).display(),
        manifest.library.size
    );
    Ok(())
}

fn pack_file(dir: &Path, name: &str) -> Result<PackFile, String> {
    let bytes = fs::read(dir.join(name)).map_err(|e| format!("{name}: {e}"))?;
    Ok(PackFile {
        path: name.to_owned(),
        sha256: sha256(&bytes),
        size: bytes.len() as u64,
        url: None,
    })
}

/// Compiles parser.c and scanner.c in `src` into the shared library
/// `output` for `target`.
fn compile(src: &Path, target: &str, objects: &Path, output: &Path) -> Result<(), String> {
    let _ = fs::remove_dir_all(objects);
    fs::create_dir_all(objects).map_err(|e| e.to_string())?;
    let mut build = cc::Build::new();
    build
        .cargo_metadata(false)
        .cargo_warnings(false)
        .warnings(false)
        .target(target)
        .host(HOST)
        .opt_level(2)
        .debug(false)
        .out_dir(objects)
        .include(src)
        .file(src.join("parser.c"));
    if src.join("scanner.c").exists() {
        build.file(src.join("scanner.c"));
    }
    let compiler = build.try_get_compiler().map_err(|e| e.to_string())?;
    if !compiler.is_like_msvc() {
        // parser.c marks only the language function visible, so this keeps
        // every other symbol out of the library's export table.
        build.flag("-fvisibility=hidden").flag("-std=c11");
    }
    let objs = build
        .try_compile_intermediates()
        .map_err(|e| e.to_string())?;

    let mut link = if compiler.is_like_msvc() {
        let mut link = cc::windows_registry::find_tool(target, "link.exe")
            .ok_or("link.exe not found; run from a Visual Studio developer shell")?
            .to_command();
        link.args(["/NOLOGO", "/DLL"])
            .arg(format!("/OUT:{}", output.display()))
            .arg(format!("/IMPLIB:{}", objects.join("parser.lib").display()));
        link
    } else {
        let mut link = compiler.to_command();
        if target.contains("apple") {
            link.args(["-dynamiclib", "-Wl,-dead_strip", "-Wl,-x"]);
        } else {
            link.args(["-shared", "-s"]);
        }
        link.arg("-o").arg(output);
        link
    };
    link.args(&objs);
    let status = link.status().map_err(|e| format!("linker: {e}"))?;
    if !status.success() {
        return Err(format!("linking {} failed: {status}", output.display()));
    }
    Ok(())
}

// ---- Index --------------------------------------------------------------

fn signing_seed() -> Result<[u8; 32], String> {
    let text = std::env::var(KEY_VAR).map_err(|_| format!("${KEY_VAR} is not set"))?;
    let text = text.trim();
    let bytes: Vec<u8> = (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2).unwrap_or("x"), 16))
        .collect::<Result<_, _>>()
        .map_err(|_| format!("${KEY_VAR} is not hex"))?;
    bytes
        .try_into()
        .map_err(|_| format!("${KEY_VAR} is not 32 bytes"))
}

fn index(options: &Options) -> Result<(), String> {
    let seed = signing_seed()?;
    let dir = options.out.join(&options.target);
    let mut packs = Vec::new();
    let entries = fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    dirs.sort();
    for pack_dir in dirs {
        let path = pack_dir.join(pack::MANIFEST);
        if !path.is_file() {
            continue;
        }
        let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let manifest: PackManifest =
            serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        manifest
            .validate(&options.target)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        // The index is the only thing signed, so check the files it vouches
        // for are the ones being published.
        pack::verify_files(&pack_dir, &manifest).map_err(|e| e.to_string())?;
        packs.push(manifest);
    }
    if packs.is_empty() {
        return Err(format!("no packs under {}", dir.display()));
    }
    let payload = json!({
        "schema": pack::SCHEMA,
        "target": options.target,
        "packs": packs,
    });
    let document = quark_update::manifest::sign(&seed, &payload).map_err(|e| e.to_string())?;
    let key = quark_update::manifest::public_key(&seed).map_err(|e| e.to_string())?;
    let key: [u8; 32] = (0..32)
        .map(|i| u8::from_str_radix(&key.to_hex()[i * 2..i * 2 + 2], 16).unwrap_or(0))
        .collect::<Vec<_>>()
        .try_into()
        .unwrap_or([0; 32]);
    pack::verify_index(document.as_bytes(), &[key], &options.target)
        .map_err(|e| format!("the signed index does not verify: {e}"))?;
    let path = dir.join("index.json");
    fs::write(&path, document + "\n").map_err(|e| e.to_string())?;
    println!("{} ({} packs)", path.display(), packs.len());
    Ok(())
}
