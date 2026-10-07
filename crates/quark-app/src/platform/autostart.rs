//! Launch the app when the user logs in. Off until the app calls
//! [`Autostart::enable`], which it should do only from a user setting.
//!
//! - **macOS**: a bundled app on macOS 13 or later registers itself as a
//!   login item through `SMAppService.mainAppService`, which lists it under
//!   System Settings > General > Login Items. An unbundled binary, or an
//!   older macOS, gets a LaunchAgent plist in `~/Library/LaunchAgents`.
//! - **Windows**: a value named after [`Autostart::id`] under
//!   `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`. If
//!   the user turns the entry off in Task Manager, Windows records that
//!   under `...\Explorer\StartupApproved\Run` and skips it; quark leaves
//!   that choice alone.
//! - **Linux**: an XDG autostart entry, `$XDG_CONFIG_HOME/autostart/<id>.desktop`.
//!   An AppImage registers the AppImage file (`$APPIMAGE`), not the
//!   temporary mount it runs from. Flatpak apps cannot write that directory
//!   and need the `org.freedesktop.portal.Background` portal instead.

use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Autostart {
    /// Stable and unique, such as `com.example.Notes`: the LaunchAgent
    /// label, the Run value name, and the `.desktop` file stem. ASCII
    /// letters, digits, `-`, `_`, and `.` only.
    pub id: String,
    /// Shown in login item lists.
    pub name: String,
    pub exe: PathBuf,
    /// Arguments for login launches, such as `--hidden`.
    pub args: Vec<String>,
}

impl Autostart {
    /// An entry for the running executable (or its AppImage).
    pub fn current(id: &str, name: &str) -> io::Result<Self> {
        let exe = std::env::var_os("APPIMAGE")
            .filter(|_| cfg!(target_os = "linux"))
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .map_or_else(std::env::current_exe, Ok)?;
        Ok(Self {
            id: id.to_owned(),
            name: name.to_owned(),
            exe,
            args: Vec::new(),
        })
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn enable(&self) -> io::Result<()> {
        self.check_id()?;
        imp::enable(self)
    }

    pub fn disable(&self) -> io::Result<()> {
        self.check_id()?;
        imp::disable(self)
    }

    pub fn is_enabled(&self) -> bool {
        self.check_id().is_ok() && imp::is_enabled(self)
    }

    /// The XDG autostart `.desktop` file.
    pub fn xdg_desktop_entry(&self) -> String {
        use super::desktop_entry::{escape_value, quote_exec_arg};
        let exec = std::iter::once(self.exe.to_string_lossy().into_owned())
            .chain(self.args.iter().cloned())
            .map(|arg| quote_exec_arg(&arg))
            .collect::<Vec<_>>()
            .join(" ");
        format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name={}\n\
             Exec={}\n\
             Terminal=false\n\
             NoDisplay=true\n\
             X-GNOME-Autostart-enabled=true\n",
            escape_value(&self.name),
            escape_value(&exec),
        )
    }

    /// The LaunchAgent property list.
    pub fn launch_agent_plist(&self) -> String {
        let args: String = std::iter::once(self.exe.to_string_lossy().into_owned())
            .chain(self.args.iter().cloned())
            .map(|arg| format!("\t\t<string>{}</string>\n", xml_escape(&arg)))
            .collect();
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n\
             <dict>\n\
             \t<key>Label</key>\n\
             \t<string>{}</string>\n\
             \t<key>ProgramArguments</key>\n\
             \t<array>\n\
             {args}\
             \t</array>\n\
             \t<key>RunAtLoad</key>\n\
             \t<true/>\n\
             \t<key>ProcessType</key>\n\
             \t<string>Interactive</string>\n\
             </dict>\n\
             </plist>\n",
            xml_escape(&self.id),
        )
    }

    /// The command line stored in the Windows Run key, quoted so
    /// `CommandLineToArgvW` splits it back into `exe` and `args`.
    pub fn run_key_command(&self) -> String {
        std::iter::once(self.exe.to_string_lossy().into_owned())
            .chain(self.args.iter().cloned())
            .map(|arg| windows_quote(&arg))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Ids become file names and registry value names.
    fn check_id(&self) -> io::Result<()> {
        let valid = !self.id.is_empty()
            && !self.id.starts_with('.')
            && self
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        if valid {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid autostart id {:?}", self.id),
            ))
        }
    }
}

fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// One argument as `CommandLineToArgvW` parses it: quoted when it is empty
/// or holds whitespace or quotes, with backslashes doubled only where they
/// precede a quote.
fn windows_quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '"']) {
        return arg.to_owned();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            c => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn xdg_autostart_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|home| !home.is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
        })
        .map(|config| config.join("autostart"))
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn launch_agent_path(id: &str) -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(|home| {
            PathBuf::from(home)
                .join("Library")
                .join("LaunchAgents")
                .join(format!("{id}.plist"))
        })
}

#[cfg_attr(windows, allow(dead_code))]
fn write_file(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, contents)
}

#[cfg_attr(windows, allow(dead_code))]
fn remove_file(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

#[cfg_attr(windows, allow(dead_code))]
fn no_home() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "no home directory")
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;

    fn path(entry: &Autostart) -> io::Result<PathBuf> {
        Ok(xdg_autostart_dir()
            .ok_or_else(no_home)?
            .join(format!("{}.desktop", entry.id)))
    }

    pub(super) fn enable(entry: &Autostart) -> io::Result<()> {
        write_file(&path(entry)?, &entry.xdg_desktop_entry())
    }

    pub(super) fn disable(entry: &Autostart) -> io::Result<()> {
        remove_file(&path(entry)?)
    }

    pub(super) fn is_enabled(entry: &Autostart) -> bool {
        path(entry).is_ok_and(|p| p.exists())
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use objc2::runtime::AnyClass;
    use objc2_service_management::{SMAppService, SMAppServiceStatus};

    /// `SMAppService` exists from macOS 13 and only knows bundled apps.
    fn main_app_service(entry: &Autostart) -> Option<objc2::rc::Retained<SMAppService>> {
        let bundled = entry
            .exe
            .ancestors()
            .any(|p| p.extension().is_some_and(|ext| ext == "app"));
        if !bundled || AnyClass::get(c"SMAppService").is_none() {
            return None;
        }
        // SAFETY: the class exists (checked above); the call has no
        // preconditions.
        Some(unsafe { SMAppService::mainAppService() })
    }

    fn ns_error(error: objc2::rc::Retained<objc2_foundation::NSError>) -> io::Error {
        io::Error::other(error.localizedDescription().to_string())
    }

    fn plist_path(entry: &Autostart) -> io::Result<PathBuf> {
        launch_agent_path(&entry.id).ok_or_else(no_home)
    }

    pub(super) fn enable(entry: &Autostart) -> io::Result<()> {
        match main_app_service(entry) {
            // SAFETY: a valid service object.
            Some(service) => unsafe { service.registerAndReturnError() }.map_err(ns_error),
            None => write_file(&plist_path(entry)?, &entry.launch_agent_plist()),
        }
    }

    pub(super) fn disable(entry: &Autostart) -> io::Result<()> {
        // Also clear a plist an older build may have written.
        remove_file(&plist_path(entry)?)?;
        match main_app_service(entry) {
            // SAFETY: a valid service object.
            Some(service) if (unsafe { service.status() }) == SMAppServiceStatus::Enabled => {
                // SAFETY: a valid service object.
                unsafe { service.unregisterAndReturnError() }.map_err(ns_error)
            }
            _ => Ok(()),
        }
    }

    pub(super) fn is_enabled(entry: &Autostart) -> bool {
        match main_app_service(entry) {
            // SAFETY: a valid service object.
            Some(service) => (unsafe { service.status() }) == SMAppServiceStatus::Enabled,
            None => plist_path(entry).is_ok_and(|p| p.exists()),
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::platform::deep_link::registry;

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    pub(super) fn enable(entry: &Autostart) -> io::Result<()> {
        registry::set_string(RUN_KEY, Some(&entry.id), &entry.run_key_command())
    }

    pub(super) fn disable(entry: &Autostart) -> io::Result<()> {
        registry::delete_value(RUN_KEY, &entry.id)
    }

    pub(super) fn is_enabled(entry: &Autostart) -> bool {
        registry::current_user_string(RUN_KEY, &entry.id).is_some()
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod imp {
    use super::*;

    pub(super) fn enable(_: &Autostart) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "autostart is not supported here",
        ))
    }

    pub(super) fn disable(_: &Autostart) -> io::Result<()> {
        Ok(())
    }

    pub(super) fn is_enabled(_: &Autostart) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(exe: &str, args: &[&str]) -> Autostart {
        Autostart {
            id: "com.example.Notes".into(),
            name: "Notes & Co".into(),
            exe: exe.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
        }
    }

    #[test]
    fn xdg_entry_quotes_exec() {
        assert_eq!(
            entry("/home/u/Apps/My Notes.AppImage", &["--hidden", "100%"]).xdg_desktop_entry(),
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Notes & Co\n\
             Exec=\"/home/u/Apps/My Notes.AppImage\" --hidden 100%%\n\
             Terminal=false\n\
             NoDisplay=true\n\
             X-GNOME-Autostart-enabled=true\n"
        );
    }

    #[test]
    fn launch_agent_escapes_arguments() {
        let plist = entry(
            "/Applications/Notes & Co.app/Contents/MacOS/notes",
            &["<hidden>"],
        )
        .launch_agent_plist();
        let dict = plist.split_once("<dict>\n").unwrap().1;
        assert_eq!(
            dict,
            "\t<key>Label</key>\n\
             \t<string>com.example.Notes</string>\n\
             \t<key>ProgramArguments</key>\n\
             \t<array>\n\
             \t\t<string>/Applications/Notes &amp; Co.app/Contents/MacOS/notes</string>\n\
             \t\t<string>&lt;hidden&gt;</string>\n\
             \t</array>\n\
             \t<key>RunAtLoad</key>\n\
             \t<true/>\n\
             \t<key>ProcessType</key>\n\
             \t<string>Interactive</string>\n\
             </dict>\n\
             </plist>\n"
        );
    }

    #[test]
    fn run_key_command_round_trips_through_windows_argv_rules() {
        for (args, expected) in [
            (&[][..], r#""C:\Program Files\Notes\notes.exe""#),
            (
                &["--hidden"],
                r#""C:\Program Files\Notes\notes.exe" --hidden"#,
            ),
            (&[""], r#""C:\Program Files\Notes\notes.exe" """#),
            (
                &[r#"say "hi""#],
                r#""C:\Program Files\Notes\notes.exe" "say \"hi\"""#,
            ),
            (
                &[r"C:\dir with space\"],
                r#""C:\Program Files\Notes\notes.exe" "C:\dir with space\\""#,
            ),
            (
                &[r#"a\"b"#],
                r#""C:\Program Files\Notes\notes.exe" "a\\\"b""#,
            ),
        ] {
            assert_eq!(
                entry(r"C:\Program Files\Notes\notes.exe", args).run_key_command(),
                expected,
                "{args:?}"
            );
        }
    }

    #[test]
    fn rejects_ids_that_are_not_file_names() {
        for id in ["", "../x", "a/b", ".hidden", r"a\b"] {
            let entry = Autostart {
                id: id.into(),
                ..entry("/bin/notes", &[])
            };
            assert_eq!(
                entry.enable().unwrap_err().kind(),
                io::ErrorKind::InvalidInput,
                "{id:?}"
            );
            assert!(!entry.is_enabled(), "{id:?}");
        }
    }
}
