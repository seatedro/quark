//! What is on top at a desktop point: one of the app's windows, something
//! else, or unknown. Window rectangles alone cannot say, since another
//! app's window may cover ours, so each platform asks its window system.

use winit::window::Window;

use crate::platform::drag_out::DragOutError;
use crate::runner::{DesktopPoint, EventContext, WindowHandle};

/// What a [`WindowStack`] found at a desktop point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackHit {
    /// One of the app's windows is on top there, its decorations included.
    Owned(WindowHandle),
    /// Another app's window, or the bare desktop.
    Other,
    /// The window system could not say.
    Unknown,
}

/// The desktop's windows in stacking order, as the platform knows them.
pub trait WindowStack {
    /// What is on top at `at` (desktop units, see
    /// [`crate::runner::DesktopPoint`]), looking through `skip`, the window
    /// following the pointer. `cx` maps the app's windows to native ones.
    fn hit(&mut self, cx: &EventContext, at: DesktopPoint, skip: Option<WindowHandle>) -> StackHit;

    /// Whether the primary button is down now, if the platform can say:
    /// a drag whose window lost focus with the button up lost the pointer.
    fn primary_held(&mut self) -> Option<bool> {
        None
    }

    /// Where the pointer is on the desktop now, if the platform can say
    /// without a window's help: for motion reported against a window
    /// that is itself moving with the pointer.
    fn pointer(&mut self) -> Option<DesktopPoint> {
        None
    }
}

/// The platform's stack for the display `window` is on.
#[cfg(target_os = "linux")]
pub(super) fn native(window: &Window) -> Result<Box<dyn WindowStack>, DragOutError> {
    use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
    match window.display_handle().map(|handle| handle.as_raw()) {
        Ok(RawDisplayHandle::Xlib(_) | RawDisplayHandle::Xcb(_)) => super::x11::X11Stack::connect()
            .map(|stack| Box::new(stack) as Box<dyn WindowStack>)
            .map_err(DragOutError::Platform),
        _ => Err(DragOutError::Unsupported),
    }
}

#[cfg(windows)]
pub(super) fn native(_: &Window) -> Result<Box<dyn WindowStack>, DragOutError> {
    Ok(Box::new(super::win32::Win32Stack))
}

#[cfg(target_os = "macos")]
pub(super) fn native(_: &Window) -> Result<Box<dyn WindowStack>, DragOutError> {
    Ok(Box::new(super::macos::AppKitStack))
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
pub(super) fn native(_: &Window) -> Result<Box<dyn WindowStack>, DragOutError> {
    Err(DragOutError::Unsupported)
}

/// A stack the test sets up: the app's windows where their placements put
/// them, and other apps' windows as plain rectangles, in an order the test
/// controls. Clones share one stack, so a test keeps one to change while
/// a [`super::DockDrag`] holds another.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, Default)]
pub struct ScriptedStack(std::rc::Rc<std::cell::RefCell<Script>>);

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Default)]
struct Script {
    /// Bottom to top.
    layers: Vec<Layer>,
    held: Option<bool>,
    pointer: Option<DesktopPoint>,
    broken: bool,
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, Copy)]
enum Layer {
    Owned(WindowHandle),
    /// Top left and size, in desktop units.
    Other(DesktopPoint, (f64, f64)),
}

#[cfg(any(test, feature = "test-support"))]
impl ScriptedStack {
    pub fn new() -> Self {
        Self::default()
    }

    /// Put the app's `window` on top. Its extent is read from its
    /// placement on every hit: the outer corner to the content area's far
    /// corner.
    pub fn raise(&self, window: WindowHandle) -> &Self {
        let mut script = self.0.borrow_mut();
        script
            .layers
            .retain(|layer| !matches!(layer, Layer::Owned(w) if *w == window));
        script.layers.push(Layer::Owned(window));
        self
    }

    /// Put another app's window on top, at `origin` with `size`, in
    /// desktop units.
    pub fn raise_other(&self, origin: DesktopPoint, size: (f64, f64)) -> &Self {
        self.0.borrow_mut().layers.push(Layer::Other(origin, size));
        self
    }

    /// What [`WindowStack::primary_held`] answers; `None` (the default)
    /// when the platform cannot say.
    pub fn set_primary_held(&self, held: Option<bool>) -> &Self {
        self.0.borrow_mut().held = held;
        self
    }

    /// What [`WindowStack::pointer`] answers; `None` (the default) when
    /// the platform cannot say.
    pub fn set_pointer(&self, pointer: Option<DesktopPoint>) -> &Self {
        self.0.borrow_mut().pointer = pointer;
        self
    }

    /// Make every hit fail, as a window system that stops answering.
    pub fn break_queries(&self) -> &Self {
        self.0.borrow_mut().broken = true;
        self
    }
}

#[cfg(any(test, feature = "test-support"))]
impl WindowStack for ScriptedStack {
    fn hit(
        &mut self,
        cx: &EventContext,
        (x, y): DesktopPoint,
        skip: Option<WindowHandle>,
    ) -> StackHit {
        let script = self.0.borrow();
        if script.broken {
            return StackHit::Unknown;
        }
        let inside = |(left, top): DesktopPoint, (right, bottom): DesktopPoint| {
            (left..right).contains(&x) && (top..bottom).contains(&y)
        };
        for layer in script.layers.iter().rev() {
            match *layer {
                Layer::Owned(window) if Some(window) != skip => {
                    let Some(placement) = cx.placement(window) else {
                        continue;
                    };
                    let (Some(outer), Some(inner)) =
                        (placement.outer_position, placement.inner_position)
                    else {
                        continue;
                    };
                    let scale = placement.desktop_scale;
                    let far = (
                        inner.0 + f64::from(placement.size.0) * scale,
                        inner.1 + f64::from(placement.size.1) * scale,
                    );
                    if inside(outer, far) {
                        return StackHit::Owned(window);
                    }
                }
                Layer::Owned(_) => {}
                Layer::Other(origin, (width, height)) => {
                    if inside(origin, (origin.0 + width, origin.1 + height)) {
                        return StackHit::Other;
                    }
                }
            }
        }
        StackHit::Other
    }

    fn primary_held(&mut self) -> Option<bool> {
        self.0.borrow().held
    }

    fn pointer(&mut self) -> Option<DesktopPoint> {
        self.0.borrow().pointer
    }
}
