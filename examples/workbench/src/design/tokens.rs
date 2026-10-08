//! Workbench geometry, type, elevation, and motion tokens, in logical
//! points at 100% zoom (design section 3). Colors live in
//! `assets/themes/workbench.json`, which the theme adapter loads. Multiply
//! a size by the theme's `metrics.ui_scale()` (the zoom) once where it is
//! used; [`super::recipes`] does that for text and common surfaces.

/// Spacing scale.
pub const SPACE_2: f32 = 2.0;
pub const SPACE_4: f32 = 4.0;
pub const SPACE_6: f32 = 6.0;
pub const SPACE_8: f32 = 8.0;
pub const SPACE_12: f32 = 12.0;
pub const SPACE_16: f32 = 16.0;
pub const SPACE_24: f32 = 24.0;
pub const SPACE_32: f32 = 32.0;

/// Radii.
pub const RADIUS_BADGE: f32 = 4.0;
pub const RADIUS_ROW: f32 = 6.0;
pub const RADIUS_MENU: f32 = 8.0;
pub const RADIUS_COMPOSER: f32 = 12.0;
pub const RADIUS_MODAL: f32 = 16.0;

/// Type sizes (size, line height).
pub const TYPE_META: (f32, f32) = (11.0, 16.0);
pub const TYPE_CONTROL: (f32, f32) = (13.0, 18.0);
pub const TYPE_PROSE: (f32, f32) = (14.0, 22.0);
pub const TYPE_CODE: (f32, f32) = (13.0, 20.0);
pub const TYPE_HEADING: (f32, f32) = (16.0, 22.0);
pub const TYPE_EMPTY_HEADING: (f32, f32) = (22.0, 28.0);

/// Targets.
pub const ICON_BUTTON: f32 = 28.0;
pub const SIDEBAR_ROW: f32 = 32.0;
pub const PRIMARY_CONTROL: f32 = 36.0;
pub const ICON: f32 = 16.0;

/// Shell geometry.
pub const TOP_BAR: f32 = 44.0;
pub const SIDEBAR_WIDTH: f32 = 232.0;
pub const RIGHT_DOCK_WIDTH: f32 = 400.0;
pub const TIMELINE_MAX_WIDTH: f32 = 760.0;
/// macOS traffic lights' reserved leading zone.
pub const TRAFFIC_LIGHT_ZONE: f32 = 80.0;

/// Borders and focus.
pub const DIVIDER: f32 = 1.0;
pub const FOCUS_RING: f32 = 2.0;
pub const FOCUS_RING_OFFSET: f32 = 2.0;

/// A shadow: vertical offset, blur, and black alpha in light and dark.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shadow {
    pub offset_y: f32,
    pub blur: f32,
    pub alpha_light: u8,
    pub alpha_dark: u8,
}

// The renderer composites in linear light (sRGB targets), where black at a
// CSS alpha darkens far less than in a browser. Each alpha here is the
// spec's `a` converted so the darkening matches: 1 - (1 - a)^2.2.
// On the dark canvas no black shadow shows much; raised surfaces there also
// get the lighter `raised_border` hairline from the theme file.

/// Menus, popovers, toasts: the spec's black 12% light, 28% dark.
pub const SHADOW_MENU: Shadow = Shadow {
    offset_y: 4.0,
    blur: 16.0,
    alpha_light: 62,
    alpha_dark: 131,
};
/// Modals: the spec's black 18% light, 40% dark.
pub const SHADOW_MODAL: Shadow = Shadow {
    offset_y: 12.0,
    blur: 40.0,
    alpha_light: 90,
    alpha_dark: 172,
};

/// Motion durations, in milliseconds. Reduced motion drops movement and
/// disclosures entirely (see `recipes`).
pub const MOTION_HOVER_MS: u32 = 100;
pub const MOTION_MENU_MS: u32 = 120;
/// How far a menu travels as it fades in.
pub const MOTION_MENU_SHIFT: f32 = 4.0;
pub const MOTION_DISCLOSURE_MS: u32 = 160;
pub const MOTION_TOAST_MS: u32 = 180;
/// Tooltips appear after the pointer rests this long.
pub const TOOLTIP_DELAY_MS: u64 = 500;
