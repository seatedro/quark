//! What each platform's font API answers for the generic UI and monospace
//! families. Queried once per [`crate::TextSystem`] (and settings change),
//! never during layout or rendering.

use std::path::PathBuf;

use super::FontRole;

/// One face or family a platform offers for a generic name, best first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    /// A family name to find among the loaded faces.
    pub(crate) family: Option<String>,
    /// A font file to find among the loaded faces, or to load.
    pub(crate) path: Option<PathBuf>,
    /// Picks a face from a file with several.
    pub(crate) post_script_name: Option<String>,
    /// The platform API's own answer, rather than a well-known family tried
    /// because the API gave none.
    pub(crate) from_api: bool,
}

impl Candidate {
    pub(crate) fn family(name: &str, from_api: bool) -> Self {
        Self {
            family: Some(name.to_owned()),
            path: None,
            post_script_name: None,
            from_api,
        }
    }
}

/// The candidates for `role`, best first. `locale` is a BCP 47 tag such as
/// `en-US`; fontconfig matches with it.
pub(crate) fn candidates(role: FontRole, locale: &str) -> Vec<Candidate> {
    let _ = locale;
    #[cfg(target_os = "macos")]
    return super::coretext::candidates(role);
    #[cfg(target_os = "windows")]
    return windows_candidates(role);
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "android")))]
    return super::fontconfig::candidates(role, locale);
    #[allow(unreachable_code)]
    {
        let _ = role;
        Vec::new()
    }
}

/// Windows' design guidance, by installed family: Segoe UI Variable (the
/// Windows 11 UI face, whose text optical size is the default) and then
/// Segoe UI; Cascadia Mono and then Consolas. The font database already
/// lists the system and per-user font directories DirectWrite reads, so a
/// family not installed is skipped rather than assumed.
#[cfg(target_os = "windows")]
fn windows_candidates(role: FontRole) -> Vec<Candidate> {
    let names: &[&str] = match role {
        FontRole::Ui => &["Segoe UI Variable Text", "Segoe UI Variable", "Segoe UI"],
        FontRole::Mono => &["Cascadia Mono", "Consolas"],
    };
    names
        .iter()
        .map(|name| Candidate::family(name, true))
        .collect()
}
