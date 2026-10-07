//! Linux `.desktop` files that register an app as the handler for its URL
//! schemes. Nothing here installs the file; an installer or packaging step
//! writes it (for example with [`DesktopEntry::write_to`] and
//! [`user_applications_dir`]) and then runs
//! `xdg-mime default <id>.desktop x-scheme-handler/<scheme>`.
//!
//! The desktop environment launches `Exec` with the URLs appended, so pair
//! this with [`super::single_instance`] to hand them to the running app.

use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DesktopEntry {
    /// File stem, conventionally a reverse domain name: `com.example.Notes`.
    pub id: String,
    pub name: String,
    /// The executable to launch.
    pub exec: PathBuf,
    /// An icon theme name or an absolute path.
    pub icon: Option<String>,
    pub comment: Option<String>,
    /// URL schemes to handle, without `://`: `["notes"]`.
    pub schemes: Vec<String>,
    /// Freedesktop menu categories: `["Office"]`.
    pub categories: Vec<String>,
}

impl DesktopEntry {
    pub fn file_name(&self) -> String {
        format!("{}.desktop", self.id)
    }

    /// The file's contents.
    pub fn render(&self) -> String {
        let mut out = String::from("[Desktop Entry]\nType=Application\n");
        push_key(&mut out, "Name", &escape_value(&self.name));
        let mut exec = quote_exec_arg(&self.exec.to_string_lossy());
        if !self.schemes.is_empty() {
            exec.push_str(" %U");
        }
        push_key(&mut out, "Exec", &escape_value(&exec));
        if let Some(icon) = &self.icon {
            push_key(&mut out, "Icon", &escape_value(icon));
        }
        if let Some(comment) = &self.comment {
            push_key(&mut out, "Comment", &escape_value(comment));
        }
        push_key(&mut out, "Terminal", "false");
        if !self.categories.is_empty() {
            push_key(&mut out, "Categories", &list(&self.categories));
        }
        if !self.schemes.is_empty() {
            let mime: Vec<String> = self
                .schemes
                .iter()
                .map(|scheme| format!("x-scheme-handler/{scheme}"))
                .collect();
            push_key(&mut out, "MimeType", &list(&mime));
        }
        out
    }

    /// Write `<dir>/<id>.desktop`, creating `dir` if needed.
    pub fn write_to(&self, dir: &Path) -> io::Result<PathBuf> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(self.file_name());
        std::fs::write(&path, self.render())?;
        Ok(path)
    }
}

/// `$XDG_DATA_HOME/applications`, else `~/.local/share/applications`.
pub fn user_applications_dir() -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|home| !home.is_empty())
                .map(|home| PathBuf::from(home).join(".local").join("share"))
        })?;
    Some(data.join("applications"))
}

fn push_key(out: &mut String, key: &str, value: &str) {
    out.push_str(key);
    out.push('=');
    out.push_str(value);
    out.push('\n');
}

/// The spec's `string` escapes, applied to every value.
fn escape_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

fn list(items: &[String]) -> String {
    let mut out = String::new();
    for item in items {
        out.push_str(&escape_value(item).replace(';', "\\;"));
        out.push(';');
    }
    out
}

/// Quote one `Exec` argument by the spec's rules: reserved characters force
/// double quotes, inside which `"`, `` ` ``, `$`, and `\` take a backslash.
/// A literal `%` is always doubled so it is not read as a field code.
fn quote_exec_arg(arg: &str) -> String {
    let arg = arg.replace('%', "%%");
    let reserved = |c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '"' | '\''
                    | '\\'
                    | '>'
                    | '<'
                    | '~'
                    | '|'
                    | '&'
                    | ';'
                    | '$'
                    | '*'
                    | '?'
                    | '#'
                    | '('
                    | ')'
                    | '`'
            )
    };
    if !arg.chars().any(reserved) {
        return arg;
    }
    let mut out = String::from("\"");
    for c in arg.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_scheme_handler_with_quoted_exec() {
        let entry = DesktopEntry {
            id: "com.example.Notes".into(),
            name: "Notes".into(),
            exec: "/opt/My Notes/notes$1%".into(),
            icon: Some("notes".into()),
            comment: None,
            schemes: vec!["notes".into(), "notes-dev".into()],
            categories: vec!["Office".into()],
        };

        assert_eq!(
            entry.render(),
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Notes\n\
             Exec=\"/opt/My Notes/notes\\\\$1%%\" %U\n\
             Icon=notes\n\
             Terminal=false\n\
             Categories=Office;\n\
             MimeType=x-scheme-handler/notes;x-scheme-handler/notes-dev;\n"
        );
    }
}
