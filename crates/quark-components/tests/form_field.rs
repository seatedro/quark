//! A form field ties its label and its error or help to the control for
//! assistive tech.

use quark::reactive::SignalStore;
use quark_components::FormField;
use quark_ui::element::{ElementContext, IntoAnyElement, div, render_element};
use quark_ui::style::Styled;
use quark_ui::theme::Theme;

/// The field group's (label, description, invalid, required), and the
/// error region's label if one is painted.
fn semantics(field: FormField) -> ((String, String, bool, bool), Option<String>) {
    let theme = Theme::default_light();
    let mut text = quark_text::TextSystem::vendored_only(&Default::default());
    let mut layouts = quark_text::LayoutCache::default();
    let store = SignalStore::new();
    let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &store);
    let mut root = field.into_any();
    let mut scene = quark_render::Scene::default();
    render_element(&mut root, &mut scene, &mut cx, 400.0, 200.0);
    let nodes = cx.semantic.nodes();
    let find = |id: &str| {
        nodes
            .iter()
            .find(|n| n.test_id.as_ref().is_some_and(|t| t.as_str() == id))
    };
    let group = find("form-field").expect("field group");
    let text = |s: &Option<std::sync::Arc<str>>| s.as_deref().unwrap_or_default().to_owned();
    (
        (
            text(&group.label),
            text(&group.description),
            group.state.invalid,
            group.state.required,
        ),
        find("form-field-error").map(|n| text(&n.label)),
    )
}

#[test]
fn field_group_is_named_by_its_label_and_described_by_error_or_help() {
    let control = || div().w(200.0).h(28.0);
    let cases = [
        (
            FormField::new("key", "API key", control())
                .help("Stored in the keychain")
                .required(true),
            (
                (
                    "API key".into(),
                    "Stored in the keychain".into(),
                    false,
                    true,
                ),
                None,
            ),
        ),
        (
            FormField::new("key", "API key", control())
                .help("Stored in the keychain")
                .error(Some("Keys start with sk-")),
            (
                ("API key".into(), "Keys start with sk-".into(), true, false),
                Some("Keys start with sk-".into()),
            ),
        ),
    ];
    for (field, expected) in cases {
        assert_eq!(semantics(field), expected);
    }
}
