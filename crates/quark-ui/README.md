# quark-ui

Elements, layout, styling, themes, text input, virtual lists, the block
transcript, markdown, and accessibility for [Quark](../../README.md).

A view builds a tree of element values every frame (`div`, `text`,
`text_input`, ...) styled through the `Styled` trait; Taffy lays it out
and paint emits a scene. `quark-app`'s `UiApp` runs it in a window.

The [guide](../../docs/guide/README.md) covers each part. The crate docs
in [src/lib.rs](src/lib.rs) have runnable examples.

## Features

| Feature | Effect |
|---|---|
| `syntax` | Highlights transcript code blocks with the built-in grammars |
| `images` | Decodes PNG and JPEG markdown images on a worker |
| `devtools` | The inspector overlay, frame HUD, and live style overrides |
| `integrity-checks` | Full integrity checks after every mutation in debug builds |
| `test-alloc` | `quark_ui::test_alloc`, a counting allocator for frame budget tests |
| `profile` | Profiler scopes around layout and paint |

All are off by default.
