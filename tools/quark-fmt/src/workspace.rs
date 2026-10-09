//! File selection: workspace members from Cargo metadata, or explicit paths.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use walkdir::WalkDir;

/// Directories of a package that hold Rust sources Cargo builds.
const SOURCE_DIRS: [&str; 4] = ["src", "examples", "tests", "benches"];

/// A Rust file selected for formatting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selected {
    /// Absolute path the file is read from and written to.
    pub path: PathBuf,
    /// The owning package's edition, when Cargo metadata supplied it.
    pub edition: Option<String>,
    /// Named on the command line, as opposed to found by walking.
    pub explicit: bool,
}

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    workspace_members: Vec<String>,
}

#[derive(Deserialize)]
struct Package {
    id: String,
    manifest_path: PathBuf,
    edition: String,
    targets: Vec<Target>,
}

#[derive(Deserialize)]
struct Target {
    src_path: PathBuf,
}

/// Every Rust source of every workspace member: each target's root file plus
/// the member's `src`, `examples`, `tests`, and `benches` trees.
pub fn members(manifest_path: Option<&Path>) -> Result<Vec<Selected>, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.args([
        "metadata",
        "--no-deps",
        "--format-version",
        "1",
        "--offline",
        "--locked",
    ]);
    if let Some(path) = manifest_path {
        cmd.arg("--manifest-path").arg(path);
    }
    let out = cmd.output().map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("cargo metadata failed: {}", stderr.trim()));
    }
    let meta: Metadata =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))?;

    let mut packages: Vec<&Package> = meta
        .packages
        .iter()
        .filter(|p| meta.workspace_members.contains(&p.id))
        .collect();
    // Innermost package first, so a file nested in two members' trees gets the
    // edition of the package that actually owns it.
    packages.sort_by_key(|p| std::cmp::Reverse(p.manifest_path.components().count()));

    let mut files = BTreeMap::new();
    for package in packages {
        let Some(root) = package.manifest_path.parent() else {
            continue;
        };
        let mut add = |path: PathBuf| {
            files.entry(path).or_insert_with(|| package.edition.clone());
        };
        for target in &package.targets {
            add(target.src_path.clone());
        }
        for dir in SOURCE_DIRS {
            walk(&root.join(dir), &mut add);
        }
    }
    Ok(files
        .into_iter()
        .map(|(path, edition)| Selected {
            path,
            edition: Some(edition),
            explicit: false,
        })
        .collect())
}

/// Explicit command-line paths. Files are taken as given; directories are
/// walked for `.rs` files.
pub fn paths(paths: &[PathBuf]) -> Result<Vec<Selected>, String> {
    let mut files = BTreeMap::new();
    for given in paths {
        // Canonical paths write through symlinks instead of replacing them.
        let path = given
            .canonicalize()
            .map_err(|e| format!("{}: {e}", given.display()))?;
        if path.is_dir() {
            walk(&path, &mut |p| {
                files.entry(p).or_insert(false);
            });
        } else {
            files.insert(path, true);
        }
    }
    Ok(files
        .into_iter()
        .map(|(path, explicit)| Selected {
            path,
            edition: None,
            explicit,
        })
        .collect())
}

/// Collects `.rs` files under `dir`, skipping hidden directories (VCS and
/// tool state), build output, and symlinks, which could leave the selected
/// tree. Cargo marks every target directory with `CACHEDIR.TAG`.
fn walk(dir: &Path, add: &mut dyn FnMut(PathBuf)) {
    let entries = WalkDir::new(dir)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter();
    let entries = entries.filter_entry(|e| {
        let hidden = e.depth() > 0 && e.file_name().to_str().is_some_and(|n| n.starts_with('.'));
        let build_output = e.file_type().is_dir() && e.path().join("CACHEDIR.TAG").is_file();
        !hidden && !build_output && !e.path_is_symlink()
    });
    for entry in entries.flatten() {
        if entry.file_type().is_file() && entry.path().extension() == Some(OsStr::new("rs")) {
            add(entry.into_path());
        }
    }
}
