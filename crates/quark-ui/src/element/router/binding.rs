//! Key bindings: the keymap format, parsed once per thread and shared.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use super::*;

/// One key with its modifiers, such as `mod+shift+p` or `escape`. Parse
/// one from the keymap format with [`str::parse`]; it prints back in that
/// format, modifiers first in a fixed order.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Binding {
    pub mods: Mods,
    /// Lowercase key name: a character (`"s"`, `"/"`) or a named key
    /// (`"enter"`, `"arrowup"`).
    pub key: String,
}

impl Binding {
    pub fn new(mods: Mods, key: &str) -> Self {
        Self {
            mods,
            key: normalize_key(key),
        }
    }

    /// Whether some key press satisfies both bindings. For a pattern and a
    /// pressed key this is "the press triggers the pattern"; for two
    /// patterns it is "they conflict".
    pub fn matches(&self, other: &Binding) -> bool {
        self.key == other.key
            && self.mods.alt == other.mods.alt
            && self.mods.shift == other.mods.shift
            && [(false, false), (true, false), (false, true), (true, true)]
                .into_iter()
                .any(|(cmd, ctrl)| self.accepts(cmd, ctrl) && other.accepts(cmd, ctrl))
    }

    /// Whether a press with Cmd and Ctrl held as given satisfies this
    /// binding's Cmd and Ctrl part.
    fn accepts(&self, cmd: bool, ctrl: bool) -> bool {
        let m = self.mods;
        if m.primary {
            (cmd || ctrl) && (cmd || !m.cmd) && (ctrl || !m.ctrl)
        } else {
            cmd == m.cmd && ctrl == m.ctrl
        }
    }
}

/// A binding string that does not parse; see [`Binding`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingError(String);

impl fmt::Display for BindingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid key binding {:?}", self.0)
    }
}

impl std::error::Error for BindingError {}

impl FromStr for Binding {
    type Err = BindingError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let error = || BindingError(text.to_owned());
        if text.is_empty() || text.contains(char::is_whitespace) {
            return Err(error());
        }
        // A trailing "+" after a separator names the plus key: "ctrl++".
        let (mods_text, key) = match text.strip_suffix("++") {
            Some(mods) => (mods, "+"),
            None if text == "+" => ("", "+"),
            None => text.rsplit_once('+').unwrap_or(("", text)),
        };
        if key.is_empty() {
            return Err(error());
        }
        let mut mods = Mods::default();
        if !mods_text.is_empty() {
            for part in mods_text.split('+') {
                let flag = match part.to_ascii_lowercase().as_str() {
                    "mod" | "primary" => &mut mods.primary,
                    "cmd" | "command" | "super" | "meta" => &mut mods.cmd,
                    "ctrl" | "control" => &mut mods.ctrl,
                    "alt" | "option" | "opt" => &mut mods.alt,
                    "shift" => &mut mods.shift,
                    _ => return Err(error()),
                };
                *flag = true;
            }
        }
        Ok(Self::new(mods, key))
    }
}

impl fmt::Display for Binding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = self.mods;
        for (held, name) in [
            (m.primary, "mod"),
            (m.cmd, "cmd"),
            (m.ctrl, "ctrl"),
            (m.alt, "alt"),
            (m.shift, "shift"),
        ] {
            if held {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(&self.key)
    }
}

/// Bindings stay registered for as long as a view declares them, so a
/// steady frame re-registers the same few strings: parse each once per
/// thread and share it. Bounded so generated bindings cannot grow it
/// without limit.
const PARSED_BINDINGS_MAX: usize = 1024;

pub(super) fn parsed_binding(text: &str) -> Result<Rc<Binding>, BindingError> {
    thread_local! {
        static PARSED: std::cell::RefCell<HashMap<Box<str>, Rc<Binding>>> =
            std::cell::RefCell::new(HashMap::new());
    }
    PARSED.with(|parsed| {
        let mut parsed = parsed.borrow_mut();
        if let Some(binding) = parsed.get(text) {
            return Ok(binding.clone());
        }
        let binding = Rc::new(text.parse::<Binding>()?);
        if parsed.len() >= PARSED_BINDINGS_MAX {
            parsed.clear();
        }
        parsed.insert(text.into(), binding.clone());
        Ok(binding)
    })
}

fn normalize_key(key: &str) -> String {
    let key = key.to_lowercase();
    let canonical = match key.as_str() {
        "esc" => "escape",
        "return" => "enter",
        "up" => "arrowup",
        "down" => "arrowdown",
        "left" => "arrowleft",
        "right" => "arrowright",
        "del" => "delete",
        "pgup" => "pageup",
        "pgdn" => "pagedown",
        " " => "space",
        _ => return key,
    };
    canonical.to_owned()
}
