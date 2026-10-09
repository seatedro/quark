# Text and input

Text fields, the multiline editor, IME, undo, and the chat composer pieces.

- The app owns the models: `quark_ui::text_input::TextField` (single line)
  and `Editor` (multiline).
- A model holds text, caret, selection anchor, IME composition, and undo
  log.
- Models take `TextEditCommand`s and return a `TextEditOutcome`: whether
  text or selection changed, and what to write to the clipboard.
- Elements paint a model and register its hit areas:
  `text_input(...).field(&model)`, `text_editor_element`.

## Driving a model

From [crates/quark-ui/src/lib.rs](../../crates/quark-ui/src/lib.rs):

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

- Positions are `TextOffset`s: byte offsets on grapheme boundaries, so a
  caret never lands inside an emoji or combining sequence.
- Commands carry raw byte indices (from old pointer hits, the app, or
  assistive tech); the model snaps each onto its current text.

## Undo

- Consecutive edits of the same kind merge into one undo step while they are
  contiguous, arrive within `COALESCE_PAUSE_MS` (1 second) of each other,
  and (for typing) do not start a new word.
- Pass time with `apply_at(command, now_ms)`, from the app's clock.
- `apply` reuses the last `apply_at` time (zero before the first), so edits
  through `apply` alone never split a step.

## Wiring a field into a `UiApp`

1. Give the element `.focus_target(ID)` and `.focused(cx.is_focused(ID))`.
2. Implement `UiApp::edit_text(target, command)`: apply the command to that
   target's model and return the outcome.
3. Implement `UiApp::set_preedit(target, text, cursor)` for IME composition,
   and `set_text_value` for assistive tech that sets the value.

While the field has focus, the adapter turns input into commands:

| Input | Covers |
|---|---|
| Text | Typed text, IME commits |
| Editing keys | Word and line movement, selection, undo, redo |
| Clipboard | Paste, copy, cut |
| Pointer | Click, drag, double and triple click, Shift to extend, autoscroll past the edge |

- Clipboard: `clipboard_write` goes through `UiApp::write_clipboard`, paste
  reads `read_clipboard`. Both default to the system clipboard.
- Full wiring: [hello_ui.rs](../../crates/quark-app/examples/hello_ui.rs).

## IME

- A composition shows at the caret without touching committed text
  (`TextEditCommand::SetPreedit`).
- The adapter reports the caret position each frame, so the platform places
  its candidate window there.
- Focus leaving the field cancels the composition
  (`TextEditCommand::CancelPreedit`).
- `quark_ui::text_input::compose` merges text and composition for painting.
- An element handling composition in `UiApp::event` (such as a terminal)
  gets `InputEvent::ImePreedit` and `InputEvent::ImeCommit`; typed text
  arrives separately as `InputEvent::TextInput`.
- When focus leaves the composing element, the rest of that composition is
  dropped until the next frame resets the platform IME.

## Composer pieces

`Editor` parts for chat inputs:

| Piece | Does |
|---|---|
| `InlineAtom`, `RichText` | Text ranges that edit as one unit (mention chips). The label is ordinary text for layout and screen readers. The caret never rests inside; an edit touching part of one takes all of it |
| `TriggerRule`, `find_trigger` | Finds the completion being typed, such as `@` mentions or `/` commands at line start, from the text alone |
| `Completion`, `CompletionProvider` | Popup state: query, items, selection, keys. A provider answers at once or returns `Answer::Pending`; answers for an outdated query are dropped |
| `InputHooks` | The app decides what a paste or drop becomes, such as an attachment |
| `PromptHistory` | Arrow Up at the start recalls earlier entries; stepping past the newest restores the draft |

- Example:
  [composer_demo.rs](../../crates/quark-app/examples/composer_demo.rs), with
  completions arriving through a `UiSender`.

## Text layout

- `quark-text` shapes with cosmic-text.
- One `TextLayout` per string and style serves measuring, hit testing,
  selection, and painting, so the caret a click lands on is the one drawn.
- Layouts live in a `LayoutCache` that evicts entries unused for a number of
  frames.
- Bundled fonts: Geist, Geist Mono, Inter, IBM Plex Sans and Mono, Source
  Sans 3, JetBrains Mono, Fira Code. System fonts are the fallback.
- Features `emoji-font` and `cjk-font` add Noto Color Emoji and a Noto Sans
  CJK subset.
