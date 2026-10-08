//! The settings form: values, validation, and the field rows.

use std::rc::Rc;

use quark::view;
use quark_app::quark_ui::FocusId;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::TextField;
use quark_app::quark_ui::theme::Theme;
use quark_components::{FormField, SelectMsg, SelectOption, SelectState, select};

use crate::contracts::ThemeChoice;
use crate::design::tokens;

/// The tool output limit's text field.
pub const LIMIT: FocusId = FocusId::from_key("settings.output-limit");
pub const LIMIT_RANGE: (u32, u32) = (100, 10_000);
const LIMIT_ERROR: &str = "Enter a whole number from 100 to 10,000.";

pub const THEMES: [(ThemeChoice, &str); 3] = [
    (ThemeChoice::System, "Match system"),
    (ThemeChoice::Light, "Light"),
    (ThemeChoice::Dark, "Dark"),
];

pub const MODELS: [&str; 3] = ["Atlas Balanced", "Atlas Fast", "Atlas Thorough"];

/// What Save keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Values {
    pub theme: ThemeChoice,
    /// Index into [`MODELS`].
    pub model: usize,
    /// Tool output lines shown before a card collapses the rest.
    pub output_limit: u32,
}

impl Default for Values {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::System,
            model: 0,
            output_limit: 400,
        }
    }
}

pub fn theme_index(choice: ThemeChoice) -> usize {
    THEMES.iter().position(|(c, _)| *c == choice).unwrap_or(0)
}

/// The tool output limit typed as `text`, or why it is not one.
pub fn parse_limit(text: &str) -> Result<u32, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Enter a line limit.".to_owned());
    }
    let (lo, hi) = LIMIT_RANGE;
    match text.replace(',', "").parse::<u32>() {
        Ok(n) if (lo..=hi).contains(&n) => Ok(n),
        _ => Err(LIMIT_ERROR.to_owned()),
    }
}

pub fn options(labels: impl IntoIterator<Item = &'static str>) -> Rc<[SelectOption]> {
    labels.into_iter().map(SelectOption::new).collect()
}

/// A section heading over its fields.
pub fn section(title: &str, fields: Vec<AnyElement>, theme: &Theme) -> AnyElement {
    let (size, _) = tokens::TYPE_HEADING;
    view! {
        <div class="flex-col w-full" gap={tokens::SPACE_12}
             accessibility_role={accesskit::Role::Group} aria-label={title.to_owned()}>
            <text size={size} semibold color={theme.colors.text_strong}>{title.to_owned()}</text>
            {...fields}
        </div>
    }
    .into_any()
}

/// A labelled select whose list opens over the dialog (a nested picker).
pub fn select_field(
    id: &str,
    label: &str,
    state: &SelectState,
    options: Rc<[SelectOption]>,
    viewport: (f32, f32),
    on_msg: impl Fn(SelectMsg) -> quark_app::quark_ui::Action + 'static,
) -> AnyElement {
    let control = select(state, options, on_msg)
        .label(label)
        .width(240.0)
        .viewport(viewport);
    FormField::new(id, label, control).into_any()
}

pub fn limit_field(
    field: &TextField,
    focused: bool,
    error: Option<&str>,
    on_click: quark_app::quark_ui::Action,
) -> AnyElement {
    let label = "Tool output limit";
    let input = text_input(label, "")
        .field(field)
        .focus_target(LIMIT)
        .focused(focused)
        .on_click(on_click)
        .w(240.0)
        .h(tokens::PRIMARY_CONTROL);
    FormField::new("settings.output-limit", label, input)
        .help("Lines of tool output shown before a card collapses the rest.")
        .error(error)
        .required(true)
        .into_any()
}
