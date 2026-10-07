# quark-app

Windows, the event loop, input, and platform services for
[Quark](../../README.md) apps, and the `UiApp` adapter most apps use.

Implement `UiApp` (a `view` that returns an element tree and an
`update` for its actions) and call `run_ui`. The crate docs in
[src/lib.rs](src/lib.rs) have a complete app, a test that drives it
headlessly, and platform service calls, all compiled as doctests.

- [Getting started](../../docs/guide/getting-started.md)
- [Platform services](../../docs/guide/platform-services.md)
- [Testing](../../docs/guide/testing.md)
- [Examples](examples)

Feature flags are listed in the [root README](../../README.md#feature-flags).
