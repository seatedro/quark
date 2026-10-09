//! Presentation choices for the diff view: comparison layout, change
//! markers, line numbers, hunk separators, file headers, and the fill of a
//! missing split side, plus local color overrides for the light and the
//! dark theme.
//!
//! The state stores these and hands them to the renderer through
//! [`super::prepared::ViewFrame`]; the defaults reproduce the view as it
//! was before these options existed (unified, signs, two unified number
//! columns, gap controls, built-in headers, solid filler). Changing a
//! marker, filler, or color choice invalidates paint only; changing the
//! layout or numbers moves columns, so the state re-measures.

use quark_diff::{Mode, RowKind, Side};
use quark_syntax::HighlightKind;
use quark_ui::theme::{Color, ThemeMode};

use super::prepared::{Columns, Metrics};

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

impl DiffPresentation {
    /// An embedded preview, as in a transcript card: unified rows, bar
    /// markers, the relevant side's number only, thin fold bars, and no
    /// file header (the card around it names the file).
    pub const fn compact() -> Self {
        Self {
            layout: DiffLayout::Unified,
            markers: DiffMarkers::Bars,
            numbers: DiffNumbers::RelevantSide,
            separators: HunkSeparator::Compact,
            headers: FileHeaders::Hidden,
            empty_side: EmptySideFill::Hatch,
            sticky_headers: false,
        }
    }

    /// A review panel: [`DiffLayout::AUTO`], bar markers, one number per
    /// side, gap controls, built-in headers, and hatched missing sides.
    pub const fn review() -> Self {
        Self {
            layout: DiffLayout::AUTO,
            markers: DiffMarkers::Bars,
            numbers: DiffNumbers::RelevantSide,
            separators: HunkSeparator::ContextControls,
            headers: FileHeaders::BuiltIn,
            empty_side: EmptySideFill::Hatch,
            sticky_headers: false,
        }
    }
}

/// The mode an automatic `layout` wants for a view `width` points wide
/// that shows `current` now, or `None` when `layout` is explicit (the
/// state keeps whatever the app chose).
///
/// Split needs each side's text column, after its gutter and padding, to
/// hold `min_text_columns` monospace columns; entering split from unified
/// needs `hysteresis_columns` more, so a width near the threshold keeps
/// whichever layout it has.
pub fn auto_mode(
    layout: DiffLayout,
    current: Mode,
    width: f32,
    m: &Metrics,
    presentation: &DiffPresentation,
) -> Option<Mode> {
    let DiffLayout::Auto {
        min_text_columns,
        hysteresis_columns,
    } = layout
    else {
        return None;
    };
    let split = Columns::new(Mode::Split, width, m, presentation);
    // The narrower side decides: the old side is a point narrower.
    let text_w = (split.of(Side::Old).text_w - m.text_pad * 2.0).max(0.0);
    let columns = (text_w / m.char_w.max(f32::EPSILON)).floor();
    let needed = match current {
        Mode::Split => f32::from(min_text_columns),
        Mode::Unified => f32::from(min_text_columns) + f32::from(hysteresis_columns),
    };
    Some(if columns >= needed {
        Mode::Split
    } else {
        Mode::Unified
    })
}

/// Height of a header, separator, or line row of `kind` before its text
/// is measured.
pub fn band_height(kind: RowKind, m: &Metrics, presentation: &DiffPresentation) -> f32 {
    match kind {
        RowKind::FileHeader => match presentation.headers {
            FileHeaders::Hidden => 0.0,
            FileHeaders::BuiltIn | FileHeaders::Custom => (m.line_h * 2.0).round(),
        },
        RowKind::HunkHeader | RowKind::Gap if presentation.separators == HunkSeparator::Compact => {
            compact_separator_height(m)
        }
        RowKind::HunkHeader | RowKind::Gap => (m.line_h * 1.4).round(),
        _ => m.line_h,
    }
}

/// A compact fold bar: a third of a line, at least four points.
fn compact_separator_height(m: &Metrics) -> f32 {
    (m.line_h * 0.3).round().max(4.0)
}

/// Colors the app sets over the theme's diff tokens; `None` keeps the
/// theme's. Each applies to every row it names; the view still marks
/// changes by shape (signs, or striped versus solid bars) and in its
/// accessible description, whatever the colors.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DiffColorOverrides {
    /// Background of added and removed lines, gutter included.
    pub add_line: Option<Color>,
    pub del_line: Option<Color>,
    /// Background of changed words, over the line's.
    pub add_word: Option<Color>,
    pub del_word: Option<Color>,
    /// Bars and signs.
    pub add_marker: Option<Color>,
    pub del_marker: Option<Color>,
    /// Line numbers of added and removed lines.
    pub add_number: Option<Color>,
    pub del_number: Option<Color>,
    pub gutter: Option<Color>,
    pub gutter_text: Option<Color>,
    /// The missing side of a split row, and its hatch lines.
    pub empty_side: Option<Color>,
    pub hatch: Option<Color>,
    pub file_header: Option<Color>,
    /// Gap rows and hunk headers.
    pub separator: Option<Color>,
    pub search_match: Option<Color>,
    pub search_active: Option<Color>,
    pub selection: Option<Color>,
    /// Outline of the row holding the keyboard focus.
    pub focused_row: Option<Color>,
    /// Text of each [`HighlightKind`], indexed by its discriminant.
    pub syntax: [Option<Color>; HIGHLIGHT_KINDS],
}

impl DiffColorOverrides {
    /// Paint `kind` in `color`.
    pub fn with_syntax(mut self, kind: HighlightKind, color: Color) -> Self {
        self.syntax[kind as usize] = Some(color);
        self
    }
}

/// Number of [`HighlightKind`]s.
pub const HIGHLIGHT_KINDS: usize = HighlightKind::Preprocessor as usize + 1;

/// Local colors for the light and the dark theme, resolved independently:
/// the view picks the overrides of the theme's mode.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DiffAppearance {
    pub light: DiffColorOverrides,
    pub dark: DiffColorOverrides,
}

impl DiffAppearance {
    /// The same overrides in both modes.
    pub fn both(overrides: DiffColorOverrides) -> Self {
        Self {
            light: overrides,
            dark: overrides,
        }
    }

    pub fn for_mode(&self, mode: ThemeMode) -> &DiffColorOverrides {
        match mode {
            ThemeMode::Light => &self.light,
            ThemeMode::Dark => &self.dark,
        }
    }
}
