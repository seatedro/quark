//! A drawn menu bar, for platforms without a native one (Linux and the
//! BSDs) and for apps that want the same bar everywhere.
//!
//! [`MenuBar`] holds the top-level menus, each a list of
//! [`ContextMenuEntry`] rows, and draws the open one with the context
//! menu's panels, so submenus, check marks, disabled rows, and shortcut
//! hints look and navigate the same. `quark_app` builds the menus from the
//! app's platform `Menu` data (`quark_app::platform::drawn_menu`).
//!
//! Keyboard, as on Windows and GNOME:
//!
//! - F10 opens the first menu; F10 again (or Escape twice) leaves the bar.
//! - [`MenuBar::activate`] (call it for a lone Alt tap) highlights the
//!   first title without opening it.
//! - With a title highlighted: Left and Right move along the bar, Down,
//!   Enter, or Space open the menu.
//! - In an open menu: Up and Down move, Right opens a submenu or moves to
//!   the next menu, Left closes a submenu or moves to the previous menu,
//!   Enter or Space chooses, Escape closes.
//!
//! The bar must sit at the window's top-left corner: its menus are placed
//! in window coordinates, below each title.

use quark::SemanticRole;
use quark_render::Rect;
use quark_ui::Action;
use quark_ui::element::{AnyElement, Binding, IntoAnyElement, div, text};
use quark_ui::style::Styled;
use quark_ui::theme::{Color, Theme};

use crate::context_menu::{ContextMenuEntry, ContextMenuOutcome, ContextMenuState};

/// One top-level menu of the bar.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuBarMenu {
    pub label: String,
    pub enabled: bool,
    pub entries: Vec<ContextMenuEntry>,
}

impl MenuBarMenu {
    pub fn new(label: impl Into<String>, entries: Vec<ContextMenuEntry>) -> Self {
        Self {
            label: label.into(),
            enabled: true,
            entries,
        }
    }
}

/// Where the keyboard is on the bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Inactive,
    /// Title `index` is highlighted, its menu closed.
    Highlighted(usize),
    /// Menu `index` is open.
    Open(usize),
}

/// A menu bar's menus and state. See the [module docs](self).
#[derive(Debug, Clone)]
pub struct MenuBar {
    menus: Vec<MenuBarMenu>,
    mode: Mode,
    dropdown: ContextMenuState,
    /// Title rects of the last render, in window coordinates.
    titles: Vec<Rect>,
}

impl MenuBar {
    pub fn new(menus: Vec<MenuBarMenu>) -> Self {
        Self {
            menus,
            mode: Mode::Inactive,
            dropdown: ContextMenuState::default(),
            titles: Vec::new(),
        }
    }

    /// Replace the menus, closing any open one.
    pub fn set_menus(&mut self, menus: Vec<MenuBarMenu>) {
        self.menus = menus;
        self.close();
    }

    pub fn menus(&self) -> &[MenuBarMenu] {
        &self.menus
    }

    /// The open menu's index.
    pub fn open_menu(&self) -> Option<usize> {
        match self.mode {
            Mode::Open(index) => Some(index),
            _ => None,
        }
    }

    /// The highlighted or open title.
    pub fn active_title(&self) -> Option<usize> {
        match self.mode {
            Mode::Inactive => None,
            Mode::Highlighted(index) | Mode::Open(index) => Some(index),
        }
    }

    pub fn is_active(&self) -> bool {
        self.mode != Mode::Inactive
    }

    /// Highlight the first title without opening it, or leave the bar if
    /// it is already active: what a lone Alt tap does.
    pub fn activate(&mut self) {
        if self.is_active() {
            self.close();
        } else if let Some(first) = self.next_enabled(None, 1) {
            self.mode = Mode::Highlighted(first);
        }
    }

    pub fn close(&mut self) {
        self.mode = Mode::Inactive;
        self.dropdown.close();
    }

    /// A press on title `index`: open its menu, or close it if it is the
    /// open one.
    pub fn title_clicked(&mut self, index: usize) {
        if self.mode == Mode::Open(index) {
            self.close();
        } else {
            self.open(index, false);
        }
    }

    /// The pointer moved: with a menu open, hovering another title opens
    /// that one, and hovering the open menu highlights its rows. Returns
    /// whether anything changed.
    pub fn pointer_moved(&mut self, x: f32, y: f32) -> bool {
        let Mode::Open(open) = self.mode else {
            return false;
        };
        if let Some(index) = self.titles.iter().position(|r| r.contains(x, y))
            && index != open
            && self.menus[index].enabled
        {
            self.open(index, false);
            return true;
        }
        self.dropdown.pointer_moved(x, y)
    }

    /// Open menu `index`; from the keyboard, with its first row
    /// highlighted.
    fn open(&mut self, index: usize, keyboard: bool) {
        let Some(menu) = self.menus.get(index).filter(|menu| menu.enabled) else {
            return;
        };
        let title = self.titles.get(index).copied().unwrap_or_default();
        self.dropdown
            .open(menu.entries.clone(), title.x, title.y + title.height);
        if keyboard {
            self.dropdown
                .handle_key(&Binding::new(Default::default(), "arrowdown"));
        }
        self.mode = Mode::Open(index);
    }

    /// The next enabled menu after `from` in `step` direction, wrapping.
    fn next_enabled(&self, from: Option<usize>, step: isize) -> Option<usize> {
        let len = self.menus.len() as isize;
        let start = match from {
            Some(i) => i as isize,
            None if step > 0 => -1,
            None => len,
        };
        (1..=len)
            .map(|k| (start + step * k).rem_euclid(len) as usize)
            .find(|&i| self.menus[i].enabled)
    }

    /// Handle a key press; `None` when the key is not the bar's. An
    /// [`ContextMenuOutcome::Activate`] carries the chosen item's action,
    /// and the bar has closed.
    pub fn handle_key(&mut self, pressed: &Binding) -> Option<ContextMenuOutcome> {
        let m = pressed.mods;
        let plain = !(m.cmd || m.ctrl || m.alt || m.shift || m.primary);
        if plain && pressed.key == "f10" {
            if self.is_active() {
                self.close();
                return Some(ContextMenuOutcome::Closed);
            }
            let first = self.next_enabled(None, 1)?;
            self.open(first, true);
            return Some(ContextMenuOutcome::Handled);
        }
        match self.mode {
            Mode::Inactive => None,
            Mode::Highlighted(index) => {
                if !plain {
                    return None;
                }
                match pressed.key.as_str() {
                    "arrowleft" | "arrowright" => {
                        let step = if pressed.key == "arrowright" { 1 } else { -1 };
                        if let Some(next) = self.next_enabled(Some(index), step) {
                            self.mode = Mode::Highlighted(next);
                        }
                    }
                    "arrowdown" | "enter" | "space" => self.open(index, true),
                    "escape" => {
                        self.close();
                        return Some(ContextMenuOutcome::Closed);
                    }
                    _ => return None,
                }
                Some(ContextMenuOutcome::Handled)
            }
            Mode::Open(index) => {
                // Left and Right on the top panel walk the bar, unless Right
                // is opening a submenu.
                let top = self.dropdown.depth() == 1;
                let step = match pressed.key.as_str() {
                    "arrowleft" if plain && top => Some(-1),
                    "arrowright" if plain && !self.dropdown.on_submenu() => Some(1),
                    _ => None,
                };
                if let Some(step) = step {
                    if let Some(next) = self.next_enabled(Some(index), step) {
                        self.open(next, true);
                    }
                    return Some(ContextMenuOutcome::Handled);
                }
                let outcome = self.dropdown.handle_key(pressed)?;
                Some(match outcome {
                    ContextMenuOutcome::Activate(action) => {
                        self.close();
                        ContextMenuOutcome::Activate(action)
                    }
                    // Escape on the top panel: back to the highlighted title.
                    ContextMenuOutcome::Closed => {
                        self.mode = Mode::Highlighted(index);
                        ContextMenuOutcome::Handled
                    }
                    ContextMenuOutcome::Handled => ContextMenuOutcome::Handled,
                })
            }
        }
    }

    /// Estimated title width: the bar lays titles out at these widths so
    /// it knows where each menu opens without measuring text.
    fn title_width(label: &str, theme: &Theme) -> f32 {
        let m = &theme.metrics;
        let glyphs = label.chars().count() as f32;
        (glyphs * m.ui_small_font_size * 0.62 + m.spacing_md * 2.0).ceil()
    }

    /// The bar, `viewport`-wide, with the open menu over the window.
    /// `on_title` is the action a press on title `index` emits; hand it
    /// back to [`Self::title_clicked`].
    pub fn render(
        &mut self,
        viewport: (f32, f32),
        theme: &Theme,
        on_title: impl Fn(usize) -> Action,
    ) -> AnyElement {
        let tc = &theme.colors;
        let m = &theme.metrics;
        let height = (m.ui_small_font_size * 1.35 + m.spacing_xs * 2.0 + 4.0).ceil();
        let mut x = m.spacing_xs;
        self.titles.clear();
        let mut bar = div()
            .flex_row()
            .items_center()
            .w(viewport.0)
            .h(height)
            .flex_shrink_0()
            .px(m.spacing_xs)
            .bg(tc.title_bar_background)
            .border_b(tc.border)
            .test_id("menu-bar")
            .semantic_role(SemanticRole::Menu)
            .accessibility_role(accesskit::Role::MenuBar)
            .accessibility_label("Menu bar")
            .key_context("menu-bar");
        for (index, menu) in self.menus.iter().enumerate() {
            let width = Self::title_width(&menu.label, theme);
            self.titles.push(Rect {
                x,
                y: 0.0,
                width,
                height,
            });
            x += width;
            let active = self.active_title() == Some(index);
            let fg = if menu.enabled { tc.text } else { tc.text_muted };
            let mut title = div()
                .flex_row()
                .items_center()
                .justify_center()
                .w(width)
                .h(height - 4.0)
                .flex_shrink_0()
                .rounded(m.spacing_xs)
                .bg(if active {
                    tc.sidebar_row_hover
                } else {
                    Color::TRANSPARENT
                })
                .key(menu.label.as_str())
                .test_id(format!("menu-bar:{}", menu.label))
                .semantic_role(SemanticRole::MenuItem)
                .accessibility_role(accesskit::Role::MenuItem)
                .accessibility_label(menu.label.as_str())
                .accessibility_expanded(self.mode == Mode::Open(index))
                .accessibility_disabled(!menu.enabled)
                .child(text(menu.label.as_str()).text_sm().color(fg));
            if menu.enabled {
                title = title
                    .on_click(on_title(index))
                    .hover_bg(tc.sidebar_row_hover);
            }
            bar = bar.child(title);
        }
        match self.dropdown.render(viewport, theme) {
            Some(layer) => bar.child(layer).into_any(),
            None => bar.into_any(),
        }
    }
}
