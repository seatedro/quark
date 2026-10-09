//! `quark-fmt`: formats Rust sources and the quark `view!` templates in them.

fn main() -> std::process::ExitCode {
    quark_fmt::cli::main(quark_fmt::cli::Entry::QuarkFmt)
}
