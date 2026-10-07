//! Opening the app from URLs of its own schemes (`notes://open/42`).
//!
//! - **macOS** launches the app, or tells the running one, with a
//!   `kAEGetURL` Apple Event. The runner handles it and delivers the URL as
//!   [`AppEvent::OpenUrls`]. The scheme is declared in the bundle's
//!   `Info.plist` under `CFBundleURLTypes`; nothing is needed at runtime.
//! - **Windows** launches a new process with the URL as an argument, so pair
//!   [`register_url_scheme`] with [`super::single_instance`] to hand it to
//!   the running app.
//! - **Linux** works the same way through a `.desktop` file; see
//!   [`super::desktop_entry`].
//!
//! [`AppEvent::OpenUrls`]: crate::AppEvent::OpenUrls

use std::io;
use std::path::Path;

/// One registry value: the key under `HKEY_CURRENT_USER`, the value name
/// (`None` for the key's default value), and its string data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryValue {
    pub key: String,
    pub name: Option<&'static str>,
    pub data: String,
}

/// The registry values that make `exe` the current user's handler for
/// `scheme` URLs, launched as `"exe" "<url>"`.
pub fn url_scheme_registry_values(
    scheme: &str,
    description: &str,
    exe: &Path,
) -> io::Result<Vec<RegistryValue>> {
    check_scheme(scheme)?;
    let exe = exe.to_string_lossy();
    if exe.contains('"') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the executable path contains a quote",
        ));
    }
    let key = scheme_key(scheme);
    let value = |key: String, name, data: String| RegistryValue { key, name, data };
    Ok(vec![
        value(key.clone(), None, format!("URL:{description}")),
        value(key.clone(), Some("URL Protocol"), String::new()),
        value(format!(r"{key}\DefaultIcon"), None, format!("\"{exe}\",0")),
        value(
            format!(r"{key}\shell\open\command"),
            None,
            format!("\"{exe}\" \"%1\""),
        ),
    ])
}

/// Register `exe` as the current user's handler for `scheme` URLs, under
/// `HKEY_CURRENT_USER\Software\Classes`. No administrator rights needed.
/// Opt in from an installer step or a first-run setting: it replaces any
/// other app's registration of the scheme for this user.
#[cfg(windows)]
pub fn register_url_scheme(scheme: &str, description: &str, exe: &Path) -> io::Result<()> {
    for value in url_scheme_registry_values(scheme, description, exe)? {
        registry::set_string(&value.key, value.name, &value.data)?;
    }
    Ok(())
}

/// Remove a registration made by [`register_url_scheme`].
#[cfg(windows)]
pub fn unregister_url_scheme(scheme: &str) -> io::Result<()> {
    check_scheme(scheme)?;
    registry::delete_tree(&scheme_key(scheme))
}

fn scheme_key(scheme: &str) -> String {
    format!(r"Software\Classes\{scheme}")
}

/// RFC 3986: a letter, then letters, digits, `+`, `-`, or `.`.
fn check_scheme(scheme: &str) -> io::Result<()> {
    let mut chars = scheme.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if valid {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{scheme:?} is not a URL scheme"),
        ))
    }
}

#[cfg(windows)]
pub(crate) mod registry {
    use std::io;

    use windows::Win32::Foundation::WIN32_ERROR;
    #[cfg(feature = "autostart")]
    use windows::Win32::System::Registry::RegDeleteKeyValueW;
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
        RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegGetValueW, RegSetValueExW,
    };
    use windows::core::{HSTRING, PCWSTR};

    fn check(status: WIN32_ERROR) -> io::Result<()> {
        if status.is_ok() {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(status.0 as i32))
        }
    }

    pub(crate) fn set_string(key: &str, name: Option<&str>, data: &str) -> io::Result<()> {
        let key = HSTRING::from(key);
        let name = name.map(HSTRING::from);
        let data: Vec<u16> = data.encode_utf16().chain([0]).collect();
        let mut hkey = HKEY::default();
        // SAFETY: every pointer outlives the call, and the key is closed.
        unsafe {
            check(RegCreateKeyExW(
                HKEY_CURRENT_USER,
                &key,
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut hkey,
                None,
            ))?;
            let bytes = std::slice::from_raw_parts(data.as_ptr().cast::<u8>(), data.len() * 2);
            let result = check(RegSetValueExW(
                hkey,
                name.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr())),
                None,
                REG_SZ,
                Some(bytes),
            ));
            let _ = RegCloseKey(hkey);
            result
        }
    }

    pub(crate) fn delete_tree(key: &str) -> io::Result<()> {
        let key = HSTRING::from(key);
        // SAFETY: the key name outlives the call.
        unsafe { check(RegDeleteTreeW(HKEY_CURRENT_USER, &key)) }
    }

    /// Delete one value under `HKEY_CURRENT_USER\<key>`. Missing is fine.
    #[cfg(feature = "autostart")]
    pub(crate) fn delete_value(key: &str, name: &str) -> io::Result<()> {
        let key = HSTRING::from(key);
        let name = HSTRING::from(name);
        // SAFETY: both strings outlive the call.
        match unsafe { check(RegDeleteKeyValueW(HKEY_CURRENT_USER, &key, &name)) } {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }

    /// A string value under `HKEY_CURRENT_USER\<key>`.
    #[cfg(feature = "autostart")]
    pub(crate) fn current_user_string(key: &str, name: &str) -> Option<String> {
        get_string(HKEY_CURRENT_USER, key, name)
    }

    /// A string value under `HKEY_LOCAL_MACHINE\<key>`.
    pub(crate) fn local_machine_string(key: &str, name: &str) -> Option<String> {
        get_string(HKEY_LOCAL_MACHINE, key, name)
    }

    fn get_string(root: HKEY, key: &str, name: &str) -> Option<String> {
        let key = HSTRING::from(key);
        let name = HSTRING::from(name);
        let mut buf = [0u16; 1024];
        let mut size = std::mem::size_of_val(&buf) as u32;
        // SAFETY: `size` is the buffer's length in bytes, and the strings
        // outlive the call.
        unsafe {
            check(RegGetValueW(
                root,
                &key,
                &name,
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut size),
            ))
            .ok()?;
        }
        let len = (size as usize / 2).saturating_sub(1);
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}

#[cfg(target_os = "macos")]
pub(crate) use macos::listen;

#[cfg(target_os = "macos")]
mod macos {
    use objc2::rc::Retained;
    use objc2::runtime::NSObject;
    use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send, sel};
    use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager};

    use crate::runner::{AppEvent, EventSink};

    /// `kInternetEventClass` and `kAEGetURL`, both `'GURL'`.
    const GET_URL: u32 = u32::from_be_bytes(*b"GURL");
    /// `keyDirectObject`, `'----'`.
    const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");

    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "QuarkGetUrlHandler"]
        #[ivars = EventSink]
        struct GetUrlHandler;

        impl GetUrlHandler {
            #[unsafe(method(handleGetURLEvent:withReplyEvent:))]
            fn handle(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
                // Through msg_send: objc2-foundation gates this method behind
                // its CoreServices feature, a whole extra crate for one call.
                let param: Option<Retained<NSAppleEventDescriptor>> =
                    unsafe { msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT] };
                let url = param.and_then(|param| param.stringValue());
                if let Some(url) = url {
                    self.ivars().send(AppEvent::OpenUrls(vec![url.to_string()]));
                }
            }
        }
    );

    /// Deliver `kAEGetURL` events as [`AppEvent::OpenUrls`]. Called before
    /// the app finishes launching, so the URL that launched it is caught,
    /// and again once it has, in case AppKit installed its own handler in
    /// between. Replaces the previous handler.
    pub(crate) fn listen(events: &EventSink) {
        let handler = GetUrlHandler::alloc().set_ivars(events.clone());
        let handler: Retained<GetUrlHandler> = unsafe { msg_send![super(handler), init] };
        let manager = NSAppleEventManager::sharedAppleEventManager();
        // SAFETY: the selector names the method above, with the signature
        // the Apple Event Manager calls. msg_send for the same reason as in
        // `handle`.
        unsafe {
            let _: () = msg_send![
                &manager,
                setEventHandler: &*handler,
                andSelector: sel!(handleGetURLEvent:withReplyEvent:),
                forEventClass: GET_URL,
                andEventID: GET_URL
            ];
        }
        // The manager does not retain its handler.
        std::mem::forget(handler);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered(values: &[RegistryValue]) -> String {
        values
            .iter()
            .map(|v| format!("{} [{}] = {}\n", v.key, v.name.unwrap_or("@"), v.data))
            .collect()
    }

    #[test]
    fn registry_values_quote_the_executable_and_url() {
        let values = url_scheme_registry_values(
            "notes",
            "Notes link",
            Path::new(r"C:\Program Files\Notes\notes.exe"),
        )
        .unwrap();
        assert_eq!(
            rendered(&values),
            concat!(
                r"Software\Classes\notes [@] = URL:Notes link",
                "\n",
                r"Software\Classes\notes [URL Protocol] = ",
                "\n",
                r#"Software\Classes\notes\DefaultIcon [@] = "C:\Program Files\Notes\notes.exe",0"#,
                "\n",
                r#"Software\Classes\notes\shell\open\command [@] = "C:\Program Files\Notes\notes.exe" "%1""#,
                "\n",
            )
        );
    }

    #[test]
    fn rejects_schemes_and_paths_that_would_corrupt_the_registration() {
        let exe = Path::new(r"C:\notes.exe");
        for scheme in ["", "1notes", "no tes", r"notes\shell", "notes:"] {
            assert!(
                url_scheme_registry_values(scheme, "", exe).is_err(),
                "{scheme:?}"
            );
        }
        assert!(url_scheme_registry_values("web+notes", "", exe).is_ok());
        assert!(url_scheme_registry_values("notes", "", Path::new(r#"C:\a"b.exe"#)).is_err());
    }
}
