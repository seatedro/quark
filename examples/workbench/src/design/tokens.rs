//! Workbench geometry tokens, in logical points at 100% zoom (design
//! section 3). Placeholder values from the spec; stream B owns refinement.

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
