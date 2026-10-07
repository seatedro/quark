//! Window size and position persistence. Opt in per window with
//! [`WindowOptions::persist_key`]: the runner restores the saved geometry,
//! pulled back onto a connected monitor, when the window opens, and saves it
//! when the window closes or the app exits.
//!
//! Geometry is in physical pixels and stored as a small JSON file per key
//! under `<state dir>/quark/`. Wayland does not report window positions, so
//! only the size is restored there.
//!
//! [`WindowOptions::persist_key`]: crate::WindowOptions::persist_key

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Smallest size a restored window may have, so a corrupt or odd state file
/// cannot open an unusably small window.
const MIN_SIZE: u32 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowGeometry {
    /// Outer top-left corner; `None` where the platform hides it (Wayland).
    pub position: Option<(i32, i32)>,
    /// Inner size.
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

/// A monitor's area in physical pixels, from winit's `MonitorHandle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorArea {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl MonitorArea {
    fn overlap(&self, x: i32, y: i32, width: u32, height: u32) -> i64 {
        let left = i64::from(self.x).max(i64::from(x));
        let top = i64::from(self.y).max(i64::from(y));
        let right =
            (i64::from(self.x) + i64::from(self.width)).min(i64::from(x) + i64::from(width));
        let bottom =
            (i64::from(self.y) + i64::from(self.height)).min(i64::from(y) + i64::from(height));
        (right - left).max(0) * (bottom - top).max(0)
    }
}

impl WindowGeometry {
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
            Some((x, y)) => monitors
                .iter()
                .map(|m| (m, m.overlap(x, y, geometry.width, geometry.height)))
                .filter(|&(_, overlap)| overlap > 0)
                .max_by_key(|&(_, overlap)| overlap)
                .map_or(first, |(m, _)| m),
            None => first,
        };
        geometry.width = geometry.width.min(monitor.width).max(MIN_SIZE);
        geometry.height = geometry.height.min(monitor.height).max(MIN_SIZE);
        if let Some((x, y)) = geometry.position {
            let clamp = |pos: i32, start: i32, monitor_len: u32, len: u32| {
                let end = i64::from(start) + i64::from(monitor_len) - i64::from(len);
                i64::from(pos).clamp(i64::from(start), end.max(i64::from(start))) as i32
            };
            geometry.position = Some((
                clamp(x, monitor.x, monitor.width, geometry.width),
                clamp(y, monitor.y, monitor.height, geometry.height),
            ));
        }
        geometry
    }

    /// Read saved geometry. Missing or unreadable files give `None`, so a bad
    /// file costs only the saved placement.
    pub fn load_from(path: &Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_vec(self).map_err(io::Error::other)?;
        // Write then rename, so a crash mid-write leaves the old file.
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, json)?;
        std::fs::rename(&temp, path)
    }
}

/// `<state dir>/quark/window-<key>.json`.
pub fn state_path(key: &str) -> Option<PathBuf> {
    let key: String = key
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
            .join(format!("window-{key}.json")),
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
    };
    const RIGHT: MonitorArea = MonitorArea {
        x: 1920,
        y: 0,
        width: 1280,
        height: 1024,
    };

    fn geometry(position: Option<(i32, i32)>, width: u32, height: u32) -> WindowGeometry {
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
                geometry(Some((2000, 100)), 800, 600),
                geometry(Some((2000, 100)), 800, 600),
            ),
            (
                "hanging off the right edge is pulled in",
                geometry(Some((3000, 900)), 800, 600),
                geometry(Some((2400, 424)), 800, 600),
            ),
            (
                "on an unplugged monitor moves to the first",
                geometry(Some((-2500, 200)), 800, 600),
                geometry(Some((0, 200)), 800, 600),
            ),
            (
                "larger than its monitor shrinks to fit",
                geometry(Some((1900, 0)), 4000, 3000),
                geometry(Some((1920, 0)), 1280, 1024),
            ),
            (
                "no position keeps none and fits the first monitor",
                geometry(None, 2500, 10),
                geometry(None, 1920, MIN_SIZE),
            ),
        ];
        for (case, saved, expected) in cases {
            assert_eq!(saved.clamped_to(&[LEFT, RIGHT]), expected, "{case}");
        }
    }

    #[test]
    fn saved_geometry_round_trips_and_corrupt_files_load_as_none() {
        let dir = std::env::temp_dir().join(format!("quark-window-state-{}", std::process::id()));
        let path = dir.join("window-main.json");
        let saved = WindowGeometry {
            position: Some((-40, 25)),
            width: 900,
            height: 700,
            maximized: true,
        };

        saved.save_to(&path).unwrap();
        assert_eq!(WindowGeometry::load_from(&path), Some(saved));

        std::fs::write(&path, b"{\"width\": 9").unwrap();
        assert_eq!(WindowGeometry::load_from(&path), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
