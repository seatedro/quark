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

## End-to-end specs with cua

[e2e/run.sh](../../e2e/run.sh) runs each spec against a real example app on
a private X11 desktop: Xvfb, openbox, a session D-Bus with the AT-SPI bus
enabled, and the [cua](https://github.com/trycua/cua) driver, pinned and
checksummed by [e2e/install-cua.sh](../../e2e/install-cua.sh). Each spec
gets a fresh desktop and a fresh copy of its app.

```bash
cargo build -p quark-app --examples --features ui,notifications
e2e/install-cua.sh
e2e/run.sh                              # every spec
e2e/run.sh e2e/specs/hello_ui/*.py      # some specs
```

A spec lives at `e2e/specs/<example>/<behavior>.py` and runs against the
binary of that name, from `QUARK_E2E_BIN_DIR` (default
`target/debug/examples`) or else from `target/debug`, where package
binaries such as `workbench` and `codex-demo` land:

```bash
cargo build -p quark-workbench --bin workbench
e2e/run.sh e2e/specs/workbench/*.py
```

The Workbench and Codex demos link `quark-terminal`, so building them
needs Zig 0.16 (see [Terminal](terminal.md#building)). The runner starts
apps without arguments; a spec sets its app's environment with header
lines, applied before launch:

```python
# quark-e2e-env: QUARK_WORKBENCH_SCENARIO=stress
```

It also exports `QUARK_SYNTAX_PACKS=target/syntax-packs` when that
directory exists, so code shows highlighted once the
[grammar packs](syntax-packs.md#building-packs) are built.
[theme_motion.py](../../e2e/specs/workbench/theme_motion.py) reads
screenshots with Pillow (`python3-pil`), which the runner does not check
for.

[e2e/quark_e2e.py](../../e2e/quark_e2e.py) gives a spec two clients:

- `Cua` drives the app as a computer-use agent does: snapshots of the
  AT-SPI tree, clicks by element or id (`press`, `press_id`), keys, the
  clipboard, and screenshots.
- `atspi_tree` reads the raw AT-SPI tree over D-Bus for what cua does not
  report: states such as focused, attributes such as `id`, and the Text
  interface.

Specs wait by polling observable state against a deadline (`wait_for`),
never by sleeping. Each starts with a docstring naming the regression it
catches; [greet_click.py](../../e2e/specs/hello_ui/greet_click.py) is a
short one. A failed spec leaves `screen.png`, `tree.txt`, `cua-tree.txt`,
and every log under `target/e2e/artifacts/<example>-<behavior>/`, with
`meta.txt` naming the spec, its environment, and the screen, and CI
uploads them. `QUARK_E2E_KEEP=1` keeps the artifacts of passing specs
too, for reviewing what the app looked like.

The specs run on Linux only, in the `e2e` workflow
([.github/workflows/e2e.yml](../../.github/workflows/e2e.yml)).
