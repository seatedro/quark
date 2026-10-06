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
pub mod focus;
pub mod geometry;
pub mod hit;
pub mod identity;
pub mod reactive;
pub mod retained;
pub mod scene;
pub mod selection;
pub mod semantic;
pub mod style;
pub mod style_state;

pub use animation::{AnimKey, AnimKind, AnimationTable, Curve, Motion, PropId, SpringParams};
pub use color::Color;
pub use event::{
    DragSession, PointerCapture, RoutedEventStep, UiEventBinding, UiEventKind, UiEventPhase,
    UiEventPropagation, UiEventResult, UiEventRoute,
};
pub use focus::{FocusId, FocusNode, FocusScopeId, FocusTree, KeyContext, TabStop};
pub use geometry::Rect;
pub use hit::{
    ClickEvent, CursorHint, EMPTY_CLIP, HitFlags, HitId, HitIdentity, HitTable, TooltipRegion,
    UNCLIPPED,
};
pub use identity::{TestId, UiKey, UiNodeId, stable_hash};
pub use retained::{DisposedNode, RetainedNode, RetainedTree};
pub use scene::{
    BlurRegionPrimitive, BorderPrimitive, ClipPrimitive, EffectQuadPrimitive, EffectType, FontKind,
    FontWeight, IconPrimitive, ImagePrimitive, Primitive, RectPrimitive, RichTextPrimitive,
    RichTextSpan, RoundedRectPrimitive, Scene, ShadowPrimitive, TextPrimitive,
};
pub use selection::{BlockKey, BlockOrder, Selection, SelectionPoint, SelectionText, copy_text};
pub use semantic::{
    SemanticActions, SemanticFrame, SemanticNode, SemanticNodeState, SemanticRole, dump_semantic,
};
pub use style::{BackgroundEffect, ElementStyle, ShadowStyle, StyleOverride, apply_override};
pub use style_state::{StyleInvalidation, StyleInvalidationReason, StyleState};
