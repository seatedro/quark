# Text and input

Text editing is split between models the app owns and elements that draw
them. `quark_ui::text_input::TextField` (single line) and `Editor`
(multiline) hold the text, caret, selection anchor, IME composition, and
undo log. Both are driven by `TextEditCommand`s and report a
`TextEditOutcome` saying whether the text or selection changed and what to
write to the clipboard. The elements, `text_input(...).field(&model)` and
`text_editor_element`, paint a model and register its hit areas.

## Driving a model

This doctest from [crates/quark-ui/src/lib.rs](../../crates/quark-ui/src/lib.rs)
edits a field without a window:

```rust
use quark_ui::text_input::{TextEditCommand, TextField};

let mut field = TextField::new("");
field.apply(TextEditCommand::InsertText("hello".into()));
field.apply(TextEditCommand::SelectAll);
let cut = field.apply(TextEditCommand::Cut);
assert_eq!(cut.clipboard_write.as_deref(), Some("hello"));
assert_eq!(field.text(), "");

field.apply(TextEditCommand::Undo);
assert_eq!(field.text(), "hello");
```

Positions are `TextOffset`s: byte offsets that sit on grapheme boundaries,
so a caret never lands inside an emoji or a combining sequence. Commands
carry raw byte indices, because they come from pointer hits on an earlier
frame, the app, or assistive tech; the model snaps each one onto its
current text before using it.

Undo groups consecutive edits of the same kind into one step while they
are contiguous, arrive within `COALESCE_PAUSE_MS` (1 second) of each other,
and, for typing, do not start a new word. Time is passed in:
`apply_at(command, now_ms)`, with the app's clock. `apply` keeps the time
of the last `apply_at` (zero before the first), so edits through `apply`
alone never pause long enough to split a step.

## Wiring a field into a `UiApp`

The adapter does the input work once a field has a focus target:

1. Give the element `.focus_target(ID)` and `.focused(cx.is_focused(ID))`.
2. Implement `UiApp::edit_text(target, command)`: apply the command to the
   model for `target` and return the outcome.
3. Implement `UiApp::set_preedit(target, text, cursor)` for IME
   composition, and `set_text_value` for assistive tech that sets the value.

While the field has focus, the adapter turns typed text, IME commits,
editing keys (word and line movement, selection, undo, redo), paste,
copy, cut, and pointer selection (click, drag, double and triple click,
Shift to extend, autoscroll past the edge) into commands. It writes
`clipboard_write` through `UiApp::write_clipboard` and reads paste text
through `read_clipboard`; both default to the system clipboard.
[hello_ui.rs](../../crates/quark-app/examples/hello_ui.rs) shows the whole
wiring.

## IME

An IME composition is shown at the caret without touching the committed
text (`TextEditCommand::SetPreedit`). The adapter tells the window where
the caret is each frame, so the platform places its candidate window there,
and cancels a composition when focus leaves the field
(`TextEditCommand::CancelPreedit`). `quark_ui::text_input::compose` merges
the text and the composition for painting.

## Composer pieces

For chat inputs, `Editor` adds parts an app assembles into a composer:

| Piece | What it does |
|---|---|
| `InlineAtom`, `RichText` | Ranges of the text that edit as one unit (mention chips). The label is ordinary text, so layout and screen readers read it; the caret never rests inside an atom, and an edit touching part of one takes all of it. |
| `TriggerRule`, `find_trigger` | Finds the completion the caret is typing, such as `@` mentions or `/` commands at line start, from the text alone |
| `Completion`, `CompletionProvider` | The popup state: query, items, selection, and keys. A provider answers at once or returns `Answer::Pending` and resolves later; answers for an outdated query are dropped |
| `InputHooks` | Lets the app decide what a paste or drop becomes, such as an attachment instead of text |
| `PromptHistory` | Arrow Up at the start recalls earlier entries; stepping past the newest restores the draft |

[composer_demo.rs](../../crates/quark-app/examples/composer_demo.rs) puts
all of them together, with completion answers arriving through a
`UiSender` as a worker lookup would. Its test module drives the composer
through the test harness.

## Text layout underneath

`quark-text` shapes text with cosmic-text. One `TextLayout` per string and
style serves measurement, hit testing, selection, and painting, kept in a
`LayoutCache` that evicts entries unused for a number of frames, so the
caret a click lands on is the caret that is drawn. Fonts are bundled
(Geist, Geist Mono, Inter, IBM Plex Sans and Mono, Source Sans 3,
JetBrains Mono, Fira Code) with system fonts as fallback, plus Noto Color
Emoji and a Noto Sans CJK subset when the `emoji-font` and `cjk-font`
features are on.
