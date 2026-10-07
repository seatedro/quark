//! A single-window app runner: owns the winit event loop, the wgpu renderer,
//! and the accesskit adapter, and asks an [`App`] for a [`Scene`] whenever the
//! window needs repainting.

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
use quark_text::{LayoutCache, TextSystem};
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
mod text;
mod window;

use accessibility::*;
pub use app::*;
pub use context::*;
pub use event_loop::*;
pub use text::AppText;
use window::*;
