//! Opt-in usage events. A [`Telemetry`] handle records nothing until the
//! app gives it a sink, and the one sink quark ships, [`JsonLinesSink`],
//! appends to a local file. Sending events anywhere is the app's own sink,
//! behind its own consent setting.
//!
//! ```no_run
//! use quark_app::platform::telemetry::{JsonLinesSink, Telemetry};
//!
//! # let user_opted_in = false;
//! let telemetry = if user_opted_in {
//!     Telemetry::with_sink(JsonLinesSink::open("/tmp/notes-events.jsonl")?)
//! } else {
//!     Telemetry::disabled()
//! };
//! telemetry.event("note_created").prop("chars", 120).record();
//! # Ok::<(), std::io::Error>(())
//! ```

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TelemetryEvent {
    pub name: String,
    pub unix_ms: u64,
    pub props: BTreeMap<String, Value>,
}

/// Where events go. Called on the thread that records the event, so a sink
/// that does I/O over the network should queue and send from its own
/// thread.
pub trait TelemetrySink: Send + Sync {
    fn record(&self, event: &TelemetryEvent);
    fn flush(&self) {}
}

/// A cheap, cloneable handle. Disabled by default.
#[derive(Clone, Default)]
pub struct Telemetry {
    sink: Option<Arc<dyn TelemetrySink>>,
}

impl Telemetry {
    pub fn disabled() -> Self {
        Self::default()
    }

    pub fn with_sink(sink: impl TelemetrySink + 'static) -> Self {
        Self {
            sink: Some(Arc::new(sink)),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.sink.is_some()
    }

    /// Start an event stamped with the current time. Does nothing when
    /// disabled.
    pub fn event(&self, name: &str) -> EventBuilder<'_> {
        let unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.event_at(name, unix_ms)
    }

    pub fn event_at(&self, name: &str, unix_ms: u64) -> EventBuilder<'_> {
        EventBuilder {
            telemetry: self,
            event: self.sink.as_ref().map(|_| TelemetryEvent {
                name: name.to_owned(),
                unix_ms,
                props: BTreeMap::new(),
            }),
        }
    }

    pub fn flush(&self) {
        if let Some(sink) = &self.sink {
            sink.flush();
        }
    }
}

pub struct EventBuilder<'a> {
    telemetry: &'a Telemetry,
    /// `None` when telemetry is disabled, so properties are dropped unbuilt.
    event: Option<TelemetryEvent>,
}

impl EventBuilder<'_> {
    pub fn prop(mut self, key: &str, value: impl Into<Value>) -> Self {
        if let Some(event) = &mut self.event {
            event.props.insert(key.to_owned(), value.into());
        }
        self
    }

    pub fn record(self) {
        if let (Some(event), Some(sink)) = (self.event, &self.telemetry.sink) {
            sink.record(&event);
        }
    }
}

/// Appends one JSON object per line: `{"name":…,"unix_ms":…,"props":{…}}`.
pub struct JsonLinesSink {
    file: Mutex<File>,
}

impl JsonLinesSink {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            file: Mutex::new(file),
        })
    }
}

impl TelemetrySink for JsonLinesSink {
    fn record(&self, event: &TelemetryEvent) {
        let Ok(mut line) = serde_json::to_string(event) else {
            return;
        };
        line.push('\n');
        let mut file = self.file.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(error) = file.write_all(line.as_bytes()) {
            tracing::warn!(target: "quark::telemetry", "could not write a telemetry event: {error}");
        }
    }

    fn flush(&self) {
        let _ = self.file.lock().unwrap_or_else(|e| e.into_inner()).flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_lines_sink_appends_one_object_per_event() {
        let path =
            std::env::temp_dir().join(format!("quark-telemetry-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let telemetry = Telemetry::with_sink(JsonLinesSink::open(&path).unwrap());
        telemetry
            .event_at("note_created", 1_000)
            .prop("chars", 120)
            .prop("kind", "todo")
            .record();
        telemetry.event_at("app_quit", 2_000).record();
        telemetry.flush();

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\"name\":\"note_created\",\"unix_ms\":1000,\"props\":{\"chars\":120,\"kind\":\"todo\"}}\n\
             {\"name\":\"app_quit\",\"unix_ms\":2000,\"props\":{}}\n"
        );
        std::fs::remove_file(&path).unwrap();
    }
}
