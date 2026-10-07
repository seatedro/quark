//! Dragging files out of the app, onto Finder, Explorer, a desktop, or
//! another app's window, with the app as the native drag source.
//!
//! Call [`crate::EventContext::start_drag_out`] (or
//! [`crate::UiContext::start_drag_out`]) while the primary button is held,
//! from the action a pointer drag delivers once it leaves a threshold. The
//! platform takes over the pointer from there: the app gets no release for
//! the drag it was tracking, so the UI adapter ends its pointer capture
//! (delivering the drag's release) when a drag out starts.
//!
//! | Platform | Status |
//! |---|---|
//! | macOS | `NSView beginDraggingSession` with file URLs and Finder icons; returns at once, the drag runs on the event loop |
//! | Windows | `SHDoDragDrop` over the shell's data object for the files (`CF_HDROP` and friends); blocks in OLE's drag loop until the drop |
//! | Linux, BSD | [`DragOutError::Unsupported`]: winit owns the X11 and Wayland connections and exposes no drag source, and XDND or `wl_data_device.start_drag` cannot be driven from outside it |

use std::path::{Path, PathBuf};

use winit::window::Window;

/// Why a drag out did not start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DragOutError {
    /// No paths to drag.
    NoPaths,
    /// The platform has no drag source this crate can drive.
    Unsupported,
    /// The context has no native window (a headless test window).
    NoWindow,
    /// No pointer event is being handled to start the drag from (macOS
    /// starts drags from the current mouse event).
    NoPointerEvent,
    /// The platform refused, with its message.
    Platform(String),
}

impl std::fmt::Display for DragOutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoPaths => f.write_str("no paths to drag"),
            Self::Unsupported => f.write_str("dragging out is not supported on this platform"),
            Self::NoWindow => f.write_str("no native window to drag from"),
            Self::NoPointerEvent => f.write_str("no pointer event to start the drag from"),
            Self::Platform(message) => write!(f, "drag out failed: {message}"),
        }
    }
}

impl std::error::Error for DragOutError {}

/// Whether [`crate::EventContext::start_drag_out`] can work here.
pub const fn supported() -> bool {
    cfg!(any(target_os = "macos", windows))
}

/// Absolute paths for the platform, or why there are none.
pub(crate) fn absolute_paths<P: AsRef<Path>>(
    paths: impl IntoIterator<Item = P>,
) -> Result<Vec<PathBuf>, DragOutError> {
    let paths = paths
        .into_iter()
        .map(|path| std::path::absolute(path.as_ref()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| DragOutError::Platform(error.to_string()))?;
    if paths.is_empty() {
        return Err(DragOutError::NoPaths);
    }
    Ok(paths)
}

/// Start dragging `paths` (absolute, non-empty) from `window`.
#[allow(unused_variables)]
pub(crate) fn start(window: &Window, paths: &[PathBuf]) -> Result<(), DragOutError> {
    #[cfg(target_os = "macos")]
    return macos::start(window, paths);
    #[cfg(windows)]
    return windows_shell::start(window, paths);
    #[cfg(not(any(target_os = "macos", windows)))]
    Err(DragOutError::Unsupported)
}

#[cfg(target_os = "macos")]
mod macos {
    use std::cell::RefCell;
    use std::path::PathBuf;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send};
    use objc2_app_kit::{
        NSApplication, NSDragOperation, NSDraggingContext, NSDraggingItem, NSDraggingSession,
        NSDraggingSource, NSView, NSWorkspace,
    };
    use objc2_foundation::{NSArray, NSPoint, NSRect, NSSize, NSString, NSURL};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::Window;

    use super::DragOutError;

    define_class!(
        /// Offers the files as a copy wherever they are dropped.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "QuarkDragOutSource"]
        struct Source;

        unsafe impl NSObjectProtocol for Source {}

        unsafe impl NSDraggingSource for Source {
            #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
            fn operation_mask(
                &self,
                _session: &NSDraggingSession,
                _context: NSDraggingContext,
            ) -> NSDragOperation {
                NSDragOperation::Copy
            }
        }
    );

    thread_local! {
        /// The last drag's source, kept alive for the session: AppKit does
        /// not document that the session retains it.
        static SOURCE: RefCell<Option<Retained<Source>>> = const { RefCell::new(None) };
    }

    /// Side of the icon each file drags with, in points.
    const ICON: f64 = 32.0;

    pub(super) fn start(window: &Window, paths: &[PathBuf]) -> Result<(), DragOutError> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| DragOutError::Platform("not on the main thread".into()))?;
        let handle = window
            .window_handle()
            .map_err(|error| DragOutError::Platform(error.to_string()))?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return Err(DragOutError::Unsupported);
        };
        // SAFETY: winit's AppKit handle points at the window's live NSView,
        // and this runs on the main thread.
        let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        let event = NSApplication::sharedApplication(mtm)
            .currentEvent()
            .ok_or(DragOutError::NoPointerEvent)?;
        let at = view.convertPoint_fromView(event.locationInWindow(), None);
        let workspace = NSWorkspace::sharedWorkspace();
        let items: Vec<Retained<NSDraggingItem>> = paths
            .iter()
            .enumerate()
            .map(|(i, path)| {
                let path = NSString::from_str(&path.to_string_lossy());
                let url = NSURL::fileURLWithPath(&path);
                let item = NSDraggingItem::initWithPasteboardWriter(
                    NSDraggingItem::alloc(),
                    ProtocolObject::from_ref(&*url),
                );
                // Fan the icons out a little under the pointer.
                let offset = i as f64 * 6.0;
                let frame = NSRect::new(
                    NSPoint::new(at.x - ICON / 2.0 + offset, at.y - ICON / 2.0 - offset),
                    NSSize::new(ICON, ICON),
                );
                let icon = workspace.iconForFile(&path);
                let contents: &AnyObject = &icon;
                // SAFETY: an NSImage is valid dragging-frame contents.
                unsafe { item.setDraggingFrame_contents(frame, Some(contents)) };
                item
            })
            .collect();
        let items = NSArray::from_retained_slice(&items);
        let source: Retained<Source> = unsafe { msg_send![Source::alloc(mtm), init] };
        view.beginDraggingSessionWithItems_event_source(
            &items,
            &event,
            ProtocolObject::from_ref(&*source),
        );
        SOURCE.with(|slot| *slot.borrow_mut() = Some(source));
        Ok(())
    }
}

#[cfg(windows)]
mod windows_shell {
    use std::path::PathBuf;

    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{IBindCtx, IDataObject};
    use windows::Win32::System::Ole::{DROPEFFECT_COPY, IDropSource};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        BHID_DataObject, ILFree, SHCreateShellItemArrayFromIDLists, SHDoDragDrop,
        SHParseDisplayName,
    };
    use windows::core::HSTRING;
    use winit::window::Window;

    use super::DragOutError;

    pub(super) fn start(window: &Window, paths: &[PathBuf]) -> Result<(), DragOutError> {
        let handle = window
            .window_handle()
            .map_err(|error| DragOutError::Platform(error.to_string()))?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return Err(DragOutError::Unsupported);
        };
        let hwnd = HWND(handle.hwnd.get() as *mut _);
        let mut pidls: Vec<*const ITEMIDLIST> = Vec::with_capacity(paths.len());
        // SAFETY: plain shell and OLE calls on the UI thread, where winit
        // has initialized OLE for its drop target. Every ID list parsed is
        // freed below, whatever happens.
        let result = unsafe {
            (|| -> windows::core::Result<()> {
                for path in paths {
                    let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
                    SHParseDisplayName(
                        &HSTRING::from(path.as_os_str()),
                        None::<&IBindCtx>,
                        &mut pidl,
                        0,
                        None,
                    )?;
                    pidls.push(pidl);
                }
                let items = SHCreateShellItemArrayFromIDLists(&pidls)?;
                let data: IDataObject = items.BindToHandler(None::<&IBindCtx>, &BHID_DataObject)?;
                // A null drop source gets the shell's default one.
                SHDoDragDrop(Some(hwnd), &data, None::<&IDropSource>, DROPEFFECT_COPY)?;
                Ok(())
            })()
        };
        for pidl in pidls {
            // SAFETY: each came from SHParseDisplayName and is freed once.
            unsafe { ILFree(Some(pidl)) };
        }
        result.map_err(|error| DragOutError::Platform(error.message()))
    }
}
