//! A drawn menu bar from the app's [`Menu`]s, for platforms without a
//! native one ([`super::menu::native_menu_bar`] is false) or apps that
//! want it everywhere. Needs the `components` feature.
//!
//! [`DrawnMenuBar`] wraps a [`quark_components::MenuBar`] built from the
//! same `Vec<Menu>` the app hands [`crate::EventContext::set_menus`], plus
//! the input it needs: pass every [`InputEvent`] to
//! [`DrawnMenuBar::event`] from `UiApp::event`, render it with
//! [`MenuBar::render`] at the top of the window, and run what it returns:
//! an [`ContextMenuOutcome::Activate`] carries the app action `wrap` made
//! from the chosen item's [`MenuPick`]; a pointer pick arrives the same
//! way, through `UiApp::update` (close the bar there). [`MenuPick::perform`]
//! turns a pick into the app's item id or carries out a standard role.

use std::rc::Rc;

use quark_components::{ContextMenuEntry, ContextMenuOutcome, MenuBar, MenuBarMenu};
use quark_ui::Action;
use quark_ui::element::Binding;
use winit::event::ElementState;
use winit::keyboard::{ModifiersState, NamedKey};

use super::menu::{Menu, MenuItem, MenuRole};
use crate::input::InputEvent;
use crate::runner::EventContext;

/// The action a drawn menu item emits.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuPick {
    /// A [`super::menu::MenuAction`], by id.
    Item(String),
    Role(MenuRole),
}

impl MenuPick {
    /// Carry the pick out as a native menu would: roles through
    /// [`EventContext::perform_role`]; for an app item, its id, which a
    /// native menu would have sent as [`crate::AppEvent::Menu`].
    pub fn perform(self, cx: &mut EventContext) -> Option<String> {
        match self {
            Self::Item(id) => Some(id),
            Self::Role(role) => {
                cx.perform_role(role);
                None
            }
        }
    }
}

/// The bar's menus for `menus`, each item emitting `wrap` of its pick.
/// Roles only macOS has are left out, as on the native bars elsewhere.
pub fn menu_bar_menus(menus: &[Menu], wrap: &dyn Fn(MenuPick) -> Action) -> Vec<MenuBarMenu> {
    menus
        .iter()
        .map(|menu| MenuBarMenu {
            label: menu.label.clone(),
            enabled: menu.enabled,
            entries: entries(&menu.items, wrap),
        })
        .collect()
}

fn entries(items: &[MenuItem], wrap: &dyn Fn(MenuPick) -> Action) -> Vec<ContextMenuEntry> {
    let mut out: Vec<ContextMenuEntry> = Vec::with_capacity(items.len());
    for item in items {
        let entry = match item {
            MenuItem::Action(action) => {
                let mut entry = ContextMenuEntry::item(
                    action.label.as_str(),
                    wrap(MenuPick::Item(action.id.clone())),
                )
                .disabled_if(!action.enabled);
                if let Some(label) = action.accelerator_label() {
                    entry = entry.shortcut(label);
                }
                match action.checked {
                    Some(on) => entry.checked(on),
                    None => entry,
                }
            }
            MenuItem::Submenu(menu) => {
                ContextMenuEntry::submenu(menu.label.as_str(), entries(&menu.items, wrap))
                    .disabled_if(!menu.enabled)
            }
            MenuItem::Role(role) if role.macos_only() && !cfg!(target_os = "macos") => continue,
            MenuItem::Role(role) => {
                let entry = ContextMenuEntry::item(role.label(), wrap(MenuPick::Role(*role)));
                match role.shortcut() {
                    Some(keys) => entry.shortcut(crate::keymap::format_binding(keys)),
                    None => entry,
                }
            }
            MenuItem::Separator => {
                // No leading or doubled separators once roles drop out.
                if out
                    .last()
                    .is_none_or(|last| *last == ContextMenuEntry::Separator)
                {
                    continue;
                }
                ContextMenuEntry::Separator
            }
        };
        out.push(entry);
    }
    if out.last() == Some(&ContextMenuEntry::Separator) {
        out.pop();
    }
    out
}

/// A lone Alt press and release, with nothing pressed in between: the
/// gesture that moves keyboard focus to a menu bar.
#[derive(Debug, Clone, Copy, Default)]
pub struct AltTap {
    armed: bool,
}

impl AltTap {
    /// Feed every input event; true on the release that completes a tap.
    pub fn event(&mut self, event: &InputEvent) -> bool {
        match event {
            InputEvent::KeyPress(chord) => {
                let others = chord.modifiers - ModifiersState::ALT;
                self.armed = chord.named() == Some(NamedKey::Alt) && others.is_empty();
                false
            }
            InputEvent::KeyRelease(chord) if chord.named() == Some(NamedKey::Alt) => {
                std::mem::take(&mut self.armed)
            }
            InputEvent::PointerButton {
                state: ElementState::Pressed,
                ..
            }
            | InputEvent::Wheel { .. }
            | InputEvent::TextInput(_) => {
                self.armed = false;
                false
            }
            _ => false,
        }
    }
}

/// A [`MenuBar`] over the app's [`Menu`]s with its keyboard and pointer
/// input. See the [module docs](self).
#[derive(Clone)]
pub struct DrawnMenuBar {
    pub bar: MenuBar,
    alt: AltTap,
    /// Where the pointer last moved, for presses, which carry no position.
    pointer: (f32, f32),
    wrap: Rc<dyn Fn(MenuPick) -> Action>,
}

impl DrawnMenuBar {
    /// A bar over `menus`; `wrap` makes the app's action for a pick.
    pub fn new<A: Into<Action>>(menus: &[Menu], wrap: impl Fn(MenuPick) -> A + 'static) -> Self {
        let wrap: Rc<dyn Fn(MenuPick) -> Action> = Rc::new(move |pick| wrap(pick).into());
        Self {
            bar: MenuBar::new(menu_bar_menus(menus, &*wrap)),
            alt: AltTap::default(),
            pointer: (f32::NAN, f32::NAN),
            wrap,
        }
    }

    /// Show `menus` instead, as after [`EventContext::set_menus`].
    pub fn set_menus(&mut self, menus: &[Menu]) {
        self.bar.set_menus(menu_bar_menus(menus, &*self.wrap));
    }

    /// Route `event` to the bar: Alt taps, F10, and navigation keys while
    /// the bar is active. `Some` means the bar took the event (return true
    /// from `UiApp::event` and redraw); `Activate` carries the app action
    /// for the pick. Pointer moves update an open menu's highlight but are
    /// left to the adapter, which tracks the pointer and redraws for the
    /// hover change. A press outside the open menu closes it and is taken.
    pub fn event(&mut self, event: &InputEvent) -> Option<ContextMenuOutcome> {
        if self.alt.event(event) {
            self.bar.activate();
            return Some(ContextMenuOutcome::Handled);
        }
        match event {
            InputEvent::KeyPress(chord) => {
                let pressed: Binding = chord.binding_string()?.parse().ok()?;
                self.bar.handle_key(&pressed)
            }
            InputEvent::PointerMoved { x, y } => {
                self.pointer = (*x, *y);
                self.bar.pointer_moved(*x, *y);
                None
            }
            InputEvent::PointerButton {
                state: ElementState::Pressed,
                ..
            } => {
                let (x, y) = self.pointer;
                self.bar
                    .pointer_pressed(x, y)
                    .then_some(ContextMenuOutcome::Closed)
            }
            _ => None,
        }
    }
}
