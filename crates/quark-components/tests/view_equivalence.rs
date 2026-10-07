//! `view!` is a front end over the builders: each case here writes one
//! construct both ways and checks that the two paint the same scene and
//! publish the same accessibility tree. A difference means the macro
//! lowered the construct to something other than the builder calls it
//! documents.

use quark::StyleOverride;
use quark::reactive::SignalStore;
use quark::{SemanticRole, view};
use quark_components::{
    Avatar, Badge, BadgeVariant, Button, ButtonStyle, Checkbox, ProgressBar, avatar, badge,
    checkbox, progress_bar,
};
use quark_render::Scene;
use quark_ui::Action;
use quark_ui::accessibility::{AccessibilityFrame, dump_accessibility_tree};
use quark_ui::element::*;
use quark_ui::icons::lucide;
use quark_ui::style::Styled;
use quark_ui::theme::{Color, Theme};

#[derive(Debug, Clone, PartialEq)]
struct Pick(&'static str);

impl From<Pick> for Action {
    fn from(value: Pick) -> Self {
        Action::new(value)
    }
}

struct Painted {
    scene: String,
    accessibility: String,
}

/// The scene's primitives as text. Shaped text compares by pointer, so it
/// prints as its size alone; the accessibility tree carries the strings.
fn scene_text(scene: &Scene) -> String {
    let mut out = String::new();
    for primitive in &scene.primitives {
        let line = format!("{primitive:?}");
        let mut rest = line.as_str();
        while let Some(at) = rest.find("ShapedText(") {
            out.push_str(&rest[..at]);
            out.push_str("ShapedText");
            let mut depth = 0;
            let mut end = at;
            for (i, c) in rest[at..].char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = at + i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            rest = &rest[end..];
        }
        out.push_str(rest);
        out.push('\n');
    }
    out
}

/// Paint `root` at 400x300 with a hover-free pointer.
fn paint(root: AnyElement) -> Painted {
    let mut text = quark_text::TextSystem::vendored_only(&Default::default());
    let mut layouts = quark_text::LayoutCache::default();
    let store = SignalStore::new();
    let theme = Theme::default_dark();
    let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &store);
    cx.accessibility = AccessibilityFrame::new(400.0, 300.0);
    let mut root = root;
    let mut scene = Scene::default();
    render_element(&mut root, &mut scene, &mut cx, 400.0, 300.0);
    Painted {
        scene: scene_text(&scene),
        accessibility: dump_accessibility_tree(&cx.accessibility),
    }
}

#[track_caller]
fn assert_same(case: &str, from_macro: AnyElement, from_builders: AnyElement) {
    let (a, b) = (paint(from_macro), paint(from_builders));
    assert!(!b.scene.is_empty(), "{case}: nothing painted");
    assert_eq!(a.accessibility, b.accessibility, "{case}: accessibility");
    assert_eq!(a.scene, b.scene, "{case}: scene");
}

const ACCENT: Color = Color::rgba(51, 102, 153, 255);

#[test]
fn attributes_match_builder_calls() {
    let from_macro = view! {
        <div flex-row gap={8.0} p={4.0} min-w={0.0}
             bg={ACCENT}
             shadow={(4.0, 2.0, Color::rgba(0, 0, 0, 80))}
             id="save" test-id="save-button"
             role="button" aria-label="Save" aria-selected={true}
             on:click={Pick("save")}
             on:key:mod+s={Pick("save")}>
            <text color={Color::rgba(255, 255, 255, 255)}>"Save"</text>
        </div>
    };
    let from_builders = div()
        .flex_row()
        .gap(8.0)
        .p(4.0)
        .min_w(0.0)
        .bg(ACCENT)
        .shadow(4.0, 2.0, Color::rgba(0, 0, 0, 80))
        .id("save")
        .test_id("save-button")
        .semantic_role(SemanticRole::Button)
        .accessibility_label("Save")
        .accessibility_selected(true)
        .on_click(Pick("save"))
        .on_key("mod+s", Pick("save"))
        .child(text("Save").color(Color::rgba(255, 255, 255, 255)))
        .into_any();
    assert_same("attributes", from_macro, from_builders);
}

#[test]
fn classes_match_builder_calls() {
    let from_macro = view! {
        <div class="flex-col items-center p-4 gap-x-2 w-[320px] h-40 rounded-lg
                    bg-[#336699] border-[ACCENT] opacity-50 hover:bg-[#ffffff]">
            <text class="text-sm font-semibold truncate text-[Color::rgba(1, 2, 3, 255)]">"x"</text>
            <icon svg={lucide::X} size={12.0} class="fill-[ACCENT]" />
        </div>
    };
    let from_builders = div()
        .flex_col()
        .items_center()
        .p(16.0)
        .gap_x(8.0)
        .w(320.0)
        .h(160.0)
        .rounded_lg()
        .bg(Color::rgba(0x33, 0x66, 0x99, 255))
        .border(ACCENT)
        .opacity(0.5)
        .hover(|s: StyleOverride| s.bg(Color::rgba(255, 255, 255, 255)))
        .child(
            text("x")
                .text_sm()
                .semibold()
                .truncate()
                .color(Color::rgba(1, 2, 3, 255)),
        )
        .child(svg_icon(lucide::X, 12.0).color(ACCENT))
        .into_any();
    assert_same("classes", from_macro, from_builders);
}

#[test]
fn text_interpolation_matches_format() {
    let name = "Ada";
    let count = 3;
    let from_macro = view! {
        <div class="flex-col">
            <text>"Hello, {name}!"</text>
            <text>{count} " unread, " {count * 2} " total"</text>
            "{count} left"
        </div>
    };
    let from_builders = div()
        .flex_col()
        .child(text(format!("Hello, {name}!")))
        .child(text(format!("{count} unread, {} total", count * 2)))
        .child(format!("{count} left"))
        .into_any();
    assert_same("text", from_macro, from_builders);
}

#[test]
fn rich_text_matches_styled_spans() {
    let url = "https://quark.dev";
    let from_macro = view! {
        <p size={14.0} width={300.0}>"Run " <code>"cargo test"</code> ", then " <b>"read " <i>"the docs"</i></b>
           <br/> <a href={url}>"online"</a></p>
    };
    let from_builders = selectable_rich_text(vec![
        StyledSpan::plain("Run "),
        StyledSpan::plain("cargo test").code(),
        StyledSpan::plain(", then "),
        StyledSpan::plain("read ").bold(),
        StyledSpan::plain("the docs").bold().italic(),
        StyledSpan::plain("\n"),
        StyledSpan::plain("online").link(url),
    ])
    .size(14.0)
    .width(300.0)
    .into_any();
    assert_same("rich text", from_macro, from_builders);
}

#[test]
fn control_flow_matches_builder_children() {
    #[derive(Clone, Copy)]
    enum Status {
        Idle,
        Busy(u8),
    }
    let items = ["a", "b"];
    let build = |show: bool, status: Status, label: Option<&str>| {
        let from_macro = view! {
            <div class="flex-col">
                if show { <text>"shown"</text> } else { <spacer /> }
                if let Some(label) = label { <text>{label}</text> }
                match status {
                    Status::Idle => <text>"idle"</text>
                    Status::Busy(n) => {
                        <text>"busy"</text>
                        <text>"{n}%"</text>
                    }
                }
                for item in items key={item} {
                    <div p={2.0}>{item}</div>
                }
                <>
                    <text>"f1"</text>
                    <text>"f2"</text>
                </>
            </div>
        };
        let mut b = div().flex_col();
        b = if show {
            b.child(text("shown"))
        } else {
            b.child(spacer())
        };
        b = b.optional_child(label.map(text));
        b = match status {
            Status::Idle => b.child(text("idle")),
            Status::Busy(n) => b.child(text("busy")).child(text(format!("{n}%"))),
        };
        for item in items {
            b = b.child(div().p(2.0).key(item).child(item));
        }
        let from_builders = b.child(text("f1")).child(text("f2")).into_any();
        (from_macro, from_builders)
    };
    for (show, status, label) in [
        (true, Status::Idle, Some("l")),
        (false, Status::Busy(40), None),
    ] {
        let (m, b) = build(show, status, label);
        assert_same("control flow", m, b);
    }
}

#[test]
fn builders_and_macro_output_nest_in_each_other() {
    let row = view! { <div class="flex-row" gap={4.0}>"inner"</div> };
    let from_macro = view! {
        <div class="flex-col">
            {div().p(2.0).child(text("built"))}
            {row}
            if true {
                {text("a")}
                {div().h(4.0)}
            }
        </div>
    };
    let from_builders = div()
        .flex_col()
        .child(div().p(2.0).child(text("built")))
        .child(div().flex_row().gap(4.0).child("inner"))
        .child(text("a"))
        .child(div().h(4.0))
        .into_any();
    assert_same("composition", from_macro, from_builders);
}

#[test]
fn props_components_match_their_builders() {
    let from_macro = view! {
        <div class="flex-col">
            <Button on:click={Pick("send")} icon={lucide::CHECK} label="Send"
                    variant={ButtonStyle::Filled} active />
            <Badge label="New" variant={BadgeVariant::Success} />
            <Checkbox checked={true} label="Wrap" on:toggle={Pick("wrap")} />
            <ProgressBar value={0.4} show_label />
            <Avatar name="Ada Lovelace" size={40.0} />
        </div>
    };
    let from_builders = div()
        .flex_col()
        .child(
            Button::new(Pick("send"))
                .icon(lucide::CHECK)
                .label("Send")
                .style(ButtonStyle::Filled)
                .active(true),
        )
        .child(badge("New").success())
        .child(checkbox(true).label("Wrap").on_toggle(Pick("wrap")))
        .child(progress_bar(0.4).show_label())
        .child(avatar("Ada Lovelace").size(40.0))
        .into_any();
    assert_same("props components", from_macro, from_builders);
}

#[test]
fn constructor_form_matches_builder_chain() {
    let from_macro = view! {
        <Button(Pick("close")) tooltip="Close">
            <.icon>{lucide::X}</.icon>
        </Button>
    };
    let from_builders = Button::new(Pick("close"))
        .tooltip("Close")
        .icon(lucide::X)
        .into_any();
    assert_same("constructor form", from_macro, from_builders);
}

// Catches a button whose content is its children going unnamed for
// screen readers.
#[test]
fn button_children_name_the_button() {
    let painted = paint(view! { <Button on:click={Pick("send")}>"Send"</Button> });
    assert!(
        painted.accessibility.contains("Button | Send"),
        "{}",
        painted.accessibility
    );
}

// Catches a class vocabulary entry that names a method the builders do not
// have: this test stops compiling.
#[test]
fn class_vocabulary_names_real_builder_methods() {
    quark::__class_vocabulary!();
}
