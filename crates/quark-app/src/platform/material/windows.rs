//! Windows 11: DWM system backdrops and corner preferences.
//!
//! A backdrop belongs to the whole window. With the frame extended over
//! the client area it shows through every pixel the swapchain leaves
//! transparent; the swapchain decides whether any are.

use std::ffi::c_void;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DWM_SYSTEMBACKDROP_TYPE, DWM_WINDOW_CORNER_PREFERENCE, DWMSBT_MAINWINDOW, DWMSBT_NONE,
    DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_DEFAULT, DWMWCP_DONOTROUND, DWMWCP_ROUND, DWMWCP_ROUNDSMALL, DwmSetWindowAttribute,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
};
use windows::core::{HSTRING, PCWSTR};
use winit::window::Window;

use super::{CornerResult, EffectiveBackground, MaterialKind, MaterialRect, WindowSurface};

/// `MARGINS`, which the windows crate only has with its Controls feature.
#[repr(C)]
struct Margins {
    left: i32,
    right: i32,
    top: i32,
    bottom: i32,
}

#[link(name = "dwmapi", kind = "raw-dylib")]
unsafe extern "system" {
    fn DwmExtendFrameIntoClientArea(hwnd: HWND, margins: *const Margins) -> i32;
}

#[derive(Default)]
pub(crate) struct NativeMaterial;

fn hwnd(window: &Window) -> Option<HWND> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(HWND(handle.hwnd.get() as *mut c_void)),
        _ => None,
    }
}

/// Mica for long-lived surfaces, Desktop Acrylic for transient ones.
fn backdrop(kind: MaterialKind) -> DWM_SYSTEMBACKDROP_TYPE {
    match kind {
        MaterialKind::Popover
        | MaterialKind::Menu
        | MaterialKind::Tooltip
        | MaterialKind::Hud
        | MaterialKind::Sheet => DWMSBT_TRANSIENTWINDOW,
        _ => DWMSBT_MAINWINDOW,
    }
}

fn set<T>(
    hwnd: HWND,
    attribute: windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE,
    value: &T,
) -> bool {
    // SAFETY: a live HWND and a value of the attribute's documented type.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            attribute,
            std::ptr::from_ref(value).cast(),
            std::mem::size_of::<T>() as u32,
        )
        .is_ok()
    }
}

impl NativeMaterial {
    /// Give `window` the backdrop and corner preference `surface` resolved
    /// to. Returns false when DWM refused the backdrop.
    pub(crate) fn apply(&mut self, window: &Window, surface: &WindowSurface) -> bool {
        let Some(hwnd) = hwnd(window) else {
            return false;
        };
        let corner: DWM_WINDOW_CORNER_PREFERENCE = match surface.corners {
            CornerResult::Square => DWMWCP_DONOTROUND,
            CornerResult::Approximate { native, .. } if native < 6.0 => DWMWCP_ROUNDSMALL,
            CornerResult::Approximate { .. } => DWMWCP_ROUND,
            _ => DWMWCP_DEFAULT,
        };
        // Older builds reject the attribute; their corners stay square.
        set(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner);
        let (backdrop, extend) = match surface.background {
            EffectiveBackground::Material { kind, .. } => (backdrop(kind), -1),
            _ => (DWMSBT_NONE, 0),
        };
        let margins = Margins {
            left: extend,
            right: extend,
            top: extend,
            bottom: extend,
        };
        // SAFETY: a live HWND and a MARGINS-shaped struct.
        unsafe { DwmExtendFrameIntoClientArea(hwnd, &margins) };
        set(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &backdrop)
    }

    /// One backdrop serves the whole window here.
    pub(crate) fn set_regions(&mut self, _window: &Window, _regions: &[MaterialRect]) {}
}

/// A registry value under `key`.
fn registry<T>(
    mut value: T,
    key: HKEY,
    path: &str,
    name: &str,
    kind: windows::Win32::System::Registry::REG_ROUTINE_FLAGS,
) -> Option<T> {
    let mut size = std::mem::size_of::<T>() as u32;
    let (path, name) = (HSTRING::from(path), HSTRING::from(name));
    // SAFETY: a value buffer of `size` bytes.
    let status = unsafe {
        RegGetValueW(
            key,
            PCWSTR(path.as_ptr()),
            PCWSTR(name.as_ptr()),
            kind,
            None,
            Some(std::ptr::from_mut(&mut value).cast()),
            Some(&mut size),
        )
    };
    status.is_ok().then_some(value)
}

/// A string registry value, as UTF-16 up to its terminator.
fn registry_string(key: HKEY, path: &str, name: &str) -> Option<String> {
    let buffer = registry([0u16; 64], key, path, name, RRF_RT_REG_SZ)?;
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..end]))
}

/// The Windows build number, as `CurrentBuildNumber` in the registry
/// reports it (GetVersionEx lies to unmanifested apps).
pub(crate) fn build() -> Option<u32> {
    registry_string(
        HKEY_LOCAL_MACHINE,
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "CurrentBuildNumber",
    )?
    .trim()
    .parse()
    .ok()
}

/// "Transparency effects" turned off, and high contrast turned on.
pub(crate) fn accessibility() -> (bool, bool) {
    let transparency = registry(
        0u32,
        HKEY_CURRENT_USER,
        r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
        "EnableTransparency",
        RRF_RT_REG_DWORD,
    );
    // HCF_HIGHCONTRASTON is bit 0 of the flags, stored as a decimal string.
    let contrast = registry_string(
        HKEY_CURRENT_USER,
        r"Control Panel\Accessibility\HighContrast",
        "Flags",
    )
    .and_then(|flags| flags.trim().parse::<u32>().ok())
    .is_some_and(|flags| flags & 1 != 0);
    (transparency == Some(0), contrast)
}
