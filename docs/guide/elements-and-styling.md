# Elements and styling

Element constructors, styling, themes, motion, scrolling, and overlays.

- A view returns an `AnyElement`: a tree of element values, rebuilt every
  frame.
- Taffy lays it out as flexbox; paint emits scene primitives.
- Elements hold no state between frames: keep it in the app or in a handle
  (`ScrollHandle`, `TextField`).

## Constructors

From `quark_ui::element`:

| Constructor | Element |
|---|---|
| `div()` | Container: layout, background, border, events, accessibility |
| `text(s)` | A run of text |
| `text_input(label, value)` | Single-line field, bound to a `TextField` with `.field(&model)` |
| `selectable_text(s)` | Text the pointer can select |
| `code_block(lines)` | Monospaced lines of styled spans |
| `spacer()` | Flexible space |
| `canvas(paint)` | A closure painting scene primitives into the element's bounds |
| `cached(key, hash, build)` | A cache boundary ([Performance model](performance.md)) |

- `quark-components` adds: buttons, checkbox, switch, select, combobox,
  dropdown, picker, radio group, segmented control, slider, search field,
  tabs, breadcrumb, badge, avatar, progress, tooltip, hover card, context
  menu, popover, modal, toast, command palette, split panes, dock, tree,
  table, diff view.
- Components emit the app's actions through caller-supplied mappings and
  know nothing of the app's types.

## Text wrapping

- `text(s)` is as wide as its text, up to the width its container offers,
  and as tall as its wrapped lines.
- In a column it wraps at the column width; in a row it shrinks beside
  siblings down to its widest word.
- Lines break between words; a word wider than a box forced narrower breaks
  between characters.
- Give fixed-width siblings (an icon) `flex_shrink_0()` so the text shrinks
  instead.

| Opt-out | Effect |
|---|---|
| `.no_wrap()` (class `whitespace-nowrap`) | One line at natural width |
| `.truncate()` | One line, ellipsis where the box is narrower |
| `.wrap_width(w)` | Wraps at `w` points regardless of the container; can overflow |

- `selectable_text(s)`, `selectable_rich_text(spans)`, and `<p>` wrap the
  same way and take `.no_wrap()`. Their `.width(w)` is the explicit wrap
  width.

## Styling

- `quark_ui::style::Styled` gives every element the same builders:
  `flex_row`, `flex_col`, `items_center`, `justify_center`, `gap`, `p`,
  `px`, `w`, `h`, `w_full`, `bg`, `border`, `rounded`, and more.
- Tokens in `quark_ui::design`: `Sp` (spacing), `Rad` (radii), `Sz`,
  `Shadow`, text styles.
- Colors come from the theme.
- In `view!`, each attribute is the builder of the same name and each class
  one builder call ([Writing views](writing-views.md)).

From [crates/quark-ui/src/lib.rs](../../crates/quark-ui/src/lib.rs):

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

| Builder | Effect |
|---|---|
| `on_click(action)` | Emits an action |
| `on_context_menu(\|at\| action)` | Secondary click (Ctrl-click on macOS) at the pointer, or Shift+F10 / Menu key on the focused element at its bottom left; `at.keyboard` tells which |
| `hover_bg(color)` | Restyles under the pointer |
| `focus_ring(focus_id)` | Focusable as `focus_id`, and a Tab stop |
| `trap_focus(true)` | Keeps Tab inside a modal's focus scope |
| `tooltip(text)` | Tooltip on hover |
| `id`, `key`, `test_id` | Stable identity. Keys survive sibling reorders; animations, transitions, and inspector overrides key on it |

- A clickable div with a stable id is also a Tab stop
  ([Accessibility](accessibility-and-automation.md#ids)).
- The focus ring is 2 points wide (`Sz::FOCUS_RING_W`), drawn outside the
  element, `focus_ring_offset(gap)` points further out.
- Checkboxes, switches, and radio rows use a 2-point gap
  (`Sz::FOCUS_RING_GAP`).
- A clipping container (scrolling ones included) cuts off the ring of a
  control at its edge. Pad its content by the ring's reach, or draw the ring
  inside with `focus_ring_offset(-Sz::FOCUS_RING_W)` (as dock tabs do).

## Themes

- `quark_ui::theme::Theme` holds `ThemeColors` and `ThemeMetrics`.
- Built-in pair: `Theme::default_dark()`, `Theme::default_light()`.
- `quark_ui::palette` generates 12-step Oklch color scales for custom
  themes.
- By default the adapter follows the desktop's light or dark preference
  (dark until the platform reports one) and redraws on change.

Choose themes by building the adapter yourself (fragment; `app`, `my_theme`,
`options` are yours):

```rust
let adapter = UiAdapter::new(app, "Notes").with_theme(my_theme);
quark_app::run(adapter, options)
```

- `with_themes(light, dark)` keeps following the desktop with your pair.

### Component metrics

- `Theme::components` (`ComponentMetrics`) sizes components app-wide:
  heights, radii, padding, gaps, font and icon sizes, shadows for buttons,
  selects and options, popovers, tooltips, toasts, modals, skeletons, form
  fields.
- Values are points at 100% zoom; components multiply by
  `ThemeMetrics::ui_scale()` once. Do not pass scaled values.
- `None` keeps the component's default; `theme::scaled_or` applies the same
  rule in an app's own components.

## Colors and blending

The renderer blends in linear light (sRGB surface); browsers blend encoded
values. Opaque colors match; translucent ones do not.

- Black at a CSS alpha darkens less. Match a CSS scrim or shadow with alpha
  `1 - (1 - a)^2.2`: CSS 12% black is about 25% (62 of 255); 50% is about
  78% (200).
- A light color at low alpha over a dark surface comes out much brighter
  than in a browser. Mix tints opaque instead, in sRGB as CSS `color-mix`
  does.
- Black shadows barely show on near-black backgrounds. In dark themes give
  raised surfaces (menus, popovers, cards) a hairline border lighter than
  the canvas.
- `theme::contrast_ratio` composites a translucent foreground the way the
  renderer does.

The Workbench's tint helper
([recipes.rs](../../examples/workbench/src/design/recipes.rs)):

```rust
pub fn tint(base: Color, color: Color, amount: f32) -> Color {
    base.lerp(color.with_alpha(255), amount).with_alpha(255)
}
```

## Points and pixels

- Element geometry and scene coordinates are logical points.
- The scale factor (physical pixels per point:
  `ElementContext::scale_factor`, `cx.frame.scale_factor()` in a view)
  applies at draw time. Apps need it only where pixels matter, such as
  `TextQuery::scale_factor`.
- App zoom is separate: `theme.metrics.ui_scale()` (set with
  `Theme::with_ui_scale`); components, `svg_icon`, and `raster_image`
  multiply sizes by it.
- SVG icons rasterize at device pixels and stay sharp at any scale.
- Raster images are filtered into their bounds: ship them at 2x and size the
  element in points.
- `animated_image(&image).size(w, h)` takes points. Without `size`, pixel
  size is used as points, so an @2x image shows twice as large.
- Block document: `MarkdownDocument::hint_image_size(src, w, h)` gives an
  image's size in points. Pixels that are a whole multiple of it
  (`DecodedImage::density`) show at the hinted size.

## Transitions and animation

- `div().transition(props, motion)` animates the listed style properties
  whenever their resolved value changes, from the value on screen.
- Colors interpolate in premultiplied Oklab, so a fade between theme colors
  skips grey.
- `ViewContext::animations()` exposes the window's `AnimationTable` for
  values a component animates itself, keyed by stable identity. Moving rows
  schedule the next frame.
- Example:
  [animation_demo.rs](../../crates/quark-app/examples/animation_demo.rs),
  including a spring that reverses with velocity intact.

## Scrolling

- A `ScrollHandle` owns a container's offset across frames.
- Attach with `div().track_scroll(&handle)` plus `overflow_y_scroll()` (or
  `_x_`, or both).
- The input router moves it on wheel, scrollbar, and keys with no round trip
  through the app.
- The app can call `set_offset`, `animate_to`, `scroll_to_item`.
- App-owned offsets instead: module docs of
  [element/scroll.rs](../../crates/quark-ui/src/element/scroll.rs).

Scrollbars:

- Always shown unless the container asks for `scrollbar_auto_hide()`.
- Auto-hide shows them while hovered or a thumb is held, and for a second
  after the offset moves or the container gains focus.
- A handle keeps that state itself.
- With an app-owned offset, keep a `ScrollbarVisibility` beside it and
  attach `.scrollbar_visibility(&state)`.
- Tree, table, diff view, and document: `with_scrollbar_auto_hide()` (on the
  document element, `scrollbar_auto_hide()`).

## Popovers, modals, and toasts

- Overlays are elements above the content with a z index.
- `quark_components::popover::anchored` places a popover after this frame's
  layout, so it never lags its trigger by a frame. Make it the anchor's last
  child.
- It uses the preferred side, flips when that side does not fit and the
  other has more room, then clamps inside the viewport (the window, from
  0,0).
- Select and combobox lists use it; `place_popover` is the same rule as a
  function.

Fragment (`open`, `theme`, `window` are yours):

```rust
view! {
    <div class="flex-row items-center px-3 h-8">
        <text>"Sort"</text>
        if open {
            <anchored(
                view! {
                    <popover_panel(theme) w={180.0} p={4.0}>
                        <text>"Newest first"</text>
                    </popover_panel>
                },
                PopoverSide::Bottom,
                window,
            ) />
        }
    </div>
}
```

- `Modal` and `CommandPalette::render` draw their own scrim over the given
  window size, in `theme.colors.overlay_scrim`. Convert CSS scrim alphas
  ([Colors and blending](#colors-and-blending)).
- `bottom` and `right` resolve against the parent: a layer holding overlays
  must be window-sized, or a bottom toast stack lands above the window's top
  edge.
- `ToastQueue::stack` takes `status_bar_height`, the points kept clear
  below. Pass a bottom input's height (a chat composer) so toasts clear its
  Send button.
- `Toast::new(ToastKind::Success, ..)` shows a check in the success color.
- `CommandPalette::width(w)`: panel width in points at 100% zoom (default
  640), narrowed to fit the window.
- `CommandPalette::keycaps(true)`: each shortcut as one key cap per key.

## Disclosures and form fields

`quark_components::DisclosureState` animates content open and closed (layout
has no height transition, so it measures the content).

- Keep one per disclosure.
- Call `tick(cx.animations(), now_ms, reduced_motion)` in the view.
- Wrap the content in `region(animations, content)` while `is_mounted()`, so
  closed content is not built.
- Content stays mounted, clipped, until it has collapsed.
- `trigger(label, on_toggle, theme)` is the row with the turning chevron.

`FormField::new(id, label, control)` puts a label over any control.

- `help(..)` or `error(Some(..))` goes under it.
- The field is an accessibility group named by its label and described by
  its error or help.
- Marked invalid and required as told; the error is a polite live region.
- Give the control the same label as its own name.

## Developer tools

Feature `devtools`:

| Key | Tool |
|---|---|
| `ctrl+shift+h` | Frame HUD |
| `ctrl+shift+i` | Element inspector: bounds, clip, semantics, style; live edits of padding, gap, colors, radius |
| `ctrl+shift+l` | Outline every element's bounds |

- `QUARK_DEVTOOLS=hud,inspector,layout` turns them on at startup.
