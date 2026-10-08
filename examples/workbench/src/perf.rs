//! Timing marks for the launch and history budgets (design section 5).
//!
//! The app records runner-relative milliseconds for its first main-window
//! frame and for the moment queued stress history finished loading. With
//! `QUARK_WORKBENCH_MARKS=1` (or `--perf`) each mark is printed once to
//! stderr as a JSON line, `{"mark":"first-frame","ms":412}`, which
//! `e2e/specs/workbench/performance_smoke.py` reads.

#[derive(Debug, Default)]
pub struct Marks {
    print: bool,
    pub first_frame_ms: Option<u64>,
    pub history_ready_ms: Option<u64>,
}

impl Marks {
    pub fn new(force: bool) -> Self {
        let env = std::env::var_os("QUARK_WORKBENCH_MARKS").is_some_and(|v| v != "0");
        Self {
            print: force || env,
            ..Self::default()
        }
    }

    fn emit(print: bool, slot: &mut Option<u64>, name: &str, ms: u64) {
        if slot.is_none() {
            *slot = Some(ms);
            if print {
                eprintln!("{{\"mark\":\"{name}\",\"ms\":{ms}}}");
            }
        }
    }

    pub fn first_frame(&mut self, ms: u64) {
        Self::emit(self.print, &mut self.first_frame_ms, "first-frame", ms);
    }

    pub fn history_ready(&mut self, ms: u64) {
        Self::emit(self.print, &mut self.history_ready_ms, "history-ready", ms);
    }
}
