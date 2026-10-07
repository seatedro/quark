//! Quark — declarative UI toolkit with fine-grained reactivity.
//!
//! - `view!` declarative macro (re-exported from `quark_macros`)
//! - `reactive` module with `Signal`, `SignalStore`, memos, tracking
//! - `geometry::Rect` — pure 2D rectangle
//! - `hit` — pointer hit-testing primitives generic over a click-result payload
//! - `scene` — immediate-mode render primitives
//! - `semantic` — retained native UI semantics for accessibility, focus,
//!   events, hit testing, and devtools
//! - `selection` — document-wide text selection keyed by stable block keys
//! - `animation` — tween/spring table keyed by stable UI identity and prop

pub use quark_macros::{Store, view};

pub mod animation;
pub mod color;
pub mod event;
pub mod fenwick;
pub mod focus;
pub mod geometry;
pub mod hit;
pub mod identity;
pub mod path;
pub mod reactive;
pub mod scene;
pub mod selection;
pub mod semantic;
pub mod style;
pub mod style_state;
#[cfg(test)]
mod test_support;
pub mod transform;

pub use animation::{AnimKey, AnimKind, AnimationTable, Curve, Motion, PropId, SpringParams};
pub use color::Color;
pub use event::{UiEventBinding, UiEventKind, UiEventPhase, UiEventPropagation, UiEventResult};
pub use focus::{FocusId, FocusNode, FocusScopeId, FocusTree, KeyContext, TabStop};
pub use geometry::Rect;
pub use hit::{
    ClickEvent, CursorHint, EMPTY_CLIP, HitFlags, HitId, HitIdentity, HitTable, TooltipRegion,
    UNCLIPPED,
};
pub use identity::{TestId, UiKey, UiNodeId, stable_hash};
pub use path::{FillRule, LineCap, LineJoin, Path, PathBuilder, PathVerb, StrokeStyle};
pub use scene::{
    BlurRegionPrimitive, BorderPrimitive, ClipPrimitive, EffectQuadPrimitive, EffectType, FontKind,
    FontWeight, IconPrimitive, ImagePrimitive, LayerPrimitive, PathFill, PathPrimitive, PathStroke,
    Primitive, RectPrimitive, RichTextPrimitive, RoundedRectPrimitive, Scene, ShadowPrimitive,
    ShapedText, TextPrimitive,
};
pub use selection::{BlockKey, BlockOrder, Selection, SelectionPoint, SelectionText, copy_text};
pub use semantic::{
    SemanticActions, SemanticFrame, SemanticNode, SemanticNodeState, SemanticRole, dump_semantic,
};
pub use style::{BackgroundEffect, ElementStyle, ShadowStyle, StyleOverride, apply_override};
pub use style_state::StyleState;
pub use transform::Transform2D;
