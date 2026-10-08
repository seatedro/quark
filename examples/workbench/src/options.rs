//! Command-line flags and their environment equivalents.
//!
//! The e2e runner starts binaries without arguments, so every flag can
//! also come from the environment: `QUARK_WORKBENCH_SCENARIO`,
//! `QUARK_WORKBENCH_THEME`, `QUARK_WORKBENCH_SEED`,
//! `QUARK_WORKBENCH_MANUAL_CLOCK=1`, `QUARK_WORKBENCH_STATE_DIR`, and
//! `QUARK_WORKBENCH_PERF`, `QUARK_WORKBENCH_TERMINAL`, and
//! `QUARK_WORKBENCH_CWD`. Flags win over the environment.
//!
//! Launches get a real shell in the terminal panel unless they ask for the
//! scripted one; `Options::default()`, which tests build on, is scripted.

use std::path::PathBuf;

use crate::contracts::{Options, ScenarioKind, TerminalMode, ThemeChoice};

pub const USAGE: &str = "\
usage: workbench [--scenario review|empty|error|stress] [--theme system|light|dark]
                 [--seed N] [--manual-clock] [--state-dir DIR] [--perf FILE]
                 [--terminal real|scripted] [--cwd DIR]";

fn scenario(s: &str) -> Result<ScenarioKind, String> {
    match s {
        "review" => Ok(ScenarioKind::Review),
        "empty" => Ok(ScenarioKind::Empty),
        "error" => Ok(ScenarioKind::Error),
        "stress" => Ok(ScenarioKind::Stress),
        _ => Err(format!("unknown scenario {s:?}")),
    }
}

fn theme(s: &str) -> Result<ThemeChoice, String> {
    match s {
        "system" => Ok(ThemeChoice::System),
        "light" => Ok(ThemeChoice::Light),
        "dark" => Ok(ThemeChoice::Dark),
        _ => Err(format!("unknown theme {s:?}")),
    }
}

fn terminal(s: &str) -> Result<TerminalMode, String> {
    match s {
        "real" => Ok(TerminalMode::Real),
        "scripted" => Ok(TerminalMode::Scripted),
        _ => Err(format!("unknown terminal {s:?}")),
    }
}

fn seed(s: &str) -> Result<u64, String> {
    s.parse().map_err(|_| format!("bad seed {s:?}"))
}

/// Options from `env` (name, value pairs) overridden by `args` (without
/// the program name).
pub fn parse(
    args: impl IntoIterator<Item = String>,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Options, String> {
    let mut o = Options {
        seed: 7,
        terminal: TerminalMode::Real,
        ..Options::default()
    };
    if let Some(v) = env("QUARK_WORKBENCH_SCENARIO") {
        o.scenario = scenario(&v)?;
    }
    if let Some(v) = env("QUARK_WORKBENCH_THEME") {
        o.theme = theme(&v)?;
    }
    if let Some(v) = env("QUARK_WORKBENCH_SEED") {
        o.seed = seed(&v)?;
    }
    if let Some(v) = env("QUARK_WORKBENCH_MANUAL_CLOCK") {
        o.manual_clock = v != "0";
    }
    o.state_dir = env("QUARK_WORKBENCH_STATE_DIR").map(PathBuf::from);
    o.perf_out = env("QUARK_WORKBENCH_PERF").map(PathBuf::from);
    if let Some(v) = env("QUARK_WORKBENCH_TERMINAL") {
        o.terminal = terminal(&v)?;
    }
    o.cwd = env("QUARK_WORKBENCH_CWD").map(PathBuf::from);

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--scenario" => o.scenario = scenario(&value()?)?,
            "--theme" => o.theme = theme(&value()?)?,
            "--seed" => o.seed = seed(&value()?)?,
            "--manual-clock" => o.manual_clock = true,
            "--state-dir" => o.state_dir = Some(PathBuf::from(value()?)),
            "--perf" => o.perf_out = Some(PathBuf::from(value()?)),
            "--terminal" => o.terminal = terminal(&value()?)?,
            "--cwd" => o.cwd = Some(PathBuf::from(value()?)),
            "-h" | "--help" => return Err(USAGE.to_owned()),
            _ => return Err(format!("unknown argument {arg:?}\n{USAGE}")),
        }
    }
    Ok(o)
}

/// Options for this process.
pub fn from_env() -> Result<Options, String> {
    parse(std::env::args().skip(1), |k| std::env::var(k).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Catches the e2e path (environment only) and the CLI path disagreeing,
    // and flags failing to override the environment.
    #[test]
    fn flags_override_environment_equivalents() {
        let env = |k: &str| match k {
            "QUARK_WORKBENCH_SCENARIO" => Some("stress".to_owned()),
            "QUARK_WORKBENCH_MANUAL_CLOCK" => Some("1".to_owned()),
            "QUARK_WORKBENCH_THEME" => Some("light".to_owned()),
            "QUARK_WORKBENCH_TERMINAL" => Some("real".to_owned()),
            _ => None,
        };
        let args = ["--theme", "dark", "--seed", "42", "--terminal", "scripted"].map(String::from);
        let o = parse(args, env).expect("parses");
        assert_eq!(
            (o.scenario, o.manual_clock, o.theme, o.seed, o.terminal),
            (
                ScenarioKind::Stress,
                true,
                ThemeChoice::Dark,
                42,
                TerminalMode::Scripted
            )
        );
    }
}
