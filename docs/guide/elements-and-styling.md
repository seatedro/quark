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

## Text wrapping

`text(s)` wraps to the width layout gives it. It is as wide as its
text, up to the width its container offers, and as tall as the lines it
wraps into, so in a column it wraps at the column's width, and in a row it
shrinks beside its siblings down to its widest word. Lines break between
words; a word wider than a box forced narrower than it breaks between
characters. Give a fixed-width sibling such as an icon `flex_shrink_0()`
so the text shrinks instead of it.

To opt out, `.no_wrap()` (class `whitespace-nowrap`) keeps one line at the
text's natural width, `.truncate()` keeps one line and ends it with an
ellipsis where its box is narrower, and `.wrap_width(w)` wraps at `w`
points whatever the container's width, and can overflow it. Before
automatic wrapping, text kept its natural width unless given a wrap
width; code that relied on that needs `.no_wrap()`.

`selectable_text(s)`, `selectable_rich_text(spans)`, and `<p>` wrap the same
way and take `.no_wrap()`; their `.width(w)` is the explicit wrap width.

## Styling

`quark_ui::style::Styled` gives every element the same builder methods:
flex direction (`flex_row`, `flex_col`), alignment (`items_center`,
`justify_center`), spacing (`gap`, `p`, `px`), size (`w`, `h`, `w_full`),
colors (`bg`, `border`), and corners (`rounded`). `quark_ui::design` holds
the tokens: `Sp` for spacing, `Rad` for radii, `Sz`, `Shadow`, and text
styles. Colors come from the theme.

Views write these calls with `view!` ([Writing views](writing-views.md)):
each attribute is the builder method of the same name, and each class one
builder call. This doctest from [crates/quark-ui/src/lib.rs](../../crates/quark-ui/src/lib.rs)
builds a toolbar with one button:

```rust
use quark::view;
use quark_ui::Action;
use quark_ui::design::{Rad, Sp};
use quark_ui::element::{AnyElement, IntoAnyElement, div, text};
use quark_ui::style::Styled;
use quark_ui::theme::Theme;

#[derive(Debug, PartialEq)]
struct Save;

fn toolbar(theme: &Theme) -> AnyElement {
    let colors = &theme.colors;
    view! {
        <div class="flex-row" gap={Sp::SM} p={Sp::MD} bg={colors.surface}>
            <div test_id="toolbar.save" on:click={Action::new(Save)} px={Sp::LG}
                 rounded={Rad::XL} bg={colors.accent} hover_bg={colors.accent_strong}>
                <text color={colors.text_strong} class="font-semibold">"Save"</text>
            </div>
        </div>
    }
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

Scrollbars always show unless the container asks for
`scrollbar_auto_hide()`. Then they show while the pointer is over it or a
thumb is held, and for a second after the offset moves or the container
gains focus. A handle keeps that state itself. A container whose offset
the app owns keeps a `ScrollbarVisibility` next to the offset and attaches
it with `.scrollbar_visibility(&state)`; the tree, table, diff view, and
document turn this on with `with_scrollbar_auto_hide()` (on the document
element, `scrollbar_auto_hide()`).

## The `view!` macro

`quark::view!` writes the same builder calls as HTML-like markup, with
Tailwind-style classes and typed component props. [Writing views](writing-views.md)
covers the syntax side by side with the builders.

## Developer tools

With the `devtools` feature, `ctrl+shift+h` toggles a frame HUD,
`ctrl+shift+i` an element inspector that shows bounds, clip, semantics, and
style and edits padding, gap, colors, and radius live, and `ctrl+shift+l`
outlines every element's bounds. `QUARK_DEVTOOLS=hud,inspector,layout`
turns them on at startup.
