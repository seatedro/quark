//! Installed fonts changing while the app runs: the runner rescans them
//! ([`quark_text::TextSystem::reload_system_fonts`]) and redraws, so text
//! in a newly installed system font, or the platform's new UI font, shows
//! without a restart.
//!
//! macOS posts `kCTFontManagerRegisteredFontsChangedNotification`, which
//! wakes the loop. Windows broadcasts `WM_FONTCHANGE` to top-level windows,
//! which winit does not pass on, and fontconfig has no change notification,
//! so elsewhere the runner compares the font directories' modification
//! times when a window gains focus: fonts are installed from another app,
//! which takes focus from the window.

use std::path::PathBuf;
use std::time::SystemTime;

/// The font directories' modification times, to compare on focus.
pub(crate) struct FontDirs {
    dirs: Vec<PathBuf>,
    stamps: Vec<Option<SystemTime>>,
}

impl FontDirs {
    /// The platform's system and user font directories.
    pub(crate) fn new() -> Self {
        Self::of(font_dirs())
    }

    fn of(dirs: Vec<PathBuf>) -> Self {
        let stamps = stamps(&dirs);
        Self { dirs, stamps }
    }

    /// Whether a font directory (or one directly inside it) was modified
    /// since the last call. A few dozen `stat`s at most.
    pub(crate) fn changed(&mut self) -> bool {
        let now = stamps(&self.dirs);
        let changed = now != self.stamps;
        self.stamps = now;
        changed
    }
}

fn stamps(dirs: &[PathBuf]) -> Vec<Option<SystemTime>> {
    let mut out = Vec::new();
    for dir in dirs {
        let modified = |path: &std::path::Path| path.metadata().and_then(|m| m.modified()).ok();
        out.push(modified(dir));
        // Package managers install into subdirectories (fonts/truetype/x),
        // which leave the top directory's time alone.
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut subdirs: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .collect();
        subdirs.sort();
        out.extend(subdirs.iter().map(|d| modified(d)));
    }
    out
}

fn font_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut dirs = Vec::new();
    if cfg!(target_os = "macos") {
        dirs.push(PathBuf::from("/Library/Fonts"));
        dirs.extend(home.map(|h| h.join("Library/Fonts")));
    } else if cfg!(windows) {
        if let Some(windir) = std::env::var_os("WINDIR") {
            dirs.push(PathBuf::from(windir).join("Fonts"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("Microsoft/Windows/Fonts"));
        }
    } else {
        dirs.push(PathBuf::from("/usr/share/fonts"));
        dirs.push(PathBuf::from("/usr/local/share/fonts"));
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(".local/share")));
        dirs.extend(data.map(|d| d.join("fonts")));
        dirs.extend(home.map(|h| h.join(".fonts")));
    }
    dirs
}

/// Wake the event loop through `waker` when the platform reports that the
/// installed fonts changed; see [`take_change`]. Once per process.
pub(crate) fn watch(waker: &crate::runner::Waker) {
    #[cfg(target_os = "macos")]
    macos::watch(waker);
    let _ = waker;
}

/// Whether the platform reported a font change since the last call.
pub(crate) fn take_change() -> bool {
    #[cfg(target_os = "macos")]
    return macos::take_change();
    #[cfg(not(target_os = "macos"))]
    false
}

#[cfg(target_os = "macos")]
mod macos {
    use std::sync::Once;
    use std::sync::atomic::{AtomicBool, Ordering};

    use block2::RcBlock;
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_foundation::NSString;

    /// Set by the notification observer, taken by the event loop.
    static CHANGED: AtomicBool = AtomicBool::new(false);

    pub(super) fn watch(waker: &crate::runner::Waker) {
        static WATCH: Once = Once::new();
        WATCH.call_once(|| {
            let Some(class) = AnyClass::get(c"NSNotificationCenter") else {
                return;
            };
            let waker = waker.clone();
            let block = RcBlock::new(move |_note: *mut AnyObject| {
                CHANGED.store(true, Ordering::Release);
                waker.wake();
            });
            // kCTFontManagerRegisteredFontsChangedNotification's value. Core
            // Text posts it on the local center, which the default
            // NSNotificationCenter is.
            let name = NSString::from_str("CTFontManagerFontChangedNotification");
            // SAFETY: the default notification center and its block-based
            // observer API; a nil object and queue deliver every such
            // notification on the posting thread, and the block only
            // stores an atomic and wakes the loop.
            unsafe {
                let center: Retained<AnyObject> = msg_send![class, defaultCenter];
                let observer: Option<Retained<AnyObject>> = msg_send![
                    &center,
                    addObserverForName: &*name,
                    object: std::ptr::null::<AnyObject>(),
                    queue: std::ptr::null::<AnyObject>(),
                    usingBlock: &*block
                ];
                // Observed for the life of the process.
                std::mem::forget(observer);
            }
        });
    }

    pub(super) fn take_change() -> bool {
        CHANGED.swap(false, Ordering::AcqRel)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    // Catches a font installed into a vendor subdirectory going unnoticed:
    // the subdirectory's new time changes the snapshot, and an unchanged
    // tree does not.
    #[test]
    fn a_font_added_to_a_subdirectory_changes_the_snapshot() {
        let root = std::env::temp_dir().join(format!("quark-font-dirs-{}", std::process::id()));
        let sub = root.join("truetype");
        std::fs::create_dir_all(&sub).unwrap();
        let mut dirs = FontDirs::of(vec![root.clone()]);
        assert!(!dirs.changed());
        std::fs::write(sub.join("new.ttf"), b"").unwrap();
        // Set the time the write gave the directory explicitly, so a
        // filesystem with coarse times cannot hide it.
        let at = UNIX_EPOCH + Duration::from_secs(1_000_000);
        std::fs::File::open(&sub).unwrap().set_modified(at).unwrap();
        assert!(dirs.changed());
        assert!(!dirs.changed());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
