//! Interception of pastes and drops, so the app decides what each becomes.

use std::path::Path;

use super::atoms::RichText;

/// What a paste or drop puts in the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Insertion {
    Text(String),
    /// Text with atoms, such as a mention chip.
    Rich(RichText),
    /// Nothing: the app took it (as an attachment, say).
    Nothing,
}

/// The app's policy for pastes and drops, passed to
/// [`super::Editor::apply_with`] and [`super::Editor::drop_path`]. Each
/// method gets what would be inserted and returns what to insert instead;
/// the defaults insert it unchanged. Implement it on the part of the app
/// that keeps attachments, so a hook can move content there and return
/// [`Insertion::Nothing`].
pub trait InputHooks {
    /// A paste. `pasted` is [`Insertion::Rich`] when the clipboard holds
    /// text this app copied with atoms in it, else [`Insertion::Text`].
    fn paste(&mut self, pasted: Insertion) -> Insertion {
        pasted
    }

    /// A file dropped on the window while this editor has focus. Inserts
    /// the path by default.
    fn drop_path(&mut self, path: &Path) -> Insertion {
        Insertion::Text(path.display().to_string())
    }
}

/// The default policy: insert everything as is.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoHooks;

impl InputHooks for NoHooks {}
