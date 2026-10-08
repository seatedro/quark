//! Window size and position persistence. Opt in per window with
//! [`WindowOptions::persist_key`]: the runner restores the saved geometry,
//! pulled back onto a connected monitor, when the window opens, and saves it
//! when the window closes or the app exits.
//!
//! Geometry is stored in logical points, so a window keeps its apparent
//! size when the display's scale changes between runs, as a small JSON file
//! per key under `<state dir>/quark/`. Files from before that, in physical
//! pixels, are converted with the scale of the monitor they were on.
//! Wayland does not report window positions, so only the size is restored
//! there.
//!
//!
//! This module also holds the storage for [`PlacementRecord`]s (the
//! monitor-aware placement in [`super::placement`]), which reads every older
//! file format, and the atomic, versioned checkpoint files that both
//! placement files and app snapshots such as a docking workspace are written
//! through.
//!
//! [`WindowOptions::persist_key`]: crate::WindowOptions::persist_key

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::placement::{MIN_SIZE, MonitorInfo, PlacementRecord, monitor_at};

/// A window's placement in logical points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    /// Outer top-left corner; `None` where the platform hides it (Wayland).
    pub position: Option<(f64, f64)>,
    /// Inner size.
    pub width: f64,
    pub height: f64,
    pub maximized: bool,
}

/// The file format: geometry under a `logical` key, which files written in
/// physical pixels lack.
#[derive(Serialize, Deserialize)]
struct LogicalFile {
    logical: WindowGeometry,
}

/// The format before geometry was logical.
#[derive(Deserialize)]
struct PhysicalFile {
    position: Option<(i32, i32)>,
    width: u32,
    height: u32,
    maximized: bool,
}

/// A monitor's area in physical pixels and its scale factor, from winit's
/// `MonitorHandle`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonitorArea {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
}

/// A rectangle in logical points.
#[derive(Debug, Clone, Copy)]
struct Area {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Area {
    fn overlap(&self, other: &Area) -> f64 {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = (self.x + self.width).min(other.x + other.width);
        let bottom = (self.y + self.height).min(other.y + other.height);
        (right - left).max(0.0) * (bottom - top).max(0.0)
    }
}

impl MonitorArea {
    fn logical(&self) -> Area {
        let s = self.scale_factor;
        Area {
            x: f64::from(self.x) / s,
            y: f64::from(self.y) / s,
            width: f64::from(self.width) / s,
            height: f64::from(self.height) / s,
        }
    }

    fn contains_physical(&self, x: i32, y: i32) -> bool {
        let (x, y) = (i64::from(x), i64::from(y));
        let (left, top) = (i64::from(self.x), i64::from(self.y));
        (left..left + i64::from(self.width)).contains(&x)
            && (top..top + i64::from(self.height)).contains(&y)
    }
}

impl WindowGeometry {
    /// Geometry from a window's physical position and inner size at
    /// `scale_factor`.
    pub fn from_physical(
        position: Option<(i32, i32)>,
        size: (u32, u32),
        scale_factor: f64,
        maximized: bool,
    ) -> Self {
        let s = scale_factor;
        Self {
            position: position.map(|(x, y)| (f64::from(x) / s, f64::from(y) / s)),
            width: f64::from(size.0) / s,
            height: f64::from(size.1) / s,
            maximized,
        }
    }

    /// Fit the window onto the monitor it overlaps most, or the first monitor
    /// when it overlaps none (a monitor was unplugged): shrink it to the
    /// monitor and move it fully inside. Unchanged when `monitors` is empty.
    pub fn clamped_to(&self, monitors: &[MonitorArea]) -> Self {
        let mut geometry = *self;
        geometry.width = geometry.width.max(MIN_SIZE);
        geometry.height = geometry.height.max(MIN_SIZE);
        let Some(first) = monitors.first() else {
            return geometry;
        };
        let monitor = match geometry.position {
            Some((x, y)) => {
                let window = Area {
                    x,
                    y,
                    width: geometry.width,
                    height: geometry.height,
                };
                monitors
                    .iter()
                    .map(|m| (m, m.logical().overlap(&window)))
                    .filter(|&(_, overlap)| overlap > 0.0)
                    .max_by(|a, b| a.1.total_cmp(&b.1))
                    .map_or(first, |(m, _)| m)
            }
            None => first,
        }
        .logical();
        geometry.width = geometry.width.min(monitor.width).max(MIN_SIZE);
        geometry.height = geometry.height.min(monitor.height).max(MIN_SIZE);
        if let Some((x, y)) = geometry.position {
            let clamp = |pos: f64, start: f64, monitor_len: f64, len: f64| {
                pos.clamp(start, (start + monitor_len - len).max(start))
            };
            geometry.position = Some((
                clamp(x, monitor.x, monitor.width, geometry.width),
                clamp(y, monitor.y, monitor.height, geometry.height),
            ));
        }
        geometry
    }

    /// Read saved geometry. A file in the old physical format is converted
    /// with the scale of the monitor its corner was on (else the first
    /// monitor's). Missing or unreadable files give `None`, so a bad file
    /// costs only the saved placement.
    pub fn load_from(path: &Path, monitors: &[MonitorArea]) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        if let Ok(file) = serde_json::from_slice::<LogicalFile>(&bytes) {
            return Some(file.logical);
        }
        let old: PhysicalFile = serde_json::from_slice(&bytes).ok()?;
        let scale = old
            .position
            .and_then(|(x, y)| monitors.iter().find(|m| m.contains_physical(x, y)))
            .or(monitors.first())
            .map_or(1.0, |m| m.scale_factor);
        Some(Self::from_physical(
            old.position,
            (old.width, old.height),
            scale,
            old.maximized,
        ))
    }

    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_vec(&LogicalFile { logical: *self }).map_err(io::Error::other)?;
        write_atomic(path, &json)
    }
}

/// The checkpoint kind of placement files.
pub const PLACEMENT_KIND: &str = "quark.window-placement";

/// The placement file version [`save_placement`] writes. Version 0 was
/// [`WindowGeometry`] in physical pixels and version 1 in logical points,
/// both bare JSON without a checkpoint header; [`load_placement`] reads all
/// three.
pub const PLACEMENT_VERSION: u32 = 2;

/// Write a placement file atomically.
pub fn save_placement(path: &Path, record: &PlacementRecord) -> io::Result<()> {
    write_checkpoint(path, PLACEMENT_KIND, PLACEMENT_VERSION, record)
}

/// Read a placement file of any version. Files from before placement
/// records are converted with the monitors connected now. Missing,
/// unreadable, and newer files give `None`, so a bad file costs only the
/// saved placement.
pub fn load_placement(path: &Path, monitors: &[MonitorInfo]) -> Option<PlacementRecord> {
    let bytes = std::fs::read(path).ok()?;
    if let Ok(checkpoint) = parse_checkpoint(&bytes, PLACEMENT_KIND) {
        return checkpoint
            .into_current(PLACEMENT_VERSION, |version, _| {
                Err(format!("no placement file version {version}"))
            })
            .ok();
    }
    if let Ok(file) = serde_json::from_slice::<LogicalFile>(&bytes) {
        return Some(PlacementRecord::from_legacy(&file.logical, monitors));
    }
    // Version 0 positions are physical, so they name their monitor exactly.
    let old: PhysicalFile = serde_json::from_slice(&bytes).ok()?;
    let monitor = old
        .position
        .and_then(|point| monitor_at(monitors, point))
        .map(|index| &monitors[index]);
    let scale = monitor.map_or(1.0, |m| m.scale_factor);
    Some(PlacementRecord {
        size: (f64::from(old.width) / scale, f64::from(old.height) / scale),
        offset: monitor
            .zip(old.position)
            .map(|(m, point)| m.to_offset(point)),
        monitor: monitor.map(MonitorInfo::fingerprint),
        scale_factor: monitor.map(|m| m.scale_factor),
        maximized: old.maximized,
        minimized: false,
    })
}

impl PlacementRecord {
    /// A record from [`WindowGeometry`], whose position is the window's
    /// physical position divided by its own scale. That scale was not
    /// saved, so the monitor is the first one whose bounds contain the
    /// position scaled back by that monitor's factor. With mixed scales more
    /// than one can, as the old format was ambiguous there. A position on no
    /// connected monitor is dropped.
    pub fn from_legacy(geometry: &WindowGeometry, monitors: &[MonitorInfo]) -> Self {
        let placed = geometry.position.and_then(|(x, y)| {
            monitors.iter().find_map(|monitor| {
                let s = monitor.scale_factor;
                let point = ((x * s).round() as i32, (y * s).round() as i32);
                monitor
                    .bounds
                    .contains(point)
                    .then(|| (monitor, monitor.to_offset(point)))
            })
        });
        Self {
            size: (geometry.width, geometry.height),
            offset: placed.map(|(_, offset)| offset),
            monitor: placed.map(|(monitor, _)| monitor.fingerprint()),
            scale_factor: placed.map(|(monitor, _)| monitor.scale_factor),
            maximized: geometry.maximized,
            minimized: false,
        }
    }
}

/// A checkpoint file as read: its schema version and its data in that
/// version's shape, for the caller to upgrade and deserialize.
#[derive(Debug, Clone, PartialEq)]
pub struct Checkpoint {
    pub version: u32,
    pub data: serde_json::Value,
}

#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    #[error("could not read the checkpoint: {0}")]
    Io(#[from] io::Error),
    #[error("malformed checkpoint: {0}")]
    Malformed(#[from] serde_json::Error),
    #[error("checkpoint holds {found:?}, expected {expected:?}")]
    WrongKind { found: String, expected: String },
    #[error("checkpoint version {found} is newer than the supported {supported}")]
    Newer { found: u32, supported: u32 },
    #[error("could not upgrade the checkpoint from version {from}: {message}")]
    Upgrade { from: u32, message: String },
}

/// The file layout: a kind naming what the data is, so one file cannot be
/// read as another, and the data's schema version.
#[derive(Serialize)]
struct EnvelopeOut<'a, T: ?Sized> {
    kind: &'a str,
    version: u32,
    data: &'a T,
}

#[derive(Deserialize)]
struct EnvelopeIn {
    kind: String,
    version: u32,
    data: serde_json::Value,
}

impl Checkpoint {
    /// Upgrade the data to version `current` and deserialize it. `step`
    /// turns version `n` data into version `n + 1` data and runs once per
    /// version in order. Data from a newer version is refused rather than
    /// misread.
    pub fn into_current<T: DeserializeOwned>(
        self,
        current: u32,
        mut step: impl FnMut(u32, serde_json::Value) -> Result<serde_json::Value, String>,
    ) -> Result<T, CheckpointError> {
        if self.version > current {
            return Err(CheckpointError::Newer {
                found: self.version,
                supported: current,
            });
        }
        let mut data = self.data;
        for from in self.version..current {
            data =
                step(from, data).map_err(|message| CheckpointError::Upgrade { from, message })?;
        }
        Ok(serde_json::from_value(data)?)
    }
}

/// Write `data` as version `version` of a `kind` checkpoint. The file is
/// written beside `path`, flushed to disk, and renamed over it, so a crash
/// or failed write leaves the previous checkpoint whole.
pub fn write_checkpoint<T: Serialize + ?Sized>(
    path: &Path,
    kind: &str,
    version: u32,
    data: &T,
) -> io::Result<()> {
    let json = serde_json::to_vec_pretty(&EnvelopeOut {
        kind,
        version,
        data,
    })
    .map_err(io::Error::other)?;
    write_atomic(path, &json)
}

/// Read a `kind` checkpoint, `Ok(None)` when there is none yet.
pub fn read_checkpoint(path: &Path, kind: &str) -> Result<Option<Checkpoint>, CheckpointError> {
    match std::fs::read(path) {
        Ok(bytes) => parse_checkpoint(&bytes, kind).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn parse_checkpoint(bytes: &[u8], kind: &str) -> Result<Checkpoint, CheckpointError> {
    let envelope: EnvelopeIn = serde_json::from_slice(bytes)?;
    if envelope.kind != kind {
        return Err(CheckpointError::WrongKind {
            found: envelope.kind,
            expected: kind.to_owned(),
        });
    }
    Ok(Checkpoint {
        version: envelope.version,
        data: envelope.data,
    })
}

/// Replace `path` with `bytes` through a temporary file in the same
/// directory, so the rename cannot cross file systems. Temporary names are
/// unique per process and call, so concurrent writers never share one.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let dir = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let temp = dir.join(format!(
        ".{}.{}-{}.tmp",
        name.to_string_lossy(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let written = File::create(&temp)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&temp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written?;
    // Persist the rename itself; best effort, as not every file system
    // supports syncing a directory.
    #[cfg(unix)]
    if let Ok(dir) = File::open(dir) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// `<state dir>/quark/window-<key>.json`.
pub fn state_path(key: &str) -> Option<PathBuf> {
    checkpoint_path(&format!("window-{key}"))
}

/// `<state dir>/quark/<name>.json`, with characters other than ASCII
/// letters, digits, `-`, and `_` in `name` replaced by `-`.
pub fn checkpoint_path(name: &str) -> Option<PathBuf> {
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    Some(
        crate::panic_hook::state_dir()?
            .join("quark")
            .join(format!("{name}.json")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT: MonitorArea = MonitorArea {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
        scale_factor: 1.0,
    };
    const RIGHT: MonitorArea = MonitorArea {
        x: 1920,
        y: 0,
        width: 1280,
        height: 1024,
        scale_factor: 1.0,
    };

    fn geometry(position: Option<(f64, f64)>, width: f64, height: f64) -> WindowGeometry {
        WindowGeometry {
            position,
            width,
            height,
            maximized: false,
        }
    }

    #[test]
    fn clamps_saved_geometry_onto_connected_monitors() {
        // (case, saved, expected)
        let cases = [
            (
                "inside the right monitor stays put",
                geometry(Some((2000.0, 100.0)), 800.0, 600.0),
                geometry(Some((2000.0, 100.0)), 800.0, 600.0),
            ),
            (
                "hanging off the right edge is pulled in",
                geometry(Some((3000.0, 900.0)), 800.0, 600.0),
                geometry(Some((2400.0, 424.0)), 800.0, 600.0),
            ),
            (
                "on an unplugged monitor moves to the first",
                geometry(Some((-2500.0, 200.0)), 800.0, 600.0),
                geometry(Some((0.0, 200.0)), 800.0, 600.0),
            ),
            (
                "larger than its monitor shrinks to fit",
                geometry(Some((1900.0, 0.0)), 4000.0, 3000.0),
                geometry(Some((1920.0, 0.0)), 1280.0, 1024.0),
            ),
            (
                "no position keeps none and fits the first monitor",
                geometry(None, 2500.0, 10.0),
                geometry(None, 1920.0, MIN_SIZE),
            ),
        ];
        for (case, saved, expected) in cases {
            assert_eq!(saved.clamped_to(&[LEFT, RIGHT]), expected, "{case}");
        }
    }

    // Regression: geometry was saved in physical pixels, so a window saved
    // on a 2x display reopened at twice its size on a 1x one, and files from
    // that format must still load.
    #[test]
    fn geometry_at_2x_restores_in_logical_points() {
        let hidpi = MonitorArea {
            scale_factor: 2.0,
            width: 3840,
            height: 2160,
            ..LEFT
        };
        let dir = std::env::temp_dir().join(format!("quark-window-state-{}", std::process::id()));
        let path = dir.join("window-main.json");
        let expected = Some(geometry(Some((100.0, 50.0)), 800.0, 600.0));

        WindowGeometry::from_physical(Some((200, 100)), (1600, 1200), 2.0, false)
            .save_to(&path)
            .unwrap();
        let saved = WindowGeometry::load_from(&path, &[LEFT]);
        std::fs::write(
            &path,
            br#"{"position":[200,100],"width":1600,"height":1200,"maximized":false}"#,
        )
        .unwrap();
        let migrated = WindowGeometry::load_from(&path, &[hidpi]);
        std::fs::write(&path, b"{\"width\": 9").unwrap();
        let corrupt = WindowGeometry::load_from(&path, &[hidpi]);
        let _ = std::fs::remove_dir_all(dir);

        assert_eq!(saved, expected, "saved at 2x, restored on another monitor");
        assert_eq!(migrated, expected, "physical file from a 2x monitor");
        assert_eq!(corrupt, None);
    }

    fn scratch_dir(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("quark-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    // Placement files from before placement records must keep their
    // window's spot, on the right monitor even when monitors differ in
    // scale.
    #[test]
    fn placement_files_of_every_version_load_as_records() {
        use crate::platform::placement::tests::{left, right};
        let monitors = [left(), right()];
        let record = |offset, monitor: Option<MonitorInfo>, maximized| PlacementRecord {
            size: (800.0, 600.0),
            offset,
            scale_factor: monitor.as_ref().map(|m| m.scale_factor),
            monitor: monitor.map(|m| m.fingerprint()),
            maximized,
            minimized: false,
        };
        // (case, file, expected)
        let cases: [(&str, &[u8], Option<PlacementRecord>); 6] = [
            (
                "version 0 in physical pixels on the 2x monitor",
                br#"{"position":[2120,100],"width":1600,"height":1200,"maximized":true}"#,
                Some(record(Some((100.0, 50.0)), Some(right()), true)),
            ),
            (
                "version 1 in logical points on the 2x monitor",
                br#"{"logical":{"position":[2000.0,100.0],"width":800.0,"height":600.0,"maximized":false}}"#,
                Some(record(Some((1040.0, 100.0)), Some(right()), false)),
            ),
            (
                "version 1 off every monitor keeps only the size",
                br#"{"logical":{"position":[-5000.0,0.0],"width":800.0,"height":600.0,"maximized":false}}"#,
                Some(record(None, None, false)),
            ),
            (
                "version 2",
                br#"{"kind":"quark.window-placement","version":2,"data":{"size":[800.0,600.0],"offset":[100.0,50.0],"monitor":{"name":"DP-2","position":[1920,0],"size":[3840,2160],"scale_factor":2.0},"scale_factor":2.0}}"#,
                Some(record(Some((100.0, 50.0)), Some(right()), false)),
            ),
            (
                "a newer version is ignored",
                br#"{"kind":"quark.window-placement","version":3,"data":{}}"#,
                None,
            ),
            ("a corrupt file is ignored", br#"{"size":[8"#, None),
        ];
        let dir = scratch_dir("placement-versions");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("window-main.json");
        for (case, file, expected) in cases {
            std::fs::write(&path, file).unwrap();
            assert_eq!(load_placement(&path, &monitors), expected, "{case}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn checkpoint_upgrades_old_data_one_version_at_a_time() {
        let dir = scratch_dir("checkpoint-upgrade");
        let path = dir.join("workspace.json");

        write_checkpoint(
            &path,
            "app.workspace",
            1,
            &serde_json::json!({ "steps": [] }),
        )
        .unwrap();
        let data: serde_json::Value = read_checkpoint(&path, "app.workspace")
            .unwrap()
            .unwrap()
            .into_current(3, |from, mut data| {
                data["steps"].as_array_mut().unwrap().push(from.into());
                Ok(data)
            })
            .unwrap();
        let _ = std::fs::remove_dir_all(dir);

        assert_eq!(data, serde_json::json!({ "steps": [1, 2] }));
    }

    #[test]
    fn checkpoint_refuses_files_it_cannot_read_safely() {
        let outcome = |result: Result<Option<serde_json::Value>, CheckpointError>| match result {
            Ok(None) => "none",
            Ok(Some(_)) => "read",
            Err(CheckpointError::Newer { .. }) => "newer",
            Err(CheckpointError::WrongKind { .. }) => "wrong kind",
            Err(CheckpointError::Malformed(_)) => "malformed",
            Err(CheckpointError::Upgrade { .. }) => "upgrade failed",
            Err(CheckpointError::Io(_)) => "io",
        };
        // (case, file, expected)
        let cases: [(&str, Option<&[u8]>, &str); 6] = [
            ("no file yet", None, "none"),
            (
                "current version",
                Some(br#"{"kind":"app.workspace","version":2,"data":{}}"#),
                "read",
            ),
            (
                "written by a newer build",
                Some(br#"{"kind":"app.workspace","version":5,"data":{}}"#),
                "newer",
            ),
            (
                "another kind of checkpoint",
                Some(br#"{"kind":"quark.window-placement","version":2,"data":{}}"#),
                "wrong kind",
            ),
            ("truncated", Some(br#"{"kind":"app.wor"#), "malformed"),
            (
                "an old version without an upgrade",
                Some(br#"{"kind":"app.workspace","version":1,"data":{}}"#),
                "upgrade failed",
            ),
        ];
        let dir = scratch_dir("checkpoint-refuse");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("workspace.json");
        for (case, file, expected) in cases {
            match file {
                Some(bytes) => std::fs::write(&path, bytes).unwrap(),
                None => {
                    let _ = std::fs::remove_file(&path);
                }
            }
            let result = read_checkpoint(&path, "app.workspace").and_then(|checkpoint| {
                checkpoint
                    .map(|c| c.into_current(2, |from, _| Err(format!("no step from {from}"))))
                    .transpose()
            });
            assert_eq!(outcome(result), expected, "{case}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    // A write that fails must leave the previous checkpoint readable and no
    // temporary files behind.
    #[test]
    fn failed_checkpoint_write_keeps_the_previous_file() {
        struct Unserializable;
        impl Serialize for Unserializable {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("refused"))
            }
        }
        let dir = scratch_dir("checkpoint-failed-write");
        let path = dir.join("workspace.json");

        write_checkpoint(
            &path,
            "app.workspace",
            1,
            &serde_json::json!({ "panels": 3 }),
        )
        .unwrap();
        let failed = write_checkpoint(&path, "app.workspace", 1, &Unserializable);
        let kept = read_checkpoint(&path, "app.workspace").unwrap().unwrap();
        let files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        let _ = std::fs::remove_dir_all(dir);

        assert!(failed.is_err());
        assert_eq!(kept.data, serde_json::json!({ "panels": 3 }));
        assert_eq!(files, ["workspace.json"]);
    }
}
