# Writing views

`quark::view!` writes element trees as HTML-like markup that expands at
compile time to the same builder calls you would write by hand.

- The builders stay the only API: every attribute is a builder method, every
  class a builder call.
- Markup and builder values nest in each other freely.
- The macro calls whatever `div()`, `text()`, `svg_icon()`,
  `selectable_rich_text()`, `StyledSpan`, and component types are in scope,
  plus the `IntoAnyElement` and `Styled` traits.
- `use quark_ui::element::*;` and `use quark_ui::style::Styled;` cover them.
- Roles, hex colors, and cursor classes expand to `::quark::` paths: the
  crate needs `quark` as a direct dependency.

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
| `<{expr} attr=..>` | A builder value: `(expr)` then one call per attribute, as in `<{self.input} focused={f} />`; with children it closes with `</>` |
| `<name(a, b) attr=..>` | A function returning a builder: `name(a, b)` then one call per attribute, as in `<canvas(paint)/>` or `<popover_panel(theme)>` |
| `<.method>` | Inside a component: each child becomes `.method(child)` |

- Lowercase names without arguments are built-in tags; any other lowercase
  name is an error suggesting the closest one.
- Components start uppercase or are paths (`<widgets::Card>`).
- A closing tag must match its opening tag; `</Card>` may close
  `<widgets::Card>`.

Built-in tags call whatever function of that name is in scope, so a local
binding shadows it: inside `Step::Prose { text, .. } => ..`, `<text>`
calls the bound `text` and fails to compile. Rename the binding
(`text: body`) to use the tag there.

## Attributes

- Every builder method is an attribute.
- Unknown attribute: rustc's "no method named `x`" at that attribute, with
  its "similar name" hint.
- Wrong value type: a type error at the value.
- rust-analyzer hover, go to definition, and completion work through the
  call.

| Written | Expands to |
|---|---|
| `hidden` | `.hidden()` (on a props component: `.hidden(true)`) |
| `gap={8.0}`, `title="Inbox"`, `level=2` | `.gap(8.0)`, `.title("Inbox")`, `.level(2)` |
| `min-w={0.0}` | `.min_w(0.0)`: kebab-case and snake_case are the same name |
| `shadow={(4.0, 2.0, c)}` | `.shadow(4.0, 2.0, c)`: a tuple splats into arguments |
| `gap={@sig}` | `.gap(cx.read(sig))`, with `cx` in scope |
| `bg={if hot { a } else { b }}` | `.bg(if hot { a } else { b })`; with no `else` the call is skipped |
| `@when {cond} { attrs }` | The attributes apply only when `cond` holds |
| `@for pat in iter { attrs }` | The attributes apply once per item, as in `@for key in KEYS { on_key={(key, act)} }` |
| `on:click={a}`, `on:drag={f}`, `on:scroll={b}` | `.on_click(a)`, `.on_drag(f)`, `.on_scroll(b)`: any `on:name` is `.on_name`; `{if ..}` without `else` skips the call |
| `on:key:mod+s={a}` | `.on_key("mod+s", a)`; unknown modifiers fail to compile |
| `role="button"` | `.semantic_role(SemanticRole::Button)`, which also sets the platform role |
| `aria-label`, `aria-description`, `aria-valuetext` | `.accessibility_label`, `_description`, `_value` |
| `aria-selected`, `aria-checked`/`aria-pressed`, `aria-expanded`, `aria-disabled` | `.accessibility_selected`, `_toggled`, `_expanded`, `_disabled`; alone they mean `true` |
| `aria-invalid`, `aria-required`, `aria-readonly`, `aria-multiselectable`, `aria-level`, `aria-rowindex`, `aria-colindex`, `aria-sort`, `aria-live` | The matching `accessibility_*` builder, or `.live` |
| `key={..}`, `id="..."`, `test-id="..."` | `.key(..)`, `.id(..)`, `.test_id(..)` |
| `track_scroll={&handle}`, `scrollbar_visibility={&state}` | `.track_scroll(&handle)`, `.scrollbar_visibility(&state)`: the only handle attributes, because the builders have no others |

- `role` names: `alert`, `alertdialog`, `button`, `cell`, `checkbox`,
  `combobox`, `complementary`, `dialog`, `document`, `grid`, `gridcell`,
  `group`, `heading`, `img`, `label`, `link`, `list`, `listbox`,
  `listitem`, `log`, `menu`, `menubar`, `menuitem`, `navigation`, `option`,
  `progressbar`, `radio`, `radiogroup`, `row`, `separator`, `slider`,
  `spinbutton`, `status`, `switch`, `tab`, `table`, `tablist`, `tabpanel`,
  `textbox`, `toolbar`, `tooltip`, `tree`, `treeitem`, plus quark's
  `scrollarea` and `window`.
- `role={expr}` takes a `SemanticRole`.
- The last `role=` wins: it replaces both roles set earlier (by a helper,
  say), including an `accessibility_role={..}`. A later
  `accessibility_role={..}` changes only the platform role, as in
  `role="menu" accessibility_role={Role::MenuBar}`. `group` and
  `scrollarea` publish no platform node; use
  `accessibility_role={Role::Group}` for one.
- `on:hover` expands to `.on_hover(..)`, which no builder has. Hover is a
  style state: use `hover:` classes or `hover_bg={..}`.
- In `view! { scale, ... }`, `gap`, `p`, `px`, `py`, `pt`, `pb`, `pl`, `pr`,
  and `rounded` on a `div` are multiplied by `scale` and rounded.

## Classes

`class="..."` lists Tailwind-style names, each expanded at compile time to
one builder call. No CSS at runtime.

- Spacing and sizes use Tailwind's 4-point scale: `p-4` is `.p(16.0)`,
  `gap-0.5` is `.gap(2.0)`, `h-px` is `.h(1.0)`.
- Brackets take an exact value or any Rust expression: `w-[320px]`,
  `gap-[6]`, `w-[sidebar_w]`, `bg-[#336699]`, `bg-[colors.surface]`,
  `aspect-[16/9]`. Spaces inside brackets are allowed.
- After `text-` or `border-`, a bracketed expression is a color; a number is
  a size or width.
- `hover:` gathers all hover classes of an element into one
  `.hover(|s| ...)`: `hover:bg-[c] hover:opacity-80`.
- `hover:` covers what `StyleOverride` holds: background, border color, text
  and icon color, opacity, `rounded-[..]`.
- A `hover_bg={..}` after the class replaces it (each `.hover` call replaces
  the last).
- `focus:`, `focus-visible:`, `active:`, `disabled:`, `dark:`,
  `group-hover:` fail to compile and name the alternative:
  `@when {cond} { .. }`, or theme colors for dark mode.
- Breakpoints (`sm:` to `2xl:`) fail too: no viewport media queries; branch
  on the window size in the view.
- An unknown class fails with the closest known class. A class for another
  element kind (`text-sm` on a `div`) says which element it styles.
- The error spans the whole `class` literal (stable Rust cannot point inside
  a string); the message names the class.

The tables below are generated from the macro's own tables.

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
| `whitespace-nowrap` | `.no_wrap()` | `<text>` |
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

- A string literal is text; `{expr}` inside it is interpolated:
  `"Hello, {name}!"` is `format!("Hello, {}!", name)`.
- `{ratio:.2}` passes a format spec; `{{` and `}}` are literal braces.
- A literal without braces is passed as is.
- `<text>` holds plain text. One `{expr}` child goes to `text()` unchanged
  (`impl Into<String>`).
- Several literal and `{expr}` children are joined with `format!`:
  `<text>{count} " unread"</text>`.
- Text must be quoted: Rust's tokenizer drops whitespace and rejects stray
  apostrophes. `<div>Send</div>` is an error suggesting `"Send"`.

`<p>` is selectable rich text; inline tags style runs:

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

- Nested tags combine.
- `{expr}` in `<p>` is formatted with `Display`; `{...spans}` adds
  ready-made `StyledSpan`s.

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
| `for x in xs key={..} { match x { A => <a/> B => <b/> } }` | The body may instead be one `if` or `match` (after any `let`s); the key goes on the root of the branch taken. A branch holds one root or none; several are an error |
| `let name = expr;` | Binds `name` for the children after it in the same list, branch, or loop body |

- A keyed `for` drops the key on a `<text>`, `<p>`, `<icon>`, or `<spacer>`
  root, which have no `.key`, so those items stay unkeyed; wrap one in a
  `<div>` to key it.
- A branch or arm with several children adds them to the parent directly; no
  wrapper `div`.
- `if`, `match`, and `for` lower to plain Rust around `.child(..)` calls,
  allocating nothing beyond the children.
- `view!` returns the root element; a root fragment becomes a `div`.

## Returning a builder

- `view!` returns an `AnyElement`.
- `view! { -> Type, <root> }` returns the root's builder instead, checked
  against `Type`, so callers can keep styling it and use the helper as a
  function tag.
- The root must be one element: a fragment, control flow, or `{expr}` at the
  root is an error.

```rust
fn menu_panel(p: &Pal, x: f32, y: f32) -> Div {
    view! { -> Div,
        <div class="absolute flex-col p-1 rounded-[10]" left={x} top={y} bg={p.menu} />
    }
}

view! { <menu_panel(p, x, y) h={h}>{...rows}</menu_panel> }
```

## Components with typed props

`#[derive(Props)]` gives a component a typed builder:
`<Button on:click={msg} icon={lucide::CHECK} label="Send" />` is
`Button::builder().on_click(msg).icon(..).label("Send").build()`. Existing
constructors and builder methods stay.

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

- Missing required prop: "missing required prop `title` on `<Card>`" at the
  tag.
- Wrong type: a type error at the value. Unknown prop: "no method named `x`"
  at the attribute.
- Children reach `children` through `Into`, so text stays text until then. A
  `Vec<Child>` (quark-components) keeps strings; `Button` uses them as its
  accessible name when it has no label or tooltip.
- Slots work as for builder components: `<.icon>{lucide::X}</.icon>`.
- Enum props take values, not strings: `variant={ButtonStyle::Filled}` (the
  macro does not know the prop's type).
- `Button`, `Badge`, `Avatar`, `ProgressBar`, `Checkbox`, `Switch` derive
  `Props`; other components use `<Name(args) ..>`.
