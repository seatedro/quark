//! Builds libghostty-vt from a pinned Ghostty commit with Zig and links it
//! statically.
//!
//! Downloads go through `curl`, which uses the system's CA bundle,
//! `SSL_CERT_FILE`, and proxy settings; Zig's own HTTP client reads only
//! fixed CA paths and fails on some systems (NixOS among them). build.rs
//! downloads Ghostty's source tarball, reads its `build.zig.zon` files, and
//! downloads every dependency the libghostty-vt build needs (see
//! build/ghostty_deps.rs). `zig fetch <file>` unpacks each one into a local
//! package directory and prints its Zig package hash (a SHA-256 over the
//! unpacked files), which must equal the pinned hash. `zig build --system`
//! then runs on that directory with fetching disabled, so a missing package
//! is an error, never a download.
//!
//! Ghostty's build runs with `-Demit-lib-vt`, which installs one static
//! archive with its SIMD dependencies (simdutf, highway) and compiler-rt
//! folded in; it needs only libc at link time. On ELF targets build.rs
//! then renames compiler-rt's copies of C library functions in the archive
//! so programs use the C library's (see build/compiler_rt.rs); Ghostty's
//! build does the same for macOS.
//!
//! The archive is cached in the target directory per commit, Zig target,
//! optimize mode, Zig version, and (for MSVC) C runtime, so feature changes
//! that move `OUT_DIR` do not rebuild it. Each build installs into a
//! temporary prefix that is renamed into place, so an interrupted build
//! never leaves a partial cache entry. Downloads are kept in
//! `<target>/<profile>/ghostty-vt/downloads`.
//!
//! The archive is `libghostty-vt.a` on Unix. On Windows (x86-64 MSVC only)
//! it is the COFF archive `ghostty-vt-static.lib`, named apart from the DLL
//! import library `ghostty-vt.lib` that the same build installs; Zig's
//! standard library in it also needs ntdll and kernel32. Zig emits no
//! `/DEFAULTLIB` directives and compiles without `_DLL`, so its objects
//! call the C runtime directly and link against either the DLL or the
//! static CRT.
//!
//! Environment:
//! - `ZIG`: the Zig binary (default `zig` on PATH). Ghostty needs 0.16.
//! - `CURL`: the curl binary (default `curl` on PATH).
//! - `QUARK_GHOSTTY_VT_SOURCE_DIR`: a directory of pre-fetched packages,
//!   used instead of downloading (for offline and Nix builds). Each package
//!   is a file or directory named either by its Zig package hash or by its
//!   URL's last path segment (a `downloads` directory from another build
//!   works). Hashes are still checked.
//! - `QUARK_GHOSTTY_VT_LIB_DIR`: a directory holding a prebuilt archive
//!   (`libghostty-vt.a`, or `ghostty-vt-static.lib` on Windows), used
//!   instead of building (required when the `zig-build` feature is off).
//! - `QUARK_GHOSTTY_VT_OPTIMIZE`: Zig optimize mode (default ReleaseFast).
//!   The fuzz CI job uses ReleaseSafe, so Zig's own checks (bounds,
//!   overflow, unreachable code) abort on a violation; Rust's sanitizers
//!   and libFuzzer's coverage never instrument the Zig code.
//!
//! There is no build without the VT: a target this script cannot build for
//! (Windows other than x86-64 MSVC) fails here unless
//! `QUARK_GHOSTTY_VT_LIB_DIR` supplies the archive. On Windows, Zig finds
//! the MSVC headers and Windows SDK through the Visual Studio installation.

#[path = "build/compiler_rt.rs"]
mod compiler_rt;
#[path = "build/ghostty_deps.rs"]
mod ghostty_deps;

use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use ghostty_deps::{Source, needed, parse_dependencies};

/// The Ghostty commit libghostty-vt is built from. Moving it means
/// updating the hash below (`zig fetch URL` prints it), checking
/// ghostty_deps.rs's lazy dependency lists, and regenerating src/sys.rs
/// with scripts/bindgen.sh.
const GHOSTTY_COMMIT: &str = "81681158b1f04b9900c3e58ba6db790384f5b6f5";
const GHOSTTY_HASH: &str = "ghostty-1.3.2-dev-5UdBC4gaYwVruUDKYxdTyGQF6L_6LjdKdJCbx7L4iR5V";

/// Part of the cache key; bump it when build.rs changes the archive it
/// installs without changing anything else in the key.
const ARCHIVE_REVISION: u32 = 1;

/// What to tell a user whose build failed.
const HELP: &str = "To build without network access, set QUARK_GHOSTTY_VT_SOURCE_DIR to a \
directory of pre-fetched packages, or QUARK_GHOSTTY_VT_LIB_DIR to a directory holding a \
prebuilt archive (see crates/quark-terminal/build.rs)";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build/compiler_rt.rs");
    println!("cargo:rerun-if-changed=build/ghostty_deps.rs");
    for var in [
        "ZIG",
        "CURL",
        "QUARK_GHOSTTY_VT_SOURCE_DIR",
        "QUARK_GHOSTTY_VT_LIB_DIR",
        "QUARK_GHOSTTY_VT_OPTIMIZE",
    ] {
        println!("cargo:rerun-if-env-changed={var}");
    }
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let windows = os == "windows";
    let (archive, link_name) = if windows {
        ("ghostty-vt-static.lib", "ghostty-vt-static")
    } else {
        ("libghostty-vt.a", "ghostty-vt")
    };

    let lib_dir = match env::var_os("QUARK_GHOSTTY_VT_LIB_DIR") {
        Some(dir) => PathBuf::from(dir),
        None if cfg!(feature = "zig-build") => build_with_zig(&os, archive),
        None => panic!(
            "quark-terminal: the zig-build feature is off; set QUARK_GHOSTTY_VT_LIB_DIR \
             to a directory holding {archive}"
        ),
    };
    assert!(
        lib_dir.join(archive).is_file(),
        "quark-terminal: no {archive} in {}",
        lib_dir.display()
    );
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    // `static=` so the shared library installed next to it is never picked.
    println!("cargo:rustc-link-lib=static={link_name}");
    if windows {
        for lib in ["ntdll", "kernel32"] {
            println!("cargo:rustc-link-lib=dylib={lib}");
        }
    }
}

/// Builds (or reuses) the archive and returns the directory holding it.
fn build_with_zig(os: &str, archive: &str) -> PathBuf {
    let zig = env::var("ZIG").unwrap_or_else(|_| "zig".to_owned());
    let optimize = env::var("QUARK_GHOSTTY_VT_OPTIMIZE").unwrap_or_else(|_| "ReleaseFast".into());
    let target = zig_target(os);
    let zig_version = zig_version(&zig);
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    // OUT_DIR is <target>/<profile>/build/<pkg>-<hash>/out.
    let cache_root = out
        .ancestors()
        .nth(3)
        .map_or_else(|| out.clone(), Path::to_path_buf)
        .join("ghostty-vt");
    let short = &GHOSTTY_COMMIT[..12];
    let mut key = format!("{short}-{target}-{optimize}-zig{zig_version}-r{ARCHIVE_REVISION}");
    if target.ends_with("-msvc") {
        // Rust picks the C runtime per build (`+crt-static`). The archive
        // does not depend on it today (see the top of this file), but an
        // archive built for one CRT is never reused for the other.
        let features = env::var("CARGO_CFG_TARGET_FEATURE").unwrap_or_default();
        let crt_static = features.split(',').any(|f| f == "crt-static");
        key.push_str(if crt_static {
            "-crt-static"
        } else {
            "-crt-dll"
        });
    }
    let prefix = cache_root.join(&key);
    let lib_dir = prefix.join("lib");
    if lib_dir.join(archive).is_file() {
        return lib_dir;
    }

    let mut fetcher = Fetcher::new(zig.clone(), &cache_root, short);
    let source = fetcher.package(
        &format!("https://github.com/ghostty-org/ghostty/archive/{GHOSTTY_COMMIT}.tar.gz"),
        GHOSTTY_HASH,
    );
    fetcher.dependencies_of(&source, os);

    // Install beside the cache entry, then rename it into place. The
    // installed pkg-config files name this temporary prefix; nothing here
    // reads them.
    let staging = cache_root.join(format!("{key}.partial-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    let mut build = Command::new(&zig);
    build
        .arg("build")
        .arg("--build-file")
        .arg(source.join("build.zig"))
        .arg("--cache-dir")
        .arg(cache_root.join("zig-cache"))
        .arg("--prefix")
        .arg(&staging)
        // Packages come only from this directory; Zig never downloads.
        // --system also turns on Ghostty's system library integrations by
        // default, so keep the SIMD libraries bundled.
        .arg("--system")
        .arg(&fetcher.packages)
        .args(["-fno-sys=simdutf", "-fno-sys=highway"])
        .arg("-Demit-lib-vt")
        .arg("-Demit-themes=false")
        .arg(format!("-Dtarget={target}"))
        // Portable machine code: the build host's CPU is not the user's.
        .arg("-Dcpu=baseline")
        .arg(format!("-Doptimize={optimize}"));
    run(
        &mut build,
        "If Zig reports a missing package, the lazy dependency lists in \
         crates/quark-terminal/build/ghostty_deps.rs need it added.",
    );
    assert!(
        staging.join("lib").join(archive).is_file(),
        "quark-terminal: zig build installed no lib/{archive} in {}",
        staging.display()
    );
    if !matches!(os, "windows" | "macos" | "ios") {
        compiler_rt::prefer_libc(&staging.join("lib").join(archive), &zig);
    }
    // A leftover entry without the archive (from before installs were
    // atomic) would block the rename.
    if !lib_dir.join(archive).is_file() {
        let _ = std::fs::remove_dir_all(&prefix);
    }
    if let Err(e) = std::fs::rename(&staging, &prefix) {
        // Another build finished the same entry first; use theirs.
        assert!(
            lib_dir.join(archive).is_file(),
            "quark-terminal: rename {} to {}: {e}",
            staging.display(),
            prefix.display()
        );
        let _ = std::fs::remove_dir_all(&staging);
    }
    lib_dir
}

/// `zig version`'s output, for the cache key.
fn zig_version(zig: &str) -> String {
    let mut command = Command::new(zig);
    command.arg("version");
    let output = command
        .output()
        .unwrap_or_else(|e| missing_tool(&command, &e));
    assert!(
        output.status.success(),
        "quark-terminal: {command:?} failed: {}",
        output.status
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Downloads and verifies Zig packages into one package directory.
struct Fetcher {
    zig: String,
    curl: String,
    /// Holds a stub build.zig: `zig fetch` unpacks into the `zig-pkg`
    /// directory next to the nearest build.zig.
    project: PathBuf,
    /// `<project>/zig-pkg`: one directory per package, named by hash.
    packages: PathBuf,
    downloads: PathBuf,
    source_dir: Option<PathBuf>,
    visited: HashSet<PathBuf>,
}

impl Fetcher {
    fn new(zig: String, cache_root: &Path, short: &str) -> Self {
        let project = cache_root.join(format!("{short}-src"));
        let downloads = cache_root.join("downloads");
        for dir in [&project, &downloads] {
            std::fs::create_dir_all(dir)
                .unwrap_or_else(|e| panic!("quark-terminal: create {}: {e}", dir.display()));
        }
        std::fs::write(
            project.join("build.zig"),
            "pub fn build(b: *@import(\"std\").Build) void {\n    _ = b;\n}\n",
        )
        .expect("write build.zig");
        Self {
            zig,
            curl: env::var("CURL").unwrap_or_else(|_| "curl".to_owned()),
            packages: project.join("zig-pkg"),
            project,
            downloads,
            source_dir: env::var_os("QUARK_GHOSTTY_VT_SOURCE_DIR").map(PathBuf::from),
            visited: HashSet::new(),
        }
    }

    /// Fetches the needed dependencies of the package at `dir`, and theirs.
    fn dependencies_of(&mut self, dir: &Path, os: &str) {
        if !self.visited.insert(dir.to_path_buf()) {
            return;
        }
        let manifest = dir.join("build.zig.zon");
        let Ok(zon) = std::fs::read_to_string(&manifest) else {
            // A package without a manifest has no dependencies.
            return;
        };
        let deps = parse_dependencies(&zon)
            .unwrap_or_else(|e| panic!("quark-terminal: parse {}: {e}", manifest.display()));
        for dep in deps.iter().filter(|d| needed(d, os)) {
            let dir = match &dep.source {
                Source::Url { url, hash } => self.package(url, hash),
                Source::Path(path) => dir.join(path),
            };
            self.dependencies_of(&dir, os);
        }
    }

    /// The unpacked package with Zig package hash `hash`, fetching `url`
    /// (or taking it from QUARK_GHOSTTY_VT_SOURCE_DIR) when it is missing.
    fn package(&mut self, url: &str, hash: &str) -> PathBuf {
        let dir = self.packages.join(hash);
        // `zig fetch` names the directory by the hash it computed, so an
        // existing directory was verified when it was made.
        if dir.is_dir() {
            return dir;
        }
        let file_name = url.rsplit('/').next().unwrap_or(hash);
        let local = match &self.source_dir {
            Some(src) => [src.join(hash), src.join(file_name)]
                .into_iter()
                .find(|p| p.exists())
                .unwrap_or_else(|| {
                    panic!(
                        "quark-terminal: QUARK_GHOSTTY_VT_SOURCE_DIR ({}) has neither {hash} \
                         nor {file_name}; download {url} into it",
                        src.display()
                    )
                }),
            None => self.download(url, file_name),
        };

        let mut fetch = Command::new(&self.zig);
        fetch.arg("fetch").arg(&local).current_dir(&self.project);
        let output = fetch.output().unwrap_or_else(|e| missing_tool(&fetch, &e));
        let got = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        assert!(
            output.status.success(),
            "quark-terminal: {fetch:?} failed: {}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        if got != hash {
            // A cached download that does not match is useless; drop it so
            // the next build downloads again.
            if self.source_dir.is_none() {
                let _ = std::fs::remove_file(&local);
            }
            panic!(
                "quark-terminal: {} (from {url}) has Zig package hash {got}, expected {hash}",
                local.display()
            );
        }
        dir
    }

    fn download(&self, url: &str, file_name: &str) -> PathBuf {
        let dest = self.downloads.join(file_name);
        if dest.is_file() {
            return dest;
        }
        // Download beside the destination and rename, so an interrupted
        // download never looks complete.
        let part = self.downloads.join(format!("{file_name}.part"));
        let mut curl = Command::new(&self.curl);
        curl.args(["--fail", "--location", "--silent", "--show-error"])
            .args(["--retry", "5", "--retry-delay", "2"])
            .args(["--connect-timeout", "30"])
            .arg("--output")
            .arg(&part)
            .arg(url);
        let status = curl.status().unwrap_or_else(|e| missing_tool(&curl, &e));
        assert!(
            status.success(),
            "quark-terminal: downloading {url} with curl failed: {status}. Check the network, \
             proxy (https_proxy), and CA bundle (SSL_CERT_FILE). {HELP}."
        );
        std::fs::rename(&part, &dest)
            .unwrap_or_else(|e| panic!("quark-terminal: rename {}: {e}", part.display()));
        dest
    }
}

/// The Zig target triple for Cargo's target.
fn zig_target(os: &str) -> String {
    let arch = env::var("CARGO_CFG_TARGET_ARCH").expect("target arch");
    let abi = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    match (os, abi.as_str()) {
        // Only x86-64 MSVC has been built and linked; ARM64 and the GNU
        // ABI need their own verification first.
        ("windows", "msvc") if arch == "x86_64" => "x86_64-windows-msvc".to_owned(),
        ("windows", _) => panic!(
            "quark-terminal: libghostty-vt is built only for x86_64-pc-windows-msvc on \
             Windows, not {arch}-{abi}. Set QUARK_GHOSTTY_VT_LIB_DIR to a directory holding \
             a ghostty-vt-static.lib built for this target to use it anyway."
        ),
        ("macos", _) => format!("{arch}-macos"),
        ("linux", "musl") => format!("{arch}-linux-musl"),
        ("linux", _) => format!("{arch}-linux-gnu"),
        (os, "") => format!("{arch}-{os}"),
        (os, abi) => format!("{arch}-{os}-{abi}"),
    }
}

fn run(command: &mut Command, hint: &str) {
    let status = command
        .status()
        .unwrap_or_else(|e| missing_tool(command, &e));
    assert!(
        status.success(),
        "quark-terminal: {command:?} failed: {status}. {hint} {HELP}."
    );
}

fn missing_tool(command: &Command, err: &std::io::Error) -> ! {
    panic!(
        "quark-terminal: could not run {:?} ({err}). libghostty-vt is built with Zig 0.16 \
         (set ZIG to its path) from sources downloaded with curl (set CURL). {HELP}.",
        command.get_program()
    )
}
