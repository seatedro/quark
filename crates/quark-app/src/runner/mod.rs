//! The app runner: owns the winit event loop and a table of windows, each
//! with its own wgpu renderer and accesskit adapter, and asks an [`App`] for a
//! [`Scene`] whenever a window needs repainting.

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
use quark_render::{RenderError, Renderer, TextMetrics};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::error::{EventLoopError, OsError};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{CursorIcon, Icon, Window, WindowAttributes, WindowId};

use crate::input::{InputEvent, InputNormalizer};

mod accessibility;
mod app;
mod context;
mod event_loop;
mod events;
mod table;
mod window;

use accessibility::*;
pub use app::*;
pub use context::*;
pub use event_loop::*;
pub use events::*;
pub use table::WindowHandle;
use table::WindowTable;
use window::*;
