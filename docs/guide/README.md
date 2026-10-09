# Quark guide

Building an app with Quark, one topic per page.

- Snippets are doctests or example excerpts; each names the file that compiles it.
- Crates, platform support, feature flags, and build requirements: root [README](../../README.md).

| Page | Covers |
|---|---|
| [Getting started](getting-started.md) | The dependency, the first `UiApp`, the examples |
| [Elements and styling](elements-and-styling.md) | Constructors, `Styled`, tokens, themes, color blending, points and pixels, transitions, scrolling, overlays, devtools |
| [Writing views](writing-views.md) | `view!` markup beside its builders, typed props, the class reference |
| [State, actions, and messages](state-actions-messages.md) | App state, typed actions, `UiSender`, focus, redraws, key bindings, signals |
| [Text and input](text-and-input.md) | `TextField`, `Editor`, IME, undo, composer pieces |
| [Lists and documents](lists-and-documents.md) | Virtual lists, the block document, selection, find, trees, tables, diffs |
| [Terminal](terminal.md) | `quark-terminal`: building, PTY wiring, Ghostty styles and cell metrics |
| [Syntax highlighting and grammar packs](syntax-packs.md) | `GrammarStore`, building packs, signed indexes, the threat model |
| [Accessibility and automation](accessibility-and-automation.md) | The AccessKit tree, ids, AT-SPI coverage, keyboard coverage |
| [Performance model](performance.md) | Redraw triggers, cache boundaries, profiling |
| [Docking across windows](docking.md) | `DockWindows`: tear-off, closing and quitting, saved workspaces, platform differences |
| [Platform services](platform-services.md) | Menus, notifications, badges, tray, dialogs, deep links, single instance, window state |
| [Testing](testing.md) | `UiTestHarness`, pixel probes, allocation counts, scripted terminals |
