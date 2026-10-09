//! Context menus with submenus, checkable items, keybinding hints, and
//! keyboard navigation.
//!
//! [`ContextMenuState`] keeps the open menu and its open submenus. The app
//! passes key presses to [`ContextMenuState::handle_key`] and pointer
//! moves to [`ContextMenuState::pointer_moved`] (hovering a submenu row
//! opens it), and renders it with [`ContextMenuState::render`]. Clicking an
//! item emits its action; the app closes the menu when it handles one.

use quark::view;

use quark_render::Rect;
use quark_ui::Action;
use quark_ui::design::{Ico, Shadow, Sz};
use quark_ui::element::{AnyElement, Binding, IntoAnyElement, NoopAction, div, svg_icon, text};
use quark_ui::icons::lucide;
use quark_ui::style::Styled;
use quark_ui::theme::{Color, Theme};

use crate::hover_card::{Side, place_anchored};
use crate::palette::binding_label;

/// One row of a context menu.
#[derive(Debug, Clone, PartialEq)]
pub enum ContextMenuEntry {
    Item {
        label: String,
        icon: Option<&'static str>,
        action: Action,
        shortcut: Option<String>,
        destructive: bool,
        disabled: bool,
        /// `Some` makes the item checkable, showing a check mark when true.
        checked: Option<bool>,
    },
    /// A row that opens a nested menu.
    Submenu {
        label: String,
        icon: Option<&'static str>,
        disabled: bool,
        entries: Vec<ContextMenuEntry>,
    },
    Separator,
}

impl ContextMenuEntry {
    pub fn item(label: impl Into<String>, action: impl Into<Action>) -> Self {
        Self::Item {
            label: label.into(),
            icon: None,
            action: action.into(),
            shortcut: None,
            destructive: false,
            disabled: false,
            checked: None,
        }
    }

    pub fn submenu(label: impl Into<String>, entries: Vec<ContextMenuEntry>) -> Self {
        Self::Submenu {
            label: label.into(),
            icon: None,
            disabled: false,
            entries,
        }
    }

    pub fn icon(mut self, svg: &'static str) -> Self {
        if let Self::Item { icon, .. } | Self::Submenu { icon, .. } = &mut self {
            *icon = Some(svg);
        }
        self
    }

    pub fn shortcut(mut self, s: impl Into<String>) -> Self {
        if let Self::Item { shortcut, .. } = &mut self {
            *shortcut = Some(s.into());
        }
        self
    }

    /// Show `binding` from the app's keymap as the item's hint.
    pub fn binding(self, binding: &Binding) -> Self {
        self.shortcut(binding_label(binding))
    }

    pub fn checked(mut self, on: bool) -> Self {
        if let Self::Item { checked, .. } = &mut self {
            *checked = Some(on);
        }
        self
    }

    pub fn destructive(mut self) -> Self {
        if let Self::Item { destructive, .. } = &mut self {
            *destructive = true;
        }
        self
    }

    pub fn disabled(mut self) -> Self {
        if let Self::Item { disabled, .. } | Self::Submenu { disabled, .. } = &mut self {
            *disabled = true;
        }
        self
    }

    pub fn disabled_if(self, disabled: bool) -> Self {
        if disabled { self.disabled() } else { self }
    }

    pub fn separator() -> Self {
        Self::Separator
    }

    /// Whether the keyboard and pointer can highlight it.
    fn is_enabled(&self) -> bool {
        match self {
            Self::Item { disabled, .. } | Self::Submenu { disabled, .. } => !disabled,
            Self::Separator => false,
        }
    }
}

/// Row heights, matching what the rows lay out to.
#[derive(Debug, Clone, Copy, Default)]
struct Metrics {
    item_h: f32,
    separator_h: f32,
    pad_y: f32,
    width: f32,
}

impl Metrics {
    fn new(theme: &Theme, width: Option<f32>) -> Self {
        let m = &theme.metrics;
        let scale = m.ui_scale();
        Self {
            item_h: (m.ui_small_font_size * 1.35 + m.spacing_xs * 2.0)
                .ceil()
                .max(24.0 * scale),
            separator_h: (m.spacing_xs * 2.0 + Sz::SEPARATOR_W).ceil(),
            pad_y: m.spacing_xs,
            width: width.unwrap_or(Sz::CONTEXT_MENU_MIN_W * scale),
        }
    }

    fn row_h(&self, entry: &ContextMenuEntry) -> f32 {
        match entry {
            ContextMenuEntry::Separator => self.separator_h,
            _ => self.item_h,
        }
    }

    fn panel_h(&self, entries: &[ContextMenuEntry]) -> f32 {
        self.pad_y * 2.0 + entries.iter().map(|e| self.row_h(e)).sum::<f32>()
    }

    /// Top of row `index` relative to the panel's top.
    fn row_top(&self, entries: &[ContextMenuEntry], index: usize) -> f32 {
        self.pad_y + entries[..index].iter().map(|e| self.row_h(e)).sum::<f32>()
    }
}

/// One open menu panel: the root or a submenu.
#[derive(Debug, Clone, Copy, Default)]
struct Level {
    highlighted: Option<usize>,
    /// Placed by the last render.
    rect: Rect,
}

/// What the app should do after the menu handled input.
#[derive(Debug, Clone, PartialEq)]
pub enum ContextMenuOutcome {
    /// Nothing beyond a redraw.
    Handled,
    /// An item was chosen and the menu closed: run the action.
    Activate(Action),
    /// The menu closed without choosing.
    Closed,
}

/// A single menu panel at `(x, y)` with no highlight. For a stateful menu
/// with submenus and keyboard navigation use [`ContextMenuState`].
pub fn context_menu_layer(
    entries: Vec<ContextMenuEntry>,
    x: f32,
    y: f32,
    theme: &Theme,
) -> AnyElement {
    let metrics = Metrics::new(theme, None);
    let rect = Rect {
        x,
        y,
        width: metrics.width,
        height: metrics.panel_h(&entries),
    };
    menu_panel(&entries, 0, rect, None, &metrics, theme)
}

fn menu_panel(
    entries: &[ContextMenuEntry],
    level: usize,
    rect: Rect,
    highlighted: Option<usize>,
    metrics: &Metrics,
    theme: &Theme,
) -> AnyElement {
    let tc = &theme.colors;
    let m = &theme.metrics;
    let id = if level == 0 {
        "context-menu".to_owned()
    } else {
        format!("context-menu:{level}")
    };
    // A leading column only when some row fills it, so a menu of plain
    // items does not indent every label past an empty slot.
    let leading = entries.iter().any(|entry| match entry {
        ContextMenuEntry::Item { icon, checked, .. } => icon.is_some() || checked.is_some(),
        ContextMenuEntry::Submenu { icon, .. } => icon.is_some(),
        ContextMenuEntry::Separator => false,
    });
    view! {
        <div
            class="absolute flex-col"
            left={rect.x}
            top={rect.y}
            w={rect.width}
            z_index={250 + level as i32}
            py={metrics.pad_y}
            px={metrics.pad_y}
            bg={tc.elevated_surface}
            border={tc.border}
            rounded={m.panel_radius}
            shadow_preset={Shadow::CONTEXT_MENU}
            on:click={NoopAction}
            id={id.clone()}
            test_id="context-menu"
            role="menu"
            accessibility_role={accesskit::Role::Menu}
            accessibility_id={id.clone()}
            focus_scope={id.clone()}
            key_context="context-menu"
        >
            for (index, entry) in entries.iter().enumerate() {
                match entry {
                    ContextMenuEntry::Separator => {
                        <div h={metrics.separator_h} py={m.spacing_xs} px={m.spacing_sm}>
                            <div class="w-full" h={Sz::SEPARATOR_W} bg={tc.border_variant} />
                        </div>
                    }
                    _ => menu_row(
                        entry,
                        &id,
                        highlighted == Some(index),
                        leading,
                        metrics,
                        theme
                    ),
                }
            }
        </div>
    }
}

fn menu_row(
    entry: &ContextMenuEntry,
    menu_id: &str,
    highlighted: bool,
    leading_slot: bool,
    metrics: &Metrics,
    theme: &Theme,
) -> AnyElement {
    let tc = &theme.colors;
    let m = &theme.metrics;
    let scale = m.ui_scale();
    let (label, icon, disabled) = match entry {
        ContextMenuEntry::Item {
            label,
            icon,
            disabled,
            ..
        }
        | ContextMenuEntry::Submenu {
            label,
            icon,
            disabled,
            ..
        } => (label.as_str(), *icon, *disabled),
        ContextMenuEntry::Separator => unreachable!("separators render separately"),
    };
    let destructive = matches!(
        entry,
        ContextMenuEntry::Item {
            destructive: true,
            ..
        }
    );
    let fg = if disabled {
        tc.text_disabled
    } else if destructive {
        tc.status_error
    } else {
        tc.text
    };
    let icon_color = if disabled {
        tc.text_disabled
    } else if destructive {
        tc.status_error
    } else {
        tc.icon
    };
    let accessibility_id = format!("{menu_id}:{label}");
    // The leading slot: a check mark for checkable items, else the icon.
    // An empty slot keeps labels aligned; SvgIcon scales its base size, so
    // the placeholder is the base size times the scale.
    let leading = match entry {
        ContextMenuEntry::Item {
            checked: Some(true),
            ..
        } => Some(lucide::CHECK),
        ContextMenuEntry::Item {
            checked: Some(false),
            ..
        } => None,
        _ => icon,
    };
    let checkable = matches!(
        entry,
        ContextMenuEntry::Item {
            checked: Some(_),
            ..
        }
    );
    view! {
        <div
            class="flex-row items-center shrink-0"
            h={metrics.item_h}
            gap={m.spacing_sm}
            px={m.spacing_sm}
            rounded={m.spacing_xs}
            bg={if highlighted {
                tc.sidebar_row_hover
            } else {
                Color::TRANSPARENT
            }}
            id={accessibility_id.clone()}
            key={label}
            test_id="context-menu-item"
            accessibility_id={accessibility_id}
            aria-label={label}
            aria-disabled={disabled}
            aria-selected={highlighted}
            role="menuitem"
            @when {checkable} { accessibility_role={accesskit::Role::MenuItemCheckBox} }
            @when {!checkable} { accessibility_role={accesskit::Role::MenuItem} }
            @when {let ContextMenuEntry::Item {
                checked: Some(on), ..
            } = entry} { aria-checked={*on} }
            @when {let ContextMenuEntry::Item { action, .. } = entry
                && !disabled} { on:click={action.clone()} hover_bg={tc.sidebar_row_hover} }
            @when {matches!(entry, ContextMenuEntry::Submenu { .. })} {
                aria-expanded={highlighted}
                on:click={NoopAction}
            }
        >
            match leading {
                Some(svg) => <icon svg={svg} size={Ico::SM} color={icon_color} />
                None if leading_slot =>
                    <div class="shrink-0" w={Ico::SM * scale} h={Ico::SM * scale} />
                None => {}
            }
            <div class="flex-1">
                <text class="text-sm" color={fg}>{label}</text>
            </div>
            if let ContextMenuEntry::Item {
                shortcut: Some(key),
                ..
            } = entry {
                <text class="text-xs" color={tc.text_muted}>{key.as_str()}</text>
            }
            if let ContextMenuEntry::Submenu { .. } = entry {
                <icon svg={lucide::CHEVRON_RIGHT} size={Ico::XS} color={tc.text_muted} />
            }
        </div>
    }
}

#[derive(Debug, Clone, Default)]
pub struct ContextMenuState {
    pub entries: Vec<ContextMenuEntry>,
    pub x: f32,
    pub y: f32,
    pub visible: bool,
    /// The root panel's rect from the last render.
    pub bounds: Option<Rect>,
    /// Panel width; `None` uses the theme's minimum menu width.
    pub width: Option<f32>,
    /// The root menu, then each open submenu.
    levels: Vec<Level>,
    metrics: Metrics,
}

impl ContextMenuState {
    /// Open `entries` with the top-left corner at `(x, y)`; rendering moves
    /// the menu to stay inside the window.
    pub fn open(&mut self, entries: Vec<ContextMenuEntry>, x: f32, y: f32) {
        self.entries = entries;
        self.x = x;
        self.y = y;
        self.visible = true;
        self.bounds = None;
        self.levels.clear();
        self.levels.push(Level::default());
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.entries.clear();
        self.bounds = None;
        self.levels.clear();
    }

    /// How many panels are open: 1 for the root, plus open submenus.
    pub fn depth(&self) -> usize {
        self.levels.len()
    }

    /// The entries of panel `level`, following the highlighted submenus.
    fn entries_at(&self, level: usize) -> &[ContextMenuEntry] {
        let mut entries = self.entries.as_slice();
        for open in &self.levels[..level] {
            match open.highlighted.and_then(|i| entries.get(i)) {
                Some(ContextMenuEntry::Submenu {
                    entries: nested, ..
                }) => entries = nested,
                _ => return &[],
            }
        }
        entries
    }

    /// Whether the deepest open panel's highlighted row opens a submenu,
    /// so Right would open it.
    pub fn on_submenu(&self) -> bool {
        let Some(level) = self.levels.last() else {
            return false;
        };
        let entries = self.entries_at(self.deepest());
        matches!(
            level.highlighted.and_then(|i| entries.get(i)),
            Some(ContextMenuEntry::Submenu {
                disabled: false,
                ..
            })
        )
    }

    fn deepest(&self) -> usize {
        self.levels.len().saturating_sub(1)
    }

    /// The next enabled entry after `from` in `step` direction, wrapping.
    fn step(entries: &[ContextMenuEntry], from: Option<usize>, step: isize) -> Option<usize> {
        let len = entries.len() as isize;
        if len == 0 {
            return None;
        }
        let start = match from {
            Some(i) => i as isize,
            None if step > 0 => -1,
            None => len,
        };
        (1..=len)
            .map(|k| (start + step * k).rem_euclid(len) as usize)
            .find(|&i| entries[i].is_enabled())
    }

    fn open_submenu(&mut self) -> bool {
        let level = self.deepest();
        let entries = self.entries_at(level);
        let Some(ContextMenuEntry::Submenu {
            entries: nested, ..
        }) = self.levels[level].highlighted.and_then(|i| entries.get(i))
        else {
            return false;
        };
        if !entries[self.levels[level].highlighted.unwrap_or(0)].is_enabled() {
            return false;
        }
        let first = Self::step(nested, None, 1);
        self.levels.push(Level {
            highlighted: first,
            rect: Rect::default(),
        });
        true
    }

    /// Handle a key press while open: Up and Down move through enabled
    /// items, Right or Enter opens a submenu, Left or Escape closes one,
    /// Enter or Space chooses an item, and Escape on the root closes the
    /// menu. `None` means the key is not the menu's.
    pub fn handle_key(&mut self, pressed: &Binding) -> Option<ContextMenuOutcome> {
        if !self.visible {
            return None;
        }
        let m = pressed.mods;
        if m.cmd || m.ctrl || m.alt || m.shift {
            return None;
        }
        let level = self.deepest();
        let entries = self.entries_at(level);
        let highlighted = self.levels[level].highlighted;
        match pressed.key.as_str() {
            "arrowdown" | "arrowup" => {
                let step = if pressed.key == "arrowdown" { 1 } else { -1 };
                self.levels[level].highlighted = Self::step(entries, highlighted, step);
            }
            "arrowright" => {
                self.open_submenu();
            }
            "arrowleft" => {
                if level > 0 {
                    self.levels.pop();
                }
            }
            "enter" | "space" => match highlighted.and_then(|i| entries.get(i)) {
                Some(ContextMenuEntry::Item {
                    action,
                    disabled: false,
                    ..
                }) => {
                    let action = action.clone();
                    self.close();
                    return Some(ContextMenuOutcome::Activate(action));
                }
                Some(ContextMenuEntry::Submenu { .. }) => {
                    self.open_submenu();
                }
                _ => {}
            },
            "escape" => {
                if level == 0 {
                    self.close();
                    return Some(ContextMenuOutcome::Closed);
                }
                self.levels.pop();
            }
            _ => return None,
        }
        Some(ContextMenuOutcome::Handled)
    }

    /// The pointer moved to `(x, y)`: highlight the row under it, opening a
    /// submenu row's menu and closing menus deeper than the row's. Uses the
    /// layout of the last render. Returns whether anything changed.
    pub fn pointer_moved(&mut self, x: f32, y: f32) -> bool {
        if !self.visible {
            return false;
        }
        let Some(level) = (0..self.levels.len())
            .rev()
            .find(|&l| self.levels[l].rect.contains(x, y))
        else {
            return false;
        };
        let rect = self.levels[level].rect;
        let entries = self.entries_at(level);
        let row = (0..entries.len()).find(|&i| {
            let top = rect.y + self.metrics.row_top(entries, i);
            y >= top && y < top + self.metrics.row_h(&entries[i])
        });
        let row = row.filter(|&i| entries[i].is_enabled());
        let submenu = row.is_some_and(|i| matches!(entries[i], ContextMenuEntry::Submenu { .. }));
        if self.levels[level].highlighted == row
            && self.levels.len() == level + 1 + submenu as usize
        {
            return false;
        }
        let keep_open = self.levels[level].highlighted == row && submenu;
        self.levels[level].highlighted = row;
        if !keep_open {
            self.levels.truncate(level + 1);
            if submenu {
                self.levels.push(Level::default());
            }
        }
        true
    }

    /// The open menu and submenus, kept inside a `viewport`-sized window.
    pub fn render(&mut self, viewport: (f32, f32), theme: &Theme) -> Option<AnyElement> {
        if !self.visible || self.entries.is_empty() || self.levels.is_empty() {
            self.bounds = None;
            return None;
        }
        self.metrics = Metrics::new(theme, self.width);
        let metrics = self.metrics;
        let margin = theme.metrics.spacing_xs;
        let mut anchor = Rect {
            x: self.x,
            y: self.y,
            width: 0.0,
            height: 0.0,
        };
        let mut side = Side::Bottom;
        for level in 0..self.levels.len() {
            let entries = self.entries_at(level);
            let size = (metrics.width, metrics.panel_h(entries));
            let rect = place_anchored(anchor, size, side, 0.0, viewport, margin);
            // A submenu opens beside its row, with its first row level with
            // the row.
            if let Some(row) = self.levels[level].highlighted {
                anchor = Rect {
                    x: rect.x,
                    y: rect.y + metrics.row_top(entries, row) - metrics.pad_y,
                    width: rect.width,
                    height: metrics.item_h,
                };
            }
            side = Side::Right;
            self.levels[level].rect = rect;
        }
        self.bounds = Some(self.levels[0].rect);
        Some(view! {
            <div class="absolute top-0 left-0">
                for (level, open) in self.levels.iter().enumerate() {
                    {menu_panel(
                        self.entries_at(level),
                        level,
                        open.rect,
                        open.highlighted,
                        &metrics,
                        theme,
                    )}
                }
            </div>
        })
    }

    /// Whether `(x, y)` is on any open panel.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.visible && self.levels.iter().any(|level| level.rect.contains(x, y))
    }
}
