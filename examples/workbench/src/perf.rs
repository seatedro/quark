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

/// One frame of the `--perf` recording.
#[derive(Debug, Clone, Copy, Default)]
struct Sample {
    frame: u64,
    t_ms: u64,
    /// The app's own view build (surfaces, element construction).
    build_us: u64,
    /// Renderer CPU, swapchain acquire, and present of this same frame,
    /// filled in from the next frame's `last_render_stats`.
    render_cpu_us: u64,
    acquire_us: u64,
    present_us: u64,
    rows: usize,
    running: bool,
}

/// `--perf FILE`: replays the scripted run from launch and writes one JSON
/// line per main-window frame, then the marks, and asks the app to exit.
#[derive(Debug, Default)]
pub struct Recorder {
    out: Option<std::path::PathBuf>,
    samples: Vec<Sample>,
    /// The run was started (it is sent on the first frame).
    pub started: bool,
    pub finished: bool,
}

impl Recorder {
    pub fn new(out: Option<std::path::PathBuf>) -> Self {
        Self {
            out,
            ..Self::default()
        }
    }

    pub fn is_active(&self) -> bool {
        self.out.is_some() && !self.finished
    }

    /// Record frame `frame`, and the render stats the runner reports for
    /// the frame before it.
    pub fn record(
        &mut self,
        t_ms: u64,
        build_us: u64,
        // CPU, acquire, and present microseconds of the frame before.
        previous: (u64, u64, u64),
        rows: usize,
        running: bool,
    ) {
        if let Some(last) = self.samples.last_mut() {
            (last.render_cpu_us, last.acquire_us, last.present_us) = previous;
        }
        self.samples.push(Sample {
            frame: self.samples.len() as u64,
            t_ms,
            build_us,
            rows,
            running,
            ..Sample::default()
        });
    }

    /// Write the recording (the last frame has no render stats yet and is
    /// left out). Returns the path written.
    pub fn write(&mut self, marks: &Marks) -> std::io::Result<Option<std::path::PathBuf>> {
        use std::io::Write;
        self.finished = true;
        let Some(path) = self.out.clone() else {
            return Ok(None);
        };
        let mut f = std::io::BufWriter::new(std::fs::File::create(&path)?);
        let done = self.samples.len().saturating_sub(1);
        for s in &self.samples[..done] {
            writeln!(
                f,
                "{{\"frame\":{},\"t_ms\":{},\"build_us\":{},\"render_cpu_us\":{},\"acquire_us\":{},\"present_us\":{},\"rows\":{},\"running\":{}}}",
                s.frame,
                s.t_ms,
                s.build_us,
                s.render_cpu_us,
                s.acquire_us,
                s.present_us,
                s.rows,
                s.running
            )?;
        }
        for (name, ms) in [
            ("first-frame", marks.first_frame_ms),
            ("history-ready", marks.history_ready_ms),
        ] {
            if let Some(ms) = ms {
                writeln!(f, "{{\"mark\":\"{name}\",\"ms\":{ms}}}")?;
            }
        }
        f.flush()?;
        Ok(Some(path))
    }
}
