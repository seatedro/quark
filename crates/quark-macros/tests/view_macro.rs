//! Compile-and-run integration tests for the `view!` macro.
//!
//! `view!` is duck-typed: it emits calls to whatever `div()`, `text()`,
//! `spacer()`, component constructors, and builder methods are in scope at
//! the call site (quark-ui provides them in `element`). The `dsl` module
//! below is a minimal recording implementation of that contract, so each
//! lowering rule can be asserted as the list of builder calls it makes.
//! quark-components' `tests/view_equivalence.rs` checks the same rules
//! against the real builders, and rejected inputs are the trybuild cases in
//! `tests/ui/`.

use std::cell::Cell;
use std::rc::Rc;

use quark::reactive::{Signal, SignalStore};
use quark_macros::{Props, view};

use dsl::*;

#[allow(dead_code)]
mod dsl {
    use std::rc::Rc;

    /// Built element tree node; records the tag, builder calls in order,
    /// the slot it was assigned to (for component child slots), and children.
    pub struct AnyElement {
        pub tag: &'static str,
        pub value: Option<String>,
        pub calls: Vec<String>,
        pub slot: Option<&'static str>,
        pub children: Vec<AnyElement>,
        pub on_click: Option<Rc<dyn Fn()>>,
    }

    impl AnyElement {
        fn new(tag: &'static str) -> Self {
            AnyElement {
                tag,
                value: None,
                calls: Vec::new(),
                slot: None,
                children: Vec::new(),
                on_click: None,
            }
        }
    }

    pub trait IntoAnyElement {
        fn into_any(self) -> AnyElement;
    }

    impl IntoAnyElement for AnyElement {
        fn into_any(self) -> AnyElement {
            self
        }
    }

    impl IntoAnyElement for &str {
        fn into_any(self) -> AnyElement {
            text(self).into_any()
        }
    }

    impl IntoAnyElement for String {
        fn into_any(self) -> AnyElement {
            text(self).into_any()
        }
    }

    impl From<&str> for AnyElement {
        fn from(s: &str) -> Self {
            s.into_any()
        }
    }

    impl From<String> for AnyElement {
        fn from(s: String) -> Self {
            s.into_any()
        }
    }

    /// One builder type stands in for div, text, icon, and spacer: every
    /// method records its name and arguments.
    pub struct El {
        el: AnyElement,
    }

    impl IntoAnyElement for El {
        fn into_any(self) -> AnyElement {
            self.el
        }
    }

    pub fn div() -> El {
        El {
            el: AnyElement::new("div"),
        }
    }

    pub fn text(content: impl Into<String>) -> El {
        let mut el = AnyElement::new("text");
        el.value = Some(content.into());
        El { el }
    }

    pub fn spacer() -> El {
        El {
            el: AnyElement::new("spacer"),
        }
    }

    pub fn svg_icon(svg: &'static str, size: f32) -> El {
        let mut el = AnyElement::new("icon");
        el.value = Some(format!("{svg}@{size}"));
        El { el }
    }

    /// A recorded `StyleOverride` for `hover:` classes.
    #[derive(Default)]
    pub struct Hover(Vec<String>);

    impl Hover {
        pub fn bg(mut self, c: &str) -> Self {
            self.0.push(format!("bg({c})"));
            self
        }
        pub fn opacity(mut self, v: f32) -> Self {
            self.0.push(format!("opacity({v})"));
            self
        }
    }

    macro_rules! record {
        ($($name:ident($($arg:ident: $ty:ty),*);)*) => {
            impl El {
                $(pub fn $name(mut self, $($arg: $ty),*) -> Self {
                    let args: Vec<String> = vec![$(format!("{:?}", $arg)),*];
                    self.el.calls.push(if args.is_empty() {
                        stringify!($name).to_owned()
                    } else {
                        format!("{}({})", stringify!($name), args.join(", "))
                    });
                    self
                })*
            }
        };
    }

    record! {
        flex_row(); flex_col(); flex_grow(); flex_shrink_0(); items_center(); hidden();
        bold(); mono(); text_sm(); medium();
        gap(v: f32); px(v: f32); p(v: f32); w(v: f32); min_w(v: f32); opacity(v: f32);
        flex_grow_val(v: f32); size(v: f32);
        bg(c: &str); color(c: &str); test_id(v: &str); id(v: String); key(v: String);
        shadow(blur: f32, y: f32, c: &str);
        accessibility_label(v: &str); accessibility_selected(v: bool);
        accessibility_disabled(v: bool);
        semantic_role(r: quark::SemanticRole);
        on_key(binding: &str, action: &str);
    }

    impl El {
        pub fn child(mut self, child: impl IntoAnyElement) -> Self {
            self.el.children.push(child.into_any());
            self
        }

        pub fn optional_child(mut self, child: Option<impl IntoAnyElement>) -> Self {
            if let Some(child) = child {
                self.el.children.push(child.into_any());
            }
            self
        }

        pub fn children(mut self, children: impl IntoIterator<Item = AnyElement>) -> Self {
            self.el.children.extend(children);
            self
        }

        pub fn hover(mut self, f: impl FnOnce(Hover) -> Hover) -> Self {
            self.el
                .calls
                .push(format!("hover[{}]", f(Hover::default()).0.join(", ")));
            self
        }

        pub fn on_click(mut self, handler: impl Fn() + 'static) -> Self {
            self.el.calls.push("on_click".into());
            self.el.on_click = Some(Rc::new(handler));
            self
        }
    }

    /// Rich text: `selectable_rich_text` and its spans.
    pub struct StyledSpan(pub String);

    impl StyledSpan {
        pub fn plain(text: impl Into<String>) -> Self {
            StyledSpan(text.into())
        }
        pub fn bold(self) -> Self {
            StyledSpan(format!("**{}**", self.0))
        }
        pub fn italic(self) -> Self {
            StyledSpan(format!("_{}_", self.0))
        }
        pub fn code(self) -> Self {
            StyledSpan(format!("`{}`", self.0))
        }
        pub fn link(self, url: &str) -> Self {
            StyledSpan(format!("[{}]({url})", self.0))
        }
    }

    pub fn selectable_rich_text(spans: Vec<StyledSpan>) -> El {
        let mut el = AnyElement::new("p");
        el.value = Some(spans.into_iter().map(|s| s.0).collect());
        El { el }
    }

    /// Component with one constructor arg plus `icon`/`label` builders.
    pub struct Button {
        el: AnyElement,
    }

    impl IntoAnyElement for Button {
        fn into_any(self) -> AnyElement {
            self.el
        }
    }

    impl Button {
        pub fn new(action: &'static str) -> Self {
            let mut el = AnyElement::new("Button");
            el.calls.push(format!("action({action})"));
            Button { el }
        }

        pub fn tooltip(mut self, value: &'static str) -> Self {
            self.el.calls.push(format!("tooltip({value})"));
            self
        }

        pub fn icon(mut self, value: &'static str) -> Self {
            self.el.calls.push(format!("icon({value})"));
            self
        }

        pub fn label(mut self, value: impl ToString) -> Self {
            self.el.calls.push(format!("label({})", value.to_string()));
            self
        }

        pub fn flex_grow(mut self) -> Self {
            self.el.calls.push("flex_grow".into());
            self
        }
    }

    /// Component with `left_child`/`right_child` builders for slots.
    pub struct Toolbar {
        el: AnyElement,
    }

    impl IntoAnyElement for Toolbar {
        fn into_any(self) -> AnyElement {
            self.el
        }
    }

    impl Toolbar {
        pub fn new() -> Self {
            Toolbar {
                el: AnyElement::new("Toolbar"),
            }
        }

        pub fn compact(mut self) -> Self {
            self.el.calls.push("compact".into());
            self
        }

        pub fn left_child(mut self, mut child: AnyElement) -> Self {
            child.slot = Some("left");
            self.el.children.push(child);
            self
        }

        pub fn right_child(mut self, mut child: AnyElement) -> Self {
            child.slot = Some("right");
            self.el.children.push(child);
            self
        }
    }
}

/// A `#[derive(Props)]` component: every kind of prop.
#[derive(Props)]
pub struct Card {
    title: String,
    #[prop(into)]
    on_close: String,
    #[prop(optional, into)]
    subtitle: Option<String>,
    #[prop(default = 2)]
    level: u8,
    #[prop(default)]
    selected: bool,
    #[prop(default)]
    children: Vec<AnyElement>,
}

impl IntoAnyElement for Card {
    fn into_any(self) -> AnyElement {
        let mut el = div().into_any();
        el.tag = "Card";
        el.calls = vec![
            format!("title({})", self.title),
            format!("on_close({})", self.on_close),
            format!("subtitle({:?})", self.subtitle),
            format!("level({})", self.level),
            format!("selected({})", self.selected),
        ];
        el.children = self.children;
        el
    }
}

/// `cx` contract required by `{@sig}` attributes: any type with a
/// `read<T>(Signal<T>) -> T` method, named `cx` at the call site.
struct Cx<'a> {
    store: &'a SignalStore,
}

impl Cx<'_> {
    fn read<T: 'static + Clone>(&self, signal: Signal<T>) -> T {
        self.store.read(signal)
    }
}

/// Child values (text, or tag for elements without one), in order.
fn kids(el: &AnyElement) -> Vec<&str> {
    el.children
        .iter()
        .map(|c| c.value.as_deref().unwrap_or(c.tag))
        .collect()
}

#[test]
fn basic_element_emit() {
    let el = view! {
        <div flex_row gap={4.0}>
            <text color="red">"hello"</text>
            <spacer />
        </div>
    };

    assert_eq!(el.tag, "div");
    assert_eq!(el.calls, ["flex_row", "gap(4.0)"]);
    assert_eq!(kids(&el), ["hello", "spacer"]);
    assert_eq!(el.children[0].calls, ["color(\"red\")"]);
}

// Catches a regression in any attribute form: flags, literals, kebab-case
// names, tuple splats, and the `aria-`/`role`/`on:key` mappings.
#[test]
fn attribute_forms_lower_to_builder_calls() {
    let el = view! {
        <div hidden
             min-w={0.0}
             test-id="row"
             id={"r".to_string()}
             shadow={(2.0, 1.0, "black")}
             aria-label="Close"
             aria-selected={false}
             aria-disabled
             role="button"
             on:key:mod+s={"save"} />
    };
    assert_eq!(
        el.calls,
        [
            "hidden",
            "min_w(0.0)",
            "test_id(\"row\")",
            "id(\"r\")",
            "shadow(2.0, 1.0, \"black\")",
            "accessibility_label(\"Close\")",
            "accessibility_selected(false)",
            "accessibility_disabled(true)",
            "semantic_role(Button)",
            "on_key(\"mod+s\", \"save\")",
        ]
    );
}

#[test]
fn nested_children_expression_forms_and_fragment() {
    fn badge(n: usize) -> AnyElement {
        text(format!("badge-{n}")).into_any()
    }

    let extras = vec![badge(1), badge(2)];
    let present: Option<AnyElement> = Some(badge(3));
    let absent: Option<AnyElement> = None;

    let el = view! {
        <div>
            <div flex_row>
                {badge(0)}
            </div>
            {?present}
            {?absent}
            {...extras}
            <>
                <text>"a"</text>
                "b"
            </>
        </div>
    };

    // nested div + present optional + 2 spread + 2 fragment children, with the
    // fragment flattened into the parent (no wrapper node).
    assert_eq!(
        kids(&el),
        ["div", "badge-3", "badge-1", "badge-2", "a", "b"]
    );
    assert_eq!(el.children[0].children[0].value.as_deref(), Some("badge-0"));
}

// Catches builders and macro output failing to mix: a builder value is a
// child anywhere, including spread positions that need `AnyElement`s.
#[test]
fn builder_values_and_macro_output_compose() {
    let inner = view! { <text>"from macro"</text> };
    let make = |many: bool| {
        view! {
            <div>
                {div().child(inner_text())}
                if many {
                    {text("x")}
                    {div()}
                }
                {...[text("s1"), text("s2")]}
            </div>
        }
    };
    fn inner_text() -> AnyElement {
        text("built").into_any()
    }
    assert_eq!(kids(&make(true)), ["div", "x", "div", "s1", "s2"]);
    assert_eq!(kids(&make(false)), ["div", "s1", "s2"]);
    let el = div().child(inner).into_any();
    assert_eq!(kids(&el), ["from macro"]);
}

#[test]
fn text_interpolates_and_joins_parts() {
    let name = "Ada";
    let n = 3;
    let el = view! {
        <div>
            <text>"Hello, {name}! {{braces}}"</text>
            <text>"n=" {n} " ratio={1.0 / 3.0:.2}"</text>
            "{n} items"
        </div>
    };
    assert_eq!(
        kids(&el),
        ["Hello, Ada! {braces}", "n=3 ratio=0.33", "3 items"]
    );
}

#[test]
fn rich_text_inline_tags_style_spans() {
    let url = "https://x.dev";
    let el = view! {
        <p>"Run " <code>"cargo test"</code> ", " <b>"then " <i>"read"</i></b> <br/>
           <a href={url}>"docs"</a></p>
    };
    assert_eq!(
        el.value.as_deref(),
        Some("Run `cargo test`, **then **_**read**_\n[docs](https://x.dev)")
    );
}

#[test]
fn if_without_else_is_optional_child() {
    let make = |cond: bool| {
        view! {
            <div>
                if cond { <text>"shown"</text> }
            </div>
        }
    };

    assert_eq!(kids(&make(true)), ["shown"]);
    assert!(make(false).children.is_empty());
}

#[test]
fn if_else_and_else_if_chain_pick_one_branch() {
    let pick = |a: bool, b: bool| {
        view! {
            <div>
                if a { <text>"a"</text> }
                else if b { "b" }
                else { <text>"c"</text> }
            </div>
        }
    };

    assert_eq!(kids(&pick(true, false)), ["a"]);
    assert_eq!(kids(&pick(false, true)), ["b"]);
    assert_eq!(kids(&pick(false, false)), ["c"]);
}

#[test]
fn if_let_binds_into_its_branch() {
    let make = |opt: Option<&str>| {
        view! {
            <div>
                if let Some(label) = opt { <text>{label}</text> } else { <spacer/> }
            </div>
        }
    };
    assert_eq!(kids(&make(Some("x"))), ["x"]);
    assert_eq!(kids(&make(None)), ["spacer"]);
}

/// Regression: a chain with no final `else` used to inject a spacer when no
/// branch matched.
#[test]
fn else_if_without_final_else_adds_no_child() {
    let pick = |a: bool, b: bool| {
        view! {
            <div>
                if a { <text>"a"</text> }
                else if b { <text>"b"</text> }
            </div>
        }
    };

    assert_eq!(kids(&pick(false, true)), ["b"]);
    assert!(pick(false, false).children.is_empty());
}

/// Regression: only the first `if` body and the final `else` were checked
/// for multiple children, so a multi-child `else if` was wrapped in a div.
#[test]
fn multi_child_else_if_branch_spreads_into_parent() {
    let pick = |a: bool| {
        view! {
            <div>
                if a { <text>"a"</text> }
                else if !a {
                    <text>"b1"</text>
                    <text>"b2"</text>
                }
            </div>
        }
    };

    assert_eq!(kids(&pick(true)), ["a"]);
    assert_eq!(kids(&pick(false)), ["b1", "b2"]);
}

#[test]
fn multi_child_if_spreads_into_parent_without_wrapper() {
    let make = |cond: bool| {
        view! {
            <div>
                <text>"lead"</text>
                if cond {
                    <text>"x"</text>
                    <text>"y"</text>
                }
                <text>"tail"</text>
            </div>
        }
    };

    // Branch children flow inline into the parent: no wrapper div node.
    assert_eq!(kids(&make(true)), ["lead", "x", "y", "tail"]);
    assert_eq!(kids(&make(false)), ["lead", "tail"]);
}

#[test]
fn for_loop_flattens_each_iteration() {
    let items = ["a", "b", "c"];

    let el = view! {
        <div>
            for (i, item) in items.iter().enumerate() {
                <text>"{i}:{item}"</text>
            }
        </div>
    };

    assert_eq!(kids(&el), ["0:a", "1:b", "2:c"]);
}

#[test]
fn keyed_for_puts_the_key_on_each_root() {
    let items = ["a", "b"];
    let el = view! {
        <div>
            for item in items key={format!("row-{item}")} {
                <div>{item}</div>
            }
        </div>
    };
    let keys: Vec<&[String]> = el.children.iter().map(|c| c.calls.as_slice()).collect();
    assert_eq!(keys, [["key(\"row-a\")"], ["key(\"row-b\")"]]);
}

#[test]
fn match_arms_take_markup_rust_and_multiple_children() {
    let render = |n: u8| {
        view! {
            <div>
                match n {
                    0 => <text>"zero"</text>
                    1 => text("one").into_any(),
                    2 | 3 => {
                        <text>"two"</text>
                        <text>"three"</text>
                    }
                    n if n > 100 => {}
                    _ => <spacer />
                }
            </div>
        }
    };

    assert_eq!(kids(&render(0)), ["zero"]);
    assert_eq!(kids(&render(1)), ["one"]);
    assert_eq!(kids(&render(2)), ["two", "three"]);
    assert!(kids(&render(200)).is_empty());
    assert_eq!(kids(&render(7)), ["spacer"]);
}

#[test]
fn component_value_slots_and_constructor_args() {
    let el = view! {
        <Button("save") tooltip={"Save file"} class="grow">
            <.icon>{"disk"}</.icon>
            <.label>"Save"</.label>
        </Button>
    };

    assert_eq!(el.tag, "Button");
    // Constructor args first, then builder calls in attribute order (with the
    // class lowered to its mapped builder method), then slot calls.
    assert_eq!(
        el.calls,
        [
            "action(save)",
            "tooltip(Save file)",
            "flex_grow",
            "icon(disk)",
            "label(Save)",
        ]
    );
}

// Catches `<name(args)>` not calling the in-scope function, or dropping
// its attributes or children.
#[test]
fn lowercase_tag_with_arguments_calls_the_function() {
    fn panel(title: &str) -> El {
        div().test_id(title)
    }
    let el = view! {
        <panel("inbox") gap={2.0}>
            <text>"a"</text>
            if true { "b" }
        </panel>
    };
    assert_eq!(el.calls, ["test_id(\"inbox\")", "gap(2.0)"]);
    assert_eq!(kids(&el), ["a", "b"]);
}

// Catches `<{expr}>` not applying its attributes and children to the
// builder value the expression evaluates to.
#[test]
fn expression_tag_applies_attributes_to_a_builder_value() {
    let base = div().test_id("base");
    let el = view! {
        <{base} gap={2.0}>
            <text>"a"</text>
        </>
    };
    assert_eq!(el.calls, ["test_id(\"base\")", "gap(2.0)"]);
    assert_eq!(kids(&el), ["a"]);
    let lone = view! { <{Button::new("save")} tooltip="Save" /> };
    assert_eq!(lone.calls, ["action(save)", "tooltip(Save)"]);
}

#[test]
fn component_child_slots_map_to_repeated_builder_calls() {
    let el = view! {
        <Toolbar() compact>
            <.left_child>
                <spacer />
            </.left_child>
            <.right_child>
                <text>"r1"</text>
                <text>"r2"</text>
            </.right_child>
        </Toolbar>
    };

    assert_eq!(el.tag, "Toolbar");
    assert_eq!(el.calls, ["compact"]);
    let slots: Vec<(&str, &str)> = el
        .children
        .iter()
        .map(|c| {
            (
                c.slot.unwrap_or("none"),
                c.value.as_deref().unwrap_or(c.tag),
            )
        })
        .collect();
    assert_eq!(
        slots,
        [("left", "spacer"), ("right", "r1"), ("right", "r2")]
    );
}

// Catches props reaching the component wrong: required, `into`, optional,
// defaults, flags as `true`, `on:` events, and children.
#[test]
fn props_component_takes_props_and_children() {
    let el = view! {
        <Card title={"Inbox".to_string()} on:close="dismiss" selected>
            "first"
            <text>"second"</text>
        </Card>
    };
    assert_eq!(
        el.calls,
        [
            "title(Inbox)",
            "on_close(dismiss)",
            "subtitle(None)",
            "level(2)",
            "selected(true)",
        ]
    );
    assert_eq!(kids(&el), ["first", "second"]);

    let el = view! { <Card title={String::new()} on:close="x" subtitle="Sub" level={1} /> };
    assert_eq!(el.calls[2..4], ["subtitle(Some(\"Sub\"))", "level(1)"]);
}

#[test]
fn class_attribute_lowers_to_builder_methods() {
    let el =
        view! { <div class="flex-row grow grow-0 shrink-0 px-2 gap-[6] w-[320px] opacity-50" /> };
    assert_eq!(
        el.calls,
        [
            "flex_row",
            "flex_grow",
            "flex_grow_val(0.0)",
            "flex_shrink_0",
            "px(8.0)",
            "gap(6.0)",
            "w(320.0)",
            "opacity(0.5)",
        ]
    );

    let el = view! { <text class="font-bold font-mono">"x"</text> };
    assert_eq!(el.calls, ["bold", "mono"]);
}

#[test]
fn hover_classes_gather_into_one_override() {
    let accent = "accent";
    let el = view! { <div class="hover:bg-[accent] p-1 hover:opacity-80" /> };
    assert_eq!(el.calls, ["p(4.0)", "hover[bg(accent), opacity(0.8)]"]);
    let _ = accent;
}

#[test]
fn event_handler_attribute_binds_closure() {
    let clicks = Rc::new(Cell::new(0u32));
    let sink = clicks.clone();

    let el = view! {
        <div on:click={move || sink.set(sink.get() + 1)}>
            <text>"button"</text>
        </div>
    };

    assert!(el.calls.iter().any(|call| call == "on_click"));
    let handler = el.on_click.as_ref().unwrap();
    handler();
    handler();
    assert_eq!(clicks.get(), 2);
}

// Catches `on:event={if ..}` failing to compile or setting a handler when
// the condition does not hold.
#[test]
fn conditional_event_handler_is_set_only_when_present() {
    let make = |handler: Option<fn()>| {
        view! { <div on:click={if let Some(h) = handler { h }} /> }
    };
    assert_eq!(make(Some(|| {})).calls, ["on_click"]);
    assert!(make(None).calls.is_empty());
}

#[test]
fn reactive_attribute_reads_through_cx() {
    let store = SignalStore::default();
    let gap = store.create(4.0f32);
    let label = store.create("alpha");
    let cx = Cx { store: &store };

    let build = || {
        view! {
            <div gap={@gap}>
                <text color={@label}>"t"</text>
            </div>
        }
    };

    let el = build();
    assert_eq!(el.calls, ["gap(4.0)"]);
    assert_eq!(el.children[0].calls, ["color(\"alpha\")"]);

    store.write(gap, 12.0);
    store.write(label, "beta");

    let el = build();
    assert_eq!(el.calls, ["gap(12.0)"]);
    assert_eq!(el.children[0].calls, ["color(\"beta\")"]);
}

#[test]
fn scale_multiplies_spatial_attributes_only() {
    let scale = 2.0f32;
    let el = view! { scale, <div gap={3.0} w={10.0} p={if true { 1.5 } else { 0.0 }} /> };
    assert_eq!(el.calls, ["gap(6.0)", "w(10.0)", "p(3.0)"]);
}
