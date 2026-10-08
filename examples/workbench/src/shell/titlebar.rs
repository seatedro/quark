//! The 44-point top bar: sidebar toggle, project breadcrumb and thread
//! title, the "Demo workspace" label, and the palette, settings, and dock
//! controls. On macOS the window uses custom chrome and the bar reserves
//! the traffic lights' leading zone; elsewhere it sits under the system
//! title bar.

use quark::view;
use quark_app::ViewContext;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::icons::lucide;
use quark_app::quark_ui::style::Styled;
use quark_components::{Button, ButtonSize};

use crate::contracts::{CommandId, SurfaceCx};
use crate::design::tokens;

/// Leading space kept clear of controls: the traffic lights on macOS.
pub fn leading_zone() -> f32 {
    if cfg!(target_os = "macos") {
        tokens::TRAFFIC_LIGHT_ZONE
    } else {
        tokens::SPACE_8
    }
}

fn icon_button(icon: &'static str, label: &'static str, command: CommandId) -> AnyElement {
    view! {
        <Button on:click={command} icon={icon} tooltip={label}
                size={ButtonSize::Compact} fixed_size={tokens::ICON_BUTTON} />
    }
    .into_any()
}

pub fn view(_state: &mut super::State, scx: &SurfaceCx, _vcx: &mut ViewContext) -> AnyElement {
    let colors = &scx.theme.colors;
    let (width, height) = scx.size;
    let thread = scx.model.selected_thread();
    let project = thread
        .and_then(|t| scx.model.project(t.project))
        .map_or("", |p| p.name.as_str());
    let title = thread.map_or("No thread", |t| t.title.as_str());
    let sidebar_icon = if scx.width_policy.sidebar_docked {
        lucide::PANEL_LEFT_CLOSE
    } else {
        lucide::PANEL_LEFT_OPEN
    };
    view! {
        <div w={width} h={height} class="flex-row items-center gap-[8] pr-2 shrink-0"
             pl={leading_zone()} bg={colors.title_bar_background} border_b={colors.border}
             accessibility_role={accesskit::Role::Toolbar} aria-label="Workbench toolbar">
            {icon_button(sidebar_icon, "Toggle sidebar", CommandId::ToggleSidebar)}
            <div class="flex-row items-center gap-[6] flex-1" min-w={0.0}>
                <text size={13.0} color={colors.text_muted}>{project.to_owned()}</text>
                <icon svg={lucide::CHEVRON_RIGHT} size={12.0} color={colors.text_muted} />
                <div class="flex-1" min-w={0.0} test_id="titlebar.thread">
                    <text size={13.0} class="truncate font-semibold" color={colors.text_strong}>
                        {title.to_owned()}
                    </text>
                </div>
            </div>
            <div class="flex-row items-center px-2 h-[22] rounded-[4] shrink-0"
                 border={colors.border} role="status" aria-label="Demo workspace: simulated backend">
                <text size={11.0} color={colors.text_muted}>{scx.model.workspace_label}</text>
            </div>
            {icon_button(lucide::COMMAND, "Command palette", CommandId::OpenPalette)}
            {icon_button(lucide::SETTINGS, "Settings", CommandId::OpenSettings)}
            {icon_button(lucide::SPLIT, "Toggle right dock", CommandId::ToggleRightDock)}
        </div>
    }
    .into_any()
}
