# Getting started

Add Quark to a project, write the first app, and run the examples.

- An app is a type implementing `quark_app::UiApp`, started with
  `quark_app::run_ui`.
- Each frame the adapter calls `view`, lays out and paints the tree, and
  publishes its accessibility tree.
- Clicks and keys come back to `update` as the app's own action type.

## Add the dependency

Quark is not on crates.io; depend on the repository:

```toml
[dependencies]
quark-app = { git = "https://github.com/seatedro/quark" }

# Optional: the AT-SPI role names and `id` attribute that cua-driver needs on
# Linux. Without it, Linux screen readers still work; cua finds no roles.
[patch.crates-io]
accesskit_unix = { git = "https://github.com/seatedro/quark" }
```

- `[patch]` applies only in the top-level workspace, so the app must declare
  it itself.
- Toolchain and system packages: [Building](../../README.md#building) in the
  root README.

## The first app

The counter in the root README is the smallest complete app (a doctest in
[crates/quark-app/src/lib.rs](../../crates/quark-app/src/lib.rs)).

| Item | Role |
|---|---|
| `type Action = Msg` | The enum elements emit. `impl From<Msg> for Action` lets `on:click={Msg::Increment}` wrap it in the type-erased `quark_ui::Action` |
| `type Message = ()` | Values other threads send; `()` means none ([State, actions, and messages](state-actions-messages.md)) |
| `view(&mut self, cx: &mut ViewContext) -> AnyElement` | Builds the whole tree from `self`, usually with `view!` ([Writing views](writing-views.md)) |
| `update(&mut self, msg, cx)` | Changes state. The adapter redraws after every action; no frame request needed |

- `cx.theme` holds the colors; `cx.frame.size()` is the window size in
  logical points.
- `run_ui` opens one window from `WindowOptions` (title, size, minimum size,
  chrome, icon, fonts, `persist_key` for window state) and returns when the
  app exits.

## Run the examples

```bash
cargo run -p quark-app --example hello_ui
```

All in [crates/quark-app/examples](../../crates/quark-app/examples):

| Example | Shows |
|---|---|
| `hello` | The low-level `App` trait: a hand-built scene, no elements |
| `hello_ui` | A form with a text field and buttons through `UiApp` |
| `controls_demo` | Select, combobox with async options, radio group, segmented control, switch, slider |
| `a11y_demo` | Text field, checkbox, switch, a toast as a live region, an announcement |
| `animation_demo` | Hover transitions and a spring that reverses mid-flight |
| `composer_demo` | A chat composer: completions, atomic chips, attachments, prompt history |
| `chat_demo` | A chat on the block document: 5,000 markdown messages, a streaming answer, selection, find |
| `tree_table_demo` | A lazy file tree and a 100,000-row sortable table |
| `diff_demo` | A diff viewer over two files, a patch file, or a built-in sample |
| `panels_demo` | A docked workspace from `Dock` and `Split` |
| `palette_demo` | Command palette, undo toasts, context menus, hover cards |
| `platform_demo` | Menus, notifications, badge, second window, single instance, window state (needs `--features notifications`) |

## Next

- [Elements and styling](elements-and-styling.md): building views.
- [Testing](testing.md): driving the app from `cargo test`.
