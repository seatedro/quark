//! Browser data stores: a fresh ephemeral store per webview by default, or
//! a named persistent profile an app opts into for one purpose.

use std::fmt;

use crate::PlatformError;

/// A persistent browser profile: the app's identity plus a purpose, such
/// as `("com.example.portal", "devin-sign-in")`. Views share cookies and
/// storage only when they name the exact same profile.
///
/// Both parts are 1 to 64 ASCII letters, digits, `.`, `_`, or `-`, not
/// starting with `.`; backends build directory names from them.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProfileId {
    app: String,
    purpose: String,
}

impl ProfileId {
    pub fn new(app: impl Into<String>, purpose: impl Into<String>) -> Result<Self, ProfileError> {
        let (app, purpose) = (app.into(), purpose.into());
        if !valid_part(&app) || !valid_part(&purpose) {
            return Err(ProfileError::InvalidId);
        }
        Ok(Self { app, purpose })
    }

    pub fn app(&self) -> &str {
        &self.app
    }

    pub fn purpose(&self) -> &str {
        &self.purpose
    }
}

fn valid_part(part: &str) -> bool {
    (1..=64).contains(&part.len())
        && !part.starts_with('.')
        && part
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

impl fmt::Debug for ProfileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProfileId({}/{})", self.app, self.purpose)
    }
}

/// Where a webview keeps cookies, storage, caches, and service workers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum DataStore {
    /// A new isolated store that lives as long as the webview.
    #[default]
    Ephemeral,
    /// A named store that persists across views and launches. One live view
    /// at a time may use it. Needs macOS 14 on macOS.
    Persistent(ProfileId),
}

/// Profile failures from opening a view or [`clear_profile`].
///
/// [`clear_profile`]: crate::service::Service::clear_profile
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ProfileError {
    #[error("invalid profile id")]
    InvalidId,
    /// A live view uses the profile, or it is being cleared.
    #[error("the profile is in use")]
    InUse,
    /// The platform cannot keep separate named profiles (macOS before 14),
    /// or no webview backend is built in.
    #[error("named profiles are not supported here")]
    Unsupported,
    #[error("profile operation failed: {0}")]
    Platform(PlatformError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_ids_reject_path_like_parts() {
        for (app, purpose, valid) in [
            ("com.example.portal", "devin-sign-in", true),
            ("app", "a_b.c-1", true),
            ("", "sign-in", false),
            ("app", "..", false),
            ("app", ".hidden", false),
            ("app", "a/b", false),
            ("app", "a\\b", false),
            ("app", "sign in", false),
            ("app", "ü", false),
            (&"a".repeat(65), "x", false),
        ] {
            assert_eq!(
                ProfileId::new(app, purpose).is_ok(),
                valid,
                "{app:?} {purpose:?}"
            );
        }
    }
}
