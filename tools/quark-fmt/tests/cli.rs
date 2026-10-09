//! The command line as users and editors drive it: real binaries, real
//! rustfmt, files in a temporary directory.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const UNFORMATTED: &str = "fn main(){let x=1;}\n";
const FORMATTED: &str = "fn main() {\n    let x = 1;\n}\n";

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn write(&self, rel: &str, contents: &str) -> PathBuf {
        let path = self.path(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        path
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.path(rel)).unwrap()
    }

    fn run(&self, args: &[&str]) -> Run {
        self.run_with(env!("CARGO_BIN_EXE_quark-fmt"), args, None, &[])
    }

    fn run_with(
        &self,
        bin: &str,
        args: &[&str],
        stdin: Option<&str>,
        env: &[(&str, &Path)],
    ) -> Run {
        let mut cmd = Command::new(bin);
        cmd.args(args)
            .current_dir(self.dir.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
        let mut input = child.stdin.take().unwrap();
        input.write_all(stdin.unwrap_or("").as_bytes()).unwrap();
        drop(input);
        let out = child.wait_with_output().unwrap();
        Run {
            code: out.status.code().unwrap(),
            stdout: String::from_utf8(out.stdout).unwrap(),
            stderr: String::from_utf8(out.stderr).unwrap(),
        }
    }

    fn stdin(&self, args: &[&str], input: &str) -> Run {
        self.run_with(env!("CARGO_BIN_EXE_quark-fmt"), args, Some(input), &[])
    }

    /// Directory entries, sorted, to catch stray temporary files.
    fn listing(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }
}

#[test]
fn check_prints_a_diff_exits_1_and_writes_nothing() {
    let fx = Fixture::new();
    fx.write("a.rs", UNFORMATTED);
    let run = fx.run(&["--check", "a.rs"]);
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert!(
        run.stdout.contains("--- a/a.rs\n+++ b/a.rs\n"),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout
            .contains("-fn main(){let x=1;}\n+fn main() {\n+    let x = 1;\n+}\n")
    );
    assert_eq!(fx.read("a.rs"), UNFORMATTED);
}

#[test]
fn check_of_a_formatted_file_exits_0_silently() {
    let fx = Fixture::new();
    fx.write("a.rs", FORMATTED);
    let run = fx.run(&["--check", "a.rs"]);
    assert_eq!((run.code, run.stdout.as_str()), (0, ""), "{}", run.stderr);
}

#[test]
fn write_replaces_the_file_keeping_its_mode_and_leaving_no_temporaries() {
    let fx = Fixture::new();
    let path = fx.write("a.rs", UNFORMATTED);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let run = fx.run(&["a.rs"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(fx.read("a.rs"), FORMATTED);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert_eq!(fx.listing(), ["a.rs"]);
}

#[test]
fn an_already_formatted_file_is_not_rewritten() {
    let fx = Fixture::new();
    let path = fx.write("a.rs", FORMATTED);
    let inode = fs::metadata(&path).unwrap().ino();
    assert_eq!(fx.run(&["a.rs"]).code, 0);
    assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
}

#[test]
fn a_file_that_fails_is_left_unchanged_while_others_are_written() {
    let fx = Fixture::new();
    let broken = "fn main( {let x=1;}\n";
    fx.write("bad.rs", broken);
    fx.write("good.rs", UNFORMATTED);
    let run = fx.run(&["bad.rs", "good.rs"]);
    assert_eq!(run.code, 2);
    assert!(run.stderr.contains("error: bad.rs"), "{}", run.stderr);
    assert_eq!(fx.read("bad.rs"), broken);
    assert_eq!(fx.read("good.rs"), FORMATTED);
}

#[test]
fn a_failure_outranks_check_differences_in_the_exit_status() {
    let fx = Fixture::new();
    fx.write("bad.rs", "fn main( {\n");
    fx.write("good.rs", UNFORMATTED);
    assert_eq!(fx.run(&["--check", "bad.rs", "good.rs"]).code, 2);
}

#[test]
fn stdin_prints_the_formatted_source_only() {
    let fx = Fixture::new();
    let run = fx.stdin(&["--stdin"], UNFORMATTED);
    assert_eq!(
        (run.code, run.stdout.as_str()),
        (0, FORMATTED),
        "{}",
        run.stderr
    );
    assert!(fx.listing().is_empty());
}

#[test]
fn stdin_failure_keeps_stdout_empty_so_editors_keep_the_buffer() {
    let fx = Fixture::new();
    let run = fx.stdin(&["--stdin"], "fn main( {\n");
    assert_eq!((run.code, run.stdout.as_str()), (2, ""));
    assert!(run.stderr.starts_with("error: <stdin>"), "{}", run.stderr);
}

#[test]
fn stdin_check_prints_a_diff_instead_of_the_source() {
    let fx = Fixture::new();
    let run = fx.stdin(&["--stdin", "--check"], UNFORMATTED);
    assert_eq!(run.code, 1);
    assert!(run.stdout.starts_with("--- a/<stdin>\n"), "{}", run.stdout);
}

#[test]
fn stdin_filepath_finds_configuration_next_to_that_path() {
    let fx = Fixture::new();
    fx.write("proj/rustfmt.toml", "max_width = 40\n");
    let long = "fn f() {\n    call(first_argument, second_argument);\n}\n";
    let wrapped =
        "fn f() {\n    call(\n        first_argument,\n        second_argument,\n    );\n}\n";
    let run = fx.stdin(&["--stdin-filepath", "proj/src/lib.rs"], long);
    assert_eq!(
        (run.code, run.stdout.as_str()),
        (0, wrapped),
        "{}",
        run.stderr
    );
}

#[test]
fn edition_comes_from_the_workspace_when_the_package_inherits_it() {
    let fx = Fixture::new();
    fx.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"p\"]\n[workspace.package]\nedition = \"2021\"\n",
    );
    fx.write(
        "p/Cargo.toml",
        "[package]\nname = \"p\"\nedition.workspace = true\n",
    );
    // `gen` is reserved from edition 2024 on, the fallback edition.
    fx.write("p/src/lib.rs", "fn gen(){}\n");
    let run = fx.run(&["p/src/lib.rs"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(fx.read("p/src/lib.rs"), "fn gen() {}\n");
}

#[test]
fn all_formats_member_sources_and_skips_build_output_and_hidden_dirs() {
    let fx = Fixture::new();
    fx.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"p\"]\nresolver = \"3\"\n",
    );
    fx.write(
        "p/Cargo.toml",
        "[package]\nname = \"p\"\nedition = \"2024\"\n",
    );
    fx.write("p/src/lib.rs", UNFORMATTED);
    fx.write("p/tests/t.rs", UNFORMATTED);
    fx.write("p/src/target/CACHEDIR.TAG", "");
    fx.write("p/src/target/out.rs", UNFORMATTED);
    fx.write("p/src/.cache/x.rs", UNFORMATTED);
    let lock = Command::new(env!("CARGO"))
        .args(["generate-lockfile", "--offline"])
        .current_dir(fx.dir.path())
        .output()
        .unwrap();
    assert!(lock.status.success());
    let run = fx.run(&["--all"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(fx.read("p/src/lib.rs"), FORMATTED);
    assert_eq!(fx.read("p/tests/t.rs"), FORMATTED);
    assert_eq!(fx.read("p/src/target/out.rs"), UNFORMATTED);
    assert_eq!(fx.read("p/src/.cache/x.rs"), UNFORMATTED);
}

#[test]
fn quark_fmt_toml_excludes_files_even_when_named_explicitly() {
    let fx = Fixture::new();
    fx.write("quark-fmt.toml", "exclude = [\"gen/**\"]\n");
    fx.write("gen/deep/a.rs", UNFORMATTED);
    fx.write("src/a.rs", UNFORMATTED);
    let run = fx.run(&["gen/deep/a.rs", "src/a.rs"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(fx.read("gen/deep/a.rs"), UNFORMATTED);
    assert_eq!(fx.read("src/a.rs"), FORMATTED);
}

#[test]
fn unknown_quark_fmt_toml_keys_are_errors() {
    let fx = Fixture::new();
    fx.write("quark-fmt.toml", "sort_classes = true\n");
    fx.write("a.rs", UNFORMATTED);
    let run = fx.run(&["a.rs"]);
    assert_eq!(run.code, 2);
    assert!(run.stderr.contains("sort_classes"), "{}", run.stderr);
    assert_eq!(fx.read("a.rs"), UNFORMATTED);
}

#[test]
fn unsupported_arguments_are_rejected() {
    let fx = Fixture::new();
    fx.write("a.rs", UNFORMATTED);
    let run = fx.run(&["--file-lines", "[]", "a.rs"]);
    assert_eq!(run.code, 2);
    assert!(run.stderr.contains("--file-lines"), "{}", run.stderr);
    assert_eq!(fx.read("a.rs"), UNFORMATTED);
}

#[test]
fn cargo_quark_dispatches_fmt_and_rejects_other_subcommands() {
    let fx = Fixture::new();
    fx.write("a.rs", UNFORMATTED);
    let bin = env!("CARGO_BIN_EXE_cargo-quark");
    assert_eq!(fx.run_with(bin, &["quark", "build"], None, &[]).code, 2);
    assert_eq!(
        fx.run_with(bin, &["quark", "fmt", "--check", "a.rs"], None, &[])
            .code,
        1
    );
    assert_eq!(
        fx.run_with(bin, &["quark", "fmt", "a.rs"], None, &[]).code,
        0
    );
    assert_eq!(fx.read("a.rs"), FORMATTED);
}

#[test]
fn a_backend_that_never_settles_leaves_the_file_unchanged() {
    let fx = Fixture::new();
    // Stands in for a rustfmt/view interaction that keeps moving code: every
    // pass appends another line.
    let script = fx.write("fake-rustfmt", "#!/bin/sh\ncat\necho '// again'\n");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    fx.write("a.rs", FORMATTED);
    let bin = env!("CARGO_BIN_EXE_quark-fmt");
    let run = fx.run_with(bin, &["a.rs"], None, &[("QUARK_FMT_RUSTFMT", &script)]);
    assert_eq!(run.code, 2);
    assert!(run.stderr.contains("did not converge"), "{}", run.stderr);
    assert_eq!(fx.read("a.rs"), FORMATTED);
}
