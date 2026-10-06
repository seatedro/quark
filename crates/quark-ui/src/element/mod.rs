//! Core element model for declarative UI layout.
//!
//! Elements describe what they want (size, flex, padding) and a layout engine
//! (Taffy) resolves concrete pixel coordinates. The lifecycle is:
//!
//! 1. **request_layout** — declare Taffy style and children.
//! 2. **prepaint** — register hitboxes, resolve interaction state.
//! 3. **paint** — emit scene primitives using resolved hover/hit state.

use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use crate::accessibility::{AccessibilityAction, AccessibilityFrame, AccessibilityNode};
use crate::action::{Action, FocusId};
use crate::design::{Alpha, Sz};
use crate::theme::Theme;
use accesskit::Role as AccessibilityRole;
pub use quark::hit::{ClickEvent, CursorHint, HitIdentity, Hitbox, HitboxBehavior, HitboxId};
use quark::reactive::{Signal, SignalStore};
use quark::{
    FocusScopeId, KeyContext, SemanticActions, SemanticFrame, SemanticNode, SemanticNodeState,
    SemanticRole, StyleState, TabStop, TestId, UiEventBinding, UiEventKind, UiEventPhase,
    UiEventResult, UiKey, UiNodeId,
};
use quark_render::Scene;
use quark_render::scene::{BlurRegionPrimitive, EffectQuadPrimitive, EffectType, Rect};

pub use taffy::NodeId as LayoutId;

use crate::style::{ElementStyle, StyleOverride, Styled, apply_override};
use crate::theme::Color;
use quark_render::{BorderPrimitive, FontWeight, RoundedRectPrimitive, ShadowPrimitive};

pub use quark::style::{
    BackgroundEffect, color_tint, linear_gradient, noise_gradient, radial_gradient, shimmer,
    vignette,
};

use quark_render::scene::{FontStyle, RichTextPrimitive, RichTextSpan};
use quark_render::{FontKind, TextPrimitive};

mod canvas;
mod code_block;
mod context;
mod div;
mod hit;
mod image;
mod layout;
mod measure;
mod render;
mod selectable_text;
mod spacer;
mod text;
mod text_input;
mod traits;

#[cfg(test)]
mod tests;

pub use canvas::*;
pub use code_block::*;
pub use context::*;
pub use div::*;
pub use hit::*;
pub use image::*;
pub use layout::*;
pub use measure::*;
pub use render::*;
pub use selectable_text::*;
pub use spacer::*;
pub use text::*;
pub use text_input::*;
pub use traits::*;
