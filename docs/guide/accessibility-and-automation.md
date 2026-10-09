# Accessibility and automation

Every frame, elements publish an accessibility tree through AccessKit,
which serves it to VoiceOver on macOS, UI Automation on Windows, and AT-SPI
on Linux. The same tree is what the test harness queries and what the
end-to-end specs drive through cua, so one set of names and roles serves
screen readers, tests, and computer-use agents.

## What elements publish

A div publishes a node when it has an accessibility role; text elements
publish label nodes on their own:

```rust
view! {
    <div accessibility_id={id} role="button" aria-label={label} on:click={msg}>
        <text>{label}</text>
    </div>
}
```

This fragment is from [hello_ui.rs](../../crates/quark-app/examples/hello_ui.rs).
`role` takes ARIA role names; `accessibility_role={..}` takes any
`accesskit::Role` (re-exported as
`quark_ui::accessibility::AccessibilityRole`). Nodes publish name,
description, disabled, selected, checked (including mixed), expanded,
invalid, required, read-only, modal, range values, live politeness, and
focus. Text fields, the editor, and selectable text publish their text as
`TextRun` children with the caret and selection, and accept
`SetTextSelection`, replacing the selection, and setting a field's value.
Components in `quark-components` set their own roles and states.

For changes that nothing on screen names (a finished background save), call
`cx.announce(text, Politeness::Polite)`. For text that is on screen, such
as a toast, make its div a live region with `.live(politeness)` and give
it a role (`Status` for toasts) so it publishes a node.
[a11y_demo.rs](../../crates/quark-app/examples/a11y_demo.rs) shows both.

## Ids

| Builder | Used by |
|---|---|
| `accessibility_id(s)` | The AccessKit author id, which the patched Linux adapter exposes as the AT-SPI `id` attribute; `By::id` in tests |
| `id(s)`, `key(k)` | The semantic node's identity and its identity among reordered siblings (transitions and inspector overrides key on it); the author id when no `accessibility_id` is set |
| `test_id(s)` | `By::test_id` in tests; the author id when nothing else is set. Needs no accessibility node |

`id`, `test_id`, or `accessibility_id` also makes a clickable div a Tab
stop.

## Linux and AT-SPI

What AT-SPI clients get is set by accesskit_unix 0.22 and
accesskit_atspi_common 0.19:

- **Served:** roles, names, descriptions, the states above except invalid
  and expanded, the Text interface (text, caret, selection, setting the
  selection, word and line boundaries), the Value interface for ranges,
  focus, and announcement events.
- **Not served:** EditableText (AT-SPI clients cannot set or replace text;
  macOS and Windows clients can), the invalid and expanded states, and
  character extents.

The workspace patches accesskit_unix (in `vendor/accesskit_unix`) to serve
`GetRoleName` and an `id` attribute, which cua-driver needs to see roles
and find nodes by id. The patch applies only to builds of this workspace;
an app depending on Quark adds the same `[patch.crates-io]` entry itself
(see [Getting started](getting-started.md)).
[VENDORED.md](../../vendor/accesskit_unix/VENDORED.md) has the details and
the steps to drop the patch once AccessKit serves both.

## Keyboard coverage

A clickable div needs a stable id or a `focus_ring` to be a Tab stop. The
radio group, segmented control, select, and combobox are one Tab stop each
and move with the arrow keys, as do context menus and the menu bar. A
`TabBar` has no arrow-key navigation, so each tab is its own Tab stop, and
Delete closes it.

A `Dock` tab group is one Tab stop, its active tab. Left and Right select
the neighboring tab and move focus with it, Home and End the first and
last, and Delete closes it. Mod+Shift+Page Up and Page Down move it into
the previous or next group that takes it. Moving a tab or a whole group
anywhere else, a new window included, is the app's menu:
[panels_demo](../../crates/quark-app/examples/panels_demo.rs) opens one
with Shift+F10 (see [Docking across windows](docking.md)). Other drag-only
interactions have no keyboard alternative.
