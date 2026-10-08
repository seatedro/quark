//! The design system: the theme file's colors meet their contrast targets
//! in both modes, the compact recipe sizes controls once at any zoom, and
//! a broken theme edit keeps the colors in force.

use quark::Rect;
use quark::reactive::SignalStore;
use quark::scene::Scene;
use quark_components::Button;
use quark_ui::Action;
use quark_ui::element::{ElementContext, IntoAnyElement, div, render_element};
use quark_ui::style::Styled;
use quark_ui::theme::{Color, Theme, ThemeFamily, ThemeMode, contrast_ratio};
use quark_workbench::contracts::ThemeChoice;
use quark_workbench::design::reload::ThemeReloader;
use quark_workbench::design::theme::{THEME_JSON, theme, themes};
use quark_workbench::design::{Appearance, recipes};

fn family() -> ThemeFamily {
    ThemeFamily::from_json(THEME_JSON).expect("the built-in theme file parses")
}

fn workbench(mode: ThemeMode, zoom: f32) -> Theme {
    let appearance = Appearance {
        zoom,
        ..Appearance::new(ThemeChoice::System)
    };
    theme(&family(), mode, &appearance)
}

// Catches a palette or mapping edit that makes text, status, code, or a
// focus ring hard to see against a surface it is painted on.
#[test]
fn design_theme_pairs_meet_their_contrast_targets() {
    const SURFACES: &[&str] = &[
        "canvas",
        "sidebar_background",
        "surface",
        "elevated_surface",
        "element_hover",
        "element_selected",
    ];
    const PANELS: &[&str] = &[
        "canvas",
        "sidebar_background",
        "surface",
        "elevated_surface",
    ];
    const CODE: &[&str] = &[
        "text",
        "syntax_keyword",
        "syntax_string",
        "syntax_comment",
        "syntax_function",
        "syntax_type",
        "syntax_number",
        "syntax_property",
        "syntax_operator",
    ];
    let groups: &[(&[&str], &[&str], f32)] = &[
        (
            &[
                "text_strong",
                "text",
                "text_muted",
                "placeholder",
                "text_accent",
            ],
            SURFACES,
            4.5,
        ),
        (
            &[
                "status_info",
                "status_warning",
                "status_error",
                "line_add_text",
            ],
            PANELS,
            4.5,
        ),
        (&["on_accent"], &["accent", "accent_strong"], 4.5),
        (CODE, &["editor_surface", "line_add", "line_del"], 4.5),
        (&["text"], &["line_add_word_bg", "line_del_word_bg"], 4.5),
        (&["focus_border", "control_border"], SURFACES, 3.0),
    ];
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let theme = workbench(mode, 1.0);
        let c = |name: &str| theme.colors.get(name).expect(name);
        let mut failures = Vec::new();
        for (fgs, bgs, min) in groups {
            for fg in *fgs {
                for bg in *bgs {
                    let ratio = contrast_ratio(c(fg), c(bg));
                    if ratio < *min {
                        failures.push(format!("{fg} on {bg}: {ratio:.2} < {min}"));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{mode:?}: {failures:#?}");
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Pick;

impl From<Pick> for Action {
    fn from(value: Pick) -> Self {
        Action::new(value)
    }
}

/// Bounds of the buttons in a row of a labeled button and an icon button,
/// painted on a 2x device.
fn button_bounds(theme: &Theme) -> Vec<Rect> {
    let mut text = quark_text::TextSystem::vendored_only(&Default::default());
    let mut layouts = quark_text::LayoutCache::default();
    let store = SignalStore::new();
    let mut cx = ElementContext::new(theme, 2.0, &mut text, &mut layouts, None, &store);
    let mut root = div()
        .flex_row()
        .items_start()
        .child(Button::new(Pick).label("Run tests"))
        .child(recipes::icon_button(
            quark_ui::icons::lucide::COPY,
            "Copy",
            Pick,
        ))
        .into_any();
    let mut scene = Scene::default();
    render_element(&mut root, &mut scene, &mut cx, 800.0, 200.0);
    cx.semantic
        .nodes()
        .iter()
        .filter(|n| n.test_id.as_ref().is_some_and(|t| t.as_str() == "button"))
        .map(|n| n.bounds)
        .collect()
}

// Catches the compact recipe missing from the workbench theme (buttons
// fall back to quark's padded default), or sizes scaled twice at zoom.
#[test]
fn design_recipe_sizes_controls_once_at_every_zoom() {
    for zoom in [1.0, 1.25, 2.0] {
        let side = (28.0_f32 * zoom).round();
        let sizes: Vec<(f32, f32)> = button_bounds(&workbench(ThemeMode::Light, zoom))
            .iter()
            .map(|r| (r.height, if r.width == r.height { r.width } else { 0.0 }))
            .collect();
        assert_eq!(sizes, [(side, 0.0), (side, side)], "zoom {zoom}");
    }
}

// Catches a reload that applies a broken theme edit (or blanks the
// colors), or misses the edit that fixes it.
#[test]
fn design_invalid_theme_edit_keeps_the_current_colors() {
    let dir = std::env::temp_dir().join(format!("workbench-themes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("workbench.json");
    let light_accent = r##""accent": "#6257d9""##;
    assert!(THEME_JSON.contains(light_accent));
    let with_accent =
        |hex: &str| THEME_JSON.replacen(light_accent, &format!(r#""accent": "{hex}""#), 1);
    let mut reloader = ThemeReloader::new(&dir);
    let accent = |family: &ThemeFamily| {
        themes(family, &Appearance::new(ThemeChoice::Light))
            .0
            .colors
            .accent
    };

    std::fs::write(&file, with_accent("#008800")).expect("write");
    let edited = reloader.poll().expect("loaded");
    let green = Color::rgba(0, 0x88, 0, 255);
    assert_eq!(edited.family.as_ref().map(accent), Some(green));

    // Half-typed: the file stops being valid JSON.
    let broken = with_accent("#00880");
    std::fs::write(&file, &broken[..broken.len() / 2]).expect("write");
    let rejected = reloader.poll().expect("reported");
    assert_eq!(rejected.errors.len(), 1);
    assert_eq!(rejected.family.as_ref().map(accent), Some(green));

    std::fs::write(&file, with_accent("#0000cc")).expect("write");
    let fixed = reloader.poll().expect("reloaded");
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(
        fixed.family.as_ref().map(accent),
        Some(Color::rgba(0, 0, 0xcc, 255))
    );
}
