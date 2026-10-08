//! Style system — shared layout + visual properties for elements.
//!
//! Pure data types (`ElementStyle`, `StyleOverride`, `ShadowStyle`, `apply_override`)
//! live in `quark::style`. The `Styled` trait defined here layers the
//! design-token shortcuts (`Sp`, `Rad`, `ShadowLayer`) on top.

use crate::design::{Rad, ShadowLayer, Sp};
use crate::theme::Color;

pub use quark::style::{ElementStyle, ShadowStyle, StyleOverride, apply_override};
pub use quark_i18n::Direction;

use std::cell::Cell;

thread_local! {
    // Per thread: views are built on the UI thread, and tests running in
    // parallel each keep their own.
    static DIRECTION: Cell<Direction> = const { Cell::new(Direction::LeftToRight) };
}

/// Lay out rows for a locale written `direction`: right to left mirrors
/// every [`Styled::flex_row`] (first child at the right). Set it on the UI
/// thread from `quark_i18n::Localizer::direction` before building views;
/// subtrees the element cache already holds keep the old direction until
/// rebuilt.
pub fn set_layout_direction(direction: Direction) {
    DIRECTION.with(|d| d.set(direction));
}

/// The direction [`set_layout_direction`] set on this thread.
pub fn layout_direction() -> Direction {
    DIRECTION.with(Cell::get)
}

/// Grid track sizes for [`Styled::grid_cols`] and [`Styled::grid_rows`],
/// in CSS grid terms: `track::px(120.0)`, `track::fr(1.0)`,
/// `track::minmax(track::px(80.0), track::fr(1.0))`,
/// `track::repeat(3, [track::fr(1.0)])`.
///
/// Content keywords (`min_content`, `max_content`, `fit_content`) size
/// tracks from what is in them. Taffy has no content keywords for an
/// element's own `width`/`height`, so content sizing goes through a track:
/// a one-column grid with a `max_content` column is as wide as its widest
/// child.
pub mod track {
    use taffy::style_helpers::{TaffyAuto, TaffyMaxContent, TaffyMinContent};
    use taffy::{
        GridTemplateComponent, GridTemplateRepetition, LengthPercentage, MaxTrackSizingFunction,
        MinTrackSizingFunction, RepetitionCount, TrackSizingFunction,
    };

    /// One entry of a grid template: a single track or a `repeat(..)`.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Track(pub(crate) GridTemplateComponent<String>);

    impl Track {
        fn single(sizing: TrackSizingFunction) -> Self {
            Self(GridTemplateComponent::Single(sizing))
        }

        /// The size of a single track. A `repeat` has no single size; it
        /// becomes `auto` (a programming error in debug builds).
        pub(crate) fn sizing(&self) -> TrackSizingFunction {
            match &self.0 {
                GridTemplateComponent::Single(sizing) => *sizing,
                GridTemplateComponent::Repeat(_) => {
                    debug_assert!(false, "repeat() is not a single track size");
                    TrackSizingFunction::AUTO
                }
            }
        }
    }

    /// A fixed track `v` points wide.
    pub fn px(v: f32) -> Track {
        Track::single(taffy::style_helpers::length(v))
    }

    /// A fraction of the container, 0.0 to 1.0.
    pub fn percent(v: f32) -> Track {
        Track::single(taffy::style_helpers::percent(v))
    }

    /// `v` shares of the space the other tracks leave.
    pub fn fr(v: f32) -> Track {
        Track::single(taffy::style_helpers::fr(v))
    }

    pub fn auto() -> Track {
        Track::single(TrackSizingFunction::AUTO)
    }

    /// As narrow as the content can wrap: its longest unbreakable run.
    pub fn min_content() -> Track {
        Track::single(TrackSizingFunction::MIN_CONTENT)
    }

    /// As wide as the content without wrapping.
    pub fn max_content() -> Track {
        Track::single(TrackSizingFunction::MAX_CONTENT)
    }

    /// `max_content`, but no wider than `limit` points.
    pub fn fit_content(limit: f32) -> Track {
        Track::single(taffy::style_helpers::fit_content(LengthPercentage::length(
            limit,
        )))
    }

    /// At least `min`'s size and at most `max`'s: `minmax(px(80.0),
    /// fr(1.0))` is a flexible track that never gets narrower than 80.
    pub fn minmax(min: Track, max: Track) -> Track {
        let min: MinTrackSizingFunction = min.sizing().min;
        let max: MaxTrackSizingFunction = max.sizing().max;
        Track::single(TrackSizingFunction { min, max })
    }

    /// `tracks`, `count` times over.
    pub fn repeat(count: u16, tracks: impl IntoIterator<Item = Track>) -> Track {
        repetition(RepetitionCount::Count(count), tracks)
    }

    /// As many repetitions of `tracks` as fit, keeping empty ones
    /// (`repeat(auto-fill, ..)`). Every track must have a definite size.
    pub fn repeat_fill(tracks: impl IntoIterator<Item = Track>) -> Track {
        repetition(RepetitionCount::AutoFill, tracks)
    }

    /// As many repetitions of `tracks` as fit, collapsing empty ones so
    /// the filled tracks stretch (`repeat(auto-fit, ..)`).
    pub fn repeat_fit(tracks: impl IntoIterator<Item = Track>) -> Track {
        repetition(RepetitionCount::AutoFit, tracks)
    }

    fn repetition(count: RepetitionCount, tracks: impl IntoIterator<Item = Track>) -> Track {
        let tracks: Vec<TrackSizingFunction> = tracks.into_iter().map(|t| t.sizing()).collect();
        let line_names = vec![Vec::new(); tracks.len() + 1];
        Track(GridTemplateComponent::Repeat(GridTemplateRepetition {
            count,
            tracks,
            line_names,
        }))
    }
}

/// A grid line placement. Lines count from 1; negative lines count from
/// the far edge (`-1` is the last line).
fn grid_line(index: i16) -> taffy::GridPlacement<String> {
    taffy::style_helpers::line(index)
}

// ---------------------------------------------------------------------------
// Styled trait — fluent setters shared across element types
// ---------------------------------------------------------------------------

pub trait Styled: Sized {
    fn element_style_mut(&mut self) -> &mut ElementStyle;

    // -- Layout --

    /// Children side by side in reading order: left to right, or right
    /// to left under [`set_layout_direction`].
    fn flex_row(mut self) -> Self {
        self.element_style_mut().layout.flex_direction = match layout_direction() {
            Direction::LeftToRight => taffy::FlexDirection::Row,
            Direction::RightToLeft => taffy::FlexDirection::RowReverse,
        };
        self
    }

    fn flex_col(mut self) -> Self {
        self.element_style_mut().layout.flex_direction = taffy::FlexDirection::Column;
        self
    }

    fn flex_1(mut self) -> Self {
        let l = &mut self.element_style_mut().layout;
        l.flex_grow = 1.0;
        l.flex_shrink = 1.0;
        l.flex_basis = taffy::Dimension::percent(0.0);
        self
    }

    fn flex_grow(mut self) -> Self {
        self.element_style_mut().layout.flex_grow = 1.0;
        self
    }

    fn flex_grow_val(mut self, v: f32) -> Self {
        self.element_style_mut().layout.flex_grow = v;
        self
    }

    fn flex_shrink_0(mut self) -> Self {
        self.element_style_mut().layout.flex_shrink = 0.0;
        self
    }

    /// How much of a shortfall this item absorbs relative to its siblings,
    /// weighted by its basis as in CSS `flex-shrink`.
    fn flex_shrink_val(mut self, v: f32) -> Self {
        self.element_style_mut().layout.flex_shrink = v;
        self
    }

    fn gap(mut self, v: f32) -> Self {
        self.element_style_mut().layout.gap = taffy::Size {
            width: taffy::LengthPercentage::length(v),
            height: taffy::LengthPercentage::length(v),
        };
        self
    }

    fn gap_x(mut self, v: f32) -> Self {
        self.element_style_mut().layout.gap.width = taffy::LengthPercentage::length(v);
        self
    }

    fn gap_y(mut self, v: f32) -> Self {
        self.element_style_mut().layout.gap.height = taffy::LengthPercentage::length(v);
        self
    }

    // -- Tailwind-style spacing shortcuts (4px base grid) --

    fn p_1(self) -> Self {
        self.p(Sp::XS)
    }
    fn p_2(self) -> Self {
        self.p(Sp::SM)
    }
    fn p_3(self) -> Self {
        self.p(Sp::MD)
    }
    fn p_4(self) -> Self {
        self.p(Sp::LG)
    }
    fn p_5(self) -> Self {
        self.p(Sp::XL)
    }
    fn p_6(self) -> Self {
        self.p(Sp::XXL - Sp::XS)
    }
    fn p_8(self) -> Self {
        self.p(Sp::XXL + Sp::XS)
    }

    fn px_2(self) -> Self {
        self.px(Sp::SM)
    }
    fn px_3(self) -> Self {
        self.px(Sp::MD)
    }
    fn px_4(self) -> Self {
        self.px(Sp::LG)
    }
    fn px_5(self) -> Self {
        self.px(Sp::XL)
    }
    fn px_6(self) -> Self {
        self.px(Sp::XXL - Sp::XS)
    }

    fn py_1(self) -> Self {
        self.py(Sp::XS)
    }
    fn py_2(self) -> Self {
        self.py(Sp::SM)
    }
    fn py_3(self) -> Self {
        self.py(Sp::MD)
    }

    fn gap_1(self) -> Self {
        self.gap(Sp::XS)
    }
    fn gap_2(self) -> Self {
        self.gap(Sp::SM)
    }
    fn gap_3(self) -> Self {
        self.gap(Sp::MD)
    }
    fn gap_4(self) -> Self {
        self.gap(Sp::LG)
    }

    fn rounded_sm(self) -> Self {
        self.rounded(Rad::LG)
    }
    fn rounded_md(self) -> Self {
        self.rounded(Rad::XL)
    }
    fn rounded_lg(self) -> Self {
        self.rounded(Rad::XXL)
    }
    fn rounded_xl(self) -> Self {
        self.rounded(Rad::XXXL)
    }

    fn h_10(self) -> Self {
        self.h(Sp::XXXL)
    }
    fn h_12(self) -> Self {
        self.h(Sp::XXXL + Sp::SM)
    }

    // -- Raw value methods --

    fn p(mut self, v: f32) -> Self {
        let l = taffy::LengthPercentage::length(v);
        self.element_style_mut().layout.padding = taffy::Rect {
            left: l,
            right: l,
            top: l,
            bottom: l,
        };
        self
    }

    fn px(mut self, v: f32) -> Self {
        let l = taffy::LengthPercentage::length(v);
        let p = &mut self.element_style_mut().layout.padding;
        p.left = l;
        p.right = l;
        self
    }

    fn py(mut self, v: f32) -> Self {
        let l = taffy::LengthPercentage::length(v);
        let p = &mut self.element_style_mut().layout.padding;
        p.top = l;
        p.bottom = l;
        self
    }

    fn pt(mut self, v: f32) -> Self {
        self.element_style_mut().layout.padding.top = taffy::LengthPercentage::length(v);
        self
    }

    fn pb(mut self, v: f32) -> Self {
        self.element_style_mut().layout.padding.bottom = taffy::LengthPercentage::length(v);
        self
    }

    fn w(mut self, v: f32) -> Self {
        self.element_style_mut().layout.size.width = taffy::Dimension::length(v);
        self
    }

    fn h(mut self, v: f32) -> Self {
        self.element_style_mut().layout.size.height = taffy::Dimension::length(v);
        self
    }

    fn w_full(mut self) -> Self {
        self.element_style_mut().layout.size.width = taffy::Dimension::percent(1.0);
        self
    }

    fn h_full(mut self) -> Self {
        self.element_style_mut().layout.size.height = taffy::Dimension::percent(1.0);
        self
    }

    fn min_w(mut self, v: f32) -> Self {
        self.element_style_mut().layout.min_size.width = taffy::Dimension::length(v);
        self
    }

    fn min_h(mut self, v: f32) -> Self {
        self.element_style_mut().layout.min_size.height = taffy::Dimension::length(v);
        self
    }

    fn items_center(mut self) -> Self {
        self.element_style_mut().layout.align_items = Some(taffy::AlignItems::Center);
        self
    }

    fn items_start(mut self) -> Self {
        self.element_style_mut().layout.align_items = Some(taffy::AlignItems::FlexStart);
        self
    }

    fn items_end(mut self) -> Self {
        self.element_style_mut().layout.align_items = Some(taffy::AlignItems::FlexEnd);
        self
    }

    fn justify_center(mut self) -> Self {
        self.element_style_mut().layout.justify_content = Some(taffy::JustifyContent::Center);
        self
    }

    fn justify_between(mut self) -> Self {
        self.element_style_mut().layout.justify_content = Some(taffy::JustifyContent::SpaceBetween);
        self
    }

    fn justify_end(mut self) -> Self {
        self.element_style_mut().layout.justify_content = Some(taffy::JustifyContent::FlexEnd);
        self
    }

    fn overflow_hidden(mut self) -> Self {
        self.element_style_mut().layout.overflow = taffy::Point {
            x: taffy::Overflow::Hidden,
            y: taffy::Overflow::Hidden,
        };
        self
    }

    fn overflow_y_scroll(mut self) -> Self {
        self.element_style_mut().layout.overflow.y = taffy::Overflow::Scroll;
        self
    }

    // -- Visual --

    fn bg(mut self, color: Color) -> Self {
        self.element_style_mut().background = Some(color);
        self
    }

    fn border(mut self, color: Color) -> Self {
        let s = self.element_style_mut();
        s.border_color = Some(color);
        s.border_widths = [1.0; 4];
        s.layout.border = taffy::Rect {
            left: taffy::LengthPercentage::length(1.0),
            right: taffy::LengthPercentage::length(1.0),
            top: taffy::LengthPercentage::length(1.0),
            bottom: taffy::LengthPercentage::length(1.0),
        };
        self
    }

    fn border_t(mut self, color: Color) -> Self {
        let s = self.element_style_mut();
        s.border_color = Some(color);
        s.border_widths[0] = 1.0;
        s.layout.border.top = taffy::LengthPercentage::length(1.0);
        self
    }

    fn border_r(mut self, color: Color) -> Self {
        let s = self.element_style_mut();
        s.border_color = Some(color);
        s.border_widths[1] = 1.0;
        s.layout.border.right = taffy::LengthPercentage::length(1.0);
        self
    }

    fn border_b(mut self, color: Color) -> Self {
        let s = self.element_style_mut();
        s.border_color = Some(color);
        s.border_widths[2] = 1.0;
        s.layout.border.bottom = taffy::LengthPercentage::length(1.0);
        self
    }

    fn border_l(mut self, color: Color) -> Self {
        let s = self.element_style_mut();
        s.border_color = Some(color);
        s.border_widths[3] = 1.0;
        s.layout.border.left = taffy::LengthPercentage::length(1.0);
        self
    }

    fn rounded(mut self, r: f32) -> Self {
        self.element_style_mut().corner_radii = [r; 4];
        self
    }

    /// Per-corner radii: [top-left, top-right, bottom-right, bottom-left].
    fn rounded_corners(mut self, radii: [f32; 4]) -> Self {
        self.element_style_mut().corner_radii = radii;
        self
    }

    fn opacity(mut self, v: f32) -> Self {
        self.element_style_mut().opacity = v;
        self
    }

    fn shadow(mut self, blur: f32, offset_y: f32, color: Color) -> Self {
        let r = self.element_style_mut().max_corner_radius();
        self.element_style_mut().shadows.push(ShadowStyle {
            blur_radius: blur,
            offset: [0.0, offset_y],
            corner_radius: r,
            color,
        });
        self
    }

    /// Outer glow — a colored halo around the element (e.g. focus indicator).
    /// Implemented as a zero-offset shadow with the given color and radius.
    fn shadow_preset(mut self, layers: &[ShadowLayer]) -> Self {
        for layer in layers {
            self = self.shadow(
                layer.blur,
                layer.offset_y,
                Color::rgba(0, 0, 0, layer.alpha),
            );
        }
        self
    }

    fn glow(self, color: Color, radius: f32) -> Self {
        self.shadow(radius, 0.0, color)
    }

    /// Set the z-index for rendering order. Higher values render on top.
    /// Default is 0. Modals typically use 100+, toasts 200+.
    fn z_index(mut self, z: i32) -> Self {
        self.element_style_mut().z_index = z;
        self
    }

    fn absolute(mut self) -> Self {
        self.element_style_mut().layout.position = taffy::Position::Absolute;
        self
    }

    fn top(mut self, v: f32) -> Self {
        self.element_style_mut().layout.inset.top = taffy::LengthPercentageAuto::length(v);
        self
    }

    fn bottom(mut self, v: f32) -> Self {
        self.element_style_mut().layout.inset.bottom = taffy::LengthPercentageAuto::length(v);
        self
    }

    fn left(mut self, v: f32) -> Self {
        self.element_style_mut().layout.inset.left = taffy::LengthPercentageAuto::length(v);
        self
    }

    fn right(mut self, v: f32) -> Self {
        self.element_style_mut().layout.inset.right = taffy::LengthPercentageAuto::length(v);
        self
    }

    fn inset(mut self, v: f32) -> Self {
        let l = taffy::LengthPercentageAuto::length(v);
        self.element_style_mut().layout.inset = taffy::Rect {
            left: l,
            right: l,
            top: l,
            bottom: l,
        };
        self
    }

    fn max_w(mut self, v: f32) -> Self {
        self.element_style_mut().layout.max_size.width = taffy::Dimension::length(v);
        self
    }

    fn max_h(mut self, v: f32) -> Self {
        self.element_style_mut().layout.max_size.height = taffy::Dimension::length(v);
        self
    }

    fn flex_wrap(mut self) -> Self {
        self.element_style_mut().layout.flex_wrap = taffy::FlexWrap::Wrap;
        self
    }

    fn flex_none(mut self) -> Self {
        let l = &mut self.element_style_mut().layout;
        l.flex_grow = 0.0;
        l.flex_shrink = 0.0;
        l.flex_basis = taffy::Dimension::auto();
        self
    }

    fn flex_auto(mut self) -> Self {
        let l = &mut self.element_style_mut().layout;
        l.flex_grow = 1.0;
        l.flex_shrink = 1.0;
        l.flex_basis = taffy::Dimension::auto();
        self
    }

    fn flex_row_reverse(mut self) -> Self {
        self.element_style_mut().layout.flex_direction = taffy::FlexDirection::RowReverse;
        self
    }

    fn flex_col_reverse(mut self) -> Self {
        self.element_style_mut().layout.flex_direction = taffy::FlexDirection::ColumnReverse;
        self
    }

    fn justify_start(mut self) -> Self {
        self.element_style_mut().layout.justify_content = Some(taffy::JustifyContent::FlexStart);
        self
    }

    fn self_auto(mut self) -> Self {
        self.element_style_mut().layout.align_self = None;
        self
    }

    fn self_start(mut self) -> Self {
        self.element_style_mut().layout.align_self = Some(taffy::AlignSelf::FlexStart);
        self
    }

    fn self_end(mut self) -> Self {
        self.element_style_mut().layout.align_self = Some(taffy::AlignSelf::FlexEnd);
        self
    }

    fn self_center(mut self) -> Self {
        self.element_style_mut().layout.align_self = Some(taffy::AlignSelf::Center);
        self
    }

    fn self_stretch(mut self) -> Self {
        self.element_style_mut().layout.align_self = Some(taffy::AlignSelf::Stretch);
        self
    }

    fn self_baseline(mut self) -> Self {
        self.element_style_mut().layout.align_self = Some(taffy::AlignSelf::Baseline);
        self
    }

    fn items_baseline(mut self) -> Self {
        self.element_style_mut().layout.align_items = Some(taffy::AlignItems::Baseline);
        self
    }

    fn items_stretch(mut self) -> Self {
        self.element_style_mut().layout.align_items = Some(taffy::AlignItems::Stretch);
        self
    }

    fn hidden(mut self) -> Self {
        self.element_style_mut().layout.display = taffy::Display::None;
        self
    }

    fn basis(mut self, v: f32) -> Self {
        self.element_style_mut().layout.flex_basis = taffy::Dimension::length(v);
        self
    }

    fn basis_auto(mut self) -> Self {
        self.element_style_mut().layout.flex_basis = taffy::Dimension::auto();
        self
    }

    fn basis_full(mut self) -> Self {
        self.element_style_mut().layout.flex_basis = taffy::Dimension::percent(1.0);
        self
    }

    fn pl(mut self, v: f32) -> Self {
        self.element_style_mut().layout.padding.left = taffy::LengthPercentage::length(v);
        self
    }

    fn pr(mut self, v: f32) -> Self {
        self.element_style_mut().layout.padding.right = taffy::LengthPercentage::length(v);
        self
    }

    fn margin_left(mut self, v: f32) -> Self {
        self.element_style_mut().layout.margin.left = taffy::LengthPercentageAuto::length(v);
        self
    }

    fn size(mut self, v: f32) -> Self {
        let l = &mut self.element_style_mut().layout;
        l.size.width = taffy::Dimension::length(v);
        l.size.height = taffy::Dimension::length(v);
        self
    }

    fn size_full(mut self) -> Self {
        let l = &mut self.element_style_mut().layout;
        l.size.width = taffy::Dimension::percent(1.0);
        l.size.height = taffy::Dimension::percent(1.0);
        self
    }

    fn gap_0(self) -> Self {
        self.gap(Sp::NONE)
    }
    fn gap_5(self) -> Self {
        self.gap(Sp::XL)
    }
    fn gap_6(self) -> Self {
        self.gap(Sp::XXL - Sp::XS)
    }
    fn gap_8(self) -> Self {
        self.gap(Sp::XXL + Sp::XS)
    }

    fn p_0(self) -> Self {
        self.p(Sp::NONE)
    }

    fn px_0(self) -> Self {
        self.px(Sp::NONE)
    }
    fn px_1(self) -> Self {
        self.px(Sp::XS)
    }

    fn py_0(self) -> Self {
        self.py(Sp::NONE)
    }
    fn py_4(self) -> Self {
        self.py(Sp::LG)
    }
    fn py_5(self) -> Self {
        self.py(Sp::XL)
    }

    fn pt_1(self) -> Self {
        self.pt(Sp::XS)
    }
    fn pt_2(self) -> Self {
        self.pt(Sp::SM)
    }
    fn pt_3(self) -> Self {
        self.pt(Sp::MD)
    }
    fn pt_4(self) -> Self {
        self.pt(Sp::LG)
    }

    fn pb_1(self) -> Self {
        self.pb(Sp::XS)
    }
    fn pb_2(self) -> Self {
        self.pb(Sp::SM)
    }
    fn pb_3(self) -> Self {
        self.pb(Sp::MD)
    }
    fn pb_4(self) -> Self {
        self.pb(Sp::LG)
    }

    fn rounded_none(self) -> Self {
        self.rounded(0.0)
    }
    fn rounded_full(mut self) -> Self {
        self.element_style_mut().corner_radii = [9999.0; 4];
        self
    }

    fn overflow_y_hidden(mut self) -> Self {
        self.element_style_mut().layout.overflow.y = taffy::Overflow::Hidden;
        self
    }

    fn overflow_x_hidden(mut self) -> Self {
        self.element_style_mut().layout.overflow.x = taffy::Overflow::Hidden;
        self
    }

    fn relative(mut self) -> Self {
        self.element_style_mut().layout.position = taffy::Position::Relative;
        self
    }

    // -- Grid --

    /// Lay children out on a CSS grid instead of a flex line.
    fn grid(mut self) -> Self {
        self.element_style_mut().layout.display = taffy::Display::Grid;
        self
    }

    /// Grid with these column tracks; children fill them in order.
    fn grid_cols(mut self, tracks: impl IntoIterator<Item = track::Track>) -> Self {
        let l = &mut self.element_style_mut().layout;
        l.display = taffy::Display::Grid;
        l.grid_template_columns = tracks.into_iter().map(|t| t.0).collect();
        self
    }

    /// Grid with these row tracks. Rows past them are sized by
    /// [`Self::grid_auto_rows`].
    fn grid_rows(mut self, tracks: impl IntoIterator<Item = track::Track>) -> Self {
        let l = &mut self.element_style_mut().layout;
        l.display = taffy::Display::Grid;
        l.grid_template_rows = tracks.into_iter().map(|t| t.0).collect();
        self
    }

    /// `n` equal columns: `repeat(n, 1fr)`.
    fn grid_cols_n(self, n: u16) -> Self {
        self.grid_cols([track::repeat(n, [track::fr(1.0)])])
    }

    /// Size of rows the template does not list (implicit rows).
    fn grid_auto_rows(mut self, size: track::Track) -> Self {
        self.element_style_mut().layout.grid_auto_rows = vec![size.sizing()];
        self
    }

    /// Size of columns the template does not list (implicit columns).
    fn grid_auto_cols(mut self, size: track::Track) -> Self {
        self.element_style_mut().layout.grid_auto_columns = vec![size.sizing()];
        self
    }

    /// Auto-place children down each column, adding columns as needed.
    fn grid_flow_col(mut self) -> Self {
        let l = &mut self.element_style_mut().layout;
        l.grid_auto_flow = if l.grid_auto_flow.is_dense() {
            taffy::GridAutoFlow::ColumnDense
        } else {
            taffy::GridAutoFlow::Column
        };
        self
    }

    /// Auto-place children along each row (the default).
    fn grid_flow_row(mut self) -> Self {
        let l = &mut self.element_style_mut().layout;
        l.grid_auto_flow = if l.grid_auto_flow.is_dense() {
            taffy::GridAutoFlow::RowDense
        } else {
            taffy::GridAutoFlow::Row
        };
        self
    }

    /// Backfill holes earlier in the grid with later, smaller children.
    /// Keeps the row or column flow already set.
    fn grid_dense(mut self) -> Self {
        let l = &mut self.element_style_mut().layout;
        l.grid_auto_flow = match l.grid_auto_flow {
            taffy::GridAutoFlow::Row | taffy::GridAutoFlow::RowDense => {
                taffy::GridAutoFlow::RowDense
            }
            taffy::GridAutoFlow::Column | taffy::GridAutoFlow::ColumnDense => {
                taffy::GridAutoFlow::ColumnDense
            }
        };
        self
    }

    /// Span `n` columns of the parent grid.
    fn col_span(mut self, n: u16) -> Self {
        let col = &mut self.element_style_mut().layout.grid_column;
        col.end = taffy::GridPlacement::Span(n);
        if matches!(col.start, taffy::GridPlacement::Span(_)) {
            col.start = taffy::GridPlacement::Auto;
        }
        self
    }

    /// Span `n` rows of the parent grid.
    fn row_span(mut self, n: u16) -> Self {
        let row = &mut self.element_style_mut().layout.grid_row;
        row.end = taffy::GridPlacement::Span(n);
        if matches!(row.start, taffy::GridPlacement::Span(_)) {
            row.start = taffy::GridPlacement::Auto;
        }
        self
    }

    /// Start at column line `line` (1 is the left edge, -1 the right).
    fn col_start(mut self, line: i16) -> Self {
        self.element_style_mut().layout.grid_column.start = grid_line(line);
        self
    }

    /// End at column line `line`.
    fn col_end(mut self, line: i16) -> Self {
        self.element_style_mut().layout.grid_column.end = grid_line(line);
        self
    }

    /// Start at row line `line` (1 is the top edge, -1 the bottom).
    fn row_start(mut self, line: i16) -> Self {
        self.element_style_mut().layout.grid_row.start = grid_line(line);
        self
    }

    /// End at row line `line`.
    fn row_end(mut self, line: i16) -> Self {
        self.element_style_mut().layout.grid_row.end = grid_line(line);
        self
    }

    // -- Intrinsic sizing --

    /// Keep width / height at `ratio` when only one of them is set (or
    /// stretched).
    fn aspect_ratio(mut self, ratio: f32) -> Self {
        self.element_style_mut().layout.aspect_ratio = Some(ratio);
        self
    }

    /// Size to the content in a flex line: the main axis takes the
    /// content's unwrapped (max-content) size and never shrinks; the cross
    /// axis does not stretch, so it fits the content too (clamped to the
    /// space available, as CSS `fit-content` is).
    fn size_max_content(mut self) -> Self {
        let l = &mut self.element_style_mut().layout;
        l.size = taffy::Size::auto();
        l.flex_basis = taffy::Dimension::auto();
        l.flex_shrink = 0.0;
        l.align_self = Some(taffy::AlignSelf::FlexStart);
        self
    }

    fn border_w(mut self, w: f32) -> Self {
        let s = self.element_style_mut();
        s.border_widths = [w; 4];
        let width = taffy::LengthPercentage::length(w);
        s.layout.border = taffy::Rect {
            left: width,
            right: width,
            top: width,
            bottom: width,
        };
        self
    }
}

#[cfg(test)]
mod direction_tests {
    use super::*;
    use crate::element::{ElementContext, IntoAnyElement, div, render_element};
    use crate::theme::Theme;
    use quark::reactive::SignalStore;
    use quark_render::{Primitive, Scene};

    // Catches the locale's direction not reaching layout: under right to
    // left the first child of a row sits at the right edge.
    #[test]
    fn right_to_left_mirrors_rows() {
        let first = Color::rgba(255, 0, 0, 255);
        let mut text = quark_text::TextSystem::vendored_only(&Default::default());
        let mut layouts = quark_text::LayoutCache::default();
        let signals = SignalStore::new();
        let theme = Theme::default_dark();
        let mut first_x = |direction| {
            set_layout_direction(direction);
            let mut root = div()
                .w(400.0)
                .h(20.0)
                .flex_row()
                .child(div().w(100.0).h(20.0).bg(first))
                .child(div().w(50.0).h(20.0).bg(Color::rgba(0, 0, 255, 255)))
                .into_any();
            set_layout_direction(Direction::LeftToRight);
            let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
            let mut scene = Scene::default();
            render_element(&mut root, &mut scene, &mut cx, 400.0, 20.0);
            scene.primitives.iter().find_map(|p| match p {
                Primitive::RoundedRect(r) if r.color == first => Some(r.rect.x),
                Primitive::Rect(r) if r.color == first => Some(r.rect.x),
                _ => None,
            })
        };
        assert_eq!(
            [
                first_x(Direction::LeftToRight),
                first_x(Direction::RightToLeft)
            ],
            [Some(0.0), Some(300.0)]
        );
    }
}
