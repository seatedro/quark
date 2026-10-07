//! Key-binding tables with user overrides. The command type is supplied by the
//! app; bindings are strings like `"mod+shift+p"` that parse as a
//! [`Binding`], where `mod` matches either Cmd or Ctrl. A binding may be a
//! sequence of strokes separated by spaces (`"g g"`).

use quark_ui::element::Binding;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeymapOverride<C> {
    pub command: C,
    pub binding: String,
}

/// Where a binding applies. Two bindings conflict only when their scopes
/// overlap; `GLOBAL` overlaps every scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShortcutScope(pub &'static str);

impl ShortcutScope {
    pub const GLOBAL: Self = Self("global");

    pub fn overlaps(self, other: Self) -> bool {
        self == other || self == Self::GLOBAL || other == Self::GLOBAL
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShortcutEntry<C: 'static> {
    pub command: C,
    pub scope: ShortcutScope,
    pub keys: &'static [&'static str],
    pub description: &'static str,
}

impl<C> ShortcutEntry<C> {
    pub const fn new(
        scope: ShortcutScope,
        command: C,
        keys: &'static [&'static str],
        description: &'static str,
    ) -> Self {
        Self {
            command,
            scope,
            keys,
            description,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShortcutGroup<C: 'static> {
    pub title: &'static str,
    pub entries: &'static [ShortcutEntry<C>],
}

/// An app's default bindings, grouped for display.
#[derive(Debug, Clone, Copy)]
pub struct Keymap<C: 'static> {
    groups: &'static [ShortcutGroup<C>],
}

impl<C: Copy + PartialEq> Keymap<C> {
    pub const fn new(groups: &'static [ShortcutGroup<C>]) -> Self {
        Self { groups }
    }

    pub fn groups(&self) -> &'static [ShortcutGroup<C>] {
        self.groups
    }

    pub fn entries(&self) -> impl Iterator<Item = &'static ShortcutEntry<C>> + use<C> {
        self.groups.iter().flat_map(|group| group.entries.iter())
    }

    pub fn entry(&self, command: C) -> Option<&'static ShortcutEntry<C>> {
        self.entries().find(|entry| entry.command == command)
    }

    pub fn active_bindings<'a>(
        &self,
        overrides: &'a [KeymapOverride<C>],
        command: C,
    ) -> Vec<&'a str> {
        if let Some(binding) = override_for(overrides, command) {
            return vec![binding.binding.as_str()];
        }
        self.entry(command)
            .map(|entry| entry.keys.to_vec())
            .unwrap_or_default()
    }

    pub fn binding_matches(
        &self,
        overrides: &[KeymapOverride<C>],
        command: C,
        binding: &str,
    ) -> bool {
        self.active_bindings(overrides, command)
            .iter()
            .any(|candidate| binding_eq(candidate, binding))
    }

    pub fn binding_conflict(
        &self,
        overrides: &[KeymapOverride<C>],
        entry: &ShortcutEntry<C>,
        binding: &str,
    ) -> Option<&'static ShortcutEntry<C>> {
        self.entries().find(|candidate| {
            candidate.command != entry.command
                && candidate.scope.overlaps(entry.scope)
                && self.binding_matches(overrides, candidate.command, binding)
        })
    }
}

pub fn override_for<C: PartialEq>(
    overrides: &[KeymapOverride<C>],
    command: C,
) -> Option<&KeymapOverride<C>> {
    overrides.iter().find(|binding| binding.command == command)
}

pub fn set_override<C: PartialEq>(
    overrides: &mut Vec<KeymapOverride<C>>,
    command: C,
    binding: String,
) {
    if let Some(existing) = overrides
        .iter_mut()
        .find(|existing| existing.command == command)
    {
        existing.binding = binding;
    } else {
        overrides.push(KeymapOverride { command, binding });
    }
}

pub fn reset_override<C: PartialEq>(overrides: &mut Vec<KeymapOverride<C>>, command: C) {
    overrides.retain(|binding| binding.command != command);
}

/// Whether some key sequence triggers both bindings: they have the same
/// number of strokes and each pair [`Binding::matches`]. Strings that do
/// not parse match nothing.
pub fn binding_eq(left: &str, right: &str) -> bool {
    let strokes = |text: &str| {
        text.split_whitespace()
            .map(str::parse::<Binding>)
            .collect::<Result<Vec<_>, _>>()
    };
    match (strokes(left), strokes(right)) {
        (Ok(left), Ok(right)) => {
            !left.is_empty()
                && left.len() == right.len()
                && left.iter().zip(&right).all(|(l, r)| l.matches(r))
        }
        _ => false,
    }
}

pub fn format_binding(binding: &str) -> String {
    if binding.contains(' ') {
        return binding
            .split_whitespace()
            .map(format_binding)
            .collect::<Vec<_>>()
            .join(" then ");
    }
    if let Some(display) = display_shifted_symbol(binding) {
        return display.to_owned();
    }

    let mut parts = binding.split('+').collect::<Vec<_>>();
    let Some(key) = parts.pop() else {
        return binding.to_owned();
    };
    let modifiers = parts
        .into_iter()
        .map(|part| match part {
            "mod" if cfg!(target_os = "macos") => "Cmd".to_owned(),
            "mod" => "Ctrl".to_owned(),
            "cmd" => "Cmd".to_owned(),
            "ctrl" => "Ctrl".to_owned(),
            "alt" => "Alt".to_owned(),
            "shift" => "Shift".to_owned(),
            other => title_case(other),
        })
        .collect::<Vec<_>>();

    if modifiers.is_empty() {
        match key {
            "shift+/" => "?".to_owned(),
            _ => display_key(key),
        }
    } else {
        let key = display_key(key);
        [modifiers, vec![key]].concat().join("+")
    }
}

fn display_key(key: &str) -> String {
    match key {
        "escape" => "Esc".to_owned(),
        "enter" => "Enter".to_owned(),
        "tab" => "Tab".to_owned(),
        "space" => "Space".to_owned(),
        "arrowup" => "Up".to_owned(),
        "arrowdown" => "Down".to_owned(),
        "arrowleft" => "Left".to_owned(),
        "arrowright" => "Right".to_owned(),
        "pagedown" => "Page Down".to_owned(),
        "pageup" => "Page Up".to_owned(),
        key if key.len() == 1 => key.to_ascii_uppercase(),
        key => title_case(key),
    }
}

fn display_shifted_symbol(binding: &str) -> Option<&'static str> {
    match binding {
        "shift+/" => Some("?"),
        "shift+]" => Some("}"),
        "shift+[" => Some("{"),
        "shift+=" => Some("+"),
        "shift+-" => Some("_"),
        _ => None,
    }
}

fn title_case(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Command {
        Open,
        Save,
        Next,
        Submit,
    }

    const EDITOR: ShortcutScope = ShortcutScope("editor");
    const FIELD: ShortcutScope = ShortcutScope("text-field");

    const FILE: &[ShortcutEntry<Command>] = &[
        ShortcutEntry::new(ShortcutScope::GLOBAL, Command::Open, &["mod+o"], "Open"),
        ShortcutEntry::new(ShortcutScope::GLOBAL, Command::Save, &["mod+s"], "Save"),
    ];
    const EDITING: &[ShortcutEntry<Command>] = &[
        ShortcutEntry::new(EDITOR, Command::Next, &["n", "mod+enter"], "Next"),
        ShortcutEntry::new(FIELD, Command::Submit, &["mod+enter"], "Submit"),
    ];
    const KEYMAP: Keymap<Command> = Keymap::new(&[
        ShortcutGroup {
            title: "File",
            entries: FILE,
        },
        ShortcutGroup {
            title: "Editing",
            entries: EDITING,
        },
    ]);

    #[test]
    fn override_replaces_defaults_and_is_checked_for_conflicts() {
        let mut overrides = Vec::new();
        assert!(KEYMAP.binding_matches(&overrides, Command::Save, "cmd+s"));
        assert!(KEYMAP.binding_matches(&overrides, Command::Save, "ctrl+s"));

        set_override(&mut overrides, Command::Save, "mod+n".into());
        assert!(!KEYMAP.binding_matches(&overrides, Command::Save, "cmd+s"));
        assert!(KEYMAP.binding_matches(&overrides, Command::Save, "ctrl+n"));

        // A global binding conflicts with any scope.
        let next = KEYMAP.entry(Command::Next).unwrap();
        let conflict = KEYMAP.binding_conflict(&overrides, next, "mod+o");
        assert_eq!(conflict.map(|entry| entry.command), Some(Command::Open));

        reset_override(&mut overrides, Command::Save);
        assert!(overrides.is_empty());
    }

    #[test]
    fn format_binding_uses_display_names() {
        assert_eq!(format_binding("shift+/"), "?");
        assert_eq!(format_binding("ctrl+shift+arrowup"), "Ctrl+Shift+Up");
        assert_eq!(format_binding("g g"), "G then G");
    }
}
