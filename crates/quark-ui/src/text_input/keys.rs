use super::{InlineStyle, TextEditCommand};
use crate::element::Binding;

/// Something [`command_for_binding`] can read as a [`Binding`]: a binding,
/// or a string in the keymap format, which is parsed.
pub trait AsBinding {
    fn as_binding(&self) -> Option<Binding>;
}

impl AsBinding for Binding {
    fn as_binding(&self) -> Option<Binding> {
        Some(self.clone())
    }
}

impl AsBinding for str {
    fn as_binding(&self) -> Option<Binding> {
        self.parse().ok()
    }
}

impl AsBinding for String {
    fn as_binding(&self) -> Option<Binding> {
        self.parse().ok()
    }
}

/// Editing command for a key binding (`ctrl+z`, `cmd+shift+arrowleft`).
/// `cmd` takes macOS meanings (line start/end on arrows, line delete on
/// backspace); `ctrl` and `alt` move by words; `mod` counts as either for
/// undo, select all, copy, cut, and the inline style toggles (`mod+b`
/// bold, `mod+i` italic, `mod+u` underline, `mod+shift+x` strikethrough,
/// `mod+e` code). Paste is absent because it needs the
/// clipboard, which the app reads.
pub fn command_for_binding(binding: &(impl AsBinding + ?Sized)) -> Option<TextEditCommand> {
    use TextEditCommand::*;
    let binding = binding.as_binding()?;
    let m = binding.mods;
    let (cmd, ctrl, alt, shift) = (m.cmd, m.ctrl, m.alt, m.shift);
    let key = binding.key.as_str();
    let primary = cmd || ctrl || m.primary;
    let word = (ctrl || alt) && !cmd;
    let pick = |plain, select| if shift { select } else { plain };
    Some(match key {
        "z" if primary && !alt => pick(Undo, Redo),
        "y" if ctrl && !shift && !alt => Redo,
        "a" if primary && !shift && !alt => SelectAll,
        "c" if primary && !shift && !alt => Copy,
        "x" if primary && !shift && !alt => Cut,
        "b" if primary && !shift && !alt => ToggleStyle(InlineStyle::BOLD),
        "i" if primary && !shift && !alt => ToggleStyle(InlineStyle::ITALIC),
        "u" if primary && !shift && !alt => ToggleStyle(InlineStyle::UNDERLINE),
        "x" if primary && shift && !alt => ToggleStyle(InlineStyle::STRIKE),
        "e" if primary && !shift && !alt => ToggleStyle(InlineStyle::CODE),
        "arrowleft" if cmd => pick(CursorHome, SelectHome),
        "arrowright" if cmd => pick(CursorEnd, SelectEnd),
        "arrowleft" if word => pick(CursorWordLeft, SelectWordLeft),
        "arrowright" if word => pick(CursorWordRight, SelectWordRight),
        "arrowleft" if !primary && !alt => pick(CursorLeft, SelectLeft),
        "arrowright" if !primary && !alt => pick(CursorRight, SelectRight),
        "arrowup" if !primary && !alt => pick(CursorUp, SelectUp),
        "arrowdown" if !primary && !alt => pick(CursorDown, SelectDown),
        "home" if !alt => pick(CursorHome, SelectHome),
        "end" if !alt => pick(CursorEnd, SelectEnd),
        "backspace" if cmd => BackspaceLine,
        "backspace" if word => BackspaceWord,
        "backspace" => Backspace,
        "delete" if word => DeleteForwardWord,
        "delete" if !cmd => DeleteForward,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use TextEditCommand::*;

    #[test]
    fn undo_redo_and_word_bindings() {
        let cases = [
            ("ctrl+z", Some(Undo)),
            ("cmd+z", Some(Undo)),
            ("ctrl+shift+z", Some(Redo)),
            ("cmd+shift+z", Some(Redo)),
            ("ctrl+y", Some(Redo)),
            ("alt+z", None),
            ("ctrl+arrowleft", Some(CursorWordLeft)),
            ("alt+shift+arrowright", Some(SelectWordRight)),
            ("cmd+arrowleft", Some(CursorHome)),
            ("ctrl+backspace", Some(BackspaceWord)),
            ("cmd+backspace", Some(BackspaceLine)),
            ("shift+arrowleft", Some(SelectLeft)),
            ("mod+z", Some(Undo)),
            ("ctrl+q+z", None),
        ];
        for (binding, expected) in cases {
            assert_eq!(command_for_binding(binding), expected, "{binding}");
        }
    }
}
