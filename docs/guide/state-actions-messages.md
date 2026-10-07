# State, actions, and messages

App state is ordinary fields on the type that implements `UiApp`. The view
reads them every frame; three entry points change them:

| Entry point | Called with | Called on |
|---|---|---|
| `update(action, cx)` | An action an element emitted: click, scroll, drag, key binding, or assistive tech | The UI thread, after input |
| `message(message, cx)` | A value another thread sent through a `UiSender` | The UI thread, in send order |
| `app_event(event, cx)` | Theme changes, menu picks, URLs, notification and tray clicks, closed windows and dialogs | The UI thread |

Text fields have their own entry points (`edit_text`, `set_preedit`,
`set_text_value`); see [Text and input](text-and-input.md). `event(event,
cx)` sees every raw input event first and returns `true` to stop the
adapter's own handling.

## Typed actions

Elements carry actions as `quark_ui::Action`, a type-erased value, because
quark-ui does not know the app's types. The app names its type in
`type Action` and converts it with a `From` impl:

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

The adapter downcasts each emitted action to `UiApp::Action` before calling
`update`. Actions of any other type are dropped, which is how components
emit internal actions (such as `NoopAction`) without reaching the app.
Components that need the app's attention take a mapping from their own
event type into the app's action, for example a `fn(TreeEvent) -> Action`.

## Messages from other threads

`type Message` is what threads send back: socket reads, finished jobs,
search results. Get a sender from the context and move it into the worker:

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

This is an excerpt of the second doctest in
[crates/quark-app/src/lib.rs](../../crates/quark-app/src/lib.rs), which
also drives the app through the test harness.

- `cx.sender::<M>()` panics when `M` is not the app's `Message` type.
- `UiSender` is `Clone`; give each worker its own. `send` returns `false`
  once the app has exited.
- Messages sent before the window opens are delivered right after `init`.
- The adapter does not redraw after a message, since many messages change
  nothing visible. Call `cx.window.request_redraw()` when one does.

## Focus

Focus targets are `FocusId`s, usually constants:

```rust
const NAME_FIELD: FocusId = FocusId::from_key("hello.name");
```

`ViewContext::is_focused(id)` styles the focused element, and
`UiContext::set_focus(Some(id))` moves focus. Tab moves between focusable
elements in the order they were built, unless `tab_stop` gives an order,
and stays inside a scope marked `trap_focus`. [hello_ui.rs](../../crates/quark-app/examples/hello_ui.rs)
wires a field this way.

## When frames are drawn

The adapter draws a frame only for a reason it can name: the elements under
the pointer changed, focus moved, an action reached the app, a text field
changed, input moved a scroll handle, or the theme changed. Animations and
transitions schedule their own frames. State changed anywhere else (a
message, a timer, a callback) needs `cx.window.request_redraw()`, or
`request_frame_in(duration)` for a later frame.

## Key bindings

Bindings are strings such as `"mod+shift+p"`, where `mod` is Cmd on macOS
and Ctrl elsewhere, and `"g g"` is a sequence. `quark_app::keymap` holds
binding tables keyed by the app's command type, with user overrides and
conflict checks. Menu accelerators use the same binding type, so a menu
item and the keymap show the same shortcut.

## Signals

`quark::reactive` is a signal store modeled on SolidJS: `Signal<T>` is a
copyable handle, memos recompute lazily, and a memo whose new value equals
its old one stops the change from spreading. `#[derive(Store)]` turns a
struct's fields into signals. The `UiApp` path does not need it; it is
there for state that derives from other state. This doctest from
[crates/quark/src/lib.rs](../../crates/quark/src/lib.rs) shows a memo:

```rust
use quark::reactive::SignalStore;

let store = SignalStore::new();
let count = store.create(2);
let doubled = store.create_memo(move |s| count.get(s) * 2);
assert_eq!(doubled.get(&store), 4);

count.set(&store, 5);
assert_eq!(doubled.get(&store), 10);
```

[crates/quark/ARCHITECTURE.md](../../crates/quark/ARCHITECTURE.md) explains
the propagation states and the sharp edges, such as a write that reaches
back into the store from inside `Signal::update` panicking.
