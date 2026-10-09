//! A form field: a label over any control, with help text or an error
//! under it.
//!
//! The field is an accessibility group named by its label and described by
//! its error (or, without one, its help), and marked invalid and required
//! as told, so assistive tech reads them when focus enters the control.
//! The error is also a polite live region: it is spoken when it appears.
//! Give the control the same label as its own name.

use quark::view;
use quark_ui::accessibility::Politeness;
use quark_ui::element::{
    AnyElement, ElementContext, IntoAnyElement, RenderOnce, div, svg_icon, text,
};
use quark_ui::icons::lucide;
use quark_ui::style::Styled;
use quark_ui::theme::scaled_or;

pub struct FormField {
    id: String,
    label: String,
    control: AnyElement,
    help: Option<String>,
    error: Option<String>,
    required: bool,
}

impl FormField {
    /// `id` must be unique in the window; it names the field's
    /// accessibility ids.
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        control: impl IntoAnyElement,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            control: control.into_any(),
            help: None,
            error: None,
            required: false,
        }
    }

    /// Guidance shown under the control while there is no error.
    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// What is wrong with the value; `None` clears it.
    pub fn error(mut self, error: Option<impl Into<String>>) -> Self {
        self.error = error.map(Into::into);
        self
    }

    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }
}

impl RenderOnce for FormField {
    fn render(self, cx: &ElementContext) -> AnyElement {
        let theme = cx.theme;
        let tc = &theme.colors;
        let m = &theme.metrics;
        let scale = m.ui_scale();
        let recipe = theme.components.field;
        let gap = scaled_or(recipe.gap, scale, m.spacing_xs);
        let label_size = recipe.font_size.map_or(m.ui_small_font_size, |s| s * scale);
        let message_size = label_size - scale;
        let description = self.error.clone().or_else(|| self.help.clone());
        let invalid = self.error.is_some();
        let id = self.id;
        view! {
            <div
                class="flex-col w-full"
                gap={gap}
                test_id="form-field"
                accessibility_id={format!("field:{id}")}
                accessibility_role={accesskit::Role::Group}
                aria-label={self.label.clone()}
                aria-description={description.unwrap_or_default()}
                aria-invalid={invalid}
                aria-required={self.required}
            >
                <div class="flex-row items-center" gap={(2.0 * scale).round()}>
                    <text class="font-medium" size={label_size} color={tc.text}>{self.label}</text>
                    if self.required {
                        <text size={label_size} color={tc.text_muted}>"*"</text>
                    }
                </div>
                {self.control}
                if let Some(error) = self.error {
                    <div
                        class="flex-row items-center"
                        gap={(4.0 * scale).round()}
                        test_id="form-field-error"
                        accessibility_id={format!("field:{id}:error")}
                        accessibility_role={accesskit::Role::Status}
                        aria-label={error.clone()}
                        live={Politeness::Polite}
                    >
                        {svg_icon(lucide::ALERT_CIRCLE, label_size).color(tc.status_error)}
                        <text size={message_size} color={tc.status_error}>{error}</text>
                    </div>
                } else if let Some(help) = self.help {
                    <text size={message_size} color={tc.text_muted}>{help}</text>
                }
            </div>
        }
    }
}
