//! The core crate of Quark, a native Rust UI framework. It holds the parts
//! that know nothing about windows, GPUs, or text shaping; `quark-ui` and
//! `quark-app` build on them.
//!
//! - [`reactive`]: `Signal<T>` handles into a `SignalStore`, memos, and
//!   dependency tracking.
//! - [`view!`] and [`Store`], re-exported from `quark-macros`.
//! - [`geometry`] holds [`Rect`].
//! - [`hit`]: one hit table per frame, in paint order, for hover, click,
//!   wheel, and drag.
//! - [`scene`]: the primitives the paint phase emits and the renderer draws.
//! - [`semantic`]: retained semantics for accessibility, focus, events, and
//!   devtools.
//! - [`selection`]: text selection across many blocks, keyed by stable
//!   [`BlockKey`]s.
//! - [`animation`]: tween and spring rows keyed by stable UI identity and
//!   property.
//!
//! A memo recomputes only when a signal it read changed:
//!
//! ```
//! use quark::reactive::SignalStore;
//!
//! let store = SignalStore::new();
//! let count = store.create(2);
//! let doubled = store.create_memo(move |s| count.get(s) * 2);
//! assert_eq!(doubled.get(&store), 4);
//!
//! count.set(&store, 5);
//! assert_eq!(doubled.get(&store), 10);
//! ```
//!
//! A selection names blocks by key and byte offset, so it can span blocks
//! that are not on screen:
//!
//! ```
//! use std::collections::HashMap;
//! use quark::{BlockKey, BlockOrder, Selection, SelectionPoint, copy_text};
//!
//! let mut order = BlockOrder::new();
//! order.extend([BlockKey(1), BlockKey(2)]);
//! let text = HashMap::from([
//!     (BlockKey(1), "Hello world".to_string()),
//!     (BlockKey(2), "Second block".to_string()),
//! ]);
//!
//! let selection = Selection::new(
//!     SelectionPoint::new(BlockKey(1), 6),
//!     SelectionPoint::new(BlockKey(2), 6),
//! );
//! assert_eq!(copy_text(&selection, &order, &text, "\n"), "world\nSecond");
//! ```

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
    ClickEvent, CursorHint, EMPTY_CLIP, HitFlags, HitId, HitIdentity, HitSpace, HitTable,
    TooltipRegion, UNCLIPPED,
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
