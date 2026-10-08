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
    decoder: CellDecoder,
    /// The grid's previous rows during [`Self::redraw_rows`], which rows
    /// of them moved, and where each line's row comes from and its
    /// selection: kept for their storage.
    old_rows: Vec<GridRow>,
    taken: Vec<bool>,
    placed: Vec<Placement>,
}

/// Where a line's row comes from in [`Terminal::redraw_rows`]: the index of
/// the earlier row it matches, if any, and its selection.
type Placement = (Option<usize>, Option<(u16, u16)>);

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
            decoder: CellDecoder::new(),
            old_rows: Vec::new(),
            taken: Vec::new(),
            placed: Vec::new(),
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
        timed!(Snapshot);
        let rs = self.render.as_ptr();
        // SAFETY: valid handles; every get passes its documented type.
        unsafe {
            timed!(
                Update,
                sys::ghostty_render_state_update(rs, self.raw.as_ptr())
            );
            let (dirty, colors, cols, rows) = timed!(Header, {
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
                (dirty, colors, cols, rows)
            });
            let new_colors = Colors {
                foreground: rgb(colors.foreground),
                background: rgb(colors.background),
            };
            let redraw = dirty == sys::GHOSTTY_RENDER_STATE_DIRTY_FULL;
            // Rows read before still draw the same with the same width and
            // default colors.
            let comparable = grid.cols == cols && grid.colors == new_colors;
            let full = redraw || !comparable || grid.rows.len() != rows as usize;
            // Dirty rows can reread to the same cells (the cursor's row
            // when only the cursor moved), so compare hashes instead.
            let mut rows_changed = full;
            grid.cols = cols;
            grid.colors = new_colors;
            if redraw && comparable {
                self.redraw_rows(grid, rows as usize, new_colors);
            } else {
                grid.rows.resize_with(rows as usize, GridRow::new);
                let iter = self.row_iterator();
                let mut y = 0usize;
                while let Some((row_dirty, selection)) = timed!(RowFlags, {
                    (y < grid.rows.len() && sys::ghostty_render_state_row_iterator_next(iter))
                        .then(|| (row_flag(iter), row_selection(iter)))
                }) {
                    let row = &mut grid.rows[y];
                    let before = row.hash;
                    if full || row_dirty {
                        timed!(ReadRow, self.read_row(iter, row, new_colors));
                        row.selection = selection;
                        timed!(Hash, row.rehash());
                    } else if row.selection != selection {
                        row.selection = selection;
                        timed!(Hash, row.rehash());
                    }
                    rows_changed |= row.hash != before;
                    y += 1;
                }
            }

            timed!(Cursor);
            let mut c = sized!(sys::GhosttyRenderStateCursor);
            sys::ghostty_render_state_get(
                rs,
                sys::GHOSTTY_RENDER_STATE_DATA_CURSOR,
                (&raw mut c).cast(),
            );
            let cursor = Cursor {
                at: (c.visible && c.viewport_has_value).then_some((c.viewport_x, c.viewport_y)),
                cell: c.viewport_has_value.then_some((c.viewport_x, c.viewport_y)),
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
            timed!(Clean, sys::ghostty_render_state_clean(rs));
            Changes {
                rows: rows_changed,
                cursor: cursor_changed,
            }
        }
    }

    /// The render state's row iterator, before its first row.
    fn row_iterator(&self) -> sys::GhosttyRenderStateRowIterator {
        let mut iter = self.rows.as_ptr();
        // SAFETY: valid handles and the documented output type.
        unsafe {
            sys::ghostty_render_state_get(
                self.render.as_ptr(),
                sys::GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                (&raw mut iter).cast(),
            );
        }
        iter
    }

    /// Refills `grid.rows` with all `rows` rows when every row redraws (a
    /// scroll moved the viewport, the screen switched). Usually most rows
    /// still hold what some row held before, so a row whose cells and
    /// drawing match an earlier row's (see [`Self::same_row`]) moves from
    /// there with its runs, hash, and id; the rest are read into the
    /// storage left over, preferring the storage at their own line.
    ///
    /// # Safety
    ///
    /// The render state must be updated, and `grid` must have been read
    /// at the same width and default colors.
    unsafe fn redraw_rows(&mut self, grid: &mut Grid, rows: usize, colors: Colors) {
        let mut old = std::mem::take(&mut self.old_rows);
        std::mem::swap(&mut old, &mut grid.rows);
        grid.rows.clear();
        let mut taken = std::mem::take(&mut self.taken);
        taken.clear();
        taken.resize(old.len(), false);
        let mut placed = std::mem::take(&mut self.placed);
        placed.clear();

        // Find each line's earlier row: first where the last line's came
        // from (a scroll moves every line alike), then at its own line,
        // then anywhere.
        let iter = self.row_iterator();
        let mut shift = 0isize;
        // SAFETY: (both passes) the iterator is positioned on the row read.
        while placed.len() < rows && unsafe { sys::ghostty_render_state_row_iterator_next(iter) } {
            timed!(Match);
            let y = placed.len();
            let selection = unsafe { row_selection(iter) };
            let (cells, wrapped) = unsafe { row_cells(iter) };
            let found = [y.checked_add_signed(shift), Some(y)]
                .into_iter()
                .flatten()
                .chain(0..old.len())
                .find(|&j| {
                    j < old.len()
                        && !taken[j]
                        && unsafe { self.same_row(iter, &old[j], cells, wrapped, colors) }
                });
            if let Some(j) = found {
                taken[j] = true;
                shift = j as isize - y as isize;
            }
            placed.push((found, selection));
        }

        let iter = self.row_iterator();
        let mut free = 0;
        for &(found, selection) in &placed {
            unsafe { sys::ghostty_render_state_row_iterator_next(iter) };
            let y = grid.rows.len();
            let mut row = match found {
                Some(j) => std::mem::take(&mut old[j]),
                None => {
                    let spare = if y < old.len() && !taken[y] {
                        Some(y)
                    } else {
                        while free < old.len() && taken[free] {
                            free += 1;
                        }
                        (free < old.len()).then_some(free)
                    };
                    let mut row = match spare {
                        Some(j) => {
                            taken[j] = true;
                            std::mem::take(&mut old[j])
                        }
                        None => GridRow::new(),
                    };
                    timed!(ReadRow, unsafe { self.read_row(iter, &mut row, colors) });
                    row
                }
            };
            if found.is_none() || row.selection != selection {
                row.selection = selection;
                timed!(Hash, row.rehash());
            }
            grid.rows.push(row);
        }
        grid.rows.resize_with(rows, GridRow::new);
        // Storage of rows a shorter screen no longer needs.
        old.clear();
        self.old_rows = old;
        self.taken = taken;
        self.placed = placed;
    }

    /// Whether the row under `iter`, with raw `cells` and soft wrap
    /// `wrapped`, reads to `row`. Equal raw cells can still draw
    /// differently: a style id is an index into the style table of the
    /// row's page, which may differ or have been reused, and grapheme
    /// clusters live outside the cell. So the styles the render state
    /// resolves (at each change of style key) and the clusters are checked
    /// against `row`'s runs too.
    ///
    /// # Safety
    ///
    /// `iter` must be this terminal's row iterator, positioned on the row
    /// whose raw cells are `cells`.
    unsafe fn same_row(
        &mut self,
        iter: sys::GhosttyRenderStateRowIterator,
        row: &GridRow,
        cells: &[sys::GhosttyCell],
        wrapped: bool,
        colors: Colors,
    ) -> bool {
        // Compared cell by cell rather than as slices: slice equality calls
        // `bcmp`, and the one libghostty-vt's bundled compiler-rt exports
        // (which the link picks over libc's) compares a byte at a time,
        // about nine times slower on a row.
        if row.wrapped != wrapped
            || row.cells.len() != cells.len()
            || !row.cells.iter().zip(cells).all(|(a, b)| a == b)
        {
            return false;
        }
        if !row.looked_up {
            // Every cell's text and style came from its raw value.
            return true;
        }
        let plain = CellStyle::plain(colors);
        let mut styles = RowStyles::default();
        let mut prev_key = None;
        let mut prev_raw = None;
        let mut runs = row.runs.iter().peekable();
        for (x, &raw) in cells.iter().enumerate() {
            if prev_raw.replace(raw) == Some(raw) {
                // Checked with the cell before, unless a cluster.
                if self.decoder.get(raw).tag != sys::GHOSTTY_CELL_CONTENT_CODEPOINT_GRAPHEME {
                    continue;
                }
            }
            let bits = self.decoder.get(raw);
            if bits.wide == sys::GHOSTTY_CELL_WIDE_SPACER_TAIL {
                continue;
            }
            let key = bits.style_key(raw);
            let grapheme = bits.tag == sys::GHOSTTY_CELL_CONTENT_CODEPOINT_GRAPHEME;
            if prev_key == Some(key) && !grapheme {
                continue;
            }
            let col = x as u16;
            while runs.next_if(|run| run.col + run.cols <= col).is_some() {}
            // Cells past the runs were trimmed as plain blanks.
            let run = runs.peek().filter(|run| run.col <= col);
            if bits.tag == sys::GHOSTTY_CELL_CONTENT_CODEPOINT_GRAPHEME {
                // SAFETY: `iter` is positioned on this row and `x` in it.
                let cells = unsafe { styles.cell(self, iter, x) };
                let text = self.cell_text(cells);
                let same = match run {
                    // A cluster is a run of its own.
                    Some(run) => grapheme_str(text) == row.run_text(run),
                    None => text.is_empty() || text == b" ",
                };
                if !same {
                    return false;
                }
            }
            if prev_key != Some(key) {
                prev_key = Some(key);
                // SAFETY: as above, and `raw` is cell `x`.
                let style = unsafe { styles.resolve(self, iter, x, raw, key, colors) };
                if style != run.map_or(plain, |run| run.style) {
                    return false;
                }
            }
        }
        true
    }

    /// Reads the iterator's current row into `row`.
    ///
    /// The row's raw cells come in one call, and each decodes through
    /// [`CellDecoder`]. Only grapheme clusters and styled or colored cells
    /// go through the cells iterator, and a styled cell like the one
    /// before it reuses its style, so a row of plain text costs a few C
    /// calls instead of several per cell.
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
        // SAFETY: `iter` is positioned on a row.
        let (raws, wrapped) = unsafe { row_cells(iter) };
        row.wrapped = wrapped;
        row.cells.clear();
        row.cells.extend_from_slice(raws);
        let plain = CellStyle::plain(colors);
        let mut styles = RowStyles::default();
        // The previous cell's style: a cell with the same key has the same
        // style, so runs of it skip comparing styles.
        let mut prev_key = None;
        let mut style = plain;
        let mut style_is_plain = true;
        // Length of `row.text` at the end of the last cell that is not a
        // default blank, for trimming.
        let mut keep_text = 0usize;
        let mut keep_runs = 0usize;
        // The previous cell, when it was one narrow ASCII character, and
        // whether it was kept: a cell equal to it (padding, blank space)
        // extends the same run the same way.
        let mut repeat = None;
        for (x, &raw) in raws.iter().enumerate() {
            let here = x as u16;
            if let Some((prev, kept)) = repeat
                && prev == raw
                && let (Some(&byte), Some(last)) = (row.text.as_bytes().last(), row.runs.last_mut())
            {
                last.cols += 1;
                last.text.end += 1;
                row.text.push(char::from(byte));
                if kept {
                    keep_text = row.text.len();
                    keep_runs = row.runs.len();
                }
                continue;
            }
            repeat = None;
            let bits = self.decoder.get(raw);
            if bits.wide == sys::GHOSTTY_CELL_WIDE_SPACER_TAIL {
                // Drawn by the wide character before it.
                continue;
            }
            let start = row.text.len() as u32;
            // The grapheme as GRAPHEMES_UTF8 reads it: nothing for an empty
            // or color-only cell, or a codepoint that does not encode.
            let blank = if bits.tag == sys::GHOSTTY_CELL_CONTENT_CODEPOINT_GRAPHEME {
                // SAFETY: `iter` is positioned on this row and `x` in it.
                let cells = unsafe { styles.cell(self, iter, x) };
                let text = self.cell_text(cells);
                row.text.push_str(grapheme_str(text));
                text.is_empty() || text == b" "
            } else {
                row.text.push(bits.text.unwrap_or(' '));
                matches!(bits.text, None | Some(' '))
            };
            let key = bits.style_key(raw);
            row.looked_up |= !matches!(key, StyleKey::Plain(_))
                || bits.tag == sys::GHOSTTY_CELL_CONTENT_CODEPOINT_GRAPHEME;
            let same_style = prev_key == Some(key);
            if !same_style {
                prev_key = Some(key);
                // SAFETY: as above, and `raw` is cell `x`.
                style = unsafe { styles.resolve(self, iter, x, raw, key, colors) };
                style_is_plain = style == plain;
            }
            let width = if bits.wide == sys::GHOSTTY_CELL_WIDE_WIDE {
                2
            } else {
                1
            };
            let end = row.text.len() as u32;
            let narrow_ascii = width == 1 && end - start == 1;
            match row.runs.last_mut() {
                Some(last)
                    if narrow_ascii
                        && last.col + last.cols == here
                        && last.text.len() as u16 == last.cols
                        // The previous cell's run is the last one.
                        && (same_style || last.style == style) =>
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
            let kept = !blank || !style_is_plain;
            if kept {
                keep_text = row.text.len();
                keep_runs = row.runs.len();
            }
            if narrow_ascii && bits.tag != sys::GHOSTTY_CELL_CONTENT_CODEPOINT_GRAPHEME {
                repeat = Some((raw, kept));
            }
        }
        let col = raws.len() as u16;
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

    /// Time to walk every viewport row of the current render state `reps`
    /// times, doing progressively more of what [`Self::read_row`] does:
    /// fetching each row's raw cells, decoding them, reading one style
    /// through the cells iterator, the whole conversion into runs, and
    /// hashing the row, or instead recognizing the row as the one read
    /// (what a scroll does for each moved row). Differences between steps are each
    /// part's cost.
    #[cfg(test)]
    pub(crate) fn cell_read_costs(
        &mut self,
        reps: u32,
    ) -> [(&'static str, std::time::Duration); 6] {
        let colors = Colors::default();
        let mut scratch = GridRow::default();
        let mut step = |level: u8| {
            let started = std::time::Instant::now();
            for _ in 0..reps {
                let mut iter = self.rows.as_ptr();
                // SAFETY: valid handles and documented output types.
                unsafe {
                    sys::ghostty_render_state_get(
                        self.render.as_ptr(),
                        sys::GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                        (&raw mut iter).cast(),
                    );
                    while sys::ghostty_render_state_row_iterator_next(iter) {
                        if level >= 3 {
                            self.read_row(iter, &mut scratch, colors);
                            if level == 4 {
                                scratch.rehash();
                            }
                            if level == 5 {
                                let (cells, wrapped) = row_cells(iter);
                                let same = self.same_row(iter, &scratch, cells, wrapped, colors);
                                assert!(same);
                            }
                            std::hint::black_box(&scratch);
                            continue;
                        }
                        let mut view = sys::GhosttyCellsView {
                            ptr: ptr::null(),
                            len: 0,
                        };
                        sys::ghostty_render_state_row_get(
                            iter,
                            sys::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS_RAW,
                            (&raw mut view).cast(),
                        );
                        let cells = std::slice::from_raw_parts(view.ptr, view.len);
                        if level >= 1 {
                            for &raw in cells {
                                std::hint::black_box(self.decoder.get(raw));
                            }
                        }
                        if level >= 2 && !cells.is_empty() {
                            let key = StyleKey::Cell(cells[0]);
                            std::hint::black_box(
                                RowStyles::default().resolve(self, iter, 0, cells[0], key, colors),
                            );
                        }
                        std::hint::black_box(view.len);
                    }
                }
            }
            started.elapsed() / reps
        };
        [
            ("raw cell views", step(0)),
            ("+ decode", step(1)),
            ("+ a style read per row", step(2)),
            ("read_row (all conversion)", step(3)),
            ("+ rehash", step(4)),
            ("read_row + recognizing the row", step(5)),
        ]
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

/// What [`Terminal::read_row`] needs of a raw cell, as `ghostty_cell_get`
/// reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CellBits {
    tag: sys::GhosttyCellContentTag,
    /// A codepoint cell, plain or part of a grapheme cluster, rather than
    /// a color-only one.
    textual: bool,
    /// The codepoint of a codepoint cell, as GRAPHEMES_UTF8 encodes it:
    /// `None` when empty or not encodable.
    text: Option<char>,
    /// 0 for an empty or color-only cell.
    codepoint: u32,
    wide: sys::GhosttyCellWide,
    style_id: u16,
    /// A style other than the default.
    styled: bool,
    hyperlink: bool,
}

impl CellBits {
    fn of(raw: sys::GhosttyCell) -> Self {
        let mut bits = Self {
            tag: 0,
            textual: false,
            text: None,
            codepoint: 0,
            wide: 0,
            style_id: 0,
            styled: false,
            hyperlink: false,
        };
        let keys = [
            sys::GHOSTTY_CELL_DATA_CONTENT_TAG,
            sys::GHOSTTY_CELL_DATA_CODEPOINT,
            sys::GHOSTTY_CELL_DATA_WIDE,
            sys::GHOSTTY_CELL_DATA_STYLE_ID,
            sys::GHOSTTY_CELL_DATA_HAS_STYLING,
            sys::GHOSTTY_CELL_DATA_HAS_HYPERLINK,
        ];
        let mut values: [*mut c_void; 6] = [
            (&raw mut bits.tag).cast(),
            (&raw mut bits.codepoint).cast(),
            (&raw mut bits.wide).cast(),
            (&raw mut bits.style_id).cast(),
            (&raw mut bits.styled).cast(),
            (&raw mut bits.hyperlink).cast(),
        ];
        // SAFETY: each value points at the key's documented output type;
        // every key reads from any cell value.
        unsafe {
            sys::ghostty_cell_get_multi(
                raw,
                keys.len(),
                keys.as_ptr(),
                values.as_mut_ptr(),
                ptr::null_mut(),
            );
        }
        bits.textual = matches!(
            bits.tag,
            sys::GHOSTTY_CELL_CONTENT_CODEPOINT | sys::GHOSTTY_CELL_CONTENT_CODEPOINT_GRAPHEME
        );
        bits.text = char::from_u32(bits.codepoint).filter(|&c| bits.textual && c != '\0');
        bits
    }
}

/// Decoded raw cells, direct-mapped by value. A screen holds few distinct
/// cells (a character in a style), so most decodes are a lookup rather
/// than a C call. A raw cell is a plain value, so entries stay valid
/// across rows, pages, and updates.
struct CellDecoder {
    slots: Box<[(sys::GhosttyCell, CellBits); Self::SLOTS]>,
}

impl CellDecoder {
    const SLOTS: usize = 256;

    fn new() -> Self {
        // Every slot starts as the empty cell, so none needs a validity flag.
        Self {
            slots: Box::new([(0, CellBits::of(0)); Self::SLOTS]),
        }
    }

    fn get(&mut self, raw: sys::GhosttyCell) -> CellBits {
        // Fibonacci hashing: the top bits of the product mix every bit of
        // the cell.
        let i = (raw.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> (64 - Self::SLOTS.ilog2())) as usize;
        let slot = &mut self.slots[i];
        if slot.0 != raw {
            *slot = (raw, CellBits::of(raw));
        }
        slot.1
    }
}

/// What a cell's resolved style depends on within one row: the row's
/// style table entry and the hyperlink flag for a text cell, or the cell
/// itself for a color-only cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StyleKey {
    /// The default style, with or without a hyperlink.
    Plain(bool),
    Style(u16, bool),
    Cell(sys::GhosttyCell),
}

impl CellBits {
    fn style_key(&self, raw: sys::GhosttyCell) -> StyleKey {
        if !self.textual {
            // The color is in the cell itself.
            StyleKey::Cell(raw)
        } else if self.styled {
            StyleKey::Style(self.style_id, self.hyperlink)
        } else {
            StyleKey::Plain(self.hyperlink)
        }
    }
}

/// Styles of one row's cells, read through the cells iterator only when
/// the raw cell does not settle them.
#[derive(Default)]
struct RowStyles {
    /// The cells iterator, filled for the row on first use.
    cells: Option<sys::GhosttyRenderStateRowCells>,
    /// The last style read and its key: equal keys within a row (one
    /// page, so one style table) have equal styles.
    last_read: Option<(StyleKey, CellStyle)>,
}

impl RowStyles {
    /// The cells iterator positioned on cell `x` of `iter`'s row.
    ///
    /// # Safety
    ///
    /// `iter` must be `term`'s row iterator, positioned on the row this
    /// `RowStyles` is for, with more than `x` cells.
    unsafe fn cell(
        &mut self,
        term: &Terminal,
        iter: sys::GhosttyRenderStateRowIterator,
        x: usize,
    ) -> sys::GhosttyRenderStateRowCells {
        let cells = *self.cells.get_or_insert_with(|| {
            let mut cells = term.cells.as_ptr();
            // SAFETY: a valid iterator and the documented output type.
            unsafe {
                sys::ghostty_render_state_row_get(
                    iter,
                    sys::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                    (&raw mut cells).cast(),
                );
            }
            cells
        });
        // SAFETY: a filled cells handle and a column inside its row.
        unsafe { sys::ghostty_render_state_row_cells_select(cells, x as u16) };
        cells
    }

    /// The style of cell `x`, whose raw value is `raw` and style key `key`.
    ///
    /// # Safety
    ///
    /// As [`Self::cell`].
    unsafe fn resolve(
        &mut self,
        term: &Terminal,
        iter: sys::GhosttyRenderStateRowIterator,
        x: usize,
        raw: sys::GhosttyCell,
        key: StyleKey,
        colors: Colors,
    ) -> CellStyle {
        if let StyleKey::Plain(hyperlink) = key {
            return CellStyle {
                hyperlink,
                ..CellStyle::plain(colors)
            };
        }
        if let Some((k, style)) = self.last_read
            && k == key
        {
            return style;
        }
        // SAFETY: as the caller's.
        let style = unsafe { cell_style(self.cell(term, iter, x), raw, colors) };
        self.last_read = Some((key, style));
        style
    }
}

/// A grapheme as [`Terminal::read_row`] writes it: a space for an empty
/// cell.
fn grapheme_str(text: &[u8]) -> &str {
    match std::str::from_utf8(text) {
        Ok("") => " ",
        Ok(s) => s,
        Err(_) => "\u{fffd}",
    }
}

/// Whether the iterator's current row is dirty.
///
/// # Safety
///
/// `iter` must be positioned on a row.
unsafe fn row_flag(iter: sys::GhosttyRenderStateRowIterator) -> bool {
    let mut dirty = false;
    // SAFETY: the documented output type.
    unsafe {
        sys::ghostty_render_state_row_get(
            iter,
            sys::GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY,
            (&raw mut dirty).cast(),
        );
    }
    dirty
}

/// The iterator's current row's selected columns, inclusive.
///
/// # Safety
///
/// As [`row_flag`].
unsafe fn row_selection(iter: sys::GhosttyRenderStateRowIterator) -> Option<(u16, u16)> {
    let mut sel = sized!(sys::GhosttyRenderStateRowSelection);
    // SAFETY: the documented output type.
    ok(unsafe {
        sys::ghostty_render_state_row_get(
            iter,
            sys::GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION,
            (&raw mut sel).cast(),
        )
    })
    .then_some((sel.start_x, sel.end_x))
}

/// The iterator's current row's raw cells, valid until the render state
/// next updates, and whether it soft wraps.
///
/// # Safety
///
/// As [`row_flag`], and the cells must not be used past the next update.
unsafe fn row_cells<'a>(
    iter: sys::GhosttyRenderStateRowIterator,
) -> (&'a [sys::GhosttyCell], bool) {
    let mut raw_row: sys::GhosttyRow = 0;
    let mut wrapped = false;
    let mut view = sys::GhosttyCellsView {
        ptr: ptr::null(),
        len: 0,
    };
    // SAFETY: (all calls) documented output types.
    unsafe {
        sys::ghostty_render_state_row_get(
            iter,
            sys::GHOSTTY_RENDER_STATE_ROW_DATA_RAW,
            (&raw mut raw_row).cast(),
        );
        sys::ghostty_row_get(
            raw_row,
            sys::GHOSTTY_ROW_DATA_WRAP,
            (&raw mut wrapped).cast(),
        );
        sys::ghostty_render_state_row_get(
            iter,
            sys::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS_RAW,
            (&raw mut view).cast(),
        );
    }
    let cells = if view.ptr.is_null() {
        &[][..]
    } else {
        // SAFETY: the render state owns `len` cells at `ptr` until its
        // next update.
        unsafe { std::slice::from_raw_parts(view.ptr, view.len) }
    };
    (cells, wrapped)
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

/// In test builds, `timed!(Scope, expr)` adds the time `expr` takes to that
/// scope (see [`timing`]), and the statement `timed!(Scope);` adds the time
/// until the end of its block. Elsewhere they are `expr` and nothing.
macro_rules! timed {
    ($scope:ident, $e:expr) => {{
        #[cfg(test)]
        let _guard = $crate::vt::timing::Guard::new($crate::vt::timing::Scope::$scope);
        $e
    }};
    ($scope:ident) => {
        #[cfg(test)]
        let _guard = $crate::vt::timing::Guard::new($crate::vt::timing::Scope::$scope);
    };
}
pub(crate) use timed;

/// Wall time spent in each phase of preparing and building a terminal
/// frame, summed per thread, for the ignored profiling test. Compiled only
/// into test builds; elsewhere [`timed!`] is just its expression.
#[cfg(test)]
pub(crate) mod timing {
    use std::cell::Cell;
    use std::time::{Duration, Instant};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Scope {
        /// `TerminalState::prepare`, all of it.
        Prepare,
        SyncScroll,
        /// `Terminal::snapshot`, all of it.
        Snapshot,
        /// `ghostty_render_state_update`.
        Update,
        /// Dirty state, colors, and size reads.
        Header,
        /// Per row: advancing the iterator, its dirty flag and selection.
        RowFlags,
        /// Per row of a redraw: finding the earlier row it matches.
        Match,
        /// Per dirty row: reading its cells into runs.
        ReadRow,
        /// Per changed row: hashing it.
        Hash,
        Cursor,
        /// `ghostty_render_state_clean`.
        Clean,
        /// Scrollbar read and the `Frame` after the snapshot.
        Frame,
        /// `terminal_view`, without the lazily built rows.
        View,
        /// The rows element tree, built when the frame changed.
        Build,
    }

    pub(crate) const SCOPES: [Scope; 14] = [
        Scope::Prepare,
        Scope::SyncScroll,
        Scope::Snapshot,
        Scope::Update,
        Scope::Header,
        Scope::RowFlags,
        Scope::Match,
        Scope::ReadRow,
        Scope::Hash,
        Scope::Cursor,
        Scope::Clean,
        Scope::Frame,
        Scope::View,
        Scope::Build,
    ];

    thread_local! {
        static TOTALS: [Cell<(Duration, u32)>; SCOPES.len()] =
            const { [const { Cell::new((Duration::ZERO, 0)) }; SCOPES.len()] };
    }

    pub(crate) struct Guard(Scope, Instant);

    impl Guard {
        pub(crate) fn new(scope: Scope) -> Self {
            Self(scope, Instant::now())
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            let spent = self.1.elapsed();
            TOTALS.with(|t| {
                let slot = &t[self.0 as usize];
                let (total, n) = slot.get();
                slot.set((total + spent, n + 1));
            });
        }
    }

    /// Each scope's total time and entry count since the last call.
    pub(crate) fn take() -> [(Scope, Duration, u32); SCOPES.len()] {
        TOTALS.with(|t| {
            SCOPES.map(|s| {
                let (d, n) = t[s as usize].take();
                (s, d, n)
            })
        })
    }
}
