//! Context menus. Entries are built from the shared command registry and
//! the model; choosing one emits [`super::Action::Menu`] with a [`Pick`],
//! the same value the palette produces, so menus route like everything
//! else.

use quark::Rect;
use quark_app::quark_ui::element::{Binding, LayoutSnapshot};
use quark_components::ContextMenuEntry;

use crate::contracts::{CommandId, SurfaceCx, ThreadId, command_spec};
use crate::overlays::{Action, Pick};
use crate::shell::sidebar::thread_row_id;

/// What a context menu is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuTarget {
    Thread(ThreadId),
}

/// The keys that open the focused item's menu from the keyboard.
pub fn is_menu_key(pressed: &Binding) -> bool {
    let m = pressed.mods;
    pressed.key == "f10" && m.shift && !(m.cmd || m.ctrl || m.alt)
}

/// The target painted under `(x, y)` in the last frame, and its bounds.
pub fn target_at(
    geometry: &LayoutSnapshot,
    scx: &SurfaceCx,
    (x, y): (f32, f32),
) -> Option<(MenuTarget, Rect)> {
    // Only on a right-click, so a lookup per thread is affordable; the
    // sidebar paints only the rows in view.
    scx.model.threads.iter().find_map(|thread| {
        let row = geometry.by_id(&thread_row_id(thread.id)).ok()?;
        let visible = row.visible?;
        visible
            .contains(x, y)
            .then_some((MenuTarget::Thread(thread.id), row.bounds))
    })
}

/// The bounds `target` was painted at in the last frame, to anchor a
/// keyboard-opened menu under it.
pub fn target_bounds(geometry: &LayoutSnapshot, target: MenuTarget) -> Option<Rect> {
    match target {
        MenuTarget::Thread(id) => geometry.by_id(&thread_row_id(id)).ok()?.visible,
    }
}

fn item(label: &str, pick: Pick) -> ContextMenuEntry {
    ContextMenuEntry::item(label, Action::Menu(pick))
}

fn command(id: CommandId) -> ContextMenuEntry {
    let spec = command_spec(id);
    let entry = item(spec.title, Pick::Command(id));
    match spec.binding.and_then(|b| b.parse::<Binding>().ok()) {
        Some(binding) => entry.binding(&binding),
        None => entry,
    }
}

/// The entries of `target`'s menu.
pub fn entries(target: MenuTarget, scx: &SurfaceCx) -> Vec<ContextMenuEntry> {
    match target {
        MenuTarget::Thread(id) => {
            let running = scx.model.run(id).is_some();
            let selected = scx.model.selected == id;
            vec![
                item("Open thread", Pick::Thread(id)).disabled_if(selected),
                item("Copy title", Pick::CopyTitle(id)),
                item("Stop run", Pick::Stop(id)).disabled_if(!running),
                ContextMenuEntry::separator(),
                command(CommandId::NewThread),
            ]
        }
    }
}
