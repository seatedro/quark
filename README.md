# Quark

Quark is a native Rust UI framework for desktop apps. It opens windows
with winit, draws with wgpu, shapes text with cosmic-text, and publishes an
accessibility tree with AccessKit. A view is a tree of plain element values
rebuilt every frame; the state behind it (row tables, text layouts,
animations, selection, the element cache) is kept in column tables keyed by
stable identity. Subtrees inside cache boundaries replay their layout and
paint, and tests hold a repeated frame of a cached list to zero
allocations.

## Crates

| Crate | Contents | Quark crates it uses |
|---|---|---|
| [`quark`](crates/quark) | Reactive signals, geometry, hit testing, scene primitives, semantics, focus, document selection, the animation table, style data | `quark-macros` |
| [`quark-macros`](crates/quark-macros) | The `view!` macro and `#[derive(Store)]` | |
| [`quark-text`](crates/quark-text) | Fonts, text shaping, layout, hit testing, and the layout cache | `quark` |
| [`quark-render`](crates/quark-render) | The wgpu renderer for scenes | `quark`, `quark-text` |
| [`quark-syntax`](crates/quark-syntax) | Tree-sitter highlighting with grammars loaded at runtime from signed packs | `quark-update` (feature `download`) |
| [`quark-diff`](crates/quark-diff) | Unified diff parsing, line and word diffs, display projections | |
| [`quark-ui`](crates/quark-ui) | Elements, layout, styling, themes, text input, virtual lists, the block document, markdown, accessibility | `quark`, `quark-text`, `quark-render`, `quark-syntax` (feature `syntax`) |
| [`quark-components`](crates/quark-components) | Buttons, menus, popovers, select, combobox, command palette, split panes, dock, tree, table, diff view | `quark`, `quark-text`, `quark-render`, `quark-ui`, `quark-diff`, `quark-syntax` |
| [`quark-app`](crates/quark-app) | Windows, the event loop, input, `UiApp`, platform services, the test harness | `quark`, `quark-render`, `quark-text`, `quark-ui` (feature `ui`) |

`vendor/accesskit_unix` is a patched copy of AccessKit's Linux adapter; see
[vendor/accesskit_unix/VENDORED.md](vendor/accesskit_unix/VENDORED.md).
`vendor/glyphon`, `vendor/cosmic-text` and `vendor/taffy` are those crates
with allocation and rendering patches; each has a `VENDORED.md` listing
its patches.

## A minimal app

An app implements `UiApp`: `view` builds the element tree, written with
the `view!` macro ([docs/guide/writing-views.md](docs/guide/writing-views.md)),
and `update` handles the actions its elements emit.

```rust
use quark::view;
use quark_app::quark_ui::element::{AnyElement, IntoAnyElement, div, text};
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::Action;
use quark_app::{UiApp, UiContext, ViewContext, WindowOptions};

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Increment,
}

impl From<Msg> for Action {
    fn from(msg: Msg) -> Self {
        Action::new(msg)
    }
}

struct Counter {
    count: u32,
}

impl UiApp for Counter {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let colors = &cx.theme.colors;
        let (width, height) = cx.frame.size();
        view! {
            <div w={width} h={height} class="items-center justify-center gap-3 bg-[colors.background]">
                <text color={colors.text}>"Clicked {self.count} times"</text>
                <div role="button" aria-label="Increment" test_id="counter.increment"
                     on:click={Msg::Increment}
                     class="px-4 h-9 items-center rounded-[8] bg-[colors.accent]
                            hover:bg-[colors.accent_strong]">
                    <text color={colors.text_strong}>"Increment"</text>
                </div>
            </div>
        }
    }

    fn update(&mut self, msg: Msg, _cx: &mut UiContext) {
        match msg {
            Msg::Increment => self.count += 1,
        }
    }
}

fn main() -> Result<(), quark_app::RunError> {
    quark_app::run_ui(
        Counter { count: 0 },
        WindowOptions {
            title: "Counter".into(),
            size: (480.0, 320.0),
            ..WindowOptions::default()
        },
    )
}
```

This example is a doctest in [crates/quark-app/src/lib.rs](crates/quark-app/src/lib.rs),
so `cargo test --doc -p quark-app` compiles it. Larger examples are in
[crates/quark-app/examples](crates/quark-app/examples); run one with
`cargo run -p quark-app --example hello_ui`.

## Guide

[docs/guide](docs/guide/README.md) covers elements and styling, state and
messages, text input, lists and documents, accessibility, the performance
model, platform services, and testing.

## Platform support

| Platform | Graphics backend | What CI checks |
|---|---|---|
| Linux, X11 | Vulkan, GLES fallback | Format, clippy, every feature combination, all tests (lavapipe software Vulkan), end-to-end specs under Xvfb and openbox, fuzz corpus replay, Miri on `quark`, Kani proofs in `quark`, `quark-ui`, `quark-components`, `quark-diff`, and `quark-app` |
| Linux, Wayland | Vulkan, GLES fallback | Compiles as part of the Linux build; nothing runs on a Wayland compositor |
| macOS (Apple silicon) | Metal | Clippy, all tests, `platform_smoke` (native menu bar, badge, window level, edit roles, deep links), a launch of `hello_ui` that must present one frame |
| Windows | DX12 (WARP on the hosted runner) | Clippy, all tests, `platform_smoke`, a launch of `hello_ui` that must present one frame |

The workflows are [.github/workflows/ci.yml](.github/workflows/ci.yml) and
[.github/workflows/e2e.yml](.github/workflows/e2e.yml). Some platform
services differ by desktop; [docs/guide/platform-services.md](docs/guide/platform-services.md)
lists what each one does where.

## Feature flags

`quark-app` is the crate most apps depend on. Its features:

| Feature | Default | Effect |
|---|---|---|
| `ui` | yes | `UiApp`, `run_ui`, and the quark-ui adapter |
| `emoji-font` | yes | Bundles Noto Color Emoji (10.7 MB) as the emoji fallback |
| `cjk-font` | yes | Bundles a 3.7 MB Noto Sans CJK subset as the CJK fallback |
| `syntax` | no | Syntax highlighting in document code blocks, with grammars from local packs ([guide](docs/guide/syntax-packs.md)) |
| `syntax-download` | no | `syntax`, plus downloading grammar packs from a signed index |
| `images` | no | PNG and JPEG decoding for markdown image blocks |
| `notifications` | no | Desktop notifications through `EventContext::notify` |
| `tray` | no | A system tray icon and menu through `EventContext::set_tray` |
| `dialogs` | no | Native open and save dialogs through `EventContext::file_dialog` |
| `clipboard-image` | no | Reading and writing clipboard images |
| `devtools` | no | Frame HUD, element inspector, and live style overrides (`ctrl+shift+h`, `ctrl+shift+i`, `ctrl+shift+l`, or `QUARK_DEVTOOLS=hud,inspector,layout`) |
| `hot-reload` | no | Patches `App::frame` at runtime through subsecond and a Dioxus devserver |
| `test-support` | no | `quark_app::testing`, the headless test harness |
| `headless-render` | no | Pixel readback in the test harness through a headless GPU device |
| `profile-puffin`, `profile-tracy` | no | Profiler scopes around frame phases and text shaping; enable one at a time |

Lower-level crates have their own: `quark-ui` has `syntax`,
`syntax-download`, `images`, `devtools`, `profile`, `test-alloc`, and
`integrity-checks`; `quark-text` has `emoji-font`, `cjk-font`, and
`profile`; `quark-render` has `headless-render`; `quark-syntax` has
`engine` and `download`; `quark` has `integrity-checks`. Each is described
next to its definition in the crate's `Cargo.toml`.

## Building

```bash
cargo run -p quark-app --example hello_ui
```

- Rust: rustup installs the pinned nightly from [rust-toolchain.toml](rust-toolchain.toml).
- Linux: `libxkbcommon-dev libwayland-dev libx11-dev libxcursor-dev libxi-dev libxrandr-dev libdbus-1-dev libgl1-mesa-dev` and a Vulkan or GL driver. No GTK.
- Nix: `nix develop`, or `direnv allow` once.
- `quark-terminal` also needs Zig 0.16 and curl; see its [build.rs](crates/quark-terminal/build.rs) for offline builds.

## Testing

[TEST_BIBLE.md](TEST_BIBLE.md) sets the rules: one behavior per test,
asserted on observable output, deterministic, and fast; non-trivial data
structures check their own invariants with `verify_integrity`.

```bash
cargo test --workspace --features quark-ui/integrity-checks
```

- **Unit and property tests** sit beside the code. `proptest` covers
  invariants of pure logic.
- **`UiTestHarness`** (`quark_app::testing`) runs a `UiApp` with no window,
  event loop, or GPU, and a fake clock. Tests find nodes by role, name,
  accessibility id, or test id, then click, type, and read the
  accessibility tree or painted text.
- **Frame budgets** count allocations per frame with
  `quark_ui::test_alloc`.
- **End-to-end specs** in [e2e/specs](e2e/specs) drive the example apps
  through the AT-SPI tree with the [cua](https://github.com/trycua/cua)
  driver on a private Xvfb desktop. Linux only:
  `cargo build -p quark-app --examples --features ui,notifications && e2e/run.sh`.
- **Fuzzing, Miri, and Kani** run in CI; see the `fuzz`, `miri`, and `kani`
  jobs in [ci.yml](.github/workflows/ci.yml).

[docs/guide/testing.md](docs/guide/testing.md) has the details.

## License

MIT; see [LICENSE](LICENSE). The fonts bundled in
`crates/quark-text/assets/fonts` are under the SIL Open Font License 1.1;
each has its license file beside it. `vendor/accesskit_unix` is AccessKit's
code under MIT OR Apache-2.0, `vendor/glyphon` is glyphon's under MIT,
Apache-2.0, or Zlib, `vendor/cosmic-text` is cosmic-text's under MIT or
Apache-2.0, and `vendor/taffy` is Taffy's under MIT.
