//! CoreText's answer for the system UI font, and the system monospaced
//! face. The UI font's family (`.AppleSystemUIFont`) is private, so the
//! face is found by the file CoreText reports and its PostScript name,
//! not by a public family name or an assumed file.

use std::ffi::{c_char, c_void};
use std::path::{Path, PathBuf};

use super::FontRole;
use super::platform::Candidate;

type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CTFontRef = *const c_void;

/// `kCTFontUIFontUserFixedPitch` and `kCTFontUIFontSystem` (CTFont.h).
const UI_FONT_USER_FIXED_PITCH: u32 = 1;
const UI_FONT_SYSTEM: u32 = 2;
/// `kCFStringEncodingUTF8`.
const UTF8: u32 = 0x0800_0100;

/// AppKit's `monospacedSystemFont` (SF Mono) has no file URL CoreText
/// reports, and its private names cannot be created by name; this is the
/// face behind it since macOS 10.15.
const SYSTEM_MONO_FILE: &str = "/System/Library/Fonts/SFNSMono.ttf";

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: CFTypeRef);
    fn CFStringGetCString(
        string: CFStringRef,
        buffer: *mut c_char,
        size: isize,
        encoding: u32,
    ) -> u8;
    fn CFURLGetFileSystemRepresentation(
        url: CFTypeRef,
        resolve_against_base: u8,
        buffer: *mut u8,
        max_len: isize,
    ) -> u8;
}

#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    static kCTFontURLAttribute: CFStringRef;
    fn CTFontCreateUIFontForLanguage(ui_type: u32, size: f64, language: CFStringRef) -> CTFontRef;
    fn CTFontCopyFamilyName(font: CTFontRef) -> CFStringRef;
    fn CTFontCopyPostScriptName(font: CTFontRef) -> CFStringRef;
    fn CTFontCopyAttribute(font: CTFontRef, attribute: CFStringRef) -> CFTypeRef;
}

pub(crate) fn candidates(role: FontRole) -> Vec<Candidate> {
    match role {
        FontRole::Ui => ui_font(UI_FONT_SYSTEM).into_iter().collect(),
        FontRole::Mono => {
            let system_mono = Path::new(SYSTEM_MONO_FILE).exists().then(|| Candidate {
                family: None,
                path: Some(PathBuf::from(SYSTEM_MONO_FILE)),
                post_script_name: None,
                from_api: true,
            });
            // The user's fixed-pitch font, Menlo unless changed.
            let fixed = ui_font(UI_FONT_USER_FIXED_PITCH);
            system_mono
                .into_iter()
                .chain(fixed)
                .chain([Candidate::family("Menlo", false)])
                .collect()
        }
    }
}

/// The face CoreText picks for a UI font type, at the default size.
fn ui_font(ui_type: u32) -> Option<Candidate> {
    // SAFETY: CoreText's documented create/copy calls; every object they
    // return is owned here and released once, and strings are copied out
    // before release.
    unsafe {
        let font = CTFontCreateUIFontForLanguage(ui_type, 0.0, std::ptr::null());
        if font.is_null() {
            return None;
        }
        let family = take_string(CTFontCopyFamilyName(font));
        let post_script_name = take_string(CTFontCopyPostScriptName(font));
        let url = CTFontCopyAttribute(font, kCTFontURLAttribute);
        let path = (!url.is_null()).then(|| {
            let mut buffer = [0u8; 1024];
            let ok = CFURLGetFileSystemRepresentation(url, 1, buffer.as_mut_ptr(), 1024);
            CFRelease(url);
            let len = buffer.iter().position(|&b| b == 0).unwrap_or(0);
            (ok != 0 && len > 0)
                .then(|| PathBuf::from(String::from_utf8_lossy(&buffer[..len]).into_owned()))
        });
        CFRelease(font);
        Some(Candidate {
            family,
            path: path.flatten(),
            post_script_name,
            from_api: true,
        })
    }
}

/// The contents of an owned CFString, which this releases.
unsafe fn take_string(string: CFStringRef) -> Option<String> {
    if string.is_null() {
        return None;
    }
    let mut buffer = [0 as c_char; 512];
    // SAFETY: a live CFString and a buffer of the size passed.
    let ok = unsafe { CFStringGetCString(string, buffer.as_mut_ptr(), 512, UTF8) };
    // SAFETY: the caller owns `string`.
    unsafe { CFRelease(string) };
    if ok == 0 {
        return None;
    }
    // SAFETY: CFStringGetCString wrote a NUL-terminated string.
    let text = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) };
    Some(text.to_string_lossy().into_owned())
}
