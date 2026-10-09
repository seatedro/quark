//! App-supplied content inside the diff view: header slots, annotation
//! rows, and the gutter utility of the focused row.
//!
//! The view positions and caches what a [`DiffDecorator`] returns; the app
//! owns what it shows and the actions its controls emit. The view never
//! learns what an annotation means (a comment thread, a diagnostic, a
//! review finding), and the diff crate implements no staging, review
//! scope, or undo.

use std::ops::Range;

use quark_diff::{FileStatus, RowKind, Side};
use quark_ui::element::AnyElement;

use super::prepared::AnnotationId;

/// A place in a built-in file header that a decorator can fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HeaderSlot {
    /// Before the status and path, e.g. a file type icon.
    Prefix,
    /// After the path, e.g. a "best effort" or "loading" badge.
    Metadata,
    /// At the trailing edge, after the counts: the app's file actions.
    Actions,
}

/// The file a header shows.
#[derive(Debug, Clone, Copy)]
pub struct HeaderContext<'a> {
    /// The document's file index.
    pub file: u32,
    /// The path, or `old → new` for a rename or copy.
    pub title: &'a str,
    pub status: FileStatus,
    pub additions: u32,
    pub deletions: u32,
    pub binary: bool,
    /// The header row's size in points.
    pub width: f32,
    pub height: f32,
}

/// An annotation row the view has placed under its anchor.
#[derive(Debug, Clone, Copy)]
pub struct AnnotationContext<'a> {
    pub id: AnnotationId,
    pub side: Side,
    /// The anchored source lines, zero-based and half open.
    pub lines: &'a Range<u32>,
    /// The anchor's lines changed since it was attached.
    pub outdated: bool,
    /// The row's width in points; the content decides its height.
    pub width: f32,
}

/// The line row holding the keyboard focus, whose gutter offers the
/// decorator's utility (an Add comment button, say).
#[derive(Debug, Clone, Copy)]
pub struct GutterContext {
    pub file: u32,
    pub side: Side,
    /// Zero-based source line on `side`.
    pub line: u32,
    pub kind: RowKind,
    /// The gutter's size in points.
    pub width: f32,
    pub height: f32,
}

/// Content the app places inside the view. Every method has a default
/// that adds nothing, so a decorator implements only the slots it fills.
///
/// The view caches what each slot returns and asks again only when the
/// row's inputs or [`Self::revision`] change, so `revision` must change
/// whenever any slot would return something different.
pub trait DiffDecorator {
    fn revision(&self) -> u64;

    /// Content for `slot` of a built-in header.
    fn header_slot(&self, _slot: HeaderSlot, _cx: &HeaderContext) -> Option<AnyElement> {
        None
    }

    /// A whole header under [`super::presentation::FileHeaders::Custom`];
    /// `None` falls back to the built-in one.
    fn header(&self, _cx: &HeaderContext) -> Option<AnyElement> {
        None
    }

    /// The content of an annotation row, as wide as the view.
    fn annotation(&self, _cx: &AnnotationContext) -> Option<AnyElement> {
        None
    }

    /// A control drawn over the focused line row's gutter.
    fn gutter_utility(&self, _cx: &GutterContext) -> Option<AnyElement> {
        None
    }
}
