# Getting started

A Quark app is a type that implements `quark_app::UiApp`, started with
`quark_app::run_ui`. Each frame the adapter calls `view` for an element
tree, lays it out, paints it, and publishes its accessibility tree; clicks
and keys come back to `update` as the app's own action type.

## Add the dependency

Quark is not on crates.io. Depend on the repository and copy its
accessibility patch:

```toml
[dependencies]
quark-app = { git = "https://github.com/seatedro/quark" }

# Optional: the AT-SPI role names and `id` attribute that cua-driver needs on
# Linux. Without it, Linux screen readers still work; cua finds no roles.
[patch.crates-io]
accesskit_unix = { git = "https://github.com/seatedro/quark" }
```

`[patch]` sections apply only in the top-level workspace, so an app gets the
patched `accesskit_unix` only when it declares the patch itself.
[vendor/accesskit_unix/VENDORED.md](../../vendor/accesskit_unix/VENDORED.md)
describes the change.

The build needs the toolchain and system packages listed under
[Building](../../README.md#building) in the root README.

## The first app

The counter in the root README is the smallest complete app. It is a
doctest in [crates/quark-app/src/lib.rs](../../crates/quark-app/src/lib.rs),
so `cargo test --doc -p quark-app` compiles it. Its parts:

- `type Action = Msg`: the enum elements emit. `impl From<Msg> for Action`
  lets `on_click(Msg::Increment)` wrap it in the type-erased
  `quark_ui::Action` elements carry.
- `type Message = ()`: values other threads send. `()` means none; see
  [State, actions, and messages](state-actions-messages.md).
- `view(&mut self, cx: &mut ViewContext) -> AnyElement` builds the whole
  tree from `self`. `cx.theme` holds the colors; `cx.frame.size()` is the
  window size in logical points.
- `update(&mut self, msg, cx)` changes state. The adapter redraws after
  every action, so `update` does not request a frame.

`run_ui` opens one window described by `WindowOptions` (title, size,
minimum size, chrome, icon, fonts, `persist_key` for window state) and
returns when the app exits.

## Run the examples

Every example is in [crates/quark-app/examples](../../crates/quark-app/examples):

```bash
cargo run -p quark-app --example hello_ui
```

| Example | Shows |
|---|---|
| `hello` | The low-level `App` trait: a scene built by hand, no elements |
| `hello_ui` | A form with a text field and buttons through `UiApp` |
| `controls_demo` | Select, combobox with async options, radio group, segmented control, switch, slider |
| `a11y_demo` | Text field, checkbox, switch, a toast as a live region, an announcement |
| `animation_demo` | Hover transitions and a spring that reverses mid-flight |
| `composer_demo` | A chat composer: completions, atomic chips, attachments, prompt history |
| `chat_demo` | A chat built on the block document: 5,000 markdown messages, a streaming answer, selection, and find |
| `tree_table_demo` | A lazy file tree and a 100,000-row sortable table |
| `diff_demo` | A diff viewer over two files, a patch file, or a built-in sample |
| `panels_demo` | A docked workspace from `Dock` and `Split` |
| `palette_demo` | Command palette, undo toasts, context menus, hover cards |
| `platform_demo` | Menus, notifications, badge, second window, single instance, window state (needs `--features notifications`) |

Debug builds stop after a number of presented frames when
`QUARK_EXIT_AFTER_FRAMES` is set; CI launches `hello_ui` that way as a smoke
test.

## Where to go next

- [Elements and styling](elements-and-styling.md) for building views.
- [Testing](testing.md) for driving the app from `cargo test`; most
  examples carry a test module that does.
