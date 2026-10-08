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
//! [`crate::EventContext::start_drag_out_with_options`] takes a
//! [`DragOutOptions`]: a [`DragImage`] to show under the pointer instead of
//! the platform's default, and on Wayland the seat to drag with.
//!
//! | Platform | Status |
//! |---|---|
//! | macOS | `NSView beginDraggingSession` with file URLs and Finder icons, or the given image; returns at once, the drag runs on the event loop |
//! | Windows | `SHDoDragDrop` over the shell's data object for the files (`CF_HDROP` and friends), with the given image set through `IDragSourceHelper`; blocks in OLE's drag loop until the drop |
//! | Linux, Wayland | `wl_data_device.start_drag` with a `text/uri-list` source and an icon surface, on a queue of our own over winit's `wl_display` that a thread dispatches, from the seat holding the press; returns at once |
//! | Linux, X11 | The source side of XDND on a connection of our own, run by a thread that grabs the pointer from winit, with the image in a window of its own; returns at once |
//! | BSD | [`DragOutError::Unsupported`] |
//!
//! Linux has no desktop-independent file icon to fall back on, so without
//! an image it drags a page drawn here, two stacked for several files.

use std::path::{Path, PathBuf};

use winit::window::Window;

mod image;

pub use image::{DragImage, DragImageError};

/// How a drag out looks and which pointer it follows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DragOutOptions {
    image: Option<DragImage>,
    seat: Option<String>,
}

impl DragOutOptions {
    /// Show `image` under the pointer instead of the platform's file icons.
    pub fn image(mut self, image: DragImage) -> Self {
        self.image = Some(image);
        self
    }

    /// Drag with the Wayland seat of this name (`wl_seat.name`, such as
    /// `seat0`). Without one the drag takes the only seat holding the
    /// primary button on the window, and fails with
    /// [`DragOutError::AmbiguousSeat`] if several are. Other platforms have
    /// one pointer and ignore it.
    pub fn seat(mut self, name: impl Into<String>) -> Self {
        self.seat = Some(name.into());
        self
    }
}

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
    /// starts drags from the current mouse event, Wayland from the serial
    /// of the press that is still held).
    NoPointerEvent,
    /// Several Wayland seats hold the primary button on the window; name
    /// one with [`DragOutOptions::seat`].
    AmbiguousSeat,
    /// No Wayland seat has the name [`DragOutOptions::seat`] gave.
    UnknownSeat(String),
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
            Self::AmbiguousSeat => {
                f.write_str("several seats hold the button; name the one to drag with")
            }
            Self::UnknownSeat(name) => write!(f, "no seat named {name:?}"),
            Self::Platform(message) => write!(f, "drag out failed: {message}"),
        }
    }
}

impl std::error::Error for DragOutError {}

/// Whether [`crate::EventContext::start_drag_out`] can work here.
pub const fn supported() -> bool {
    cfg!(any(target_os = "macos", windows, target_os = "linux"))
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
pub(crate) fn start(
    window: &Window,
    paths: &[PathBuf],
    options: &DragOutOptions,
) -> Result<(), DragOutError> {
    #[cfg(target_os = "macos")]
    return macos::start(window, paths, options.image.as_ref());
    #[cfg(windows)]
    return windows_shell::start(window, paths, options.image.as_ref());
    #[cfg(target_os = "linux")]
    return linux::start(window, paths, options);
    #[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
    Err(DragOutError::Unsupported)
}

#[cfg(target_os = "linux")]
mod seat;
#[cfg(target_os = "linux")]
mod ticket;
#[cfg(target_os = "linux")]
mod wayland;
#[cfg(target_os = "linux")]
mod x11;

#[cfg(target_os = "linux")]
pub(crate) use linux::{shutdown, window_created, window_destroyed};

#[cfg(target_os = "linux")]
mod linux {
    use std::path::PathBuf;

    use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
    use winit::window::Window;

    use super::{DragOutError, DragOutOptions, image, wayland, x11};

    pub(super) fn start(
        window: &Window,
        paths: &[PathBuf],
        options: &DragOutOptions,
    ) -> Result<(), DragOutError> {
        let platform =
            |error: raw_window_handle::HandleError| DragOutError::Platform(error.to_string());
        let display = window.display_handle().map_err(platform)?.as_raw();
        let window_handle = window.window_handle().map_err(platform)?;
        let uris = super::uri_list(paths);
        let image = options
            .image
            .clone()
            .unwrap_or_else(|| image::file_icon(paths.len(), window.scale_factor()));
        match (display, window_handle.as_raw()) {
            // The borrowed handle, which keeps the window (and so its
            // wl_surface) alive for as long as the drag thread may use it.
            (RawDisplayHandle::Wayland(_), RawWindowHandle::Wayland(_)) => {
                wayland::start(window_handle, uris, options.seat.clone(), &image)
            }
            (RawDisplayHandle::Xlib(display), RawWindowHandle::Xlib(_)) => {
                let display = display
                    .display
                    .ok_or_else(|| DragOutError::Platform("no Xlib display".into()))?;
                x11::start(display.as_ptr(), uris, &image)
            }
            _ => Err(DragOutError::Unsupported),
        }
    }

    /// Set up for drags from a new window. On Wayland this binds the
    /// pointers whose button serials `start_drag` needs, so it must run
    /// before the press that starts a drag.
    pub(crate) fn window_created(window: &Window) {
        let (Ok(display), Ok(handle)) = (window.display_handle(), window.window_handle()) else {
            return;
        };
        if let (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(handle)) =
            (display.as_raw(), handle.as_raw())
        {
            wayland::init(display.display.as_ptr());
            wayland::window_created(handle.surface.as_ptr());
        }
    }

    /// Forget a window that is closing, and the presses made on it.
    pub(crate) fn window_destroyed(window: &Window) {
        if let Ok(handle) = window.window_handle()
            && let RawWindowHandle::Wayland(handle) = handle.as_raw()
        {
            wayland::window_destroyed(handle.surface.as_ptr());
        }
    }

    /// Stop using winit's Wayland display, which closes after this.
    pub(crate) fn shutdown() {
        wayland::shutdown();
    }
}

/// `paths` as a `text/uri-list` of `file://` URIs: each path's bytes
/// percent-encoded except unreserved characters and `/`, one per line,
/// CRLF terminated (RFC 2483).
#[cfg(target_os = "linux")]
fn uri_list(paths: &[PathBuf]) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = Vec::new();
    for path in paths {
        out.extend_from_slice(b"file://");
        for &byte in path.as_os_str().as_bytes() {
            if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
                out.push(byte);
            } else {
                out.extend_from_slice(&[
                    b'%',
                    HEX[usize::from(byte >> 4)],
                    HEX[usize::from(byte & 0xF)],
                ]);
            }
        }
        out.extend_from_slice(b"\r\n");
    }
    out
}

#[cfg(target_os = "macos")]
mod macos {
    use std::cell::RefCell;
    use std::path::PathBuf;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send};
    use objc2_app_kit::{
        NSApplication, NSBitmapFormat, NSBitmapImageRep, NSDeviceRGBColorSpace, NSDragOperation,
        NSDraggingContext, NSDraggingItem, NSDraggingSession, NSDraggingSource, NSImage, NSView,
        NSWorkspace,
    };
    use objc2_foundation::{NSArray, NSPoint, NSRect, NSSize, NSString, NSURL};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::Window;

    use super::{DragImage, DragOutError};

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

    pub(super) fn start(
        window: &Window,
        paths: &[PathBuf],
        image: Option<&DragImage>,
    ) -> Result<(), DragOutError> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| DragOutError::Platform("not on the main thread".into()))?;
        let image = image.map(ns_image).transpose()?;
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
        let flipped = view.isFlipped();
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
                if let Some(Prepared {
                    image,
                    size,
                    hotspot: (hx, hy),
                }) = &image
                {
                    // The given image for the first file, its hotspot under
                    // the pointer; the other files ride along unseen.
                    let below = if flipped { *hy } else { size.height - hy };
                    let frame = NSRect::new(NSPoint::new(at.x - hx, at.y - below), *size);
                    let object: &AnyObject = image;
                    let contents = (i == 0).then_some(object);
                    // SAFETY: an NSImage, or nothing, is valid dragging-frame
                    // contents.
                    unsafe { item.setDraggingFrame_contents(frame, contents) };
                    return item;
                }
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

    /// A drag image as AppKit takes it.
    struct Prepared {
        image: Retained<NSImage>,
        /// In points.
        size: NSSize,
        /// In points from the top left.
        hotspot: (f64, f64),
    }

    fn ns_image(image: &DragImage) -> Result<Prepared, DragOutError> {
        let (width, height) = (image.width() as isize, image.height() as isize);
        // SAFETY: null planes have the rep allocate its own buffer, laid out
        // by the other arguments: `width * 4` bytes a row, `height` rows.
        // NSDeviceRGBColorSpace is an AppKit constant.
        let rep = unsafe {
            NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
                NSBitmapImageRep::alloc(),
                std::ptr::null_mut(),
                width,
                height,
                8,
                4,
                true,
                false,
                NSDeviceRGBColorSpace,
                NSBitmapFormat::AlphaNonpremultiplied,
                width * 4,
                32,
            )
        }
        .ok_or_else(|| DragOutError::Platform("could not make the drag image".into()))?;
        let pixels = image.rgba();
        // SAFETY: the rep's buffer holds exactly `width * height * 4` bytes,
        // which a validated DragImage has.
        unsafe { std::ptr::copy_nonoverlapping(pixels.as_ptr(), rep.bitmapData(), pixels.len()) };
        let scale = image.scale();
        let size = NSSize::new(width as f64 / scale, height as f64 / scale);
        rep.setSize(size);
        let ns_image = NSImage::initWithSize(NSImage::alloc(), size);
        ns_image.addRepresentation(&rep);
        let (hx, hy) = image.hotspot();
        Ok(Prepared {
            image: ns_image,
            size,
            hotspot: (f64::from(hx) / scale, f64::from(hy) / scale),
        })
    }
}

#[cfg(windows)]
mod windows_shell {
    use std::path::PathBuf;

    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::{COLORREF, HWND, POINT, SIZE};
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS, DeleteObject,
        HBITMAP,
    };
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, CoCreateInstance, IBindCtx, IDataObject,
    };
    use windows::Win32::System::Ole::{DROPEFFECT_COPY, IDropSource};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        BHID_DataObject, CLSID_DragDropHelper, IDragSourceHelper, ILFree,
        SHCreateShellItemArrayFromIDLists, SHDRAGIMAGE, SHDoDragDrop, SHParseDisplayName,
    };
    use windows::core::{HSTRING, IUnknown};
    use winit::window::Window;

    use super::{DragImage, DragOutError};

    pub(super) fn start(
        window: &Window,
        paths: &[PathBuf],
        image: Option<&DragImage>,
    ) -> Result<(), DragOutError> {
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
                // Without its own image the data object gets the shell's.
                if let Some(image) = image
                    && let Err(error) = set_drag_image(&data, image)
                {
                    tracing::warn!("drag out: no drag image: {error}");
                }
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

    /// Hand `image` to the shell's drag image helper for `data`, which
    /// SHDoDragDrop then shows. The helper takes straight alpha, and owns
    /// the bitmap once it accepts it.
    ///
    /// # Safety
    ///
    /// On the UI thread, with COM initialized.
    unsafe fn set_drag_image(data: &IDataObject, image: &DragImage) -> windows::core::Result<()> {
        let (width, height) = (image.width() as i32, image.height() as i32);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // Negative: rows from the top, as the pixels are.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let pixels = image.straight_bgra();
        let mut bits = std::ptr::null_mut();
        // SAFETY: a 32-bit top-down DIB section of the image's size, whose
        // `width * height * 4` bytes of bits the copy fills exactly.
        let bitmap: HBITMAP = unsafe {
            let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u8>(), pixels.len());
            bitmap
        };
        let (hx, hy) = image.hotspot();
        let drag_image = SHDRAGIMAGE {
            sizeDragImage: SIZE {
                cx: width,
                cy: height,
            },
            ptOffset: POINT {
                x: hx as i32,
                y: hy as i32,
            },
            hbmpDragImage: bitmap,
            // Alpha decides what shows; no color is keyed out.
            crColorKey: COLORREF(0xffff_ffff),
        };
        // SAFETY: plain COM calls on the UI thread; the bitmap is deleted
        // only if the helper did not take it.
        unsafe {
            let result = CoCreateInstance::<_, IDragSourceHelper>(
                &CLSID_DragDropHelper,
                None::<&IUnknown>,
                CLSCTX_INPROC_SERVER,
            )
            .and_then(|helper| helper.InitializeFromBitmap(&drag_image, data));
            if result.is_err() {
                let _ = DeleteObject(bitmap.into());
            }
            result
        }
    }
}

#[cfg(all(kani, target_os = "linux"))]
mod verification {
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;

    const LEN: usize = 3;

    fn hex(digit: u8) -> u8 {
        match digit {
            b'0'..=b'9' => digit - b'0',
            b'A'..=b'F' => digit - b'A' + 10,
            _ => panic!("not an uppercase hex digit"),
        }
    }

    /// Any path bytes encode to one `file://` line that holds only URI
    /// characters and decodes back to the same bytes.
    #[kani::proof]
    #[kani::unwind(13)]
    fn uri_list_lines_decode_back_to_the_path_bytes() {
        let bytes: [u8; LEN] = kani::any();
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(&bytes));
        let out = super::uri_list(&[path]);

        assert!(out.starts_with(b"file://") && out.ends_with(b"\r\n"));
        let body = &out[7..out.len() - 2];
        let mut decoded = [0u8; LEN];
        let (mut i, mut n) = (0, 0);
        while i < body.len() {
            let byte = body[i];
            let raw = if byte == b'%' {
                i += 2;
                hex(body[i - 1]) << 4 | hex(body[i])
            } else {
                assert!(byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte));
                byte
            };
            assert!(n < LEN);
            decoded[n] = raw;
            n += 1;
            i += 1;
        }
        assert!(n == LEN && decoded == bytes);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;

    #[test]
    fn uri_list_percent_encodes_each_path_on_its_own_crlf_line() {
        let cases: [(&[&[u8]], &str); 3] = [
            (&[b"/tmp/notes.txt"], "file:///tmp/notes.txt\r\n"),
            (
                &[b"/home/a b/r\xc3\xa9sum\xc3\xa9 #1.pdf", b"/x/100%~_-.y"],
                "file:///home/a%20b/r%C3%A9sum%C3%A9%20%231.pdf\r\nfile:///x/100%25~_-.y\r\n",
            ),
            // Not UTF-8: the raw bytes are encoded, not replaced.
            (&[b"/tmp/\xff"], "file:///tmp/%FF\r\n"),
        ];
        for (paths, expected) in cases {
            let paths: Vec<PathBuf> = paths
                .iter()
                .map(|bytes| std::ffi::OsStr::from_bytes(bytes).into())
                .collect();
            assert_eq!(
                String::from_utf8(super::uri_list(&paths)).unwrap(),
                expected
            );
        }
    }
}
