# Accessibility and automation

What the accessibility tree publishes, how to name nodes, and per-platform
coverage.

- Elements publish an accessibility tree every frame through AccessKit.
- AccessKit serves VoiceOver (macOS), UI Automation (Windows), and AT-SPI
  (Linux).
- The same names and roles serve screen readers, the
  [test harness](testing.md), and computer-use agents.

## What elements publish

- A div publishes a node when it has an accessibility role; text elements
  publish label nodes on their own.

From [hello_ui.rs](../../crates/quark-app/examples/hello_ui.rs):

```rust
view! {
    <div accessibility_id={id} role="button" aria-label={label} on:click={msg}>
        <text>{label}</text>
    </div>
}
```

- `role` takes ARIA role names; `accessibility_role={..}` takes any
  `accesskit::Role` (re-exported as
  `quark_ui::accessibility::AccessibilityRole`).
- Published state: name, description, disabled, selected, checked (including
  mixed), expanded, invalid, required, read-only, modal, range values, live
  politeness, focus.
- Text fields, the editor, and selectable text publish their text as
  `TextRun` children with caret and selection.
- They accept `SetTextSelection`, replacing the selection, and setting a
  field's value.
- Components in `quark-components` set their own roles and states.

## Announcements and live regions

- Changes nothing on screen names (a finished background save):
  `cx.announce(text, Politeness::Polite)`.
- On-screen text such as a toast: make its div a live region with
  `.live(politeness)`, and give it a role (`Status` for toasts) so it
  publishes a node.
- Example: [a11y_demo.rs](../../crates/quark-app/examples/a11y_demo.rs).

## Ids

| Builder | Sets |
|---|---|
| `accessibility_id(s)` | The AccessKit author id (the AT-SPI `id` attribute with the patched Linux adapter); `By::id` |
| `id(s)`, `key(k)` | The node's identity, and its identity among reordered siblings (transitions and inspector overrides key on it). The author id when no `accessibility_id` is set |
| `test_id(s)` | `By::test_id`; the author id when nothing else is set. Needs no accessibility node |

- `id`, `test_id`, or `accessibility_id` also makes a clickable div a Tab
  stop.

## Linux and AT-SPI

Set by accesskit_unix 0.22 and accesskit_atspi_common 0.19.

- **Served:** roles, names, descriptions, the states above except invalid
  and expanded, focus, announcement events.
- **Served:** the Text interface (text, caret, selection, setting the
  selection, word and line boundaries) and the Value interface for ranges.
- **Not served:** EditableText. AT-SPI clients cannot set or replace text;
  macOS and Windows clients can.
- **Not served:** the invalid and expanded states, character extents.

- cua-driver needs `GetRoleName` and an `id` attribute to see roles and find
  nodes by id. Quark's patched accesskit_unix serves both.
- The patch applies only to builds that declare it: add the
  `[patch.crates-io]` entry from [Getting started](getting-started.md).

## Keyboard coverage

- A clickable div needs a stable id or a `focus_ring` to be a Tab stop.
- Radio group, segmented control, select, and combobox: one Tab stop each,
  arrow keys inside. Same for context menus and the menu bar.
- `TabBar`: no arrow keys; each tab is its own Tab stop, and Delete closes
  it.

`Dock` tab group (one Tab stop, its active tab):

| Key | Does |
|---|---|
| Left, Right | Select the neighboring tab, moving focus with it |
| Home, End | First, last tab |
| Delete | Close the tab |
| Mod+Shift+Page Up, Page Down | Move the tab into the previous or next group that takes it |

- Moving a tab or group anywhere else (including a new window) needs the
  app's menu; [panels_demo](../../crates/quark-app/examples/panels_demo.rs)
  opens one with Shift+F10 ([Docking across windows](docking.md)).
- Other drag-only interactions have no keyboard alternative.
