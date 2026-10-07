//! Key-binding tables with user overrides. The command type is supplied by the
//! app; bindings are strings like `"mod+shift+p"` that parse as a
//! [`Binding`], where `mod` matches either Cmd or Ctrl. A binding may be a
//! sequence of strokes separated by spaces (`"g g"`).
//!
//! Each entry's [`ShortcutScope`] is a key-context predicate (`"editor"`,
//! `"editor && mode == insert"`; see [`quark_ui::key_context`]).
//! [`Keymap::key_bindings`] turns the table into the [`KeyBindings`] the
//! adapter resolves along the focus path.

use quark_ui::Action;
use quark_ui::element::Binding;
use quark_ui::key_context::{KeyBinding, KeyBindings, KeyPredicate};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeymapOverride<C> {
    pub command: C,
    pub binding: String,
}

/// Where a binding applies: a key-context predicate over the focus path,
/// such as `"editor"` or `"pane > editor && mode == insert"`. `GLOBAL`
/// applies everywhere, below every context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShortcutScope(pub &'static str);

impl ShortcutScope {
    pub const GLOBAL: Self = Self("global");

    /// The scope's predicate; `None` for `GLOBAL`. A scope that does not
    /// parse is a programming error: it panics in debug builds and never
    /// matches in release builds.
    pub fn predicate(self) -> Option<KeyPredicate> {
        if self == Self::GLOBAL {
            return None;
        }
        match self.0.parse() {
            Ok(predicate) => Some(predicate),
            Err(error) => {
                debug_assert!(false, "{error}");
                // No context has an empty identifier.
                Some(KeyPredicate::Id(String::new()))
            }
        }
    }

    /// Whether one focus path can satisfy both scopes, so the same key in
    /// both is ambiguous. `GLOBAL` overlaps every scope: a scoped binding
    /// hides a global one wherever it applies.
    pub fn overlaps(self, other: Self) -> bool {
        match (self.predicate(), other.predicate()) {
            (Some(a), Some(b)) => a.overlaps(&b),
            _ => true,
        }
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

    /// Every entry's active bindings as [`KeyBindings`], each under its
    /// scope's predicate, with `action` naming the command's action.
    /// Bindings of several strokes are left out: [`KeyBindings`] matches
    /// single strokes.
    pub fn key_bindings(
        &self,
        overrides: &[KeymapOverride<C>],
        action: impl Fn(C) -> Action,
    ) -> KeyBindings {
        let mut bindings = KeyBindings::new();
        for entry in self.entries() {
            let predicate = entry.scope.predicate();
            for keys in self.active_bindings(overrides, entry.command) {
                if let Ok(keys) = keys.parse::<Binding>() {
                    bindings.push(KeyBinding {
                        keys,
                        predicate: predicate.clone(),
                        action: action(entry.command),
                    });
                }
            }
        }
        bindings
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
    fn scoped_bindings_conflict_only_where_one_context_satisfies_both() {
        const INSERT: ShortcutScope = ShortcutScope("editor && mode == insert");
        const NORMAL: ShortcutScope = ShortcutScope("editor && mode == normal");
        const ENTRIES: &[ShortcutEntry<Command>] = &[
            ShortcutEntry::new(INSERT, Command::Open, &["escape"], "Leave insert"),
            ShortcutEntry::new(NORMAL, Command::Save, &["escape"], "Cancel"),
            ShortcutEntry::new(EDITOR, Command::Next, &["n"], "Next"),
        ];
        const MODAL: Keymap<Command> = Keymap::new(&[ShortcutGroup {
            title: "Modal",
            entries: ENTRIES,
        }]);
        let conflict = |command, binding| {
            let entry = MODAL.entry(command).unwrap();
            MODAL
                .binding_conflict(&[], entry, binding)
                .map(|entry| entry.command)
        };

        assert_eq!(conflict(Command::Open, "escape"), None);
        // Plain `editor` holds in insert mode too.
        assert_eq!(conflict(Command::Next, "escape"), Some(Command::Open));
    }

    #[test]
    fn key_bindings_carry_overrides_and_scopes() {
        #[derive(Debug, Clone, PartialEq)]
        struct Fired(Command);
        let mut overrides = Vec::new();
        set_override(&mut overrides, Command::Save, "mod+shift+s".into());
        let bindings = KEYMAP.key_bindings(&overrides, |command| Action::new(Fired(command)));

        let fire = |pressed: &str, context: &[&str]| {
            let path: Vec<_> = context
                .iter()
                .map(|text| quark_ui::key_context::ContextEntry::parse(text))
                .collect();
            let pressed: Binding = pressed.parse().unwrap();
            bindings
                .resolve(&pressed, &path)
                .and_then(|found| found.binding.action.downcast_ref::<Fired>().cloned())
                .map(|Fired(command)| command)
        };

        assert_eq!(fire("ctrl+s", &[]), None);
        assert_eq!(fire("ctrl+shift+s", &[]), Some(Command::Save));
        assert_eq!(fire("n", &["workspace"]), None);
        assert_eq!(fire("n", &["workspace", "editor"]), Some(Command::Next));
        assert_eq!(fire("ctrl+enter", &["text-field"]), Some(Command::Submit));
    }

    #[test]
    fn format_binding_uses_display_names() {
        assert_eq!(format_binding("shift+/"), "?");
        assert_eq!(format_binding("ctrl+shift+arrowup"), "Ctrl+Shift+Up");
        assert_eq!(format_binding("g g"), "G then G");
    }
}
