//! `workbench`: launch Quark Workbench. See README.md for flags.

use quark_workbench::{Workbench, adapter, options, window_options};

fn main() {
    let options = match options::from_env() {
        Ok(o) => o,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    let app = Workbench::new(options);
    let window = window_options(&app);
    if let Err(e) = quark_app::run(adapter(app), window) {
        eprintln!("workbench: {e}");
        std::process::exit(1);
    }
}
