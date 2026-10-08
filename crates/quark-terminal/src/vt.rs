//! Safe bindings over libghostty-vt: one [`Terminal`] owns the C terminal,
//! a render state with its row and cell iterators, and key and mouse
//! encoders, all reused across calls.
//!
//! The bindings are hand-written over bindgen's raw `sys` module rather
//! than generated wholesale: the C API is option-and-getter shaped
//! (`ghostty_terminal_get(term, DATA_X, void*)`), so each getter needs the
//! right output type, sized structs need their `size` set, and callbacks
//! need a stable userdata pointer. Those rules live here once.
//!
//! Callbacks the terminal fires during [`Terminal::write`] (replies to
//! queries, bell, title, OSC 52) land in [`Effects`], which the caller
//! drains after each write.

use std::ffi::c_void;
use std::ptr::{self, NonNull};

use crate::grid::{CellStyle, Colors, Cursor, CursorShape, Grid, GridRow, Rgb, Run, Underline};
use crate::sys;

/// A terminal mode (DEC private unless noted), for [`Terminal::mode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Mode(u16);

impl Mode {
    pub const fn dec(value: u16) -> Self {
        Self(value & 0x7fff)
    }

    pub const fn ansi(value: u16) -> Self {
        Self((value & 0x7fff) | 0x8000)
    }

    pub const CURSOR_KEYS: Self = Self::dec(1);
    pub const CURSOR_VISIBLE: Self = Self::dec(25);
    pub const ALT_SCROLL: Self = Self::dec(1007);
    pub const FOCUS_EVENT: Self = Self::dec(1004);
    pub const BRACKETED_PASTE: Self = Self::dec(2004);
    pub const ALT_SCREEN: Self = Self::dec(1049);
}

/// What the terminal asked of its host during the writes since the last
/// [`Terminal::take_effects`].
#[derive(Debug, Default)]
pub struct Effects {
    /// Bytes for the PTY: query replies, and encoded keys, mouse reports,
    /// and pastes.
    pub pty: Vec<u8>,
    pub bells: u32,
    pub title_changed: bool,
    /// The last OSC 52 clipboard write, when [`Terminal::set_clipboard_write`]
    /// allows them.
    pub clipboard: Option<String>,
}

/// Effects plus the settings callbacks read. Boxed so its address, the
/// callbacks' userdata, never moves.
#[derive(Debug, Default)]
struct Host {
    effects: Effects,
    allow_clipboard_write: bool,
    /// Rows, columns, and cell width and height in pixels.
    size: (u16, u16, u32, u32),
}

/// Where [`Terminal::scroll`] moves the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scroll {
    Top,
    Bottom,
    /// Rows down (negative is up).
    Delta(isize),
    /// The first visible row, counted from the top of the scrollback.
    Row(usize),
}

/// Rows of scrollable content: `offset` is the first visible row and `len`
/// the visible count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Scrollbar {
    pub total: u64,
    pub offset: u64,
    pub len: u64,
}

/// What [`Terminal::snapshot`] changed in the grid.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Changes {
    /// Some row's cells or selection, or the grid's size or colors.
    pub rows: bool,
    pub cursor: bool,
}

/// Key modifiers, Ghostty's bit layout.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Mods(pub u16);

impl Mods {
    pub const SHIFT: Self = Self(sys::GHOSTTY_MODS_SHIFT as u16);
    pub const CTRL: Self = Self(sys::GHOSTTY_MODS_CTRL as u16);
    pub const ALT: Self = Self(sys::GHOSTTY_MODS_ALT as u16);
    pub const SUPER: Self = Self(sys::GHOSTTY_MODS_SUPER as u16);

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for Mods {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// A key on a US layout, by its W3C `KeyboardEvent.code` (Ghostty's
/// `GhosttyKey`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key(pub(crate) sys::GhosttyKey);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Press,
    Repeat,
    Release,
}

/// One key event to encode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyInput<'a> {
    pub key: Key,
    pub mods: Mods,
    /// Text the key types on the current layout before Ctrl or Alt
    /// change it; `None` for keys that type nothing.
    pub text: Option<&'a str>,
    /// The key's character without Shift, for the Kitty protocol.
    pub unshifted: Option<char>,
    pub action: KeyAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
}

/// Geometry mouse reports are encoded against, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseGeometry {
    pub width: u32,
    pub height: u32,
    pub cell_width: u32,
    pub cell_height: u32,
}

/// A paste was refused: the text could run a command (a newline outside
/// bracketed paste, or the bracketed paste terminator).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsafePaste;

/// A libghostty-vt terminal and its render state. Not `Send`: it lives on
/// the UI thread, fed by the PTY reader's messages.
pub struct Terminal {
    raw: NonNull<sys::GhosttyTerminalImpl>,
    render: NonNull<sys::GhosttyRenderStateImpl>,
    rows: NonNull<sys::GhosttyRenderStateRowIteratorImpl>,
    cells: NonNull<sys::GhosttyRenderStateRowCellsImpl>,
    keys: NonNull<sys::GhosttyKeyEncoderImpl>,
    key_event: NonNull<sys::GhosttyKeyEventImpl>,
    mouse: NonNull<sys::GhosttyMouseEncoderImpl>,
    mouse_event: NonNull<sys::GhosttyMouseEventImpl>,
    host: Box<Host>,
    /// Grapheme bytes of the cell being read.
    cell_text: Vec<u8>,
}

impl std::fmt::Debug for Terminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Terminal")
            .field("cols", &self.cols())
            .field("rows", &self.rows())
            .finish_non_exhaustive()
    }
}

/// `ok(result)` is true for GHOSTTY_SUCCESS.
fn ok(result: sys::GhosttyResult) -> bool {
    result == sys::GHOSTTY_SUCCESS
}

fn new_handle<T>(create: impl FnOnce(*mut *mut T) -> sys::GhosttyResult) -> NonNull<T> {
    let mut out = ptr::null_mut();
    let result = create(&mut out);
    assert!(ok(result), "libghostty-vt allocation failed ({result})");
    NonNull::new(out).expect("libghostty-vt returned a null handle")
}

fn rgb(c: sys::GhosttyColorRgb) -> Rgb {
    Rgb::new(c.r, c.g, c.b)
}

fn c_rgb(c: Rgb) -> sys::GhosttyColorRgb {
    sys::GhosttyColorRgb {
        r: c.r,
        g: c.g,
        b: c.b,
    }
}

/// C structs for which all-zero bytes are a valid value.
///
/// # Safety
///
/// Implement only for plain C data: integers, bools, raw pointers, nullable
/// function pointers, and unions and arrays of those.
unsafe trait Zeroable {}

macro_rules! zeroable {
    ($($ty:ty),* $(,)?) => {
        // SAFETY: bindgen's plain C structs of the listed field kinds.
        $(unsafe impl Zeroable for $ty {})*
    };
}

zeroable!(
    sys::GhosttyClipboardWriteReply,
    sys::GhosttyGridRef,
    sys::GhosttyMouseEncoderSize,
    sys::GhosttyPaste,
    sys::GhosttyRenderStateColors,
    sys::GhosttyRenderStateCursor,
    sys::GhosttyRenderStateRowSelection,
    sys::GhosttySelection,
    sys::GhosttyStyle,
    sys::GhosttyTerminalSelectLineOptions,
    sys::GhosttyTerminalSelectWordOptions,
    sys::GhosttyTerminalSelectionFormatOptions,
);

fn zeroed<T: Zeroable>() -> T {
    // SAFETY: `Zeroable` types accept all-zero bytes.
    unsafe { std::mem::zeroed() }
}

/// A zeroed sized struct with its `size` field set, as GHOSTTY_INIT_SIZED.
macro_rules! sized {
    ($ty:ty) => {{
        let mut value: $ty = zeroed();
        value.size = std::mem::size_of::<$ty>();
        value
    }};
}

impl Terminal {
    /// A terminal of `cols` by `rows` cells (each at least 1) with Ghostty's
    /// default scrollback.
    pub fn new(cols: u16, rows: u16) -> Self {
        let (cols, rows) = (cols.max(1), rows.max(1));
        // SAFETY: each constructor gets a valid out pointer and the default
        // allocator; `new_handle` checks the result.
        let raw =
            new_handle(|out| unsafe { sys::ghostty_terminal_new(ptr::null(), out, cols, rows) });
        let render = new_handle(|out| unsafe { sys::ghostty_render_state_new(ptr::null(), out) });
        let row_iter = new_handle(|out| unsafe {
            sys::ghostty_render_state_row_iterator_new(ptr::null(), out)
        });
        let cells =
            new_handle(|out| unsafe { sys::ghostty_render_state_row_cells_new(ptr::null(), out) });
        let keys = new_handle(|out| unsafe { sys::ghostty_key_encoder_new(ptr::null(), out) });
        let key_event = new_handle(|out| unsafe { sys::ghostty_key_event_new(ptr::null(), out) });
        let mouse = new_handle(|out| unsafe { sys::ghostty_mouse_encoder_new(ptr::null(), out) });
        let mouse_event =
            new_handle(|out| unsafe { sys::ghostty_mouse_event_new(ptr::null(), out) });
        let mut term = Self {
            raw,
            render,
            rows: row_iter,
            cells,
            keys,
            key_event,
            mouse,
            mouse_event,
            host: Box::default(),
            cell_text: vec![0; 64],
        };
        term.install_callbacks();
        let name = b"xterm-256color";
        let name = sys::GhosttyString {
            ptr: name.as_ptr(),
            len: name.len(),
        };
        term.set(
            sys::GHOSTTY_TERMINAL_OPT_TERMINFO_NAME,
            (&raw const name).cast(),
        );
        term
    }

    fn set(&mut self, option: sys::GhosttyTerminalOption, value: *const c_void) {
        // SAFETY: callers pass the value type `option` documents.
        unsafe { sys::ghostty_terminal_set(self.raw.as_ptr(), option, value) };
    }

    /// Reads a terminal value of type `T`.
    ///
    /// # Safety
    ///
    /// `T` must be the output type `data` documents.
    unsafe fn get<T>(&self, data: sys::GhosttyTerminalData, out: &mut T) -> bool {
        // SAFETY: the caller guarantees the output type.
        ok(unsafe { sys::ghostty_terminal_get(self.raw.as_ptr(), data, (out as *mut T).cast()) })
    }

    fn install_callbacks(&mut self) {
        let userdata: *mut Host = &mut *self.host;
        self.set(sys::GHOSTTY_TERMINAL_OPT_USERDATA, userdata.cast());
        let write_pty: sys::GhosttyTerminalWritePtyFn = Some(on_write_pty);
        let bell: sys::GhosttyTerminalBellFn = Some(on_bell);
        let title: sys::GhosttyTerminalTitleChangedFn = Some(on_title);
        let clipboard: sys::GhosttyTerminalClipboardWriteFn = Some(on_clipboard_write);
        let attrs: sys::GhosttyTerminalDeviceAttributesFn = Some(on_device_attributes);
        let size: sys::GhosttyTerminalSizeFn = Some(on_size);
        // Callbacks are passed as the pointer value itself.
        self.set(sys::GHOSTTY_TERMINAL_OPT_WRITE_PTY, fn_ptr(write_pty));
        self.set(sys::GHOSTTY_TERMINAL_OPT_BELL, fn_ptr(bell));
        self.set(sys::GHOSTTY_TERMINAL_OPT_TITLE_CHANGED, fn_ptr(title));
        self.set(sys::GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE, fn_ptr(clipboard));
        self.set(sys::GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES, fn_ptr(attrs));
        self.set(sys::GHOSTTY_TERMINAL_OPT_SIZE, fn_ptr(size));
    }

    // ---- Stream ---------------------------------------------------------

    /// Feeds bytes from the PTY through the VT parser.
    pub fn write(&mut self, bytes: &[u8]) {
        // SAFETY: a valid terminal and slice; callbacks only touch `host`,
        // which this borrow does not alias.
        unsafe { sys::ghostty_terminal_vt_write(self.raw.as_ptr(), bytes.as_ptr(), bytes.len()) };
    }

    /// Everything callbacks recorded since the last call.
    pub fn take_effects(&mut self) -> Effects {
        std::mem::take(&mut self.host.effects)
    }

    /// Bytes queued for the PTY, without taking the other effects.
    pub fn pty_output(&mut self) -> &mut Vec<u8> {
        &mut self.host.effects.pty
    }

    /// Whether OSC 52 may write the clipboard. Off by default: any program
    /// that prints to the terminal could otherwise replace the clipboard.
    pub fn set_clipboard_write(&mut self, allow: bool) {
        self.host.allow_clipboard_write = allow;
    }

    // ---- Size and scrolling ---------------------------------------------

    /// Resizes the grid, reflowing the primary screen. Cell sizes are in
    /// physical pixels, for size reports and pixel mouse modes.
    pub fn resize(&mut self, cols: u16, rows: u16, cell_width: u32, cell_height: u32) {
        let (cols, rows) = (cols.max(1), rows.max(1));
        self.host.size = (rows, cols, cell_width, cell_height);
        // SAFETY: a valid terminal; sizes are at least one cell.
        unsafe {
            sys::ghostty_terminal_resize(self.raw.as_ptr(), cols, rows, cell_width, cell_height)
        };
    }

    pub fn cols(&self) -> u16 {
        let mut v = 0u16;
        // SAFETY: COLS is a u16.
        unsafe { self.get(sys::GHOSTTY_TERMINAL_DATA_COLS, &mut v) };
        v
    }

    pub fn rows(&self) -> u16 {
        let mut v = 0u16;
        // SAFETY: ROWS is a u16.
        unsafe { self.get(sys::GHOSTTY_TERMINAL_DATA_ROWS, &mut v) };
        v
    }

    pub fn scroll(&mut self, to: Scroll) {
        let mut behavior = sys::GhosttyTerminalScrollViewport {
            tag: sys::GHOSTTY_SCROLL_VIEWPORT_BOTTOM,
            value: sys::GhosttyTerminalScrollViewportValue { _padding: [0; 2] },
        };
        match to {
            Scroll::Top => behavior.tag = sys::GHOSTTY_SCROLL_VIEWPORT_TOP,
            Scroll::Bottom => {}
            Scroll::Delta(delta) => {
                behavior.tag = sys::GHOSTTY_SCROLL_VIEWPORT_DELTA;
                behavior.value.delta = delta;
            }
            Scroll::Row(row) => {
                behavior.tag = sys::GHOSTTY_SCROLL_VIEWPORT_ROW;
                behavior.value.row = row;
            }
        }
        // SAFETY: a valid terminal and a fully initialized tagged union.
        unsafe { sys::ghostty_terminal_scroll_viewport(self.raw.as_ptr(), behavior) };
    }

    pub fn scrollbar(&self) -> Scrollbar {
        let mut s = sys::GhosttyTerminalScrollbar {
            total: 0,
            offset: 0,
            len: 0,
        };
        // SAFETY: SCROLLBAR is a GhosttyTerminalScrollbar.
        unsafe { self.get(sys::GHOSTTY_TERMINAL_DATA_SCROLLBAR, &mut s) };
        Scrollbar {
            total: s.total,
            offset: s.offset,
            len: s.len,
        }
    }

    /// Lines kept above the screen. Ghostty rounds to whole pages, so a few
    /// dozen more may be kept.
    pub fn set_scrollback_lines(&mut self, lines: usize) {
        self.set(
            sys::GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES,
            (&raw const lines).cast(),
        );
    }

    // ---- State ----------------------------------------------------------

    pub fn mode(&self, mode: Mode) -> bool {
        let mut config = sys::GhosttyTerminalModeConfig {
            mode: mode.0,
            value: false,
        };
        // SAFETY: MODE is an in/out GhosttyTerminalModeConfig.
        unsafe { self.get(sys::GHOSTTY_TERMINAL_DATA_MODE, &mut config) };
        config.value
    }

    pub fn alternate_screen(&self) -> bool {
        let mut screen: sys::GhosttyTerminalScreen = 0;
        // SAFETY: ACTIVE_SCREEN is a GhosttyTerminalScreen.
        unsafe { self.get(sys::GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN, &mut screen) };
        screen == sys::GHOSTTY_TERMINAL_SCREEN_ALTERNATE
    }

    /// Whether the running program asked for mouse reports.
    pub fn mouse_tracking(&self) -> bool {
        let mut on = false;
        // SAFETY: MOUSE_TRACKING is a bool.
        unsafe { self.get(sys::GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING, &mut on) };
        on
    }

    /// The title from OSC 0 or 2; empty until a program sets one.
    pub fn title(&self) -> String {
        let mut s = sys::GhosttyString {
            ptr: ptr::null(),
            len: 0,
        };
        // SAFETY: TITLE is a GhosttyString borrowed until the next mutation;
        // it is copied before returning.
        if !unsafe { self.get(sys::GHOSTTY_TERMINAL_DATA_TITLE, &mut s) } || s.ptr.is_null() {
            return String::new();
        }
        // SAFETY: the terminal owns `len` bytes at `ptr`.
        String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(s.ptr, s.len) }).into_owned()
    }

    /// The default foreground and background (programs may override them
    /// with OSC 10 and 11).
    pub fn set_default_colors(&mut self, foreground: Rgb, background: Rgb) {
        let (fg, bg) = (c_rgb(foreground), c_rgb(background));
        self.set(
            sys::GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND,
            (&raw const fg).cast(),
        );
        self.set(
            sys::GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND,
            (&raw const bg).cast(),
        );
    }

    // ---- Snapshot -------------------------------------------------------

    /// Refreshes `grid` from the terminal: rows the render state marks
    /// dirty (all of them when the size or colors changed), each row's
    /// selection, the cursor, and the colors. Returns what in `grid`
    /// changed. Reuses `grid`'s buffers.
    pub fn snapshot(&mut self, grid: &mut Grid) -> Changes {
        let rs = self.render.as_ptr();
        // SAFETY: valid handles; every get passes its documented type.
        unsafe {
            sys::ghostty_render_state_update(rs, self.raw.as_ptr());
            let mut dirty: sys::GhosttyRenderStateDirty = 0;
            sys::ghostty_render_state_get(
                rs,
                sys::GHOSTTY_RENDER_STATE_DATA_DIRTY,
                (&raw mut dirty).cast(),
            );
            let mut colors = sized!(sys::GhosttyRenderStateColors);
            sys::ghostty_render_state_get(
                rs,
                sys::GHOSTTY_RENDER_STATE_DATA_COLORS,
                (&raw mut colors).cast(),
            );
            let (mut cols, mut rows) = (0u16, 0u16);
            sys::ghostty_render_state_get(
                rs,
                sys::GHOSTTY_RENDER_STATE_DATA_COLS,
                (&raw mut cols).cast(),
            );
            sys::ghostty_render_state_get(
                rs,
                sys::GHOSTTY_RENDER_STATE_DATA_ROWS,
                (&raw mut rows).cast(),
            );
            let new_colors = Colors {
                foreground: rgb(colors.foreground),
                background: rgb(colors.background),
            };
            let full = dirty == sys::GHOSTTY_RENDER_STATE_DIRTY_FULL
                || grid.cols != cols
                || grid.rows.len() != rows as usize
                || grid.colors != new_colors;
            // Dirty rows can reread to the same cells (the cursor's row
            // when only the cursor moved), so compare hashes instead.
            let mut rows_changed = full;
            grid.cols = cols;
            grid.colors = new_colors;
            grid.rows.resize_with(rows as usize, GridRow::default);

            let mut iter = self.rows.as_ptr();
            sys::ghostty_render_state_get(
                rs,
                sys::GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                (&raw mut iter).cast(),
            );
            let mut y = 0usize;
            while y < grid.rows.len() && sys::ghostty_render_state_row_iterator_next(iter) {
                let mut row_dirty = false;
                sys::ghostty_render_state_row_get(
                    iter,
                    sys::GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY,
                    (&raw mut row_dirty).cast(),
                );
                let mut sel = sized!(sys::GhosttyRenderStateRowSelection);
                let selection = ok(sys::ghostty_render_state_row_get(
                    iter,
                    sys::GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION,
                    (&raw mut sel).cast(),
                ))
                .then_some((sel.start_x, sel.end_x));
                let row = &mut grid.rows[y];
                let before = row.hash;
                if full || row_dirty {
                    self.read_row(iter, row, new_colors);
                    row.selection = selection;
                    row.rehash();
                } else if row.selection != selection {
                    row.selection = selection;
                    row.rehash();
                }
                rows_changed |= row.hash != before;
                y += 1;
            }

            let mut c = sized!(sys::GhosttyRenderStateCursor);
            sys::ghostty_render_state_get(
                rs,
                sys::GHOSTTY_RENDER_STATE_DATA_CURSOR,
                (&raw mut c).cast(),
            );
            let cursor = Cursor {
                at: (c.visible && c.viewport_has_value).then_some((c.viewport_x, c.viewport_y)),
                wide: c.viewport_has_value && c.wide_tail,
                shape: match c.visual_style {
                    sys::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BAR => CursorShape::Bar,
                    sys::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE => {
                        CursorShape::Underline
                    }
                    sys::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK_HOLLOW => {
                        CursorShape::BlockHollow
                    }
                    _ => CursorShape::Block,
                },
                blinking: c.blinking,
                color: colors.cursor_has_value.then(|| rgb(colors.cursor)),
            };
            let cursor_changed = grid.cursor != cursor;
            grid.cursor = cursor;
            sys::ghostty_render_state_clean(rs);
            Changes {
                rows: rows_changed,
                cursor: cursor_changed,
            }
        }
    }

    /// Reads the iterator's current row into `row`.
    ///
    /// # Safety
    ///
    /// `iter` must be this terminal's row iterator, positioned on a row.
    unsafe fn read_row(
        &mut self,
        iter: sys::GhosttyRenderStateRowIterator,
        row: &mut GridRow,
        colors: Colors,
    ) {
        row.clear();
        let mut raw_row: sys::GhosttyRow = 0;
        let mut cells = self.cells.as_ptr();
        // SAFETY: (all calls) valid handles and documented output types.
        unsafe {
            sys::ghostty_render_state_row_get(
                iter,
                sys::GHOSTTY_RENDER_STATE_ROW_DATA_RAW,
                (&raw mut raw_row).cast(),
            );
            let mut wrapped = false;
            sys::ghostty_row_get(
                raw_row,
                sys::GHOSTTY_ROW_DATA_WRAP,
                (&raw mut wrapped).cast(),
            );
            row.wrapped = wrapped;
            sys::ghostty_render_state_row_get(
                iter,
                sys::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                (&raw mut cells).cast(),
            );
        }
        let mut col = 0u16;
        // Length of `row.text` at the end of the last cell that is not a
        // default blank, for trimming.
        let mut keep_text = 0usize;
        let mut keep_runs = 0usize;
        // SAFETY: as above.
        while unsafe { sys::ghostty_render_state_row_cells_next(cells) } {
            let here = col;
            col += 1;
            let mut raw: sys::GhosttyCell = 0;
            let mut wide: sys::GhosttyCellWide = 0;
            // SAFETY: as above.
            unsafe {
                sys::ghostty_render_state_row_cells_get(
                    cells,
                    sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW,
                    (&raw mut raw).cast(),
                );
                sys::ghostty_cell_get(raw, sys::GHOSTTY_CELL_DATA_WIDE, (&raw mut wide).cast());
            }
            if wide == sys::GHOSTTY_CELL_WIDE_SPACER_TAIL {
                // Drawn by the wide character before it.
                continue;
            }
            let text = self.cell_text(cells);
            let style = unsafe { cell_style(cells, raw, colors) };
            let width = if wide == sys::GHOSTTY_CELL_WIDE_WIDE {
                2
            } else {
                1
            };
            let blank = text.is_empty() || text == b" ";
            let start = row.text.len() as u32;
            match std::str::from_utf8(text) {
                Ok(s) if !s.is_empty() => row.text.push_str(s),
                Ok(_) => row.text.push(' '),
                Err(_) => row.text.push('\u{fffd}'),
            }
            let end = row.text.len() as u32;
            let narrow_ascii = width == 1 && end - start == 1;
            match row.runs.last_mut() {
                Some(last)
                    if narrow_ascii
                        && last.style == style
                        && last.col + last.cols == here
                        && last.text.len() as u16 == last.cols =>
                {
                    last.cols += 1;
                    last.text.end = end;
                }
                _ => row.runs.push(Run {
                    col: here,
                    cols: width,
                    text: start..end,
                    style,
                }),
            }
            if !blank || style != CellStyle::plain(colors) {
                keep_text = row.text.len();
                keep_runs = row.runs.len();
            }
        }
        row.text.truncate(keep_text);
        row.runs.truncate(keep_runs);
        if let Some(last) = row.runs.last_mut()
            && last.text.end as usize > keep_text
        {
            let cut = last.text.end - keep_text as u32;
            last.text.end -= cut;
            last.cols -= cut as u16;
        }
        // A wide character in the last column has no second half when a
        // resize did not reflow the screen (the alternate screen); keep its
        // run inside the grid.
        if let Some(last) = row.runs.last_mut()
            && last.col + last.cols > col
        {
            last.cols = col - last.col;
        }
        debug_assert_eq!(row.verify_integrity(), Ok(()));
    }

    /// The UTF-8 grapheme of the cells iterator's current cell (empty for
    /// an empty cell), in `self.cell_text`.
    fn cell_text(&mut self, cells: sys::GhosttyRenderStateRowCells) -> &[u8] {
        loop {
            let mut buf = sys::GhosttyBuffer {
                ptr: self.cell_text.as_mut_ptr(),
                cap: self.cell_text.len(),
                len: 0,
            };
            // SAFETY: GRAPHEMES_UTF8 writes at most `cap` bytes to `ptr`.
            let result = unsafe {
                sys::ghostty_render_state_row_cells_get(
                    cells,
                    sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
                    (&raw mut buf).cast(),
                )
            };
            if result == sys::GHOSTTY_OUT_OF_SPACE && buf.len > self.cell_text.len() {
                self.cell_text.resize(buf.len, 0);
                continue;
            }
            let len = if ok(result) { buf.len } else { 0 };
            return &self.cell_text[..len];
        }
    }

    // ---- Selection ------------------------------------------------------

    /// The grid reference of viewport cell `(col, row)`.
    fn viewport_ref(&self, col: u16, row: u16) -> Option<sys::GhosttyGridRef> {
        let point = sys::GhosttyPoint {
            tag: sys::GHOSTTY_POINT_TAG_VIEWPORT,
            value: sys::GhosttyPointValue {
                coordinate: sys::GhosttyPointCoordinate {
                    x: col,
                    y: u32::from(row),
                },
            },
        };
        let mut out = sized!(sys::GhosttyGridRef);
        // SAFETY: a valid terminal, point, and out pointer.
        ok(unsafe { sys::ghostty_terminal_grid_ref(self.raw.as_ptr(), point, &mut out) })
            .then_some(out)
    }

    /// Selects from viewport cell `anchor` to `focus`, inclusive.
    pub fn select(&mut self, anchor: (u16, u16), focus: (u16, u16)) {
        let (Some(start), Some(end)) = (
            self.viewport_ref(anchor.0, anchor.1),
            self.viewport_ref(focus.0, focus.1),
        ) else {
            return;
        };
        let mut selection = sized!(sys::GhosttySelection);
        selection.start = start;
        selection.end = end;
        self.set(
            sys::GHOSTTY_TERMINAL_OPT_SELECTION,
            (&raw const selection).cast(),
        );
    }

    /// The word at viewport cell `at` as `(start, end)` cells, inclusive, in
    /// viewport coordinates.
    pub fn word_at(&self, at: (u16, u16)) -> Option<((u16, u16), (u16, u16))> {
        let mut options = sized!(sys::GhosttyTerminalSelectWordOptions);
        options.ref_ = self.viewport_ref(at.0, at.1)?;
        let mut out = sized!(sys::GhosttySelection);
        // SAFETY: valid terminal, options, and out pointer.
        let found =
            ok(unsafe { sys::ghostty_terminal_select_word(self.raw.as_ptr(), &options, &mut out) });
        found.then(|| self.selection_bounds(&out)).flatten()
    }

    /// The line at viewport cell `at` (following soft wraps), as
    /// [`Self::word_at`] reports a word.
    pub fn line_at(&self, at: (u16, u16)) -> Option<((u16, u16), (u16, u16))> {
        let mut options = sized!(sys::GhosttyTerminalSelectLineOptions);
        options.ref_ = self.viewport_ref(at.0, at.1)?;
        let mut out = sized!(sys::GhosttySelection);
        // SAFETY: valid terminal, options, and out pointer.
        let found =
            ok(unsafe { sys::ghostty_terminal_select_line(self.raw.as_ptr(), &options, &mut out) });
        found.then(|| self.selection_bounds(&out)).flatten()
    }

    fn selection_bounds(&self, s: &sys::GhosttySelection) -> Option<((u16, u16), (u16, u16))> {
        let point = |r: &sys::GhosttyGridRef| {
            let mut c = sys::GhosttyPointCoordinate { x: 0, y: 0 };
            // SAFETY: `r` came from this terminal with no mutation since.
            let found = ok(unsafe {
                sys::ghostty_terminal_point_from_grid_ref(
                    self.raw.as_ptr(),
                    r,
                    sys::GHOSTTY_POINT_TAG_VIEWPORT,
                    &mut c,
                )
            });
            found.then_some((c.x, c.y as u16))
        };
        Some((point(&s.start)?, point(&s.end)?))
    }

    pub fn clear_selection(&mut self) {
        self.set(sys::GHOSTTY_TERMINAL_OPT_SELECTION, ptr::null());
    }

    /// The selected text as a copy should put it on the clipboard: soft
    /// wraps joined and trailing blanks trimmed.
    pub fn selection_text(&self) -> Option<String> {
        let mut options = sized!(sys::GhosttyTerminalSelectionFormatOptions);
        options.emit = sys::GHOSTTY_FORMATTER_FORMAT_PLAIN;
        options.unwrap = true;
        options.trim = true;
        let (mut out, mut len) = (ptr::null_mut(), 0usize);
        // SAFETY: valid terminal and out pointers; the buffer is freed with
        // the allocator it came from.
        unsafe {
            if !ok(sys::ghostty_terminal_selection_format_alloc(
                self.raw.as_ptr(),
                ptr::null(),
                options,
                &mut out,
                &mut len,
            )) {
                return None;
            }
            let text = if out.is_null() {
                String::new()
            } else {
                String::from_utf8_lossy(std::slice::from_raw_parts(out, len)).into_owned()
            };
            sys::ghostty_free(ptr::null(), out, len);
            Some(text)
        }
    }

    /// The OSC 8 hyperlink URI of viewport cell `(col, row)`.
    pub fn hyperlink_at(&self, col: u16, row: u16) -> Option<String> {
        let at = self.viewport_ref(col, row)?;
        let mut buf = vec![0u8; 256];
        loop {
            let mut len = 0usize;
            // SAFETY: a fresh ref and a buffer of `buf.len()` bytes.
            let result = unsafe {
                sys::ghostty_grid_ref_hyperlink_uri(&at, buf.as_mut_ptr(), buf.len(), &mut len)
            };
            if result == sys::GHOSTTY_OUT_OF_SPACE && len > buf.len() {
                buf.resize(len, 0);
                continue;
            }
            if !ok(result) || len == 0 {
                return None;
            }
            buf.truncate(len);
            return String::from_utf8(buf).ok();
        }
    }

    // ---- Input encoding --------------------------------------------------

    /// Encodes `input` for the current modes (cursor keys, keypad, Kitty
    /// keyboard flags) and queues it for the PTY. Returns whether the key
    /// produced bytes.
    pub fn key(&mut self, input: &KeyInput) -> bool {
        let (enc, ev) = (self.keys.as_ptr(), self.key_event.as_ptr());
        let text = input.text.filter(|t| !t.chars().any(char::is_control));
        // SAFETY: valid handles; `text` outlives the encode call that reads it.
        let bytes = unsafe {
            sys::ghostty_key_encoder_setopt_from_terminal(enc, self.raw.as_ptr());
            sys::ghostty_key_event_set_action(
                ev,
                match input.action {
                    KeyAction::Press => sys::GHOSTTY_KEY_ACTION_PRESS,
                    KeyAction::Repeat => sys::GHOSTTY_KEY_ACTION_REPEAT,
                    KeyAction::Release => sys::GHOSTTY_KEY_ACTION_RELEASE,
                },
            );
            sys::ghostty_key_event_set_key(ev, input.key.0);
            sys::ghostty_key_event_set_mods(ev, input.mods.0);
            // macOS Option types characters (Ghostty's default, Option is
            // not Alt): when it produced the text, it is spent on it, so
            // the encoder sends the text instead of nothing.
            let consumed = if cfg!(target_os = "macos") && text.is_some() {
                input.mods.0 & sys::GHOSTTY_MODS_ALT as u16
            } else {
                0
            };
            sys::ghostty_key_event_set_consumed_mods(ev, consumed);
            sys::ghostty_key_event_set_composing(ev, false);
            match text {
                Some(t) => sys::ghostty_key_event_set_utf8(ev, t.as_ptr().cast(), t.len()),
                None => sys::ghostty_key_event_set_utf8(ev, ptr::null(), 0),
            }
            sys::ghostty_key_event_set_unshifted_codepoint(
                ev,
                input.unshifted.map_or(0, u32::from),
            );
            let mut buf = [0u8; 128];
            let mut len = 0usize;
            let result = sys::ghostty_key_encoder_encode(
                enc,
                ev,
                buf.as_mut_ptr().cast(),
                buf.len(),
                &mut len,
            );
            // Forget the borrowed text before it goes out of scope.
            sys::ghostty_key_event_set_utf8(ev, ptr::null(), 0);
            if !ok(result) {
                return false;
            }
            (buf, len)
        };
        self.host.effects.pty.extend_from_slice(&bytes.0[..bytes.1]);
        bytes.1 > 0
    }

    /// Encodes a mouse event for the program's tracking mode and format and
    /// queues it. `x` and `y` are physical pixels from the grid's top left.
    /// Returns whether a report was produced (none while tracking is off).
    pub fn mouse(
        &mut self,
        action: MouseAction,
        button: Option<MouseButton>,
        (x, y): (f32, f32),
        mods: Mods,
        geometry: MouseGeometry,
        any_button_pressed: bool,
    ) -> bool {
        let (enc, ev) = (self.mouse.as_ptr(), self.mouse_event.as_ptr());
        let mut size = sized!(sys::GhosttyMouseEncoderSize);
        size.screen_width = geometry.width;
        size.screen_height = geometry.height;
        size.cell_width = geometry.cell_width.max(1);
        size.cell_height = geometry.cell_height.max(1);
        let mut buf = [0u8; 64];
        let mut len = 0usize;
        // SAFETY: valid handles and option value types.
        let result = unsafe {
            sys::ghostty_mouse_encoder_setopt_from_terminal(enc, self.raw.as_ptr());
            sys::ghostty_mouse_encoder_setopt(
                enc,
                sys::GHOSTTY_MOUSE_ENCODER_OPT_SIZE,
                (&raw const size).cast(),
            );
            sys::ghostty_mouse_encoder_setopt(
                enc,
                sys::GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED,
                (&raw const any_button_pressed).cast(),
            );
            sys::ghostty_mouse_event_set_action(
                ev,
                match action {
                    MouseAction::Press => sys::GHOSTTY_MOUSE_ACTION_PRESS,
                    MouseAction::Release => sys::GHOSTTY_MOUSE_ACTION_RELEASE,
                    MouseAction::Motion => sys::GHOSTTY_MOUSE_ACTION_MOTION,
                },
            );
            match button {
                None => sys::ghostty_mouse_event_clear_button(ev),
                Some(b) => sys::ghostty_mouse_event_set_button(
                    ev,
                    match b {
                        MouseButton::Left => sys::GHOSTTY_MOUSE_BUTTON_LEFT,
                        MouseButton::Right => sys::GHOSTTY_MOUSE_BUTTON_RIGHT,
                        MouseButton::Middle => sys::GHOSTTY_MOUSE_BUTTON_MIDDLE,
                        MouseButton::WheelUp => sys::GHOSTTY_MOUSE_BUTTON_FOUR,
                        MouseButton::WheelDown => sys::GHOSTTY_MOUSE_BUTTON_FIVE,
                        MouseButton::WheelLeft => sys::GHOSTTY_MOUSE_BUTTON_SIX,
                        MouseButton::WheelRight => sys::GHOSTTY_MOUSE_BUTTON_SEVEN,
                    },
                ),
            }
            sys::ghostty_mouse_event_set_mods(ev, mods.0);
            sys::ghostty_mouse_event_set_position(ev, sys::GhosttyMousePosition { x, y });
            sys::ghostty_mouse_encoder_encode(enc, ev, buf.as_mut_ptr().cast(), buf.len(), &mut len)
        };
        if !ok(result) {
            return false;
        }
        self.host.effects.pty.extend_from_slice(&buf[..len]);
        len > 0
    }

    /// Queues a focus in or out report when the program enabled them.
    pub fn focus(&mut self, gained: bool) {
        if !self.mode(Mode::FOCUS_EVENT) {
            return;
        }
        let mut buf = [0u8; 8];
        let mut len = 0usize;
        let event = if gained {
            sys::GHOSTTY_FOCUS_GAINED
        } else {
            sys::GHOSTTY_FOCUS_LOST
        };
        // SAFETY: a buffer of `buf.len()` bytes.
        if ok(unsafe {
            sys::ghostty_focus_encode(event, buf.as_mut_ptr().cast(), buf.len(), &mut len)
        }) {
            self.host.effects.pty.extend_from_slice(&buf[..len]);
        }
    }

    /// Pastes `text` as the terminal's modes require: wrapped in bracketed
    /// paste markers when the program enabled them, newlines turned into
    /// carriage returns otherwise, control bytes replaced. Text that could
    /// run a command is refused unless `allow_unsafe`.
    pub fn paste(&mut self, text: &str, allow_unsafe: bool) -> Result<(), UnsafePaste> {
        let mime = b"text/plain";
        let mime = sys::GhosttyString {
            ptr: mime.as_ptr(),
            len: mime.len(),
        };
        let mut paste = sized!(sys::GhosttyPaste);
        paste.location = sys::GHOSTTY_CLIPBOARD_LOCATION_STANDARD;
        paste.source = sys::GHOSTTY_PASTE_SOURCE_CLIPBOARD;
        paste.mimes = &mime;
        paste.mimes_len = 1;
        paste.reader = sys::GhosttyMimeReader {
            read: Some(read_paste),
            userdata: (&raw const text).cast_mut().cast(),
        };
        paste.allow_unsafe = allow_unsafe;
        // SAFETY: `paste`, `mime`, and `text` outlive the call; the reader
        // only reads `text`, and output goes to the write_pty callback.
        let result =
            unsafe { sys::ghostty_terminal_paste(self.raw.as_ptr(), &paste, ptr::null_mut()) };
        if result == sys::GHOSTTY_REJECTED {
            Err(UnsafePaste)
        } else {
            Ok(())
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // SAFETY: each handle is owned and freed once.
        unsafe {
            sys::ghostty_mouse_event_free(self.mouse_event.as_ptr());
            sys::ghostty_mouse_encoder_free(self.mouse.as_ptr());
            sys::ghostty_key_event_free(self.key_event.as_ptr());
            sys::ghostty_key_encoder_free(self.keys.as_ptr());
            sys::ghostty_render_state_row_cells_free(self.cells.as_ptr());
            sys::ghostty_render_state_row_iterator_free(self.rows.as_ptr());
            sys::ghostty_render_state_free(self.render.as_ptr());
            sys::ghostty_terminal_free(self.raw.as_ptr());
        }
    }
}

impl CellStyle {
    /// An unstyled cell on `colors`.
    pub(crate) fn plain(colors: Colors) -> Self {
        Self {
            fg: colors.foreground,
            ..Self::default()
        }
    }
}

/// The resolved style of the cells iterator's current cell.
///
/// # Safety
///
/// `cells` must be positioned on a cell, and `raw` must be that cell.
unsafe fn cell_style(
    cells: sys::GhosttyRenderStateRowCells,
    raw: sys::GhosttyCell,
    colors: Colors,
) -> CellStyle {
    let mut style = CellStyle::plain(colors);
    // SAFETY: (all calls) documented output types for a positioned iterator.
    unsafe {
        let mut fg = sys::GhosttyColorRgb { r: 0, g: 0, b: 0 };
        let mut bg = sys::GhosttyColorRgb { r: 0, g: 0, b: 0 };
        let fg = ok(sys::ghostty_render_state_row_cells_get(
            cells,
            sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
            (&raw mut fg).cast(),
        ))
        .then(|| rgb(fg));
        let bg = ok(sys::ghostty_render_state_row_cells_get(
            cells,
            sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
            (&raw mut bg).cast(),
        ))
        .then(|| rgb(bg));
        let mut hyperlink = false;
        sys::ghostty_cell_get(
            raw,
            sys::GHOSTTY_CELL_DATA_HAS_HYPERLINK,
            (&raw mut hyperlink).cast(),
        );
        style.hyperlink = hyperlink;
        style.fg = fg.unwrap_or(colors.foreground);
        style.bg = bg;
        let mut styled = false;
        sys::ghostty_render_state_row_cells_get(
            cells,
            sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_HAS_STYLING,
            (&raw mut styled).cast(),
        );
        if !styled {
            return style;
        }
        let mut s = sized!(sys::GhosttyStyle);
        sys::ghostty_render_state_row_cells_get(
            cells,
            sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
            (&raw mut s).cast(),
        );
        style.bold = s.bold;
        style.italic = s.italic;
        style.faint = s.faint;
        style.blink = s.blink;
        style.strikethrough = s.strikethrough;
        style.overline = s.overline;
        style.underline = match s.underline {
            sys::GHOSTTY_SGR_UNDERLINE_SINGLE => Underline::Single,
            sys::GHOSTTY_SGR_UNDERLINE_DOUBLE => Underline::Double,
            sys::GHOSTTY_SGR_UNDERLINE_CURLY => Underline::Curly,
            sys::GHOSTTY_SGR_UNDERLINE_DOTTED => Underline::Dotted,
            sys::GHOSTTY_SGR_UNDERLINE_DASHED => Underline::Dashed,
            _ => Underline::None,
        };
        if s.underline_color.tag == sys::GHOSTTY_STYLE_COLOR_RGB {
            style.underline_color = Some(rgb(s.underline_color.value.rgb));
        }
        if s.inverse {
            let fg = style.fg;
            style.fg = style.bg.unwrap_or(colors.background);
            style.bg = Some(fg);
        }
        if s.invisible {
            style.fg = style.bg.unwrap_or(colors.background);
        }
    }
    style
}

fn fn_ptr<F: Copy>(f: Option<F>) -> *const c_void {
    debug_assert_eq!(
        std::mem::size_of::<F>(),
        std::mem::size_of::<*const c_void>()
    );
    match f {
        // SAFETY: F is an `extern "C" fn` pointer type, pointer sized.
        Some(f) => unsafe { std::mem::transmute_copy::<F, *const c_void>(&f) },
        None => ptr::null(),
    }
}

/// The host behind a callback's userdata.
///
/// # Safety
///
/// `userdata` must be the `Host` installed by `install_callbacks`, alive and
/// not otherwise borrowed during the callback.
unsafe fn host<'a>(userdata: *mut c_void) -> &'a mut Host {
    // SAFETY: guaranteed by the caller.
    unsafe { &mut *userdata.cast::<Host>() }
}

unsafe extern "C" fn on_write_pty(
    _: sys::GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) {
    if data.is_null() || len == 0 {
        return;
    }
    // SAFETY: the terminal passes our userdata and `len` readable bytes.
    unsafe {
        host(userdata)
            .effects
            .pty
            .extend_from_slice(std::slice::from_raw_parts(data, len))
    };
}

unsafe extern "C" fn on_bell(_: sys::GhosttyTerminal, userdata: *mut c_void) {
    // SAFETY: the terminal passes our userdata.
    unsafe { host(userdata).effects.bells += 1 };
}

unsafe extern "C" fn on_title(_: sys::GhosttyTerminal, userdata: *mut c_void) {
    // SAFETY: the terminal passes our userdata.
    unsafe { host(userdata).effects.title_changed = true };
}

unsafe extern "C" fn on_clipboard_write(
    _: sys::GhosttyTerminal,
    userdata: *mut c_void,
    write: *const sys::GhosttyClipboardWrite,
) {
    // SAFETY: the terminal passes our userdata and a request valid for the
    // duration of the call.
    unsafe {
        let host = host(userdata);
        let write = &*write;
        let mut reply = sized!(sys::GhosttyClipboardWriteReply);
        reply.result = sys::GHOSTTY_CLIPBOARD_WRITE_RESULT_DENIED;
        if host.allow_clipboard_write && write.location == sys::GHOSTTY_CLIPBOARD_LOCATION_STANDARD
        {
            let contents = if write.contents.is_null() {
                &[][..]
            } else {
                std::slice::from_raw_parts(write.contents, write.contents_len)
            };
            let bytes = |s: sys::GhosttyString| {
                if s.ptr.is_null() {
                    &[][..]
                } else {
                    std::slice::from_raw_parts(s.ptr, s.len)
                }
            };
            if let Some(text) = contents
                .iter()
                .find(|c| bytes(c.mime).starts_with(b"text/"))
            {
                host.effects.clipboard =
                    Some(String::from_utf8_lossy(bytes(text.data)).into_owned());
                reply.result = sys::GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS;
            }
        }
        if let Some(answer) = write.reply {
            answer(write, &reply);
        }
    }
}

unsafe extern "C" fn on_device_attributes(
    _: sys::GhosttyTerminal,
    _: *mut c_void,
    out: *mut sys::GhosttyDeviceAttributes,
) -> bool {
    // SAFETY: the terminal passes a writable struct.
    let out = unsafe { &mut *out };
    // A VT220 with ANSI color, as xterm-256color terminals report.
    out.primary.conformance_level = sys::GHOSTTY_DA_CONFORMANCE_LEVEL_2 as u16;
    out.primary.features[0] = sys::GHOSTTY_DA_FEATURE_ANSI_COLOR as u16;
    out.primary.num_features = 1;
    out.secondary.device_type = sys::GHOSTTY_DA_DEVICE_TYPE_VT220 as u16;
    out.secondary.firmware_version = 1;
    out.secondary.rom_cartridge = 0;
    out.tertiary.unit_id = 0;
    true
}

unsafe extern "C" fn on_size(
    _: sys::GhosttyTerminal,
    userdata: *mut c_void,
    out: *mut sys::GhosttySizeReportSize,
) -> bool {
    // SAFETY: the terminal passes our userdata and a writable struct.
    unsafe {
        let (rows, columns, cell_width, cell_height) = host(userdata).size;
        *out = sys::GhosttySizeReportSize {
            rows,
            columns,
            cell_width,
            cell_height,
        };
        cell_width > 0
    }
}

unsafe extern "C" fn read_paste(
    userdata: *mut c_void,
    _mime: sys::GhosttyString,
    writer: sys::GhosttyWriter,
) -> bool {
    // SAFETY: userdata is the `&str` `paste` passed, alive for the call.
    let text: &str = unsafe { *userdata.cast::<&str>() };
    match writer.write {
        // SAFETY: the writer accepts `len` readable bytes.
        Some(write) => unsafe { write(writer.userdata, text.as_ptr(), text.len()) },
        None => false,
    }
}
