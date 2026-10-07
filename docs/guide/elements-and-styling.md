# Elements and styling

A view returns an `AnyElement`: a tree of element values built with
constructor functions and builder methods, rebuilt every frame. Taffy lays
the tree out as flexbox, and paint emits scene primitives. Elements hold no
state between frames; what must survive a frame lives in the app or in a
handle the element is given (a `ScrollHandle`, a `TextField`).

## Constructors

From `quark_ui::element`:

| Constructor | Element |
|---|---|
| `div()` | Container: layout, background, border, events, accessibility |
| `text(s)` | A run of text |
| `text_input(label, value)` | A single-line field bound to a `TextField` with `.field(&model)` |
| `selectable_text(s)` | Text the pointer can select |
| `code_block(lines)` | Monospaced lines of styled spans |
| `spacer()` | Flexible space |
| `canvas(paint)` | A closure that paints scene primitives into the element's bounds |
| `cached(key, hash, build)` | A cache boundary; see [Performance model](performance.md) |

`quark-components` builds on these: buttons, checkbox, switch, select,
combobox, dropdown, picker, radio group, segmented control, slider, search
field, tabs, breadcrumb, badge, avatar, progress, tooltip, hover card,
context menu, popover, modal, toast, command palette, split panes, dock,
tree, table, and diff view. Components
emit the app's actions through mappings the caller supplies and know
nothing of the app's types.

## Styling

`quark_ui::style::Styled` gives every element the same builder methods:
flex direction (`flex_row`, `flex_col`), alignment (`items_center`,
`justify_center`), spacing (`gap`, `p`, `px`), size (`w`, `h`, `w_full`),
colors (`bg`, `border`), and corners (`rounded`). `quark_ui::design` holds
the tokens: `Sp` for spacing, `Rad` for radii, `Sz`, `Shadow`, and text
styles. Colors come from the theme.

This doctest from [crates/quark-ui/src/lib.rs](../../crates/quark-ui/src/lib.rs)
builds a toolbar with one button:

```rust
use quark_ui::Action;
use quark_ui::design::{Rad, Sp};
use quark_ui::element::{AnyElement, IntoAnyElement, div, text};
use quark_ui::style::Styled;
use quark_ui::theme::Theme;

#[derive(Debug, PartialEq)]
struct Save;

fn toolbar(theme: &Theme) -> AnyElement {
    let colors = &theme.colors;
    div()
        .flex_row()
        .gap(Sp::SM)
        .p(Sp::MD)
        .bg(colors.surface)
        .child(
            div()
                .test_id("toolbar.save")
                .on_click(Action::new(Save))
                .px(Sp::LG)
                .rounded(Rad::XL)
                .bg(colors.accent)
                .hover_bg(colors.accent_strong)
                .child(text("Save").color(colors.text_strong).semibold()),
        )
        .into_any()
}
```

## Interaction

On a `div`:

- `on_click(action)` emits an action; `hover_bg(color)` restyles under the
  pointer.
- `focus_ring(focus_id)` makes the div focusable as `focus_id` and a Tab
  stop. A clickable div with a stable id (`id`, `test_id`,
  `accessibility_id`) is a Tab stop too. `trap_focus(true)` keeps Tab
  inside a modal's focus scope.
- `tooltip(text)` shows a tooltip on hover.
- `id`, `key`, and `test_id` give the div a stable identity. Keys keep
  identity when siblings reorder; animations, transitions, and inspector
  overrides are keyed by it.

## Themes

`quark_ui::theme::Theme` holds `ThemeColors` and `ThemeMetrics`.
`Theme::default_dark()` and `Theme::default_light()` are the built-in pair.
`quark_ui::palette` generates 12-step color scales in Oklch for custom
themes.

By default the adapter follows the desktop's light or dark preference,
starting dark until the platform reports one, and redraws when it changes.
To choose themes, build the adapter yourself (a fragment; `app`,
`my_theme`, and `options` are yours):

```rust
let adapter = UiAdapter::new(app, "Notes").with_theme(my_theme);
quark_app::run(adapter, options)
```

`with_themes(light, dark)` keeps following the desktop with your pair.

## Transitions and animation

`div().transition(props, motion)` animates the listed style properties
whenever their resolved value changes between frames, from the value on
screen. Colors interpolate in premultiplied Oklab, so a fade between two
theme colors does not pass through grey. `ViewContext::animations()`
exposes the window's `AnimationTable` for values a component animates
itself, keyed by stable identity; rows still moving schedule the next frame.
[animation_demo.rs](../../crates/quark-app/examples/animation_demo.rs)
shows both, including a spring that reverses with its velocity intact.

## Scrolling

A `ScrollHandle` owns a container's offset across frames. Attach it with
`div().track_scroll(&handle)` plus `overflow_y_scroll()` (or `_x_`, or
both). The input router moves it on wheel, scrollbar, and keys without a
round trip through the app; the app can call `set_offset`, `animate_to`, or
`scroll_to_item`. The module docs in
[element/scroll.rs](../../crates/quark-ui/src/element/scroll.rs) cover
the app-owned alternative.

## The `view!` macro

`quark::view!` writes the same builder calls from markup (a fragment;
the names are placeholders):

```rust
view! {
    <div class="flex-row" gap={Sp::SM} bg={colors.surface}>
        <text color={colors.text}>{label}</text>
        if let Some(x) = opt { <div>...</div> }
        for item in items { <div>{item.name}</div> }
    </div>
}
```

It expands at compile time to calls to whatever `div()`, `text()`, and
builder methods are in scope. The syntax is summarized in
[crates/quark/ARCHITECTURE.md](../../crates/quark/ARCHITECTURE.md) and each
rule is tested in
[crates/quark-macros/tests/view_macro.rs](../../crates/quark-macros/tests/view_macro.rs).
The examples use the builder API directly.

## Developer tools

With the `devtools` feature, `ctrl+shift+h` toggles a frame HUD,
`ctrl+shift+i` an element inspector that shows bounds, clip, semantics, and
style and edits padding, gap, colors, and radius live, and `ctrl+shift+l`
outlines every element's bounds. `QUARK_DEVTOOLS=hud,inspector,layout`
turns them on at startup.
