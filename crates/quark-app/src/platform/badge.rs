//! An unread count on the app's dock or taskbar icon, through
//! [`EventContext::set_badge`]. `None` and `Some(0)` clear it.
//!
//! - **macOS**: the Dock tile's badge label.
//! - **Windows**: an overlay icon on each window's taskbar button, drawn
//!   here as a red disc with the count (`99+` past 99). Windows opened later
//!   get it too. Windows does not show overlays with small taskbar buttons.
//! - **Linux**: the Unity `LauncherEntry` D-Bus signal, which Ubuntu's dock,
//!   KDE Plasma, Plank, and Dash to Dock show. It names the app by its
//!   `.desktop` file; call [`set_desktop_id`] when that is not the
//!   executable's name.
//!
//! [`EventContext::set_badge`]: crate::EventContext::set_badge

/// The text a badge shows for `count`, or `None` to clear it.
pub fn badge_label(count: Option<u32>) -> Option<String> {
    count
        .filter(|&count| count > 0)
        .map(|count| count.to_string())
}

/// The id of the app's `.desktop` file without the extension, such as
/// `com.example.Notes`, for the Linux launcher badge. Defaults to the
/// executable's file name. No effect elsewhere.
pub fn set_desktop_id(id: &str) {
    #[cfg(target_os = "linux")]
    {
        *linux::DESKTOP_ID.lock().unwrap_or_else(|e| e.into_inner()) = Some(id.to_owned());
    }
    #[cfg(not(target_os = "linux"))]
    let _ = id;
}

/// Overlay icons are 16 px squares at 100 % scale.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(crate) const OVERLAY_SIZE: usize = 16;

/// What fits on a 16 px overlay: up to two digits, else `99+`.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(crate) fn overlay_label(count: Option<u32>) -> Option<String> {
    let count = count.filter(|&count| count > 0)?;
    Some(if count > 99 {
        "99+".to_owned()
    } else {
        count.to_string()
    })
}

/// A 3x5 pixel font for digits and `+`, one row per byte, high bit left.
const GLYPHS: [(char, [u8; 5]); 11] = [
    ('0', [0b111, 0b101, 0b101, 0b101, 0b111]),
    ('1', [0b010, 0b110, 0b010, 0b010, 0b111]),
    ('2', [0b111, 0b001, 0b111, 0b100, 0b111]),
    ('3', [0b111, 0b001, 0b111, 0b001, 0b111]),
    ('4', [0b101, 0b101, 0b111, 0b001, 0b001]),
    ('5', [0b111, 0b100, 0b111, 0b001, 0b111]),
    ('6', [0b111, 0b100, 0b111, 0b101, 0b111]),
    ('7', [0b111, 0b001, 0b001, 0b001, 0b001]),
    ('8', [0b111, 0b101, 0b111, 0b101, 0b111]),
    ('9', [0b111, 0b101, 0b111, 0b001, 0b111]),
    ('+', [0b000, 0b010, 0b111, 0b010, 0b000]),
];

/// Straight RGBA pixels of an overlay icon: a red disc with `label` in
/// white, doubled in size when it is one character.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(crate) fn overlay_pixels(label: &str) -> Vec<u8> {
    const RED: [u8; 4] = [220, 38, 38, 255];
    const WHITE: [u8; 4] = [255, 255, 255, 255];
    let size = OVERLAY_SIZE;
    let mut pixels = vec![0u8; size * size * 4];
    let center = (size as f32 - 1.0) / 2.0;
    let radius = size as f32 / 2.0;
    for y in 0..size {
        for x in 0..size {
            let (dx, dy) = (x as f32 - center, y as f32 - center);
            if dx * dx + dy * dy <= radius * radius {
                pixels[(y * size + x) * 4..][..4].copy_from_slice(&RED);
            }
        }
    }

    let glyphs: Vec<[u8; 5]> = label
        .chars()
        .filter_map(|ch| GLYPHS.iter().find(|(c, _)| *c == ch).map(|(_, g)| *g))
        .collect();
    if glyphs.is_empty() {
        return pixels;
    }
    let scale = if glyphs.len() == 1 { 2 } else { 1 };
    let width = glyphs.len() * 4 * scale - scale;
    let left = size.saturating_sub(width) / 2;
    let top = (size - 5 * scale) / 2;
    for (i, glyph) in glyphs.iter().enumerate() {
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..3 {
                if bits & (0b100 >> col) == 0 {
                    continue;
                }
                for sy in 0..scale {
                    for sx in 0..scale {
                        let x = left + (i * 4 + col) * scale + sx;
                        let y = top + row * scale + sy;
                        if x < size && y < size {
                            pixels[(y * size + x) * 4..][..4].copy_from_slice(&WHITE);
                        }
                    }
                }
            }
        }
    }
    pixels
}

#[cfg(target_os = "macos")]
pub(crate) fn set_dock_badge(count: Option<u32>) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::NSString;

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let label = badge_label(count).map(|label| NSString::from_str(&label));
    NSApplication::sharedApplication(mtm)
        .dockTile()
        .setBadgeLabel(label.as_deref());
}

#[cfg(windows)]
pub(crate) use windows_overlay::set_overlay;

#[cfg(windows)]
mod windows_overlay {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    };
    use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateIconIndirect, DestroyIcon, HICON, ICONINFO,
    };
    use windows::core::{HSTRING, PCWSTR};
    use winit::window::Window;

    use super::{OVERLAY_SIZE, overlay_label, overlay_pixels};

    /// Show `count` on the taskbar buttons of `windows`.
    pub(crate) fn set_overlay<'a>(windows: impl Iterator<Item = &'a Window>, count: Option<u32>) {
        let label = overlay_label(count);
        // SAFETY: plain COM and GDI calls on the main thread. The icon is
        // destroyed after use; the taskbar keeps its own copy.
        unsafe {
            // winit already initializes OLE on this thread; this only covers
            // the case where it did not, and its error is expected otherwise.
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let taskbar: ITaskbarList3 =
                match CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER) {
                    Ok(taskbar) => taskbar,
                    Err(error) => {
                        tracing::warn!("no taskbar for the badge: {error}");
                        return;
                    }
                };
            if taskbar.HrInit().is_err() {
                return;
            }
            let icon = label
                .as_deref()
                .and_then(|label| icon(&overlay_pixels(label)));
            let description = HSTRING::from(label.as_deref().unwrap_or(""));
            for window in windows {
                let Some(hwnd) = hwnd(window) else { continue };
                let result = taskbar.SetOverlayIcon(
                    hwnd,
                    icon.unwrap_or_default(),
                    PCWSTR(description.as_ptr()),
                );
                if let Err(error) = result {
                    tracing::debug!("could not set the taskbar badge: {error}");
                }
            }
            if let Some(icon) = icon {
                let _ = DestroyIcon(icon);
            }
        }
    }

    fn hwnd(window: &Window) -> Option<HWND> {
        match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(HWND(handle.hwnd.get() as _)),
            _ => None,
        }
    }

    /// An icon from straight RGBA pixels.
    unsafe fn icon(rgba: &[u8]) -> Option<HICON> {
        let size = OVERLAY_SIZE as i32;
        let bgra: Vec<u8> = rgba
            .chunks_exact(4)
            .flat_map(|p| [p[2], p[1], p[0], p[3]])
            .collect();
        // An all-zero mask: the color bitmap's alpha decides transparency.
        let mask = [0u8; OVERLAY_SIZE * OVERLAY_SIZE / 8];
        unsafe {
            let color = CreateBitmap(size, size, 1, 32, Some(bgra.as_ptr().cast()));
            let mask = CreateBitmap(size, size, 1, 1, Some(mask.as_ptr().cast()));
            let info = ICONINFO {
                fIcon: true.into(),
                xHotspot: 0,
                yHotspot: 0,
                hbmMask: mask,
                hbmColor: color,
            };
            let icon = CreateIconIndirect(&info).ok();
            let _ = DeleteObject(color.into());
            let _ = DeleteObject(mask.into());
            icon
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) use linux::set_launcher_badge;

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::HashMap;
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::sync::{Mutex, OnceLock};

    use zbus::zvariant::Value;

    pub(super) static DESKTOP_ID: Mutex<Option<String>> = Mutex::new(None);

    /// Counts for the badge thread, which owns the D-Bus connection: docks
    /// drop an entry when the connection that set it closes.
    static WORKER: OnceLock<Mutex<Sender<Option<u32>>>> = OnceLock::new();

    pub(crate) fn set_launcher_badge(count: Option<u32>) {
        let worker = WORKER.get_or_init(|| {
            let (sender, receiver) = mpsc::channel();
            let spawned = std::thread::Builder::new()
                .name("quark-badge".to_owned())
                .spawn(move || {
                    if let Err(error) = zbus::block_on(run(receiver)) {
                        tracing::debug!("no launcher badge: {error}");
                    }
                });
            if let Err(error) = spawned {
                tracing::warn!("could not start the badge thread: {error}");
            }
            Mutex::new(sender)
        });
        let _ = worker.lock().unwrap_or_else(|e| e.into_inner()).send(count);
    }

    async fn run(counts: Receiver<Option<u32>>) -> zbus::Result<()> {
        let connection = zbus::Connection::session().await?;
        let path = format!("/com/canonical/unity/launcherentry/{}", std::process::id());
        while let Ok(count) = counts.recv() {
            let uri = format!("application://{}.desktop", desktop_id());
            let count = count.unwrap_or(0);
            let mut properties: HashMap<&str, Value> = HashMap::new();
            properties.insert("count", Value::from(i64::from(count)));
            properties.insert("count-visible", Value::from(count > 0));
            connection
                .emit_signal(
                    None::<&str>,
                    path.as_str(),
                    "com.canonical.Unity.LauncherEntry",
                    "Update",
                    &(uri, properties),
                )
                .await?;
        }
        Ok(())
    }

    fn desktop_id() -> String {
        if let Some(id) = DESKTOP_ID.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return id;
        }
        std::env::current_exe()
            .ok()
            .and_then(|exe| Some(exe.file_stem()?.to_string_lossy().into_owned()))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badge_text_clears_at_zero_and_caps_on_the_overlay() {
        // (count, badge label, overlay label)
        let cases = [
            (None, None, None),
            (Some(0), None, None),
            (Some(7), Some("7"), Some("7")),
            (Some(99), Some("99"), Some("99")),
            (Some(1234), Some("1234"), Some("99+")),
        ];
        for (count, label, overlay) in cases {
            assert_eq!(badge_label(count).as_deref(), label, "{count:?}");
            assert_eq!(overlay_label(count).as_deref(), overlay, "{count:?}");
        }
    }

    #[test]
    fn overlay_draws_the_label_inside_a_red_disc() {
        let pixels = overlay_pixels("99+");
        let at = |x: usize, y: usize| &pixels[(y * OVERLAY_SIZE + x) * 4..][..4];
        assert_eq!(at(0, 0)[3], 0, "corners stay transparent");
        assert_eq!(at(8, 1), [220, 38, 38, 255], "disc edge is red");
        // The middle row of "99+": the 9s' bar and the plus's bar.
        let row: String = (0..OVERLAY_SIZE)
            .map(|x| if at(x, 7) == [255; 4] { '#' } else { '.' })
            .collect();
        assert_eq!(row, "..###.###.###...");
    }
}
