# State, actions, and messages

Where app state lives, and how input, other threads, and the platform change
it.

- State is ordinary fields on the `UiApp` type; `view` reads them every
  frame.

| Entry point | Called with | When |
|---|---|---|
| `update(action, cx)` | An action an element emitted: click, scroll, drag, key binding, assistive tech | After input |
| `message(message, cx)` | A value another thread sent through a `UiSender` | In send order |
| `app_event(event, cx)` | Theme changes, menu picks, URLs, notification and tray clicks, closed windows and dialogs | As they arrive |
| `event(event, cx)` | Every raw input event, first; return `true` to stop the adapter's handling | Before routing |

- All run on the UI thread.
- Text fields have their own entry points (`edit_text`, `set_preedit`,
  `set_text_value`): [Text and input](text-and-input.md).

## Typed actions

Elements carry `quark_ui::Action`, a type-erased value. Name the app's type
in `type Action` and convert with `From`:

```rust
#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Increment,
}

impl From<Msg> for Action {
    fn from(msg: Msg) -> Self {
        Action::new(msg)
    }
}
```

- The adapter downcasts each action to `UiApp::Action` before `update`.
- Actions of any other type are dropped; components use this for internal
  actions such as `NoopAction`.
- Components that need the app take a mapping into its action type, such as
  `fn(TreeEvent) -> Action`.

## Messages from other threads

`type Message` is what workers send back (socket reads, finished jobs,
search results). From the second doctest in
[crates/quark-app/src/lib.rs](../../crates/quark-app/src/lib.rs):

```rust
fn update(&mut self, _: Refresh, cx: &mut UiContext) {
    self.text = "Loading".into();
    let sender = cx.sender::<Loaded>();
    std::thread::spawn(move || {
        sender.send(Loaded("Loaded 3 items".into()));
    });
}

fn message(&mut self, Loaded(text): Loaded, cx: &mut UiContext) {
    self.text = text;
    cx.window.request_redraw();
}
```

- `cx.sender::<M>()` panics when `M` is not the app's `Message`.
- `UiSender` is `Clone`; give each worker its own.
- `send` returns `false` once the app has exited.
- Messages sent before the window opens arrive right after `init`.
- No redraw after a message: call `cx.window.request_redraw()` when it
  changes something visible.

## Focus

```rust
const NAME_FIELD: FocusId = FocusId::from_key("hello.name");
```

- `ViewContext::is_focused(id)` styles the focused element.
- `UiContext::set_focus(Some(id))` moves focus.
- Tab follows build order, unless `tab_stop` sets one.
- Tab stays inside a scope marked `trap_focus`.
- Example: [hello_ui.rs](../../crates/quark-app/examples/hello_ui.rs).

## When frames are drawn

The adapter redraws when:

- the elements under the pointer change;
- focus moves;
- an action reaches the app;
- a text field changes;
- input moves a scroll handle;
- the theme changes;
- an animation or transition schedules a frame.

Anything else (a message, a timer, a callback) needs
`cx.window.request_redraw()`, or `request_frame_in(duration)` for later.

## Key bindings

- Strings such as `"mod+shift+p"`; `mod` is Cmd on macOS, Ctrl elsewhere.
- `"g g"` is a sequence.
- `quark_app::keymap`: binding tables keyed by the app's command type, with
  user overrides and conflict checks.
- Menu accelerators use the same binding type, so menu and keymap show the
  same shortcut.

## Signals

`quark::reactive` is a signal store modeled on SolidJS, for state derived
from other state. The `UiApp` path does not need it.

- `Signal<T>` is a copyable handle.
- Memos recompute lazily; a memo whose new value equals the old stops the
  change spreading.
- `#[derive(Store)]` turns a struct's fields into signals.
- A write back into the store from inside `Signal::update` panics.
- Propagation details and sharp edges:
  [crates/quark/ARCHITECTURE.md](../../crates/quark/ARCHITECTURE.md).

From [crates/quark/src/lib.rs](../../crates/quark/src/lib.rs):

```rust
use quark::reactive::SignalStore;

let store = SignalStore::new();
let count = store.create(2);
let doubled = store.create_memo(move |s| count.get(s) * 2);
assert_eq!(doubled.get(&store), 4);

count.set(&store, 5);
assert_eq!(doubled.get(&store), 10);
```
