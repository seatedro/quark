//! The app runner: owns the winit event loop, one shared [`GpuContext`], and
//! a table of windows, each with its own surface, renderer state, and
//! accesskit adapter, and asks an [`App`] for a [`Scene`] whenever a window
//! needs repainting.
//!
//! Apps work in logical points. The runner converts winit's physical
//! positions to points once, as input arrives, and converts each window's
//! scene to physical pixels once, just before drawing it.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use accesskit::{
    ActionHandler, ActionRequest, ActivationHandler, DeactivationHandler, Node, NodeId, Role, Tree,
    TreeId, TreeUpdate,
};
use accesskit_winit::Adapter as AccessibilityAdapter;
use glyphon::FontSystem;
use quark::scene::Scene;
use quark_render::fonts::FontSettings;
use quark_render::{GpuContext, RenderError, Renderer, TextMetrics};
use quark_text::{LayoutCache, TextError, TextLayout, TextParams, TextSystem};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize, PhysicalSize};
use winit::error::{EventLoopError, OsError};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{CursorIcon, Icon, Theme, Window, WindowAttributes, WindowId};

use crate::input::{InputEvent, InputNormalizer};
use crate::platform::window_state::{MonitorArea, WindowGeometry, state_path};

mod accessibility;
mod app;
mod context;
mod event_loop;
mod events;
mod platform;
mod scale;
mod table;
#[cfg(feature = "test-support")]
mod testing;
mod text;
mod window;

use accessibility::*;
pub use app::*;
pub use context::*;
pub use event_loop::*;
pub use events::*;
use platform::PlatformState;
pub use scale::scene_to_physical;
pub use table::WindowHandle;
use table::WindowTable;
#[cfg(feature = "test-support")]
pub(crate) use testing::HeadlessRunner;
pub use text::AppText;
use window::*;
