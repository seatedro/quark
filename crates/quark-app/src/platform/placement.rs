//! Window placement records and the rules for restoring them on whatever
//! monitors are connected now. A record is what a window remembers about
//! where it was; [`restore`] turns it into the size and position to open
//! with. Records are plain data, saved on their own through
//! [`window_state::save_placement`] or inside a larger checkpoint (a docking
//! workspace) through [`window_state::write_checkpoint`].
//!
//! A record keeps the window's normal client size in logical points, apart
//! from its maximized and minimized flags, so a window closed while
//! maximized still unmaximizes to the size it had. Its position is the outer
//! top-left corner as an offset from the origin of the monitor it was on, in
//! that monitor's logical points, next to a fingerprint of that monitor.
//!
//! Positions never pass through one desktop-wide logical plane. Monitors
//! with different scales overlap or leave gaps in such a plane: a 2x monitor
//! to the right of a 1x one starts at physical x 1920 but logical x 960, so
//! a point on it would land back on the 1x monitor. Each conversion here
//! uses only the scale of the one monitor involved, and desktop positions
//! stay in winit's physical pixels.
//!
//! Everything in this module is pure and deterministic, so the runner feeds
//! it winit's monitor list and window state and applies the result.
//!
//! [`window_state::save_placement`]: super::window_state::save_placement
//! [`window_state::write_checkpoint`]: super::window_state::write_checkpoint

use serde::{Deserialize, Serialize};

/// Smallest client width and height a restored window may have, in points,
/// so a corrupt or odd record cannot open an unusably small window.
pub const MIN_SIZE: f64 = 64.0;

/// A rectangle in desktop physical pixels, as winit reports monitors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhysicalRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl PhysicalRect {
    fn right(&self) -> i64 {
        i64::from(self.x) + i64::from(self.width)
    }

    fn bottom(&self) -> i64 {
        i64::from(self.y) + i64::from(self.height)
    }

    pub fn contains(&self, (x, y): (i32, i32)) -> bool {
        (i64::from(self.x)..self.right()).contains(&i64::from(x))
            && (i64::from(self.y)..self.bottom()).contains(&i64::from(y))
    }

    /// The overlap of two rectangles, `None` when they do not overlap.
    fn intersection(&self, other: &Self) -> Option<Self> {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        let width = u32::try_from(right - i64::from(left)).ok()?;
        let height = u32::try_from(bottom - i64::from(top)).ok()?;
        (width > 0 && height > 0).then_some(Self {
            x: left,
            y: top,
            width,
            height,
        })
    }
}

/// A connected monitor, from winit's `MonitorHandle` plus the platform's
/// work area where the runner can query one.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    pub name: Option<String>,
    /// Full monitor bounds in desktop physical pixels.
    pub bounds: PhysicalRect,
    pub scale_factor: f64,
    /// The part of `bounds` not covered by docks, taskbars, and panels.
    /// `None` uses the full bounds; winit 0.30 does not report it.
    pub work_area: Option<PhysicalRect>,
}

/// A rectangle in one monitor's logical points, relative to its origin.
#[derive(Debug, Clone, Copy)]
struct LocalRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl MonitorInfo {
    pub fn fingerprint(&self) -> MonitorFingerprint {
        MonitorFingerprint {
            name: self.name.clone(),
            position: (self.bounds.x, self.bounds.y),
            size: (self.bounds.width, self.bounds.height),
            scale_factor: self.scale_factor,
        }
    }

    /// Where windows may go: the work area clipped to the monitor, or the
    /// whole monitor when there is no work area or it misses the monitor.
    pub fn usable(&self) -> PhysicalRect {
        self.work_area
            .and_then(|area| area.intersection(&self.bounds))
            .unwrap_or(self.bounds)
    }

    fn scale(&self) -> f64 {
        sanitize_scale(self.scale_factor)
    }

    /// A desktop physical point as an offset from this monitor's origin in
    /// its logical points.
    pub fn to_offset(&self, (x, y): (i32, i32)) -> (f64, f64) {
        let s = self.scale();
        (
            f64::from(x - self.bounds.x) / s,
            f64::from(y - self.bounds.y) / s,
        )
    }

    /// The desktop physical point at `offset` logical points from this
    /// monitor's origin.
    pub fn to_desktop(&self, (x, y): (f64, f64)) -> (i32, i32) {
        let s = self.scale();
        let axis = |origin: i32, offset: f64| {
            let pixel = i64::from(origin) + (offset * s).round() as i64;
            pixel.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
        };
        (axis(self.bounds.x, x), axis(self.bounds.y, y))
    }

    fn usable_local(&self) -> LocalRect {
        let s = self.scale();
        let usable = self.usable();
        LocalRect {
            x: f64::from(usable.x - self.bounds.x) / s,
            y: f64::from(usable.y - self.bounds.y) / s,
            width: f64::from(usable.width) / s,
            height: f64::from(usable.height) / s,
        }
    }
}

fn sanitize_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// The index of the monitor whose bounds contain a desktop physical point.
pub fn monitor_at(monitors: &[MonitorInfo], point: (i32, i32)) -> Option<usize> {
    monitors.iter().position(|m| m.bounds.contains(point))
}

/// What a record remembers of its monitor, to find it again. Only a hint:
/// monitors get unplugged, renamed, moved, and rescaled between runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonitorFingerprint {
    #[serde(default)]
    pub name: Option<String>,
    /// Physical position and size of the monitor's bounds.
    pub position: (i32, i32),
    pub size: (u32, u32),
    pub scale_factor: f64,
}

/// The connected monitor that best matches `fingerprint`. A candidate must
/// share the name, or the position and size when the name changed or is
/// unknown. Among candidates, the name counts most, then position, size,
/// and scale, and the first monitor wins ties, so two identical monitors
/// are told apart by where they sit.
pub fn match_monitor(fingerprint: &MonitorFingerprint, monitors: &[MonitorInfo]) -> Option<usize> {
    let mut best: Option<(usize, u32)> = None;
    for (index, monitor) in monitors.iter().enumerate() {
        let name = fingerprint.name.is_some() && fingerprint.name == monitor.name;
        let position = fingerprint.position == (monitor.bounds.x, monitor.bounds.y);
        let size = fingerprint.size == (monitor.bounds.width, monitor.bounds.height);
        let scale = (fingerprint.scale_factor - monitor.scale_factor).abs() < 1e-6;
        if !(name || (position && size)) {
            continue;
        }
        let score =
            8 * u32::from(name) + 4 * u32::from(position) + 2 * u32::from(size) + u32::from(scale);
        if best.is_none_or(|(_, top)| score > top) {
            best = Some((index, score));
        }
    }
    best.map(|(index, _)| index)
}

/// Where a window was, in a form that survives monitor changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementRecord {
    /// Normal (unmaximized) client size in logical points.
    pub size: (f64, f64),
    /// Normal outer top-left corner relative to the origin of `monitor`, in
    /// that monitor's logical points. `None` where the platform does not
    /// report window positions (Wayland) or the monitor was unknown.
    #[serde(default)]
    pub offset: Option<(f64, f64)>,
    #[serde(default)]
    pub monitor: Option<MonitorFingerprint>,
    /// The window's scale factor when it was saved.
    #[serde(default)]
    pub scale_factor: Option<f64>,
    #[serde(default)]
    pub maximized: bool,
    #[serde(default)]
    pub minimized: bool,
}

/// A live window's state as winit reports it, for [`PlacementRecord::capture`].
#[derive(Debug, Clone, Copy)]
pub struct WindowObservation<'a> {
    /// `Window::outer_position`, `None` where it fails (Wayland).
    pub outer_position: Option<(i32, i32)>,
    /// `Window::inner_size` in physical pixels.
    pub inner_size: (u32, u32),
    pub scale_factor: f64,
    pub maximized: bool,
    pub minimized: bool,
    /// `Window::current_monitor`.
    pub monitor: Option<&'a MonitorInfo>,
}

impl PlacementRecord {
    /// The record for a window in `window`'s state. While the window is
    /// maximized or minimized its reported size and position are not its
    /// normal ones, so the normal size and offset carry over from
    /// `previous`. A maximized window still records the monitor it is
    /// maximized on, so it reopens there; a minimized one keeps everything
    /// from `previous`, since platforms park minimized windows off screen.
    /// Without `previous`, the reported state is the best there is.
    pub fn capture(previous: Option<&Self>, window: &WindowObservation<'_>) -> Self {
        if let Some(previous) = previous {
            if window.minimized {
                return Self {
                    minimized: true,
                    ..previous.clone()
                };
            }
            if window.maximized {
                return Self {
                    monitor: window
                        .monitor
                        .map(MonitorInfo::fingerprint)
                        .or_else(|| previous.monitor.clone()),
                    maximized: true,
                    minimized: false,
                    ..previous.clone()
                };
            }
        }
        let s = sanitize_scale(window.scale_factor);
        Self {
            size: (
                f64::from(window.inner_size.0) / s,
                f64::from(window.inner_size.1) / s,
            ),
            offset: window
                .monitor
                .zip(window.outer_position)
                .map(|(monitor, position)| monitor.to_offset(position)),
            monitor: window.monitor.map(MonitorInfo::fingerprint),
            scale_factor: Some(s),
            maximized: window.maximized,
            minimized: window.minimized,
        }
    }
}

/// What the platform and app allow when restoring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestoreOptions {
    /// Whether windows can be positioned. False on Wayland, where the
    /// compositor places windows and positions are left unset.
    pub can_position: bool,
    /// The monitor to use when the saved one is gone, normally the main
    /// window's. Falls back to the first monitor when `None` or out of
    /// range, so callers should list the primary monitor first.
    pub fallback_monitor: Option<usize>,
    /// Reopen minimized windows minimized. Off by default: a window that
    /// comes back hidden looks like one that did not come back.
    pub restore_minimized: bool,
}

impl Default for RestoreOptions {
    fn default() -> Self {
        Self {
            can_position: true,
            fallback_monitor: None,
            restore_minimized: false,
        }
    }
}

/// How [`restore`] chose the monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorChoice {
    /// The saved monitor, found by its fingerprint.
    Saved,
    /// The saved monitor is gone or unknown.
    Fallback,
}

/// The placement to open a window with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RestoredPlacement {
    /// The monitor the window goes on, `None` when no monitor is known.
    pub monitor: Option<(usize, MonitorChoice)>,
    /// Client size in logical points, for `with_inner_size(LogicalSize)`.
    pub size: (f64, f64),
    /// Outer top-left corner in desktop physical pixels, for
    /// `with_position(PhysicalPosition)`. `None` leaves placement to the
    /// platform.
    pub position: Option<(i32, i32)>,
    pub maximized: bool,
    pub minimized: bool,
}

impl RestoredPlacement {
    /// `position` in the chosen monitor's points. macOS lays its desktop
    /// out in points and winit converts a physical position there with the
    /// window's own scale, which is not yet the target monitor's when the
    /// window opens, so macOS should use this with `LogicalPosition`.
    pub fn desktop_points(&self, monitors: &[MonitorInfo]) -> Option<(f64, f64)> {
        let (index, _) = self.monitor?;
        let s = monitors.get(index)?.scale();
        self.position
            .map(|(x, y)| (f64::from(x) / s, f64::from(y) / s))
    }
}

/// Where to open a window saved as `record`. The saved monitor is found by
/// [`match_monitor`], else the fallback monitor is used. The size shrinks
/// to fit that monitor's usable area, and the position moves the window
/// fully inside it, so the title bar is reachable even when the saved spot
/// is now off screen. The offset is applied in the chosen monitor's own
/// points, so a monitor whose scale changed keeps the window at the same
/// apparent spot.
pub fn restore(
    record: &PlacementRecord,
    monitors: &[MonitorInfo],
    options: &RestoreOptions,
) -> RestoredPlacement {
    let monitor = match record
        .monitor
        .as_ref()
        .and_then(|fingerprint| match_monitor(fingerprint, monitors))
    {
        Some(index) => Some((index, MonitorChoice::Saved)),
        None => options
            .fallback_monitor
            .filter(|&index| index < monitors.len())
            .or((!monitors.is_empty()).then_some(0))
            .map(|index| (index, MonitorChoice::Fallback)),
    };
    let side = |len: f64| {
        if len.is_finite() {
            len.max(MIN_SIZE)
        } else {
            MIN_SIZE
        }
    };
    let mut size = (side(record.size.0), side(record.size.1));
    let mut position = None;
    if let Some((index, _)) = monitor {
        let monitor = &monitors[index];
        let usable = monitor.usable_local();
        size = (
            size.0.min(usable.width).max(MIN_SIZE),
            size.1.min(usable.height).max(MIN_SIZE),
        );
        if options.can_position
            && let Some((x, y)) = record.offset
            && x.is_finite()
            && y.is_finite()
        {
            // Pinned to the usable area's top-left when the window is
            // larger than it (only below MIN_SIZE), so the title bar
            // stays on screen.
            let clamp = |pos: f64, start: f64, room: f64, len: f64| {
                pos.clamp(start, (start + room - len).max(start))
            };
            position = Some(monitor.to_desktop((
                clamp(x, usable.x, usable.width, size.0),
                clamp(y, usable.y, usable.height, size.1),
            )));
        }
    }
    RestoredPlacement {
        monitor,
        size,
        position,
        maximized: record.maximized,
        minimized: record.minimized && options.restore_minimized,
    }
}

/// A rectangle in desktop units: Cocoa points on macOS, physical pixels on
/// Windows and X11 (see [`crate::DesktopPoint`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DesktopRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl DesktopRect {
    fn contains(&self, (x, y): (f64, f64)) -> bool {
        (self.x..self.x + self.width).contains(&x) && (self.y..self.y + self.height).contains(&y)
    }

    /// The squared distance from `point` to the nearest point inside.
    fn distance_squared(&self, (x, y): (f64, f64)) -> f64 {
        let dx = (self.x - x).max(x - (self.x + self.width)).max(0.0);
        let dy = (self.y - y).max(y - (self.y + self.height)).max(0.0);
        dx * dx + dy * dy
    }
}

/// The index of the area that contains `point`, else the nearest one; the
/// first wins ties. `None` when there are no areas.
pub fn nearest_area(areas: &[DesktopRect], point: (f64, f64)) -> Option<usize> {
    if let Some(index) = areas.iter().position(|area| area.contains(point)) {
        return Some(index);
    }
    areas
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            a.distance_squared(point)
                .total_cmp(&b.distance_squared(point))
        })
        .map(|(index, _)| index)
}

/// Where a window whose outer rectangle (decorations included) is `outer`
/// goes so that all of it lies inside the work area `work`. A window larger
/// than the work area shrinks to it, but not below `min_size` (outer, in
/// desktop units); one that still does not fit is pinned to the work area's
/// top-left, so its title bar and the controls at its start stay reachable.
/// A rectangle with a non-finite side is returned unchanged.
pub fn constrain_to_work_area(
    outer: DesktopRect,
    work: DesktopRect,
    min_size: (f64, f64),
) -> DesktopRect {
    let finite = |r: &DesktopRect| [r.x, r.y, r.width, r.height].iter().all(|v| v.is_finite());
    if !finite(&outer) || !finite(&work) {
        return outer;
    }
    let axis = |pos: f64, len: f64, start: f64, room: f64, min: f64| {
        let len = len.min(room).max(min.min(len));
        (pos.clamp(start, (start + room - len).max(start)), len)
    };
    let (x, width) = axis(outer.x, outer.width, work.x, work.width, min_size.0);
    let (y, height) = axis(outer.y, outer.height, work.y, work.height, min_size.1);
    DesktopRect {
        x,
        y,
        width,
        height,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use proptest::prelude::*;

    fn rect(x: i32, y: i32, width: u32, height: u32) -> PhysicalRect {
        PhysicalRect {
            x,
            y,
            width,
            height,
        }
    }

    pub(crate) fn monitor(name: &str, bounds: PhysicalRect, scale_factor: f64) -> MonitorInfo {
        MonitorInfo {
            name: Some(name.to_owned()),
            bounds,
            scale_factor,
            work_area: None,
        }
    }

    /// 1x, with a 30 px top bar and a 40 px dock.
    pub(crate) fn left() -> MonitorInfo {
        MonitorInfo {
            work_area: Some(rect(0, 30, 1920, 1010)),
            ..monitor("DP-1", rect(0, 0, 1920, 1080), 1.0)
        }
    }

    /// 2x to the right of `left`: 1920 x 1080 points from physical x 1920.
    pub(crate) fn right() -> MonitorInfo {
        monitor("DP-2", rect(1920, 0, 3840, 2160), 2.0)
    }

    /// 1.5x up and to the left of `left`: 2000 x 1200 points.
    fn upper_left() -> MonitorInfo {
        monitor("HDMI-1", rect(-3000, -200, 3000, 1800), 1.5)
    }

    fn record(on: &MonitorInfo, offset: (f64, f64), size: (f64, f64)) -> PlacementRecord {
        PlacementRecord {
            size,
            offset: Some(offset),
            monitor: Some(on.fingerprint()),
            scale_factor: Some(on.scale_factor),
            maximized: false,
            minimized: false,
        }
    }

    fn placed(
        index: usize,
        choice: MonitorChoice,
        position: Option<(i32, i32)>,
        size: (f64, f64),
    ) -> RestoredPlacement {
        RestoredPlacement {
            monitor: Some((index, choice)),
            size,
            position,
            maximized: false,
            minimized: false,
        }
    }

    #[test]
    fn restores_saved_placement_on_current_monitors() {
        use MonitorChoice::{Fallback, Saved};
        let on_right = record(&right(), (100.0, 50.0), (800.0, 600.0));
        let positioned = RestoreOptions::default();
        // (case, saved, monitors, options, expected)
        let cases = [
            (
                "a 2x monitor right of a 1x one keeps the offset in its own points",
                on_right.clone(),
                vec![left(), right()],
                positioned,
                placed(1, Saved, Some((2120, 100)), (800.0, 600.0)),
            ),
            (
                "an unplugged monitor falls back to the main window's, at negative coordinates",
                on_right.clone(),
                vec![left(), upper_left()],
                RestoreOptions {
                    fallback_monitor: Some(1),
                    ..positioned
                },
                placed(1, Fallback, Some((-2850, -125)), (800.0, 600.0)),
            ),
            (
                "an unplugged monitor without a main window falls back to the first",
                on_right.clone(),
                vec![left()],
                RestoreOptions {
                    fallback_monitor: Some(7),
                    ..positioned
                },
                placed(0, Fallback, Some((100, 50)), (800.0, 600.0)),
            ),
            (
                "a renamed monitor is found by its position and size",
                on_right.clone(),
                vec![
                    left(),
                    MonitorInfo {
                        name: Some("DP-5".into()),
                        ..right()
                    },
                ],
                positioned,
                placed(1, Saved, Some((2120, 100)), (800.0, 600.0)),
            ),
            (
                "a lower resolution keeps the monitor and pulls the window inside",
                record(&right(), (900.0, 400.0), (800.0, 600.0)),
                vec![left(), monitor("DP-2", rect(1920, 0, 2560, 1440), 2.0)],
                positioned,
                placed(1, Saved, Some((2880, 240)), (800.0, 600.0)),
            ),
            (
                "a new scale keeps the same offset in the monitor's points",
                on_right.clone(),
                vec![
                    left(),
                    MonitorInfo {
                        scale_factor: 1.5,
                        ..right()
                    },
                ],
                positioned,
                placed(1, Saved, Some((2070, 75)), (800.0, 600.0)),
            ),
            (
                "an off-screen position moves inside the work area",
                record(&left(), (5000.0, 5000.0), (800.0, 600.0)),
                vec![left(), right()],
                positioned,
                placed(0, Saved, Some((1120, 440)), (800.0, 600.0)),
            ),
            (
                "a position above the top bar moves below it",
                record(&left(), (-500.0, -500.0), (800.0, 600.0)),
                vec![left(), right()],
                positioned,
                placed(0, Saved, Some((0, 30)), (800.0, 600.0)),
            ),
            (
                "a window larger than the work area shrinks to it",
                record(&left(), (0.0, 0.0), (4000.0, 3000.0)),
                vec![left()],
                positioned,
                placed(0, Saved, Some((0, 30)), (1920.0, 1010.0)),
            ),
            (
                "without window positioning (Wayland) only the size is restored",
                on_right.clone(),
                vec![left(), right()],
                RestoreOptions {
                    can_position: false,
                    ..positioned
                },
                placed(1, Saved, None, (800.0, 600.0)),
            ),
            (
                "a maximized window keeps its normal size and stays maximized",
                PlacementRecord {
                    maximized: true,
                    ..on_right.clone()
                },
                vec![left(), right()],
                positioned,
                RestoredPlacement {
                    maximized: true,
                    ..placed(1, Saved, Some((2120, 100)), (800.0, 600.0))
                },
            ),
            (
                "a minimized window reopens unminimized",
                PlacementRecord {
                    minimized: true,
                    ..on_right.clone()
                },
                vec![left(), right()],
                positioned,
                placed(1, Saved, Some((2120, 100)), (800.0, 600.0)),
            ),
            (
                "a minimized window reopens minimized when the app asks",
                PlacementRecord {
                    minimized: true,
                    ..on_right.clone()
                },
                vec![left(), right()],
                RestoreOptions {
                    restore_minimized: true,
                    ..positioned
                },
                RestoredPlacement {
                    minimized: true,
                    ..placed(1, Saved, Some((2120, 100)), (800.0, 600.0))
                },
            ),
            (
                "no known monitors keep the size and leave the position",
                on_right.clone(),
                vec![],
                positioned,
                RestoredPlacement {
                    monitor: None,
                    ..placed(0, Saved, None, (800.0, 600.0))
                },
            ),
        ];
        for (case, saved, monitors, options, expected) in cases {
            assert_eq!(restore(&saved, &monitors, &options), expected, "{case}");
        }
    }

    #[test]
    fn finds_the_saved_monitor_by_fingerprint() {
        let twin_a = monitor("U2720Q", rect(0, 0, 3840, 2160), 2.0);
        let twin_b = monitor("U2720Q", rect(3840, 0, 3840, 2160), 2.0);
        let unnamed = MonitorInfo {
            name: None,
            ..monitor("", rect(0, 0, 1920, 1080), 1.0)
        };
        // (case, saved on, connected, expected)
        let cases = [
            (
                "identical monitors are told apart by position",
                twin_b.clone(),
                vec![twin_a.clone(), twin_b.clone()],
                Some(1),
            ),
            (
                "a moved monitor is found by name",
                twin_a.clone(),
                vec![monitor("U2720Q", rect(-3840, 0, 3840, 2160), 2.0)],
                Some(0),
            ),
            (
                "an unnamed monitor matches on position and size",
                unnamed.clone(),
                vec![twin_b.clone(), unnamed.clone()],
                Some(1),
            ),
            (
                "an unnamed monitor that changed resolution is lost",
                unnamed.clone(),
                vec![MonitorInfo {
                    bounds: rect(0, 0, 2560, 1440),
                    ..unnamed.clone()
                }],
                None,
            ),
            (
                "a renamed monitor that also moved is lost",
                twin_a.clone(),
                vec![monitor("DP-9", rect(0, -2160, 3840, 2160), 2.0)],
                None,
            ),
        ];
        for (case, saved_on, connected, expected) in cases {
            assert_eq!(
                match_monitor(&saved_on.fingerprint(), &connected),
                expected,
                "{case}"
            );
        }
    }

    // Regression: the persisted size was whatever the window had when it
    // closed, so a window closed maximized reopened with its maximized size
    // as its normal one.
    #[test]
    fn capture_keeps_normal_bounds_while_maximized_or_minimized() {
        let (left, right) = (left(), right());
        let observe = |position, size, maximized, minimized, monitor| WindowObservation {
            outer_position: position,
            inner_size: size,
            scale_factor: 2.0,
            maximized,
            minimized,
            monitor,
        };
        let normal = PlacementRecord::capture(
            None,
            &observe(Some((2120, 100)), (1600, 1200), false, false, Some(&right)),
        );
        let maximized = PlacementRecord::capture(
            Some(&normal),
            &observe(Some((1920, 0)), (3840, 2100), true, false, Some(&right)),
        );
        let minimized = PlacementRecord::capture(
            Some(&maximized),
            &observe(Some((-32000, -32000)), (0, 0), true, true, None),
        );
        let restored = restore(&minimized, &[left, right], &RestoreOptions::default());

        assert_eq!(
            restored,
            RestoredPlacement {
                maximized: true,
                ..placed(1, MonitorChoice::Saved, Some((2120, 100)), (800.0, 600.0))
            }
        );
    }

    fn monitors() -> impl Strategy<Value = Vec<MonitorInfo>> {
        let one = (
            -6000i32..6000,
            -4000i32..4000,
            320u32..6000,
            240u32..4000,
            prop::sample::select(vec![1.0, 1.25, 1.5, 1.75, 2.0, 3.0]),
            prop::option::of((0u32..200, 0u32..200, 0u32..200, 0u32..200)),
            prop::option::of(0u8..3),
        )
            .prop_map(|(x, y, width, height, scale_factor, insets, name)| {
                let bounds = rect(x, y, width, height);
                MonitorInfo {
                    name: name.map(|n| format!("M-{n}")),
                    bounds,
                    scale_factor,
                    work_area: insets.map(|(l, t, r, b)| {
                        let width = width.saturating_sub(l + r).max(1);
                        let height = height.saturating_sub(t + b).max(1);
                        rect(x + l as i32, y + t as i32, width, height)
                    }),
                }
            });
        prop::collection::vec(one, 1..5)
    }

    /// A record saved on a monitor that is gone, or with none at all.
    fn saved() -> impl Strategy<Value = PlacementRecord> {
        (
            (-1e5f64..1e5, -1e5f64..1e5),
            (0f64..1e4, 0f64..1e4),
            any::<bool>(),
        )
            .prop_map(|(offset, size, gone)| PlacementRecord {
                size,
                offset: Some(offset),
                monitor: gone.then(|| MonitorFingerprint {
                    name: Some("gone".into()),
                    position: (77_777, 77_777),
                    size: (1, 1),
                    scale_factor: 1.0,
                }),
                scale_factor: None,
                maximized: false,
                minimized: false,
            })
    }

    /// Grabbable title bar strip the property guarantees, in points.
    const TITLE_BAR: (f64, f64) = (48.0, 24.0);

    fn area(x: f64, y: f64, width: f64, height: f64) -> DesktopRect {
        DesktopRect {
            x,
            y,
            width,
            height,
        }
    }

    // Catches a floating window left under a taskbar or past a display's
    // edge, one shrunk below its minimum size, and an oversized window
    // pinned anywhere but the work area's top-left (where its title bar
    // and leading controls are).
    #[test]
    fn a_window_is_constrained_to_the_work_area() {
        let none = (0.0, 0.0);
        // (case, outer, work area, min outer size, expected)
        let cases = [
            (
                "already inside stays",
                area(100.0, 100.0, 800.0, 600.0),
                area(0.0, 0.0, 1920.0, 1040.0),
                none,
                area(100.0, 100.0, 800.0, 600.0),
            ),
            (
                "a taskbar along the bottom",
                area(1500.0, 900.0, 600.0, 400.0),
                area(0.0, 0.0, 1920.0, 1040.0),
                none,
                area(1320.0, 640.0, 600.0, 400.0),
            ),
            (
                "a menu bar along the top",
                area(200.0, -20.0, 600.0, 400.0),
                area(0.0, 25.0, 1440.0, 875.0),
                none,
                area(200.0, 25.0, 600.0, 400.0),
            ),
            (
                "a panel along the left",
                area(10.0, 300.0, 600.0, 400.0),
                area(48.0, 0.0, 1872.0, 1080.0),
                none,
                area(48.0, 300.0, 600.0, 400.0),
            ),
            (
                "a dock along the right",
                area(1700.0, 300.0, 600.0, 400.0),
                area(0.0, 0.0, 1856.0, 1080.0),
                none,
                area(1256.0, 300.0, 600.0, 400.0),
            ),
            (
                "a display left of the primary, at negative coordinates",
                area(-2000.0, -50.0, 600.0, 400.0),
                area(-1920.0, 0.0, 1920.0, 1040.0),
                none,
                area(-1920.0, 0.0, 600.0, 400.0),
            ),
            (
                "too large shrinks to the work area",
                area(-100.0, 10.0, 2400.0, 1200.0),
                area(0.0, 30.0, 1920.0, 1010.0),
                (240.0, 160.0),
                area(0.0, 30.0, 1920.0, 1010.0),
            ),
            (
                "a minimum larger than the work area keeps it, pinned top-left",
                area(300.0, 300.0, 1600.0, 1000.0),
                area(0.0, 30.0, 1280.0, 770.0),
                (1400.0, 900.0),
                area(0.0, 30.0, 1400.0, 900.0),
            ),
            (
                "a non-finite rectangle is left alone",
                area(f64::NAN, 0.0, 600.0, 400.0),
                area(0.0, 0.0, 1920.0, 1040.0),
                none,
                area(f64::NAN, 0.0, 600.0, 400.0),
            ),
        ];
        for (case, outer, work, min, expected) in cases {
            let got = constrain_to_work_area(outer, work, min);
            let same = |a: f64, b: f64| a == b || (a.is_nan() && b.is_nan());
            assert!(
                same(got.x, expected.x)
                    && same(got.y, expected.y)
                    && got.width == expected.width
                    && got.height == expected.height,
                "{case}: {got:?}"
            );
        }
    }

    // Catches a window released in a gap between displays, or beyond the
    // last one after a display was unplugged, getting no display at all.
    #[test]
    fn the_display_for_a_point_contains_it_or_is_nearest() {
        let displays = [
            area(0.0, 0.0, 1920.0, 1080.0),
            area(1920.0, 200.0, 1440.0, 900.0),
        ];
        let cases = [
            ("on the first", (100.0, 100.0), Some(0)),
            ("on the second", (2000.0, 500.0), Some(1)),
            (
                "above the second, beside the first",
                (2500.0, 50.0),
                Some(1),
            ),
            ("far right of both", (9000.0, 600.0), Some(1)),
            ("far left of both", (-500.0, 600.0), Some(0)),
        ];
        for (case, point, expected) in cases {
            assert_eq!(nearest_area(&displays, point), expected, "{case}");
        }
        assert_eq!(nearest_area(&[], (0.0, 0.0)), None, "no displays");
    }

    proptest! {
        // A restored window's top-left title bar strip lies inside the
        // usable area of its monitor, measured in desktop pixels, whatever
        // the saved spot, size, and monitor layout.
        #[test]
        fn restored_title_bar_stays_in_the_usable_area(
            (monitors, mut record, saved_on, fallback) in monitors().prop_flat_map(|monitors| {
                let count = monitors.len();
                (Just(monitors), saved(), prop::option::of(0..count), prop::option::of(0usize..6))
            })
        ) {
            if let Some(index) = saved_on {
                record.monitor = Some(monitors[index].fingerprint());
            }
            let options = RestoreOptions { fallback_monitor: fallback, ..RestoreOptions::default() };
            let restored = restore(&record, &monitors, &options);

            let (index, _) = restored.monitor.expect("a monitor is chosen");
            let (x, y) = restored.position.expect("a position is set");
            let monitor = &monitors[index];
            let usable = monitor.usable();
            let s = monitor.scale_factor;
            let strip_right = f64::from(x) + restored.size.0.min(TITLE_BAR.0) * s;
            let strip_bottom = f64::from(y) + TITLE_BAR.1 * s;
            let fits = usable.width as f64 >= TITLE_BAR.0 * s && usable.height as f64 >= TITLE_BAR.1 * s;
            prop_assert!(x >= usable.x && y >= usable.y, "{:?} starts outside {:?}", (x, y), usable);
            if fits {
                prop_assert!(
                    strip_right <= usable.right() as f64 + 1.0 && strip_bottom <= usable.bottom() as f64 + 1.0,
                    "strip to {:?} leaves {:?}", (strip_right, strip_bottom), usable
                );
            }
        }

        // Saving a window that sits inside a monitor's usable area and
        // restoring it on the same monitors reopens it where it was.
        #[test]
        fn capture_then_restore_reopens_in_place(
            (monitors, index, fx, fy, fw, fh) in monitors().prop_flat_map(|monitors| {
                let count = monitors.len();
                (Just(monitors), 0..count, 0f64..1.0, 0f64..1.0, 0f64..1.0, 0f64..1.0)
            })
        ) {
            let monitor = &monitors[index];
            let usable = monitor.usable();
            let s = monitor.scale_factor;
            let min = (MIN_SIZE * s).ceil() as u32;
            prop_assume!(usable.width > min && usable.height > min);
            let width = min + ((usable.width - min) as f64 * fw) as u32;
            let height = min + ((usable.height - min) as f64 * fh) as u32;
            let x = usable.x + ((usable.width - width) as f64 * fx) as i32;
            let y = usable.y + ((usable.height - height) as f64 * fy) as i32;
            let record = PlacementRecord::capture(None, &WindowObservation {
                outer_position: Some((x, y)),
                inner_size: (width, height),
                scale_factor: s,
                maximized: false,
                minimized: false,
                monitor: Some(monitor),
            });
            let restored = restore(&record, &monitors, &RestoreOptions::default());

            let (rx, ry) = restored.position.expect("a position is set");
            prop_assert!((rx - x).abs() <= 1 && (ry - y).abs() <= 1, "{:?} != {:?}", (rx, ry), (x, y));
            prop_assert!((restored.size.0 * s - f64::from(width)).abs() < 1e-6);
            prop_assert!((restored.size.1 * s - f64::from(height)).abs() < 1e-6);
        }
    }
}
