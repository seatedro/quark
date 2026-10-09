//! Presentation choices for the diff view: comparison layout, change
//! markers, line numbers, hunk separators, file headers, and the fill of a
//! missing split side, plus local color overrides.
//!
//! The state stores these and hands them to the renderer through
//! [`super::prepared::ViewFrame`]; the defaults reproduce the view as it
//! was before these options existed (unified, signs, two unified number
//! columns, gap controls, built-in headers, solid filler). Changing a
//! marker, filler, or color choice invalidates paint only; changing the
//! layout or numbers moves columns, so the state re-measures.

use quark_ui::theme::Color;

/// Which comparison layout to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiffLayout {
    Unified,
    Split,
    /// Split while each side keeps at least `min_text_columns` monospace
    /// columns of usable text width, else unified. Switching back needs
    /// `hysteresis_columns` more (or fewer) than the threshold, so a
    /// resize near it does not flip the layout every frame.
    Auto {
        min_text_columns: u16,
        hysteresis_columns: u16,
    },
}

impl DiffLayout {
    /// The recommended automatic layout: 40 columns, four of hysteresis.
    pub const AUTO: Self = Self::Auto {
        min_text_columns: 40,
        hysteresis_columns: 4,
    };
}

/// How a changed line is marked in the gutter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum DiffMarkers {
    /// `+` and `-` signs.
    #[default]
    Signs,
    /// A strip at the text edge: solid for added lines, repeated
    /// horizontal segments for removed ones.
    Bars,
    None,
}

/// Which line numbers the gutter shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum DiffNumbers {
    /// Unified shows old and new numbers; split shows each side's own.
    #[default]
    Both,
    /// One column: the number of the side the row shows.
    RelevantSide,
    None,
}

/// What sits between hunks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum HunkSeparator {
    /// The `@@` header text.
    Metadata,
    /// The collapsed-gap row with reveal controls and the header text.
    #[default]
    ContextControls,
    /// A thin fold bar with reveal controls and no header text.
    Compact,
}

/// How file headers are drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum FileHeaders {
    #[default]
    BuiltIn,
    Hidden,
    /// Drawn by the app's decorator.
    Custom,
}

/// Fill of the side of a split row that has no line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum EmptySideFill {
    #[default]
    Solid,
    Hatch,
}

/// Every presentation choice. See the [module docs](self).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DiffPresentation {
    pub layout: DiffLayout,
    pub markers: DiffMarkers,
    pub numbers: DiffNumbers,
    pub separators: HunkSeparator,
    pub headers: FileHeaders,
    pub empty_side: EmptySideFill,
    pub sticky_headers: bool,
}

impl Default for DiffPresentation {
    fn default() -> Self {
        Self {
            layout: DiffLayout::Unified,
            markers: DiffMarkers::default(),
            numbers: DiffNumbers::default(),
            separators: HunkSeparator::default(),
            headers: FileHeaders::default(),
            empty_side: EmptySideFill::default(),
            sticky_headers: false,
        }
    }
}

/// Local color overrides over the theme's diff tokens; `None` keeps the
/// theme's color.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DiffAppearance {
    pub add_marker: Option<Color>,
    pub del_marker: Option<Color>,
    pub empty_side: Option<Color>,
    pub search_match: Option<Color>,
    pub search_active: Option<Color>,
    pub selection: Option<Color>,
    pub focused_row: Option<Color>,
}
