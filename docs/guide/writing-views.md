# Writing views

`quark::view!` writes element trees as HTML-like markup and expands, at
compile time, to the same builder calls you would write by hand. The
builders stay the only API: every attribute is a builder method, every
class is a builder call, and markup and builder values nest in each other
freely. [crates/quark-components/tests/view_equivalence.rs](../../crates/quark-components/tests/view_equivalence.rs)
paints each construct both ways and checks the scenes and accessibility
trees match.

The macro calls whatever `div()`, `text()`, `svg_icon()`,
`selectable_rich_text()`, `StyledSpan`, and component types are in scope,
plus the `IntoAnyElement` and `Styled` traits; importing
`quark_ui::element::*` and `quark_ui::style::Styled` covers them. Roles,
hex colors, and cursor classes expand to `::quark::` paths, so the crate
needs `quark` as a direct dependency.

## Side by side

From [hello_ui](../../crates/quark-app/examples/hello_ui.rs):

```rust
view! {
    <div accessibility_id={id} role="button" aria-label={label} on:click={msg}
         class="px-4 h-9 items-center justify-center rounded-[8]
                bg-[colors.accent] hover:bg-[colors.accent_strong]">
        <text class="font-semibold" color={colors.text_strong}>{label}</text>
    </div>
}
```

is

```rust
div()
    .accessibility_id(id)
    .semantic_role(SemanticRole::Button)
    .accessibility_label(label)
    .on_click(msg)
    .px(16.0)
    .h(36.0)
    .items_center()
    .justify_center()
    .rounded(8.0)
    .bg(colors.accent)
    .hover(|s| s.bg(colors.accent_strong))
    .child(text(label).semibold().color(colors.text_strong))
    .into_any()
```

## Tags

| Tag | Expands to |
|---|---|
| `<div>` | `div()` |
| `<text>` | `text(content)`: plain text, see [Text](#text) |
| `<p>` | `selectable_rich_text(spans)`: rich text, see [Text](#text) |
| `<icon svg={..} size={..}>` | `svg_icon(svg, size)` (size defaults to 16) |
| `<spacer/>` | `spacer()` |
| `<>...</>`, `<fragment>` | No element: the children join the parent |
| `<Name attr=..>` | A `#[derive(Props)]` component: `Name::builder()...build()` |
| `<Name(a, b) attr=..>` | A builder component: `Name::new(a, b)` then one call per attribute |
| `<.method>` | Inside a component: each child becomes `.method(child)` |

Lowercase names are built-in tags; anything else is an error that suggests
the closest one. Components start with an uppercase letter or are paths
(`<widgets::Card>`). A closing tag must match its opening tag; `</Card>`
may close `<widgets::Card>`.

## Attributes

Every builder method is an attribute. The method name carries the
attribute's span, so an unknown attribute is rustc's "no method named `x`"
error at that attribute (with rustc's own "a method with a similar name"
hint), a wrongly typed value is a type error at the value, and
rust-analyzer resolves hover, go to definition, and completion through
the call.

| Written | Expands to |
|---|---|
| `hidden` | `.hidden()` (on a props component: `.hidden(true)`) |
| `gap={8.0}`, `title="Inbox"`, `level=2` | `.gap(8.0)`, `.title("Inbox")`, `.level(2)` |
| `min-w={0.0}` | `.min_w(0.0)`: kebab-case and snake_case are the same name |
| `shadow={(4.0, 2.0, c)}` | `.shadow(4.0, 2.0, c)`: a tuple splats into arguments |
| `gap={@sig}` | `.gap(cx.read(sig))`, with `cx` in scope |
| `bg={if hot { a } else { b }}` | `.bg(if hot { a } else { b })`; with no `else` the call is skipped |
| `@when {cond} { attrs }` | The attributes apply only when `cond` holds |
| `on:click={a}`, `on:drag={f}`, `on:scroll={b}` | `.on_click(a)`, `.on_drag(f)`, `.on_scroll(b)`: any `on:name` is `.on_name` |
| `on:key:mod+s={a}` | `.on_key("mod+s", a)`; unknown modifiers fail to compile |
| `role="button"` | `.semantic_role(SemanticRole::Button)`, which also sets the platform role |
| `aria-label`, `aria-description`, `aria-valuetext` | `.accessibility_label`, `_description`, `_value` |
| `aria-selected`, `aria-checked`/`aria-pressed`, `aria-expanded`, `aria-disabled` | `.accessibility_selected`, `_toggled`, `_expanded`, `_disabled`; alone they mean `true` |
| `aria-invalid`, `aria-required`, `aria-readonly`, `aria-multiselectable`, `aria-level`, `aria-rowindex`, `aria-colindex`, `aria-sort`, `aria-live` | The matching `accessibility_*` builder, or `.live` |
| `key={..}`, `id="..."`, `test-id="..."` | `.key(..)`, `.id(..)`, `.test_id(..)` |
| `track_scroll={&handle}` | `.track_scroll(&handle)`: the only handle-style attribute, because it is the only one the builders have |

`role` takes the ARIA names `alert`, `button`, `cell`, `checkbox`,
`combobox`, `dialog`, `document`, `grid`, `gridcell`, `group`, `heading`,
`img`, `label`, `link`, `list`, `listbox`, `listitem`, `menu`, `menubar`,
`menuitem`, `option`, `progressbar`, `radio`, `radiogroup`, `row`,
`separator`, `slider`, `spinbutton`, `status`, `switch`, `tab`, `table`,
`tablist`, `tabpanel`, `textbox`, `toolbar`, `tooltip`, `tree`, and
`treeitem`, plus quark's `scrollarea`, or `role={expr}` for a
`SemanticRole`.

`on:hover` expands to `.on_hover(..)`, which no builder has yet; hover is a
style state in quark, written with `hover:` classes or `hover_bg={..}`.

Inside `view! { scale, ... }`, `gap`, `p`, `px`, `py`, `pt`, `pb`, `pl`, `pr`,
and `rounded` values on a `div` are multiplied by `scale` and rounded.

## Classes

`class="..."` is a list of Tailwind-style names, each expanded at compile
time to one builder call. There is no CSS at runtime.

- Spacing and sizes use Tailwind's 4-point scale: `p-4` is `.p(16.0)`,
  `gap-0.5` is `.gap(2.0)`, `h-px` is `.h(1.0)`.
- Brackets take an exact value or any Rust expression: `w-[320px]`,
  `gap-[6]`, `w-[sidebar_w]`, `bg-[#336699]`, `bg-[colors.surface]`,
  `aspect-[16/9]`. Spaces inside brackets are allowed. A bracketed
  expression after `text-` or `border-` is a color; a number there is a
  size or width.
- `hover:` gathers every hover class of the element into one
  `.hover(|s| ...)`: `hover:bg-[c] hover:opacity-80`. It covers background,
  border color, text and icon color, opacity, and `rounded-[..]`, which is
  what `StyleOverride` holds. A `hover_bg={..}` attribute after the class
  replaces it, since each `.hover` call replaces the last.
- `focus:`, `focus-visible:`, `active:`, `disabled:`, `dark:`, and
  `group-hover:` fail to compile with the alternative: div has no such
  style state, so use `@when {cond} { .. }`, or theme colors for dark mode.
  Breakpoints (`sm:` to `2xl:`) fail too: there are no viewport media
  queries, so branch on the window size in the view.
- An unknown class fails to compile with the closest known class, and a
  class for another element kind (`text-sm` on a `div`) says which element
  it styles. The error spans the whole `class` literal, since stable Rust
  cannot point inside a string; the message names the class.

Every entry below is compiled against the real builders by
`class_vocabulary_names_real_builder_methods` in
`tests/view_equivalence.rs`, and the table is generated from the macro's
tables (`QUARK_BLESS=1 cargo test -p quark-macros --lib` rewrites it).

<!-- class-reference:start -->
| Class | Builder call | On |
|---|---|---|
| `flex-row` | `.flex_row()` | `<div>` |
| `flex-col` | `.flex_col()` | `<div>` |
| `flex-row-reverse` | `.flex_row_reverse()` | `<div>` |
| `flex-col-reverse` | `.flex_col_reverse()` | `<div>` |
| `flex-wrap` | `.flex_wrap()` | `<div>` |
| `flex-1` | `.flex_1()` | `<div>` |
| `flex-auto` | `.flex_auto()` | `<div>` |
| `flex-none` | `.flex_none()` | `<div>` |
| `grow` | `.flex_grow()` | `<div>` |
| `grow-0` | `.flex_grow_val(0.0)` | `<div>` |
| `shrink-0` | `.flex_shrink_0()` | `<div>` |
| `basis-auto` | `.basis_auto()` | `<div>` |
| `basis-full` | `.basis_full()` | `<div>` |
| `items-start` | `.items_start()` | `<div>` |
| `items-center` | `.items_center()` | `<div>` |
| `items-end` | `.items_end()` | `<div>` |
| `items-baseline` | `.items_baseline()` | `<div>` |
| `items-stretch` | `.items_stretch()` | `<div>` |
| `justify-start` | `.justify_start()` | `<div>` |
| `justify-center` | `.justify_center()` | `<div>` |
| `justify-end` | `.justify_end()` | `<div>` |
| `justify-between` | `.justify_between()` | `<div>` |
| `self-auto` | `.self_auto()` | `<div>` |
| `self-start` | `.self_start()` | `<div>` |
| `self-center` | `.self_center()` | `<div>` |
| `self-end` | `.self_end()` | `<div>` |
| `self-stretch` | `.self_stretch()` | `<div>` |
| `self-baseline` | `.self_baseline()` | `<div>` |
| `grid` | `.grid()` | `<div>` |
| `grid-flow-row` | `.grid_flow_row()` | `<div>` |
| `grid-flow-col` | `.grid_flow_col()` | `<div>` |
| `grid-flow-dense` | `.grid_dense()` | `<div>` |
| `w-full` | `.w_full()` | `<div>` |
| `h-full` | `.h_full()` | `<div>` |
| `size-full` | `.size_full()` | `<div>` |
| `size-max-content` | `.size_max_content()` | `<div>` |
| `aspect-square` | `.aspect_ratio(1.0)` | `<div>` |
| `aspect-video` | `.aspect_ratio(16.0 / 9.0)` | `<div>` |
| `absolute` | `.absolute()` | `<div>` |
| `relative` | `.relative()` | `<div>` |
| `hidden` | `.hidden()` | `<div>` |
| `overflow-hidden` | `.overflow_hidden()` | `<div>` |
| `overflow-x-hidden` | `.overflow_x_hidden()` | `<div>` |
| `overflow-y-hidden` | `.overflow_y_hidden()` | `<div>` |
| `overflow-scroll` | `.overflow_scroll()` | `<div>` |
| `overflow-x-scroll` | `.overflow_x_scroll()` | `<div>` |
| `overflow-y-scroll` | `.overflow_y_scroll()` | `<div>` |
| `overflow-clip` | `.clip()` | `<div>` |
| `scrollbar-none` | `.hide_scrollbar()` | `<div>` |
| `scrollbar-auto-hide` | `.scrollbar_auto_hide()` | `<div>` |
| `rounded-none` | `.rounded_none()` | `<div>` |
| `rounded-sm` | `.rounded_sm()` | `<div>` |
| `rounded-md` | `.rounded_md()` | `<div>` |
| `rounded-lg` | `.rounded_lg()` | `<div>` |
| `rounded-xl` | `.rounded_xl()` | `<div>` |
| `rounded-full` | `.rounded_full()` | `<div>` |
| `cursor-default` | `.cursor(CursorHint::Default)` | `<div>` |
| `cursor-pointer` | `.cursor(CursorHint::Pointer)` | `<div>` |
| `cursor-text` | `.cursor(CursorHint::Text)` | `<div>` |
| `cursor-move` | `.cursor(CursorHint::Move)` | `<div>` |
| `cursor-grab` | `.cursor(CursorHint::Grab)` | `<div>` |
| `cursor-col-resize` | `.cursor(CursorHint::ResizeCol)` | `<div>` |
| `cursor-row-resize` | `.cursor(CursorHint::ResizeRow)` | `<div>` |
| `text-xs` | `.text_xs()` | `<text>` |
| `text-sm` | `.text_sm()` | `<text>` |
| `text-lg` | `.text_lg()` | `<text>` |
| `text-center` | `.text_center()` | `<text>` |
| `text-right` | `.text_right()` | `<text>` |
| `font-medium` | `.medium()` | `<text>` |
| `font-semibold` | `.semibold()` | `<text>` |
| `font-bold` | `.bold()` | `<text>` |
| `font-mono` | `.mono()` | `<text>` |
| `truncate` | `.truncate()` | `<text>` |
| `leading-none` | `.line_height(1.0)` | `<text>` |
| `leading-tight` | `.line_height(1.25)` | `<text>` |
| `leading-snug` | `.line_height(1.375)` | `<text>` |
| `leading-normal` | `.line_height(1.5)` | `<text>` |
| `leading-relaxed` | `.line_height(1.625)` | `<text>` |
| `leading-loose` | `.line_height(2.0)` | `<text>` |

| Family | Builder call | Values | On | `hover:` |
|---|---|---|---|---|
| `p-*` | `.p(..)` | `N` (N x 4px, halves allowed), `px`, `[13px]`, `[expr]` | `<div>` | - |
| `px-*` | `.px(..)` | `N` (N x 4px, halves allowed), `px`, `[13px]`, `[expr]` | `<div>` | - |
| `py-*` | `.py(..)` | `N` (N x 4px, halves allowed), `px`, `[13px]`, `[expr]` | `<div>` | - |
| `pt-*` | `.pt(..)` | `N` (N x 4px, halves allowed), `px`, `[13px]`, `[expr]` | `<div>` | - |
| `pb-*` | `.pb(..)` | `N` (N x 4px, halves allowed), `px`, `[13px]`, `[expr]` | `<div>` | - |
| `pl-*` | `.pl(..)` | `N` (N x 4px, halves allowed), `px`, `[13px]`, `[expr]` | `<div>` | - |
| `pr-*` | `.pr(..)` | `N` (N x 4px, halves allowed), `px`, `[13px]`, `[expr]` | `<div>` | - |
| `ml-*` | `.margin_left(..)` | `N` (N x 4px, halves allowed), `px`, `[13px]`, `[expr]` | `<div>` | - |
| `gap-*` | `.gap(..)` | `N` (N x 4px, halves allowed), `px`, `[13px]`, `[expr]` | `<div>` | - |
| `gap-x-*` | `.gap_x(..)` | `N` (N x 4px, halves allowed), `px`, `[13px]`, `[expr]` | `<div>` | - |
| `gap-y-*` | `.gap_y(..)` | `N` (N x 4px, halves allowed), `px`, `[13px]`, `[expr]` | `<div>` | - |
| `w-*` | `.w(..)` | `N` (N x 4px, halves allowed), `px`, `[13px]`, `[expr]` | `<div>` | - |
| `h-*` | `.h(..)` | `N` (N x 4px, halves allowed), `px`, `[13px]`, `[expr]` | `<div>` | - |
| `size-*` | `.size(..)` | `N` (N x 4px, halves allowed), `px`, `[13px]`, `[expr]` | `<div>` | - |
| `min-w-*` | `.min_w(..)` | `N` (N x 4px, halves allowed), `px`, `[13px]`, `[expr]` | `<div>` | - |
| `min-h-*` | `.min_h(..)` | `N` (N x 4px, halves allowed), `px`, `[13px]`, `[expr]` | `<div>` | - |
| `max-w-*` | `.max_w(..)` | `N` (N x 4px, halves allowed), `px`, `[13px]`, `[expr]` | `<div>` | - |
| `max-h-*` | `.max_h(..)` | `N` (N x 4px, halves allowed), `px`, `[13px]`, `[expr]` | `<div>` | - |
| `basis-*` | `.basis(..)` | `N` (N x 4px, halves allowed), `px`, `[13px]`, `[expr]` | `<div>` | - |
| `top-*` | `.top(..)` | `N` (N x 4px, halves allowed), `px`, `[13px]`, `[expr]` | `<div>` | - |
| `bottom-*` | `.bottom(..)` | `N` (N x 4px, halves allowed), `px`, `[13px]`, `[expr]` | `<div>` | - |
| `left-*` | `.left(..)` | `N` (N x 4px, halves allowed), `px`, `[13px]`, `[expr]` | `<div>` | - |
| `right-*` | `.right(..)` | `N` (N x 4px, halves allowed), `px`, `[13px]`, `[expr]` | `<div>` | - |
| `inset-*` | `.inset(..)` | `N` (N x 4px, halves allowed), `px`, `[13px]`, `[expr]` | `<div>` | - |
| `grow-*` | `.flex_grow_val(..)` | `[3px]`, `[1.5]`, `[expr]` | `<div>` | - |
| `rounded-*` | `.rounded(..)` | `[3px]`, `[1.5]`, `[expr]` | `<div>` | `.rounded(..)` |
| `border-*` | `.border_w(..)` | `N` points, `[3px]`, `[expr]` | `<div>` | - |
| `opacity-*` | `.opacity(..)` | `N` (N / 100), `[expr]` | `<div>` | `.opacity(..)` |
| `z-*` | `.z_index(..)` | `N`, `[expr]` | `<div>` | - |
| `grid-cols-*` | `.grid_cols_n(..)` | `N` (N > 0), `[expr]` | `<div>` | - |
| `col-span-*` | `.col_span(..)` | `N` (N > 0), `[expr]` | `<div>` | - |
| `row-span-*` | `.row_span(..)` | `N` (N > 0), `[expr]` | `<div>` | - |
| `col-start-*` | `.col_start(..)` | `N`, `[expr]` | `<div>` | - |
| `col-end-*` | `.col_end(..)` | `N`, `[expr]` | `<div>` | - |
| `row-start-*` | `.row_start(..)` | `N`, `[expr]` | `<div>` | - |
| `row-end-*` | `.row_end(..)` | `N`, `[expr]` | `<div>` | - |
| `aspect-*` | `.aspect_ratio(..)` | `[w/h]`, `[expr]` | `<div>` | - |
| `rotate-*` | `.rotate(..)` | `N` degrees, `[expr]` | `<div>` | - |
| `scale-*` | `.scale(..)` | `N` (N / 100), `[expr]` | `<div>` | - |
| `blur-*` | `.blur(..)` | `N` points, `[3px]`, `[expr]` | `<div>` | - |
| `bg-*` | `.bg(..)` | `transparent`, `white`, `black`, `[#rgb]`, `[#rrggbb]`, `[#rrggbbaa]`, `[expr]` | `<div>` | `.bg(..)` |
| `border-*` | `.border(..)` | `transparent`, `white`, `black`, `[#rgb]`, `[#rrggbb]`, `[#rrggbbaa]`, `[expr]` | `<div>` | `.border_color(..)` |
| `border-t-*` | `.border_t(..)` | `transparent`, `white`, `black`, `[#rgb]`, `[#rrggbb]`, `[#rrggbbaa]`, `[expr]` | `<div>` | - |
| `border-r-*` | `.border_r(..)` | `transparent`, `white`, `black`, `[#rgb]`, `[#rrggbb]`, `[#rrggbbaa]`, `[expr]` | `<div>` | - |
| `border-b-*` | `.border_b(..)` | `transparent`, `white`, `black`, `[#rgb]`, `[#rrggbb]`, `[#rrggbbaa]`, `[expr]` | `<div>` | - |
| `border-l-*` | `.border_l(..)` | `transparent`, `white`, `black`, `[#rgb]`, `[#rrggbb]`, `[#rrggbbaa]`, `[expr]` | `<div>` | - |
| `text-*` | `.color(..)` | `transparent`, `white`, `black`, `[#rgb]`, `[#rrggbb]`, `[#rrggbbaa]`, `[expr]` | `<text>` | `.text_color(..)` |
| `text-*` | `.size(..)` | `[3px]`, `[1.5]`, `[expr]` | `<text>` | - |
| `leading-*` | `.line_height(..)` | `[3px]`, `[1.5]`, `[expr]` | `<text>` | - |
| `fill-*` | `.color(..)` | `transparent`, `white`, `black`, `[#rgb]`, `[#rrggbb]`, `[#rrggbbaa]`, `[expr]` | `<icon>` | `.icon_color(..)` |
<!-- class-reference:end -->

## Text

A string literal is text, and `{expr}` inside it is interpolated:
`"Hello, {name}!"` is `format!("Hello, {}!", name)`, `{ratio:.2}` passes a
format spec, and `{{`/`}}` are literal braces. A literal without braces is
passed as is.

`<text>` holds plain text. One `{expr}` child is passed to `text()`
unchanged (it takes `impl Into<String>`); several literal and `{expr}`
children are joined with `format!`: `<text>{count} " unread"</text>`.

`<p>` is selectable rich text. Inline tags style runs of it:

```rust
view! {
    <p size={14.0} width={300.0}>
        "Run " <code>"cargo test"</code> ", then " <b>"read " <i>"the docs"</i></b>
        <br/> <a href={url}>"online"</a>
    </p>
}
```

| Inline tag | `StyledSpan` call |
|---|---|
| `<b>`, `<strong>` | `.bold()` |
| `<i>`, `<em>` | `.italic()` |
| `<code>` | `.code()` |
| `<u>` | `.underline()` |
| `<s>`, `<del>` | `.strikethrough()` |
| `<a href={url}>` | `.link(url)` |
| `<span>` | none; its attributes are span calls: `<span color={c}>` |
| `<br/>` | a `"\n"` span |

Nested tags combine. `{expr}` in `<p>` is formatted with `Display`, and
`{...spans}` adds ready-made `StyledSpan`s. Text must be quoted: Rust's
tokenizer drops whitespace and rejects stray apostrophes, so `<div>Send</div>`
is an error that suggests `"Send"`.

## Children and control flow

| Child | Meaning |
|---|---|
| `<tag>`, `"text"` | One child |
| `{expr}` | One child: an `AnyElement`, any builder (`{div().p(2.0)}`), or text |
| `{?expr}` | An `Option` child |
| `{...expr}` | Every item of an iterator; builder items are converted |
| `if c { .. } else if d { .. } else { .. }` | One branch; no `else` means no child |
| `if let Some(x) = opt { .. }` | Also let chains |
| `match v { A => <tag/> B => { <a/> <b/> } C => expr, }` | Arms take markup, `{ children }`, or a Rust expression |
| `for x in xs { .. }` | Each iteration's children |
| `for x in xs key={x.id} { <div>..</div> }` | Adds `.key(..)` to each iteration's single root |

A branch or arm with several children adds them to the parent directly;
no wrapper `div` changes the layout. `view!` returns the root element; a
root fragment becomes a `div`.

## Components with typed props

`#[derive(Props)]` gives a component a typed builder, so
`<Button on:click={msg} icon={lucide::CHECK} label="Send" />` works and
`Button::builder().on_click(msg).icon(..).label("Send").build()` is the
same thing spelled with builders. Components keep their existing
constructors and builder methods.

```rust
#[derive(Props)]
pub struct Card {
    title: String,                    // required
    #[prop(into)]
    on_close: Action,                 // required, takes impl Into<Action>; on:close sets it
    #[prop(optional, into)]
    subtitle: Option<String>,         // optional; the setter takes impl Into<String>
    #[prop(default = 2)]
    level: u8,
    #[prop(default)]
    selected: bool,                   // a bare `selected` attribute sets it to true
    #[prop(default)]
    children: Vec<AnyElement>,        // the tag's children
}
```

- Leaving out a required prop is "missing required prop `title` on
  `<Card>`" at the tag. A wrongly typed value is a type error at the value,
  and an unknown prop is "no method named `x`" at the attribute.
- Children reach `children` through `Into`, so text stays text until then:
  a `Vec<Child>` (from quark-components) keeps strings, and `Button` uses
  them as its accessible name when it has no label or tooltip.
- Slots work as for builder components: `<.icon>{lucide::X}</.icon>`.
- Enum props take values, not strings: `variant={ButtonStyle::Filled}`.
  A string cannot name an enum variant without knowing the prop's type,
  and the macro does not know it.

`Button`, `Badge`, `Avatar`, `ProgressBar`, `Checkbox`, and `Switch` derive
`Props`; other components use the constructor form `<Name(args) ..>`.
