//! Core element model for declarative UI layout.
//!
//! Elements describe what they want (size, flex, padding) and a layout engine
//! (Taffy) resolves concrete pixel coordinates. The lifecycle is:
//!
//! 1. **request_layout** — declare Taffy style and children.
//! 2. **prepaint** — register hit entries, resolve hover.
//! 3. **paint** — emit scene primitives using resolved hover/hit state.

use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use crate::accessibility::{
    AccessibilityAction, AccessibilityExtra, AccessibilityFrame, AccessibilityNode, AccessibleText,
    accessibility_role_for, semantic_role_for,
};
use crate::action::{Action, FocusId};
use crate::animation::{Motion, PropSet};
use crate::design::{Alpha, Sz};
use crate::theme::Theme;
use accesskit::Role as AccessibilityRole;
pub use quark::hit::{ClickEvent, CursorHint, HitFlags, HitId, HitIdentity, HitTable};
use quark::reactive::{Signal, SignalStore};
use quark::{
    FocusScopeId, FocusTree, KeyContext, SemanticActions, SemanticFrame, SemanticNode,
    SemanticNodeState, SemanticRole, StyleState, TabStop, TestId, UiEventBinding, UiEventKind,
    UiEventPhase, UiEventResult, UiKey, UiNodeId,
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

use quark_render::push_text_decorations;
use quark_render::scene::{
    FontStyle, RichTextPrimitive, ShapedText, TextDecoration, TextDecorationKind,
};
use quark_render::{FontKind, TextPrimitive};
use quark_text::{LayoutCache, TextLayout, TextParams, TextQuery, TextSpan, TextStyle, TextSystem};

mod cache;
mod canvas;
mod code_block;
mod context;
mod div;
mod hit;
mod image;
mod layout;
mod measure;
mod pool;
mod render;
mod router;
mod scroll;
mod selectable_text;
mod spacer;
mod text;
mod text_input;
mod traits;
mod transition;

#[cfg(test)]
mod tests;

pub use cache::{CacheKey, Cached, ElementCache, cached, inputs_hash};
pub use canvas::*;
pub use code_block::*;
pub use context::*;
pub use div::*;
pub use hit::*;
pub use image::*;
pub use layout::*;
use measure::*;
pub use render::*;
pub use router::*;
pub use scroll::*;
pub use selectable_text::*;
pub use spacer::*;
pub use text::*;
pub use text_input::*;
pub use traits::*;
use transition::Transitions;
