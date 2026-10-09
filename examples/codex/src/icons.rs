//! Outline icons drawn after the Codex set: a 16-point grid, 1.3-point
//! strokes, round caps. `currentColor` takes the tint. A few colored ones
//! (app and file badges) carry their own fills.

macro_rules! icon {
    ($name:ident, $body:literal) => {
        pub const $name: &str = concat!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round">"#,
            $body,
            "</svg>"
        );
    };
}

icon!(
    COMPOSE,
    r#"<path d="M7.5 2.5H4a1.5 1.5 0 0 0-1.5 1.5v8A1.5 1.5 0 0 0 4 13.5h8a1.5 1.5 0 0 0 1.5-1.5V8.5"/><path d="M11.6 2.2a1.3 1.3 0 0 1 1.9 1.9L8.4 9.2 6 10l.8-2.4z"/>"#
);
icon!(
    SEARCH,
    r#"<circle cx="7.2" cy="7.2" r="4.4"/><path d="m10.5 10.5 3 3"/>"#
);
icon!(
    CLOCK,
    r#"<circle cx="8" cy="8" r="5.6"/><path d="M8 5v3.2l-1.8 1.6"/>"#
);
icon!(
    AT,
    r#"<circle cx="8" cy="8" r="2.4"/><path d="M10.4 8v.9a1.7 1.7 0 0 0 3.4 0V8A5.8 5.8 0 1 0 11 13"/>"#
);
icon!(
    PROJECT,
    r#"<path d="M4.5 2.5h5l2.5 2.5v6.5"/><path d="M3 4.5h6v9H4.2A1.2 1.2 0 0 1 3 12.3z"/><path d="M5 8h2.2M5 10.3h2.2"/><circle cx="12" cy="12" r="1.6" fill="currentColor" stroke="none"/>"#
);
icon!(
    DOC,
    r#"<path d="M4 2.5h5.5L12 5v7.5a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1v-9a1 1 0 0 1 1-1z"/><path d="M5.5 7.5h5M5.5 10h3.5"/>"#
);
icon!(
    CLOUD,
    r#"<path d="M4.6 12.5a2.9 2.9 0 0 1-.4-5.8 3.9 3.9 0 0 1 7.5.9 2.5 2.5 0 0 1 .3 4.9z"/>"#
);
icon!(
    ALERT,
    r#"<circle cx="8" cy="8" r="5.8"/><path d="M8 5v3.4"/><circle cx="8" cy="10.9" r=".5" fill="currentColor"/>"#
);
icon!(
    PIN,
    r#"<path d="M9.8 2.5 13.5 6.2 11.6 7l-2.4 2.4.3 2.6-1 1-2.1-2.1-3 3"/><path d="M5.4 10.9 3.3 8.8l1-1 2.6.3L9.3 5.7z"/>"#
);
icon!(
    ARCHIVE,
    r#"<rect x="2.5" y="3" width="11" height="3" rx=".8"/><path d="M3.5 6v6a1 1 0 0 0 1 1h7a1 1 0 0 0 1-1V6M6.5 8.6h3"/>"#
);
icon!(
    SIDEBAR,
    r#"<rect x="2" y="3" width="12" height="10" rx="2.2"/><path d="M6 3v10"/>"#
);
icon!(
    PANEL_RIGHT,
    r#"<rect x="2" y="2.5" width="12.5" height="11" rx="2.4"/><path d="M10 2.5v11"/>"#
);
icon!(ARROW_LEFT, r#"<path d="M13 8H3.5M7.5 4 3.5 8l4 4"/>"#);
icon!(ARROW_RIGHT, r#"<path d="M3 8h9.5M8.5 4l4 4-4 4"/>"#);
icon!(ARROW_UP, r#"<path d="M8 13V3.5M4 7.5l4-4 4 4"/>"#);
icon!(CHEVRON_DOWN, r#"<path d="m4.5 6.5 3.5 3.5 3.5-3.5"/>"#);
icon!(CHEVRON_RIGHT, r#"<path d="m6.5 4.5 3.5 3.5-3.5 3.5"/>"#);
icon!(CHEVRON_LEFT, r#"<path d="M9.5 4.5 6 8l3.5 3.5"/>"#);
icon!(PLUS, r#"<path d="M8 2.8v10.4M2.8 8h10.4"/>"#);
icon!(CHECK, r#"<path d="m3.5 8.4 2.9 2.9 6.1-6.6"/>"#);
icon!(CLOSE, r#"<path d="m4 4 8 8M12 4l-8 8"/>"#);
icon!(
    ELLIPSIS,
    r#"<circle cx="3.6" cy="8" r=".9" fill="currentColor" stroke="none"/><circle cx="8" cy="8" r=".9" fill="currentColor" stroke="none"/><circle cx="12.4" cy="8" r=".9" fill="currentColor" stroke="none"/>"#
);
icon!(
    HAND,
    r#"<path d="M5.5 8.5V4.2a1 1 0 0 1 2 0V8M7.5 7.5V3.2a1 1 0 0 1 2 0v4.3M9.5 7.5V4.2a1 1 0 0 1 2 0v5.3a4 4 0 0 1-4 4h-.4a3.6 3.6 0 0 1-2.9-1.5L2.6 9.5a1 1 0 0 1 1.5-1.3l1.4 1.3"/>"#
);
icon!(
    SHIELD_TERM,
    r#"<rect x="2.5" y="3" width="11" height="10" rx="2"/><path d="m5.3 6.6 1.6 1.4-1.6 1.4M8.4 9.6h2.3"/>"#
);
icon!(
    SHIELD_ALERT,
    r#"<path d="M8 2.3 13 4v3.6c0 3-2.2 5.2-5 6.1-2.8-.9-5-3.1-5-6.1V4z"/><path d="M8 5.5v2.6"/><circle cx="8" cy="10.3" r=".45" fill="currentColor"/>"#
);
icon!(
    MIC,
    r#"<rect x="6" y="2.3" width="4" height="7" rx="2"/><path d="M3.8 7.6a4.2 4.2 0 0 0 8.4 0M8 11.8v1.9"/>"#
);
icon!(
    PAPERCLIP,
    r#"<path d="m12.7 7.6-4.6 4.6a3 3 0 0 1-4.3-4.3l4.9-4.9a2 2 0 0 1 2.9 2.9l-4.9 4.9a1 1 0 0 1-1.4-1.4l4.3-4.3"/>"#
);
icon!(
    TARGET,
    r#"<circle cx="8" cy="8" r="5.6"/><circle cx="8" cy="8" r="3"/><circle cx="8" cy="8" r=".6" fill="currentColor"/>"#
);
icon!(
    PLAN,
    r#"<path d="M7.5 4.5h6M7.5 8h6M7.5 11.5h6"/><path d="m2.5 4.4.9.9 1.6-1.7M2.5 7.9l.9.9 1.6-1.7"/><circle cx="3.6" cy="11.5" r=".7"/>"#
);
icon!(
    LAPTOP,
    r#"<rect x="3" y="3.5" width="10" height="7" rx="1"/><path d="M1.8 12.5h12.4"/>"#
);
icon!(
    BRANCH,
    r#"<circle cx="4.5" cy="3.8" r="1.4"/><circle cx="4.5" cy="12.2" r="1.4"/><circle cx="11.5" cy="5.2" r="1.4"/><path d="M4.5 5.2v5.6M11.5 6.6c0 2.6-2 3.2-7 3.6"/>"#
);
icon!(
    STOP_GLYPH,
    r#"<rect x="4" y="4" width="8" height="8" rx="1.6" fill="currentColor" stroke="none"/>"#
);
icon!(
    COPY,
    r#"<rect x="5.5" y="5.5" width="8" height="8" rx="1.6"/><path d="M10.5 5.5V4a1.5 1.5 0 0 0-1.5-1.5H4A1.5 1.5 0 0 0 2.5 4v5A1.5 1.5 0 0 0 4 10.5h1.5"/>"#
);
icon!(
    PENCIL,
    r#"<path d="M10.6 2.9a1.6 1.6 0 0 1 2.3 2.3L5.6 12.5l-3 .8.8-3z"/>"#
);
icon!(
    CUBE,
    r#"<path d="M8 1.8 13.5 5v6L8 14.2 2.5 11V5z"/><path d="M2.5 5 8 8.2 13.5 5M8 8.2v6"/>"#
);
icon!(
    INFO,
    r#"<circle cx="8" cy="8" r="5.6"/><path d="M8 7.3v3.4"/><circle cx="8" cy="5.2" r=".5" fill="currentColor"/>"#
);
icon!(
    SUMMARY,
    r#"<circle cx="3.8" cy="4.5" r="1.3"/><circle cx="3.8" cy="11.5" r="1.3"/><path d="M7.5 4.5h6M7.5 11.5h6"/>"#
);
icon!(
    REVIEW,
    r#"<rect x="2.5" y="2.5" width="11" height="11" rx="2.2"/><path d="M8 5.2v4M6 7.2h4M6 11h4"/>"#
);
icon!(
    TERMINAL,
    r#"<rect x="2" y="2.5" width="12" height="11" rx="2.2"/><path d="m5 6.3 2 1.7-2 1.7M8.6 10.2h2.4"/>"#
);
icon!(
    GLOBE,
    r#"<circle cx="8" cy="8" r="5.6"/><path d="M2.4 8h11.2M8 2.4c1.6 1.6 2.3 3.6 2.3 5.6S9.6 12 8 13.6M8 2.4C6.4 4 5.7 6 5.7 8s.7 4 2.3 5.6"/>"#
);
icon!(
    FOLDER,
    r#"<path d="M2.5 4.5a1 1 0 0 1 1-1h2.8l1.4 1.6h4.8a1 1 0 0 1 1 1v6.4a1 1 0 0 1-1 1H3.5a1 1 0 0 1-1-1z"/>"#
);
icon!(
    FOLDER_OPEN,
    r#"<path d="M2.5 11.5V4.5a1 1 0 0 1 1-1h2.8l1.4 1.6h4a1 1 0 0 1 1 1V7"/><path d="M2.5 11.5 4.3 7.6a1 1 0 0 1 .9-.6h8.3a.6.6 0 0 1 .5.9l-1.7 3.6a1 1 0 0 1-.9.6H3.4z"/>"#
);
icon!(
    CHAT,
    r#"<path d="M8 2.8c3.1 0 5.5 2.1 5.5 4.8S11.1 12.4 8 12.4c-.7 0-1.4-.1-2-.3L3 13.2l.8-2.4C3 10 2.5 8.9 2.5 7.6 2.5 4.9 4.9 2.8 8 2.8z"/>"#
);
icon!(
    SETTINGS,
    r#"<circle cx="8" cy="8" r="2"/><path d="M8 1.8v1.6M8 12.6v1.6M3.6 3.6l1.1 1.1M11.3 11.3l1.1 1.1M1.8 8h1.6M12.6 8h1.6M3.6 12.4l1.1-1.1M11.3 4.7l1.1-1.1"/>"#
);
icon!(
    GAUGE,
    r#"<path d="M2.8 11.5a5.6 5.6 0 1 1 10.4 0"/><path d="M8 8.6 10.6 6"/>"#
);
icon!(
    LOGOUT,
    r#"<path d="M6.5 13.5H4a1.5 1.5 0 0 1-1.5-1.5V4A1.5 1.5 0 0 1 4 2.5h2.5M10 11l3-3-3-3M13 8H6"/>"#
);
icon!(
    USER,
    r#"<circle cx="8" cy="8" r="5.8"/><circle cx="8" cy="6.6" r="2"/><path d="M4.3 12.4a4.3 4.3 0 0 1 7.4 0"/>"#
);
icon!(
    ORG,
    r#"<circle cx="8" cy="8" r="2"/><path d="M8 2.2 13 5v6l-5 2.8L3 11V5z"/>"#
);
icon!(
    EXPAND,
    r#"<path d="M9.5 2.5h4v4M13.5 2.5 9.2 6.8M6.5 13.5h-4v-4M2.5 13.5l4.3-4.3"/>"#
);
icon!(
    COLLAPSE,
    r#"<path d="M13 3 9.3 6.7M9.3 3.6v3.1h3.1M3 13l3.7-3.7M3.6 9.3h3.1v3.1"/>"#
);
icon!(
    LIST_FILTER,
    r#"<path d="M3 4.5h1M3 8h1M3 11.5h1M6.5 4.5h7M6.5 8h5M6.5 11.5h3"/>"#
);
icon!(
    FILE_SEARCH,
    r#"<path d="M9 13.5H4.5a1 1 0 0 1-1-1v-9a1 1 0 0 1 1-1h5L12 5v2.5"/><circle cx="10.6" cy="10.6" r="1.9"/><path d="m12 12 1.6 1.6"/>"#
);
icon!(
    SPLIT_DIFF,
    r#"<rect x="2.5" y="3" width="11" height="10" rx="1.6"/><path d="M2.5 6.5h11M2.5 9.5h11"/>"#
);
icon!(
    FILES,
    r#"<rect x="4.5" y="4.5" width="9" height="8.5" rx="1.6"/><path d="M2.5 10.5v-6a2 2 0 0 1 2-2h6.5"/>"#
);
icon!(
    COMMIT,
    r#"<circle cx="8" cy="8" r="2.3"/><path d="M1.8 8h3.9M10.3 8h3.9"/>"#
);
icon!(
    PR,
    r#"<circle cx="4.5" cy="4" r="1.4"/><circle cx="4.5" cy="12" r="1.4"/><circle cx="11.5" cy="12" r="1.4"/><path d="M4.5 5.4v5.2M11.5 10.6V6.5a1.8 1.8 0 0 0-1.8-1.8H7.5M9 3.2 7.5 4.7 9 6.2"/>"#
);
icon!(
    UNDO,
    r#"<path d="M5.5 4 3 6.5 5.5 9"/><path d="M3 6.5h6.5a3.5 3.5 0 0 1 0 7H7"/>"#
);
icon!(
    OPEN_EXTERNAL,
    r#"<path d="M7 3H4.5A1.5 1.5 0 0 0 3 4.5v7A1.5 1.5 0 0 0 4.5 13h7a1.5 1.5 0 0 0 1.5-1.5V9M9.5 3H13v3.5M13 3 8 8"/>"#
);
icon!(
    CODEX_MARK,
    r#"<path d="M8.6 2.4a3.4 3.4 0 0 1 4.9 3.2 3.4 3.4 0 0 1 0 4.8A3.4 3.4 0 0 1 8.6 13.6a3.4 3.4 0 0 1-4.9-3.2 3.4 3.4 0 0 1 0-4.8A3.4 3.4 0 0 1 8.6 2.4z"/><path d="m5.8 6.6 1.6 1.4-1.6 1.4M8.6 9.6h1.8"/>"#
);
icon!(BOLT, r#"<path d="M8.8 2 3.8 9h4l-.6 5 5-7h-4z"/>"#);
icon!(
    MEMORY,
    r#"<circle cx="8" cy="8" r="5.6"/><path d="M8 2.4v11.2M5.2 5.5c1.3.7 1.3 4.3 0 5M10.8 5.5c-1.3.7-1.3 4.3 0 5"/>"#
);
icon!(
    MCP,
    r#"<path d="m3 8.5 5.2-5.2a1.8 1.8 0 0 1 2.6 2.6L6.7 10M6.7 10l4.6-4.6a1.8 1.8 0 0 1 2.6 2.6l-4.7 4.7a.7.7 0 0 0 0 1l.8.8"/><path d="m5 6.5 3.9-3.9"/>"#
);
icon!(
    FEEDBACK,
    r#"<rect x="2.5" y="2.8" width="11" height="9" rx="1.8"/><path d="M5.5 6.3h5M5.5 8.6h3M5 11.8l-1 2"/>"#
);
icon!(
    DOWNLOAD,
    r#"<path d="M8 2.5v7.5M4.8 7 8 10.2 11.2 7M2.8 13h10.4"/>"#
);
icon!(
    SUN,
    r#"<circle cx="8" cy="8" r="2.6"/><path d="M8 1.6v1.5M8 12.9v1.5M1.6 8h1.5M12.9 8h1.5M3.5 3.5l1 1M11.5 11.5l1 1M3.5 12.5l1-1M11.5 4.5l1-1"/>"#
);
icon!(
    SLIDERS,
    r#"<path d="M3 4.5h5M11 4.5h2M3 11.5h2M8 11.5h5"/><circle cx="9.5" cy="4.5" r="1.5"/><circle cx="6.5" cy="11.5" r="1.5"/>"#
);
icon!(
    SMILE,
    r#"<circle cx="8" cy="8" r="5.6"/><path d="M5.8 9.6c1.2 1.2 3.2 1.2 4.4 0"/><circle cx="6" cy="6.6" r=".5" fill="currentColor"/><circle cx="10" cy="6.6" r=".5" fill="currentColor"/>"#
);
icon!(
    PAW,
    r#"<path d="M8 13.5c-2.6 0-4.2-1.4-4.2-3.4S5.8 6.4 8 6.4s4.2 1.7 4.2 3.7-1.6 3.4-4.2 3.4z"/><circle cx="5.2" cy="3.8" r="1.1"/><circle cx="10.8" cy="3.8" r="1.1"/>"#
);
icon!(
    KEYBOARD,
    r#"<rect x="1.8" y="3.8" width="12.4" height="8.4" rx="1.6"/><path d="M4.5 6.6h.1M7 6.6h.1M9.5 6.6h.1M12 6.6h-.1M5.5 9.4h5"/>"#
);
icon!(
    CAMERA,
    r#"<path d="M3 5.5V4a1 1 0 0 1 1-1h1.5M10.5 3H12a1 1 0 0 1 1 1v1.5M13 10.5V12a1 1 0 0 1-1 1h-1.5M5.5 13H4a1 1 0 0 1-1-1v-1.5"/><circle cx="8" cy="8" r="2"/>"#
);
icon!(
    BROWSER,
    r#"<rect x="2" y="3" width="12" height="10" rx="1.8"/><path d="M2 6h12M4.3 4.5h.1M6 4.5h.1"/>"#
);
icon!(
    CURSOR_CLICK,
    r#"<path d="m6.5 6.5 6.6 2.3-2.9 1.1-1.1 2.9z"/><path d="M4.5 1.8v1.8M1.8 4.5h1.8M2.6 2.6l1.2 1.2M7 2.6 5.9 3.8"/>"#
);
icon!(
    ANCHOR,
    r#"<circle cx="8" cy="3.6" r="1.4"/><path d="M8 5v8.5M5.3 7.3h5.4M2.8 9.5a5.2 5.2 0 0 0 10.4 0"/>"#
);
icon!(
    MONITOR,
    r#"<rect x="2" y="3" width="12" height="8.5" rx="1.5"/><path d="M6 13.5h4"/>"#
);
icon!(
    WORKTREE,
    r#"<path d="M4.5 13.5v-11M4.5 6.5h4.2a2.3 2.3 0 0 0 2.3-2.3V2.5M4.5 10.5h6.5"/><circle cx="11" cy="10.5" r="1.4"/>"#
);
icon!(
    CHAT_PLUS,
    r#"<path d="M8 2.8c3.1 0 5.5 2.1 5.5 4.8S11.1 12.4 8 12.4c-.7 0-1.4-.1-2-.3L3 13.2l.8-2.4C3 10 2.5 8.9 2.5 7.6 2.5 4.9 4.9 2.8 8 2.8z"/><path d="M8 5.6v4M6 7.6h4"/>"#
);
icon!(
    REVEAL,
    r#"<path d="M2.5 4.5a1 1 0 0 1 1-1h2.8l1.4 1.6h4.8a1 1 0 0 1 1 1v6.4a1 1 0 0 1-1 1H3.5a1 1 0 0 1-1-1z"/><path d="M8 8v3M6.6 9.4 8 8l1.4 1.4"/>"#
);
icon!(
    TRASH,
    r#"<path d="M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.6 8.1a1 1 0 0 0 1 .9h3.8a1 1 0 0 0 1-.9l.6-8.1"/>"#
);
icon!(
    ROTATE,
    r#"<path d="M13 8a5 5 0 1 1-1.5-3.6M13 2.5v2.4h-2.4"/>"#
);
icon!(
    FORK,
    r#"<circle cx="4.5" cy="3.5" r="1.3"/><circle cx="11.5" cy="3.5" r="1.3"/><circle cx="8" cy="12.5" r="1.3"/><path d="M4.5 4.8v.9a2 2 0 0 0 2 2h3a2 2 0 0 0 2-2v-.9M8 7.7v3.5"/>"#
);
icon!(
    WINDOW_NEW,
    r#"<rect x="2" y="3" width="12" height="10" rx="1.8"/><path d="M2 6h12M8 8v3M6.5 9.5h3"/>"#
);
icon!(
    CALENDAR_PLUS,
    r#"<rect x="2.5" y="3.5" width="11" height="10" rx="1.6"/><path d="M5.5 2v3M10.5 2v3M2.5 7h11M8 8.8v3M6.5 10.3h3"/>"#
);
icon!(
    SIDE_CHAT,
    r#"<rect x="2" y="2.5" width="12.5" height="11" rx="2.4"/><path d="M10 2.5v11M4.5 6h3M4.5 8.5h2"/>"#
);
icon!(FILTER, r#"<path d="M2.5 3.5h11l-4.2 5v4l-2.6 1v-5z"/>"#);
icon!(COLLAPSE_ALL, r#"<path d="m5 3 3 3 3-3M5 13l3-3 3 3"/>"#);
icon!(LOADER, r#"<path d="M8 2.5a5.5 5.5 0 1 1-5.5 5.5" />"#);
icon!(
    MARK_UNREAD,
    r#"<circle cx="8" cy="8" r="5.6"/><circle cx="8" cy="8" r="2" fill="currentColor" stroke="none"/>"#
);
icon!(
    LINK,
    r#"<path d="M6.8 9.2a2.6 2.6 0 0 0 3.7 0l2-2a2.6 2.6 0 0 0-3.7-3.7l-.7.7M9.2 6.8a2.6 2.6 0 0 0-3.7 0l-2 2a2.6 2.6 0 0 0 3.7 3.7l.7-.7"/>"#
);

/// The colorful "Open in" app icon (a Finder-like face), its own colors.
pub const APP_FINDER: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><rect x="1" y="1" width="14" height="14" rx="3.4" fill="#4aa8f5"/><path d="M8.4 1H11.6A3.4 3.4 0 0 1 15 4.4v7.2a3.4 3.4 0 0 1-3.4 3.4H7.4c.9-2.3 1-4.6.6-7.1L8.4 1z" fill="#e8f2fb"/><path d="M5 5v1.4M11 5v1.4" stroke="#1d2b3a" stroke-width="1.1" stroke-linecap="round"/><path d="M4.5 10c2 1.6 5 1.6 7 0" stroke="#1d2b3a" stroke-width="1.1" fill="none" stroke-linecap="round"/></svg>"##;
pub const APP_TERMINAL: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><rect x="1" y="1.5" width="14" height="13" rx="3" fill="#2b2b2b" stroke="#6b6b6b" stroke-width=".8"/><path d="m4.3 6 2 1.8-2 1.8M7.4 10h3" stroke="#e6e6e6" stroke-width="1.2" fill="none" stroke-linecap="round"/></svg>"##;
pub const APP_XCODE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><rect x="1" y="1" width="14" height="14" rx="3.4" fill="#1f7ef3"/><path d="m5 11.5 6-7M9.5 11.5 6.5 6" stroke="#fff" stroke-width="1.4" stroke-linecap="round"/></svg>"##;
/// The yellow "JS" file badge.
pub const FILE_JS: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><rect x="2" y="2" width="12" height="12" rx="2.4" fill="#3a3524"/><text x="8" y="11.2" font-family="sans-serif" font-size="6.4" font-weight="700" text-anchor="middle" fill="#e7c64b">JS</text></svg>"##;
pub const FILE_JSON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><path d="M6 3.5c-1.4 0-1.6.6-1.6 1.7v1.2c0 .9-.5 1.4-1.4 1.6.9.2 1.4.7 1.4 1.6v1.2c0 1.1.2 1.7 1.6 1.7M10 3.5c1.4 0 1.6.6 1.6 1.7v1.2c0 .9.5 1.4 1.4 1.6-.9.2-1.4.7-1.4 1.6v1.2c0 1.1-.2 1.7-1.6 1.7" stroke="#e8a33d" stroke-width="1.2" fill="none" stroke-linecap="round"/></svg>"##;
pub const FILE_MD: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><path d="M1.8 11V5l2.6 3.2L7 5v6M10.5 5v5.6M8.5 8.8l2 2 2-2" stroke="#4cc38a" stroke-width="1.4" fill="none" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
/// Plugin tiles in the + menu: Documents, PDF, Spreadsheets, Presentations.
pub const PLUG_DOC: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><rect x="2.5" y="1.5" width="11" height="13" rx="2" fill="#2f7bf5"/><path d="M5 6h6M5 8.3h6M5 10.6h4" stroke="#fff" stroke-width="1.1" stroke-linecap="round"/></svg>"##;
pub const PLUG_PDF: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><rect x="2.5" y="1.5" width="11" height="13" rx="2" fill="#e5484d"/><text x="8" y="10.6" font-family="sans-serif" font-size="4.6" font-weight="700" text-anchor="middle" fill="#fff">PDF</text></svg>"##;
pub const PLUG_SHEET: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><rect x="2.5" y="1.5" width="11" height="13" rx="2" fill="#2fa45a"/><path d="M5 6h6v5.5H5zM5 8.7h6M8 6v5.5" stroke="#fff" stroke-width="1" fill="none"/></svg>"##;
pub const PLUG_SLIDES: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><rect x="2.5" y="1.5" width="11" height="13" rx="2" fill="#e8892c"/><rect x="5" y="5.5" width="6" height="4.5" rx=".6" fill="#fff"/></svg>"##;
/// Connector card logos.
pub const LOGO_SLACK: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><g stroke-width="2.4" stroke-linecap="round"><path d="M6 2.5v4" stroke="#36c5f0"/><path d="M2.5 10h4" stroke="#2eb67d"/><path d="M10 13.5v-4" stroke="#ecb22e"/><path d="M13.5 6h-4" stroke="#e01e5a"/></g></svg>"##;
pub const LOGO_GITHUB: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><path fill="currentColor" d="M8 .8a7.2 7.2 0 0 0-2.3 14c.4.1.5-.2.5-.4v-1.3c-2 .4-2.4-.9-2.4-.9-.3-.8-.8-1.1-.8-1.1-.7-.4 0-.4 0-.4.7 0 1.1.7 1.1.7.6 1.1 1.7.8 2.1.6.1-.5.3-.8.5-1-1.6-.2-3.3-.8-3.3-3.6 0-.8.3-1.4.7-1.9 0-.2-.3-.9.1-1.9 0 0 .6-.2 2 .7a6.9 6.9 0 0 1 3.6 0c1.4-.9 2-.7 2-.7.4 1 .1 1.7.1 1.9.5.5.7 1.1.7 1.9 0 2.8-1.7 3.4-3.3 3.6.3.2.5.7.5 1.3v2c0 .2.1.5.5.4A7.2 7.2 0 0 0 8 .8z"/></svg>"##;
pub const LOGO_LINEAR: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6.3" fill="currentColor"/><path d="m3.2 9.6 3.2 3.2M2.9 7.4l5.7 5.7M3.6 5.3l7.1 7.1M5 3.8l7.2 7.2M7.2 3l5.8 5.8" stroke="#181818" stroke-width=".8"/></svg>"##;
/// The rate-limit banner's badge.
pub const BADGE_RESET: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="28" height="28" viewBox="0 0 28 28" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M14 3.5 16.6 5.6 20 5.3 21 8.5 24 10.3 23 13.5 24.3 16.6 21.6 18.5 21.1 21.9 17.7 22.2 15.3 24.6 12.4 22.9 9 23.3 7.9 20.1 5 18.4 5.8 15.1 4.4 12 7.2 10.1 7.7 6.7 11.1 6.4z"/><path d="m11 12.2 2.2 1.8L11 15.8M15 16h2.4"/></svg>"##;

icon!(
    HOME,
    r#"<path d="M2.6 7.2 8 2.6l5.4 4.6v6a.8.8 0 0 1-.8.8H10V10H6v4H3.4a.8.8 0 0 1-.8-.8z" fill="currentColor" stroke="none"/>"#
);
icon!(
    SPACE,
    r#"<path d="M5.5 2.5h7a1 1 0 0 1 1 1v7"/><rect x="2.5" y="5" width="8.5" height="8.5" rx="1.5"/><path d="M5 9.2l1.5 1.5L9 8"/>"#
);
icon!(
    SITES,
    r#"<rect x="2.5" y="2.5" width="4.5" height="4.5" rx="1.2"/><rect x="9" y="2.5" width="4.5" height="4.5" rx="1.2"/><rect x="2.5" y="9" width="4.5" height="4.5" rx="1.2"/><circle cx="11.2" cy="11.2" r="2.3"/>"#
);
icon!(
    NEW_TAB,
    r#"<rect x="2.5" y="2.5" width="11" height="11" rx="2.6"/><path d="M8 5.6v4.8M5.6 8h4.8"/>"#
);
icon!(
    BELL,
    r#"<path d="M4 11V7.2a4 4 0 0 1 8 0V11l1 1.3H3z"/><path d="M6.6 13.8a1.5 1.5 0 0 0 2.8 0"/>"#
);
icon!(
    BOOK,
    r#"<path d="M2.5 3.5h3.8A1.7 1.7 0 0 1 8 5.2v8a1.4 1.4 0 0 0-1.4-1.4H2.5zM13.5 3.5H9.7A1.7 1.7 0 0 0 8 5.2v8a1.4 1.4 0 0 1 1.4-1.4h4.1z"/>"#
);
icon!(
    VOICE,
    r#"<path d="M3 7v2M5.3 5v6M7.6 3v10M9.9 5.5v5M12.2 7v2" stroke-width="1.5"/>"#
);
icon!(
    READ_ALOUD,
    r#"<path d="M3 6.2h2l3-2.6v8.8l-3-2.6H3z"/><path d="M10.5 6a2.8 2.8 0 0 1 0 4M12.2 4.5a5 5 0 0 1 0 7"/>"#
);
icon!(
    RATE,
    r#"<path d="M5 7.5V13H3V7.5zM5 7.5l2.4-4.6a1.2 1.2 0 0 1 2.2.9L9 6.5h3.3a1.2 1.2 0 0 1 1.2 1.4l-.8 4.1a1.2 1.2 0 0 1-1.2 1H5"/>"#
);
icon!(
    FORK_CHAT,
    r#"<path d="M3 3h4.5M3 3v4.5M3 3l5 5v5M13 3H9.5M13 3v3.5M13 3l-3.2 3.2"/>"#
);
icon!(ARROW_DOWN, r#"<path d="M8 3v9.5M4 8.5l4 4 4-4"/>"#);
icon!(
    BRAIN,
    r#"<path d="M8 3.2v9.6M8 3.2A2.3 2.3 0 0 0 4 4.5a2.2 2.2 0 0 0-1 3.8 2.3 2.3 0 0 0 1.5 3.6A2.3 2.3 0 0 0 8 12.8M8 3.2a2.3 2.3 0 0 1 4 1.3 2.2 2.2 0 0 1 1 3.8 2.3 2.3 0 0 1-1.5 3.6A2.3 2.3 0 0 1 8 12.8"/>"#
);
icon!(
    RETURN_KEY,
    r#"<path d="M12.5 4v3.5a1.5 1.5 0 0 1-1.5 1.5H4M6.5 6.5 4 9l2.5 2.5"/>"#
);
icon!(
    REFRESH_CW,
    r#"<path d="M12.8 6.5A5 5 0 0 0 3.6 5M3.2 9.5a5 5 0 0 0 9.2 1.5M3.5 2.5V5H6M12.5 13.5V11H10"/>"#
);
icon!(
    WRAP,
    r#"<path d="M2.5 4h11M2.5 8h9a2 2 0 0 1 0 4H8M9.5 10.5 8 12l1.5 1.5M2.5 12h3"/>"#
);
icon!(
    PET,
    r#"<circle cx="8" cy="8" r="5.6"/><circle cx="6" cy="7" r=".7" fill="currentColor"/><circle cx="10" cy="7" r=".7" fill="currentColor"/><path d="M6.3 10c1 .8 2.4.8 3.4 0"/>"#
);
icon!(
    BULB,
    r#"<path d="M6 11.5h4M6.5 13.5h3M8 2.5a4 4 0 0 0-2.4 7.2c.3.3.4.6.4 1v.3h4v-.3c0-.4.1-.7.4-1A4 4 0 0 0 8 2.5z"/>"#
);
icon!(
    RECORD,
    r#"<circle cx="8" cy="8" r="5.6"/><circle cx="8" cy="8" r="3" fill="currentColor" stroke="none"/>"#
);
icon!(
    SKETCH,
    r#"<path d="M2.5 11.5c2-4 3.5-6 4.5-5s-2 4-.5 4.5 4-4.5 5-3.5-1 3-0 3.5 1.5-1 2.5-2"/>"#
);
icon!(
    HELP,
    r#"<circle cx="8" cy="8" r="5.6"/><path d="M6.4 6.3a1.7 1.7 0 0 1 3.2.8c0 1.2-1.6 1.4-1.6 2.4"/><circle cx="8" cy="11.2" r=".5" fill="currentColor"/>"#
);
icon!(
    CLEAR_ALL,
    r#"<path d="M6.5 13.5h7M3 9.5l5.5-5.5 4 4-4.5 4.5H6z"/>"#
);

/// The Codex agent glyph above the home headline: a cloud badge holding a
/// prompt.
pub const MASCOT: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 48 48" fill="none" stroke="currentColor" stroke-width="2.3" stroke-linecap="round" stroke-linejoin="round"><path d="M44.24 24.00 L43.86 24.89 L43.55 25.76 L43.33 26.62 L43.22 27.49 L43.21 28.38 L43.28 29.32 L43.40 30.30 L43.52 31.33 L43.60 32.38 L43.60 33.44 L43.48 34.48 L43.21 35.48 L42.77 36.39 L42.17 37.20 L41.43 37.90 L40.57 38.47 L39.61 38.93 L38.61 39.29 L37.61 39.57 L36.62 39.83 L35.68 40.08 L34.81 40.38 L34.01 40.75 L33.26 41.20 L32.55 41.75 L31.86 42.39 L31.17 43.10 L30.44 43.83 L29.67 44.55 L28.84 45.21 L27.95 45.76 L27.00 46.17 L26.02 46.40 L25.01 46.44 L24.00 46.29 L23.01 45.98 L22.06 45.52 L21.16 44.96 L20.31 44.35 L19.50 43.74 L18.71 43.16 L17.94 42.67 L17.15 42.26 L16.32 41.96 L15.45 41.75 L14.52 41.61 L13.54 41.51 L12.51 41.40 L11.47 41.25 L10.43 41.01 L9.45 40.66 L8.54 40.17 L7.74 39.54 L7.08 38.78 L6.57 37.90 L6.20 36.93 L5.97 35.90 L5.85 34.85 L5.79 33.80 L5.76 32.78 L5.72 31.81 L5.63 30.90 L5.45 30.03 L5.17 29.20 L4.79 28.38 L4.32 27.57 L3.79 26.74 L3.23 25.87 L2.70 24.96 L2.24 24.00 L1.90 23.01 L1.72 21.99 L1.71 20.98 L1.90 19.99 L2.26 19.04 L2.80 18.15 L3.45 17.32 L4.20 16.57 L4.98 15.87 L5.76 15.22 L6.49 14.58 L7.15 13.93 L7.72 13.25 L8.20 12.52 L8.59 11.71 L8.94 10.84 L9.26 9.90 L9.59 8.93 L9.97 7.94 L10.43 6.99 L11.00 6.10 L11.67 5.33 L12.46 4.69 L13.35 4.22 L14.33 3.91 L15.35 3.77 L16.41 3.78 L17.46 3.89 L18.50 4.07 L19.50 4.26 L20.45 4.44 L21.37 4.55 L22.25 4.57 L23.12 4.49 L24.00 4.29 L24.90 4.02 L25.83 3.68 L26.80 3.34 L27.81 3.02 L28.84 2.79 L29.88 2.68 L30.91 2.72 L31.90 2.94 L32.83 3.34 L33.67 3.91 L34.42 4.63 L35.08 5.46 L35.65 6.35 L36.16 7.27 L36.62 8.17 L37.08 9.03 L37.56 9.81 L38.10 10.52 L38.71 11.15 L39.41 11.71 L40.18 12.24 L41.02 12.76 L41.90 13.31 L42.77 13.90 L43.60 14.56 L44.34 15.31 L44.95 16.14 L45.39 17.05 L45.66 18.02 L45.74 19.04 L45.64 20.07 L45.41 21.10 L45.06 22.10 L44.66 23.07 Z"/><path d="M14.5 17 18.2 22.5 14.5 28"/><path d="M23.5 28.5H32"/></svg>"#;
