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
//! [`WindowOptions::persist_key`]: crate::WindowOptions::persist_key

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Smallest size a restored window may have, in points, so a corrupt or
/// odd state file cannot open an unusably small window.
const MIN_SIZE: f64 = 64.0;

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
}
