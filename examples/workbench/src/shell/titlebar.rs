//! The 44-point top bar: sidebar toggle, project breadcrumb and thread
//! title, the "Demo workspace" label, and the palette, settings, and dock
//! controls. On macOS the window uses custom chrome and the bar reserves
//! the traffic lights' leading zone; elsewhere it sits under the system
//! title bar.

use quark::view;
use std::rc::Rc;

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

/// What the bar shows, shared with its cached build closure.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct TitleData {
    project: String,
    title: String,
    sidebar_docked: bool,
    workspace: &'static str,
}

pub fn view(state: &mut super::State, scx: &SurfaceCx, _vcx: &mut ViewContext) -> AnyElement {
    let (width, height) = scx.size;
    let thread = scx.model.selected_thread();
    let project = thread
        .and_then(|t| scx.model.project(t.project))
        .map_or("", |p| p.name.as_str());
    let title = thread.map_or("No thread", |t| t.title.as_str());
    let docked = scx.width_policy.sidebar_docked;
    let data = match &state.title {
        Some(d)
            if d.project == project
                && d.title == title
                && d.sidebar_docked == docked
                && d.workspace == scx.model.workspace_label =>
        {
            d.clone()
        }
        _ => {
            let d = Rc::new(TitleData {
                project: project.to_owned(),
                title: title.to_owned(),
                sidebar_docked: docked,
                workspace: scx.model.workspace_label,
            });
            state.title = Some(d.clone());
            d
        }
    };
    let colors = &scx.theme.colors;
    let (bg, border, muted, strong) = (
        colors.title_bar_background,
        colors.border,
        colors.text_muted,
        colors.text_strong,
    );
    let hash = inputs_hash(&(&*data, width.to_bits(), height.to_bits()));
    cached("titlebar", hash, move || {
        let sidebar_icon = if data.sidebar_docked {
            lucide::PANEL_LEFT_CLOSE
        } else {
            lucide::PANEL_LEFT_OPEN
        };
        view! {
            <div w={width} h={height} class="flex-row items-center gap-[8] pr-2"
                 pl={leading_zone()} bg={bg} border_b={border}
                 accessibility_role={accesskit::Role::Toolbar} aria-label="Workbench toolbar">
                {icon_button(sidebar_icon, "Toggle sidebar", CommandId::ToggleSidebar)}
                <div class="flex-row items-center gap-[6] flex-1" min-w={0.0}>
                    <text size={13.0} color={muted}>{data.project.clone()}</text>
                    <icon svg={lucide::CHEVRON_RIGHT} size={12.0} color={muted} />
                    <div class="flex-1" min-w={0.0} test_id="titlebar.thread">
                        <text size={13.0} class="truncate font-semibold" color={strong}>
                            {data.title.clone()}
                        </text>
                    </div>
                </div>
                <div class="flex-row items-center px-2 h-[22] rounded-[4] shrink-0"
                     border={border} role="status" aria-label="Demo workspace: simulated backend">
                    <text size={11.0} color={muted}>{data.workspace}</text>
                </div>
                {icon_button(lucide::COMMAND, "Command palette", CommandId::OpenPalette)}
                {icon_button(lucide::SETTINGS, "Settings", CommandId::OpenSettings)}
                {icon_button(lucide::SPLIT, "Toggle right dock", CommandId::ToggleRightDock)}
            </div>
        }
    })
    .w(width)
    .h(height)
    .flex_shrink_0()
    .into_any()
}
