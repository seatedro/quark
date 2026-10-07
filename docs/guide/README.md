# Quark guide

How to build an app with Quark, one topic per page. Each page links to the
code it describes; snippets are copies of doctests or excerpts of examples,
and each names the file that compiles it.

| Page | Covers |
|---|---|
| [Getting started](getting-started.md) | Adding Quark to a project, the first `UiApp`, running the examples |
| [Elements and styling](elements-and-styling.md) | Element constructors, `Styled`, design tokens, themes, transitions, scrolling, `view!` |
| [State, actions, and messages](state-actions-messages.md) | `UiApp` state, typed actions, `UiSender`, focus, redraws, signals |
| [Text and input](text-and-input.md) | `TextField`, `Editor`, IME, undo, and the composer pieces |
| [Lists and documents](lists-and-documents.md) | Virtual lists, the block document, selection, find, trees, tables, diffs |
| [Accessibility and automation](accessibility-and-automation.md) | The AccessKit tree, AT-SPI coverage, ids, end-to-end specs with cua |
| [Performance model](performance.md) | Cache boundaries, frame memory reuse, allocation budgets, profiling |
| [Platform services](platform-services.md) | Menus, notifications, badges, tray, dialogs, deep links, single instance, window state |
| [Testing](testing.md) | The test bible, `UiTestHarness`, pixel probes, end-to-end specs, fuzzing, Miri, Kani |

The root [README](../../README.md) lists the crates, platform support,
feature flags, and build requirements.
