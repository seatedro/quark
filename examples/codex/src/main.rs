//! `codex-demo`: launch the Codex UI recreation. See README.md for flags.

use quark_codex::{Codex, Options, adapter, window_options};

fn main() {
    let options = match Options::from_env() {
        Ok(o) => o,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    if let Err(e) = quark_app::run(adapter(Codex::new(options)), window_options()) {
        eprintln!("codex-demo: {e}");
        std::process::exit(1);
    }
}
