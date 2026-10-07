//! Staged installs. A downloaded artifact becomes an [`InstallPlan`] for the
//! way this copy of the app was installed, and the plan runs as a small
//! helper script that waits for the app's process to exit, replaces the app,
//! and relaunches it.
//!
//! | Platform | Artifact | Installed as | Plan |
//! |---|---|---|---|
//! | Linux | AppImage | AppImage (`$APPIMAGE`) | move the new file over the old one |
//! | Linux | deb, rpm | anything | unsupported: the package manager updates it |
//! | macOS | dmg | inside `Name.app` | mount, replace the bundle with `ditto`, detach; admin prompt when the folder is not writable |
//! | Windows | NSIS | anything | run the installer with `/S` |
//! | Windows | MSI | anything | `msiexec /i <msi> /quiet /norestart` |
//!
//! Anything else (an AppImage update for an app not running from an
//! AppImage, a dmg for an unbundled binary, an artifact for another OS) is
//! [`Unsupported`], and the app should point the user at a manual download.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::manifest::Format;

/// Facts about the running app that decide how it can be replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallEnv {
    /// `std::env::consts::OS`.
    pub os: &'static str,
    /// The running executable.
    pub exe: PathBuf,
    /// `$APPIMAGE`: the AppImage this process was started from, on Linux.
    pub appimage: Option<PathBuf>,
    /// Whether the directory holding the `.app` bundle is writable without
    /// elevation, on macOS.
    pub bundle_dir_writable: bool,
}

impl InstallEnv {
    /// The environment of the running process.
    pub fn current() -> std::io::Result<Self> {
        let exe = std::env::current_exe()?;
        let bundle_dir_writable = macos_bundle(&exe)
            .and_then(|app| app.parent().map(dir_is_writable))
            .unwrap_or(false);
        Ok(Self {
            os: std::env::consts::OS,
            appimage: std::env::var_os("APPIMAGE")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            exe,
            bundle_dir_writable,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallPlan {
    ReplaceAppImage {
        new: PathBuf,
        installed: PathBuf,
    },
    CopyFromDmg {
        dmg: PathBuf,
        /// The installed `Name.app`; the dmg must hold a bundle of the same
        /// name.
        bundle: PathBuf,
        needs_admin: bool,
    },
    RunInstaller {
        program: PathBuf,
        args: Vec<String>,
        /// Started after the installer succeeds.
        relaunch: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Unsupported {
    #[error("a {format:?} update cannot install on {os}")]
    WrongPlatform { format: Format, os: String },
    #[error("AppImage updates need the app to run from an AppImage")]
    NotAnAppImage,
    #[error("dmg updates need the app to run from an .app bundle")]
    NotABundle,
    #[error("{0:?} packages update through the system package manager")]
    PackageManaged(Format),
}

/// How to install `artifact` (a verified download of `format`) over the app
/// described by `env`.
pub fn plan(format: Format, artifact: &Path, env: &InstallEnv) -> Result<InstallPlan, Unsupported> {
    let wrong = || Unsupported::WrongPlatform {
        format,
        os: env.os.to_owned(),
    };
    match (env.os, format) {
        (_, Format::Deb | Format::Rpm) => Err(Unsupported::PackageManaged(format)),
        ("linux", Format::AppImage) => match &env.appimage {
            Some(installed) => Ok(InstallPlan::ReplaceAppImage {
                new: artifact.to_path_buf(),
                installed: installed.clone(),
            }),
            None => Err(Unsupported::NotAnAppImage),
        },
        ("macos", Format::Dmg) => match macos_bundle(&env.exe) {
            Some(bundle) => Ok(InstallPlan::CopyFromDmg {
                dmg: artifact.to_path_buf(),
                bundle,
                needs_admin: !env.bundle_dir_writable,
            }),
            None => Err(Unsupported::NotABundle),
        },
        ("windows", Format::Nsis) => Ok(InstallPlan::RunInstaller {
            program: artifact.to_path_buf(),
            args: vec!["/S".into()],
            relaunch: env.exe.clone(),
        }),
        ("windows", Format::Msi) => Ok(InstallPlan::RunInstaller {
            program: "msiexec".into(),
            args: vec![
                "/i".into(),
                artifact.to_string_lossy().into_owned(),
                "/quiet".into(),
                "/norestart".into(),
            ],
            relaunch: env.exe.clone(),
        }),
        _ => Err(wrong()),
    }
}

/// The innermost `*.app` directory holding `exe`.
fn macos_bundle(exe: &Path) -> Option<PathBuf> {
    exe.ancestors()
        .find(|p| p.extension().is_some_and(|ext| ext == "app"))
        .map(Path::to_path_buf)
}

fn dir_is_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".quark-update-probe-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(probe);
            true
        }
        Err(_) => false,
    }
}

impl InstallPlan {
    /// The helper script: wait for process `pid` to exit, install, relaunch.
    /// POSIX `sh` for AppImage and dmg plans, PowerShell for installers.
    pub fn script(&self, pid: u32) -> String {
        match self {
            InstallPlan::ReplaceAppImage { new, installed } => format!(
                "set -eu\n\
                 while kill -0 {pid} 2>/dev/null; do sleep 0.2; done\n\
                 chmod 0755 {new}\n\
                 mv -f {new} {installed}\n\
                 nohup {installed} >/dev/null 2>&1 &\n",
                new = sh_quote(new),
                installed = sh_quote(installed),
            ),
            InstallPlan::CopyFromDmg { dmg, bundle, .. } => {
                let name = bundle.file_name().unwrap_or_default();
                format!(
                    "set -eu\n\
                     while kill -0 {pid} 2>/dev/null; do sleep 0.2; done\n\
                     mount=$(mktemp -d /tmp/quark-update.XXXXXX)\n\
                     hdiutil attach {dmg} -mountpoint \"$mount\" -nobrowse -readonly -quiet\n\
                     trap 'hdiutil detach \"$mount\" -quiet -force || true' EXIT\n\
                     test -d \"$mount\"/{name}\n\
                     rm -rf {bundle}\n\
                     ditto \"$mount\"/{name} {bundle}\n\
                     xattr -dr com.apple.quarantine {bundle} || true\n\
                     open {bundle}\n",
                    dmg = sh_quote(dmg),
                    bundle = sh_quote(bundle),
                    name = sh_quote(Path::new(name)),
                )
            }
            InstallPlan::RunInstaller {
                program,
                args,
                relaunch,
            } => {
                let args = args
                    .iter()
                    .map(|a| ps_quote(a))
                    .collect::<Vec<_>>()
                    .join(",");
                format!(
                    "$ErrorActionPreference = 'Stop'\n\
                     Wait-Process -Id {pid} -ErrorAction SilentlyContinue\n\
                     $p = Start-Process -FilePath {program} -ArgumentList @({args}) -Wait -PassThru\n\
                     if ($p.ExitCode -ne 0) {{ exit $p.ExitCode }}\n\
                     Start-Process -FilePath {relaunch}\n",
                    program = ps_quote(&program.to_string_lossy()),
                    relaunch = ps_quote(&relaunch.to_string_lossy()),
                )
            }
        }
    }

    /// Write the helper script next to `staging_dir` and start it detached.
    /// It waits for this process to exit, so call this when the app is about
    /// to return from `main`, never from a worker thread that then exits.
    pub fn spawn(&self, staging_dir: &Path) -> std::io::Result<()> {
        let pid = std::process::id();
        let script = self.script(pid);
        match self {
            InstallPlan::RunInstaller { .. } => {
                let path = staging_dir.join("install.ps1");
                std::fs::write(&path, script)?;
                let mut command = Command::new("powershell");
                command.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]);
                command.arg(&path);
                detach(&mut command);
                command.spawn().map(drop)
            }
            InstallPlan::CopyFromDmg {
                needs_admin: true, ..
            } => {
                let path = staging_dir.join("install.sh");
                std::fs::write(&path, script)?;
                let shell = format!("/bin/sh {}", sh_quote(&path));
                let apple_script = format!(
                    "do shell script \"{}\" with administrator privileges",
                    shell.replace('\\', "\\\\").replace('"', "\\\"")
                );
                let mut command = Command::new("osascript");
                command.args(["-e", &apple_script]);
                detach(&mut command);
                command.spawn().map(drop)
            }
            _ => {
                let path = staging_dir.join("install.sh");
                std::fs::write(&path, script)?;
                let mut command = Command::new("/bin/sh");
                command.arg(&path);
                detach(&mut command);
                command.spawn().map(drop)
            }
        }
    }
}

fn detach(command: &mut Command) {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
    }
    // Its own process group, so signals sent to the app's group (a terminal
    // closing) do not reach the helper.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
}

/// One `sh` word: single quotes, with embedded quotes closed and escaped.
fn sh_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

/// One PowerShell literal string.
fn ps_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(os: &'static str, exe: &str, appimage: Option<&str>, writable: bool) -> InstallEnv {
        InstallEnv {
            os,
            exe: exe.into(),
            appimage: appimage.map(PathBuf::from),
            bundle_dir_writable: writable,
        }
    }

    fn describe(result: Result<InstallPlan, Unsupported>) -> String {
        match result {
            Ok(InstallPlan::ReplaceAppImage { new, installed }) => {
                format!("replace {} with {}", installed.display(), new.display())
            }
            Ok(InstallPlan::CopyFromDmg {
                dmg,
                bundle,
                needs_admin,
            }) => format!(
                "copy {} from {}{}",
                bundle.display(),
                dmg.display(),
                if needs_admin { " as admin" } else { "" }
            ),
            Ok(InstallPlan::RunInstaller {
                program,
                args,
                relaunch,
            }) => format!(
                "run {} {} then {}",
                program.display(),
                args.join(" "),
                relaunch.display()
            ),
            Err(error) => format!("unsupported: {error}"),
        }
    }

    #[test]
    fn install_decision_table() {
        let appimage_env = env(
            "linux",
            "/tmp/.mount_x/usr/bin/hello",
            Some("/home/u/Hello.AppImage"),
            true,
        );
        let linux_plain = env("linux", "/usr/bin/hello", None, true);
        let mac_user = env(
            "macos",
            "/Users/u/Applications/Hello.app/Contents/MacOS/hello",
            None,
            true,
        );
        let mac_system = env(
            "macos",
            "/Applications/Hello.app/Contents/MacOS/hello",
            None,
            false,
        );
        let mac_cli = env("macos", "/usr/local/bin/hello", None, true);
        let windows = env(
            "windows",
            r"C:\Users\u\AppData\Local\Hello\hello.exe",
            None,
            true,
        );

        let cases = [
            (
                Format::AppImage,
                "new.AppImage",
                &appimage_env,
                "replace /home/u/Hello.AppImage with new.AppImage",
            ),
            (
                Format::AppImage,
                "new.AppImage",
                &linux_plain,
                "unsupported: AppImage updates need the app to run from an AppImage",
            ),
            (
                Format::Deb,
                "new.deb",
                &linux_plain,
                "unsupported: Deb packages update through the system package manager",
            ),
            (
                Format::Rpm,
                "new.rpm",
                &appimage_env,
                "unsupported: Rpm packages update through the system package manager",
            ),
            (
                Format::Dmg,
                "new.dmg",
                &mac_user,
                "copy /Users/u/Applications/Hello.app from new.dmg",
            ),
            (
                Format::Dmg,
                "new.dmg",
                &mac_system,
                "copy /Applications/Hello.app from new.dmg as admin",
            ),
            (
                Format::Dmg,
                "new.dmg",
                &mac_cli,
                "unsupported: dmg updates need the app to run from an .app bundle",
            ),
            (
                Format::Nsis,
                "setup.exe",
                &windows,
                r"run setup.exe /S then C:\Users\u\AppData\Local\Hello\hello.exe",
            ),
            (
                Format::Msi,
                "new.msi",
                &windows,
                r"run msiexec /i new.msi /quiet /norestart then C:\Users\u\AppData\Local\Hello\hello.exe",
            ),
            (
                Format::Dmg,
                "new.dmg",
                &windows,
                "unsupported: a Dmg update cannot install on windows",
            ),
            (
                Format::Nsis,
                "setup.exe",
                &appimage_env,
                "unsupported: a Nsis update cannot install on linux",
            ),
            (
                Format::AppImage,
                "new.AppImage",
                &mac_user,
                "unsupported: a AppImage update cannot install on macos",
            ),
        ];
        for (format, artifact, env, expected) in cases {
            assert_eq!(
                describe(plan(format, Path::new(artifact), env)),
                expected,
                "{format:?} on {env:?}"
            );
        }
    }

    // Regression: diffy formatted paths with `{:?}`, which leaves `$` and
    // backticks live inside the shell's double quotes.
    #[test]
    fn helper_scripts_quote_paths_literally() {
        let plan = InstallPlan::ReplaceAppImage {
            new: "/tmp/it's $HOME.AppImage".into(),
            installed: "/opt/`x`/Hello.AppImage".into(),
        };
        assert_eq!(
            plan.script(42),
            "set -eu\n\
             while kill -0 42 2>/dev/null; do sleep 0.2; done\n\
             chmod 0755 '/tmp/it'\\''s $HOME.AppImage'\n\
             mv -f '/tmp/it'\\''s $HOME.AppImage' '/opt/`x`/Hello.AppImage'\n\
             nohup '/opt/`x`/Hello.AppImage' >/dev/null 2>&1 &\n"
        );

        let plan = InstallPlan::RunInstaller {
            program: r"C:\Users\O'Neil\setup.exe".into(),
            args: vec!["/S".into()],
            relaunch: r"C:\Apps\hello.exe".into(),
        };
        assert_eq!(
            plan.script(7),
            "$ErrorActionPreference = 'Stop'\n\
             Wait-Process -Id 7 -ErrorAction SilentlyContinue\n\
             $p = Start-Process -FilePath 'C:\\Users\\O''Neil\\setup.exe' -ArgumentList @('/S') -Wait -PassThru\n\
             if ($p.ExitCode -ne 0) { exit $p.ExitCode }\n\
             Start-Process -FilePath 'C:\\Apps\\hello.exe'\n"
        );
    }
}
