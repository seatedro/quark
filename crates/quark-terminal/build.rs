//! Builds libghostty-vt from a pinned Ghostty commit with Zig and links it
//! statically.
//!
//! The source is a Zig package dependency: a generated `build.zig.zon`
//! names the commit's tarball and its Zig package hash (a SHA-256 over the
//! unpacked files), so `zig build --fetch` refuses anything else. Ghostty's
//! own build then runs with `-Demit-lib-vt`, which installs one static
//! archive with its SIMD dependencies (simdutf, highway) and compiler-rt
//! folded in; it needs only libc at link time.
//!
//! The archive is cached in the target directory per commit, Zig target,
//! and optimize mode, so feature changes that move `OUT_DIR` do not
//! rebuild it.
//!
//! Environment:
//! - `ZIG`: the Zig binary (default `zig` on PATH). Ghostty needs 0.16.
//! - `QUARK_GHOSTTY_VT_LIB_DIR`: a directory holding a prebuilt
//!   `libghostty-vt.a`, used instead of building (required when the
//!   `zig-build` feature is off).
//! - `QUARK_GHOSTTY_VT_OPTIMIZE`: Zig optimize mode (default ReleaseFast).
//!
//! Windows is not built yet: Zig's MSVC target needs the Windows SDK and the
//! combined archive has not been verified against the MSVC linker, so the
//! crate compiles without the VT there (see lib.rs).

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The Ghostty commit libghostty-vt is built from. Moving it means
/// updating the hash below (`zig fetch URL` prints it) and regenerating
/// src/sys.rs with scripts/bindgen.sh.
const GHOSTTY_COMMIT: &str = "81681158b1f04b9900c3e58ba6db790384f5b6f5";
const GHOSTTY_HASH: &str = "ghostty-1.3.2-dev-5UdBC4gaYwVruUDKYxdTyGQF6L_6LjdKdJCbx7L4iR5V";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    for var in [
        "ZIG",
        "QUARK_GHOSTTY_VT_LIB_DIR",
        "QUARK_GHOSTTY_VT_OPTIMIZE",
    ] {
        println!("cargo:rerun-if-env-changed={var}");
    }
    println!("cargo:rustc-check-cfg=cfg(ghostty_vt)");
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if os == "windows" {
        return;
    }

    let lib_dir = match env::var_os("QUARK_GHOSTTY_VT_LIB_DIR") {
        Some(dir) => PathBuf::from(dir),
        None if cfg!(feature = "zig-build") => build_with_zig(&os),
        None => panic!(
            "quark-terminal: the zig-build feature is off; set QUARK_GHOSTTY_VT_LIB_DIR \
             to a directory holding libghostty-vt.a"
        ),
    };
    assert!(
        lib_dir.join("libghostty-vt.a").is_file(),
        "quark-terminal: no libghostty-vt.a in {}",
        lib_dir.display()
    );
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    // `static=` so the shared library installed next to it is never picked.
    println!("cargo:rustc-link-lib=static=ghostty-vt");
    println!("cargo:rustc-cfg=ghostty_vt");
}

/// Builds (or reuses) the archive and returns the directory holding it.
fn build_with_zig(os: &str) -> PathBuf {
    let zig = env::var("ZIG").unwrap_or_else(|_| "zig".to_owned());
    let optimize = env::var("QUARK_GHOSTTY_VT_OPTIMIZE").unwrap_or_else(|_| "ReleaseFast".into());
    let target = zig_target(os);
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    // OUT_DIR is <target>/<profile>/build/<pkg>-<hash>/out.
    let cache_root = out
        .ancestors()
        .nth(3)
        .map_or_else(|| out.clone(), Path::to_path_buf)
        .join("ghostty-vt");
    let short = &GHOSTTY_COMMIT[..12];
    let prefix = cache_root.join(format!("{short}-{target}-{optimize}"));
    let lib_dir = prefix.join("lib");
    if lib_dir.join("libghostty-vt.a").is_file() {
        return lib_dir;
    }

    // Fetch through a package manifest so Zig checks the pinned hash.
    let fetch_dir = cache_root.join(format!("{short}-src"));
    std::fs::create_dir_all(&fetch_dir).expect("create ghostty fetch dir");
    std::fs::write(fetch_dir.join("build.zig.zon"), manifest()).expect("write build.zig.zon");
    std::fs::write(
        fetch_dir.join("build.zig"),
        "pub fn build(b: *@import(\"std\").Build) void {\n    _ = b;\n}\n",
    )
    .expect("write build.zig");
    run(Command::new(&zig)
        .args(["build", "--fetch"])
        .current_dir(&fetch_dir));
    let source = fetch_dir.join("zig-pkg").join(GHOSTTY_HASH);

    run(Command::new(&zig)
        .arg("build")
        .arg("--build-file")
        .arg(source.join("build.zig"))
        .arg("--cache-dir")
        .arg(cache_root.join("zig-cache"))
        .arg("--prefix")
        .arg(&prefix)
        .arg("-Demit-lib-vt")
        .arg(format!("-Dtarget={target}"))
        // Portable machine code: the build host's CPU is not the user's.
        .arg("-Dcpu=baseline")
        .arg(format!("-Doptimize={optimize}")));
    lib_dir
}

fn manifest() -> String {
    format!(
        r#".{{
    .name = .quark_terminal_ghostty,
    .version = "0.0.0",
    .fingerprint = 0x5f68b022a1f5a017,
    .minimum_zig_version = "0.16.0",
    .dependencies = .{{
        .ghostty = .{{
            .url = "https://github.com/ghostty-org/ghostty/archive/{GHOSTTY_COMMIT}.tar.gz",
            .hash = "{GHOSTTY_HASH}",
        }},
    }},
    .paths = .{{""}},
}}
"#
    )
}

/// The Zig target triple for Cargo's target.
fn zig_target(os: &str) -> String {
    let arch = env::var("CARGO_CFG_TARGET_ARCH").expect("target arch");
    let abi = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    match (os, abi.as_str()) {
        ("macos", _) => format!("{arch}-macos"),
        ("linux", "musl") => format!("{arch}-linux-musl"),
        ("linux", _) => format!("{arch}-linux-gnu"),
        (os, "") => format!("{arch}-{os}"),
        (os, abi) => format!("{arch}-{os}-{abi}"),
    }
}

fn run(command: &mut Command) {
    let status = command.status().unwrap_or_else(|e| {
        panic!(
            "quark-terminal: could not run {:?} ({e}). libghostty-vt is built with Zig 0.16: \
             install it or set ZIG to its path.",
            command.get_program()
        )
    });
    assert!(
        status.success(),
        "quark-terminal: {command:?} failed: {status}"
    );
}
