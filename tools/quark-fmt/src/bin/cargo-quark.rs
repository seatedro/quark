//! `cargo quark fmt`: Cargo runs `cargo-quark quark fmt ...` for it.

fn main() -> std::process::ExitCode {
    quark_fmt::cli::main(quark_fmt::cli::Entry::CargoQuark)
}
