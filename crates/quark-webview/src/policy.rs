//! Canonical origins and the navigation policy every backend asks before a
//! document may load.
//!
//! Origins compare as exact `(scheme, host, port)` tuples after URL parsing
//! and IDNA canonicalization; there is no prefix, suffix, or wildcard
//! matching.

use std::fmt;

use url::{Host, Url};

/// An `https` origin: scheme, canonical host, and effective port.
///
/// Displays as the browser serializes `window.location.origin`: the default
/// port is omitted and international hosts are in punycode.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Origin {
    host: String,
    port: u16,
}

/// Why a string is not an [`Origin`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum OriginError {
    #[error("not a valid URL")]
    InvalidUrl,
    /// Only `https` origins are supported.
    #[error("only https origins are supported")]
    Scheme,
    #[error("an origin carries no user name or password")]
    Credentials,
    /// The string has a path, query, or fragment beyond the origin.
    #[error("an origin has no path, query, or fragment")]
    NotAnOrigin,
}

impl Origin {
    /// Parse an origin such as `https://example.com` or
    /// `https://login.example.com:8443`. A trailing `/` is accepted; any
    /// other path, a query, or a fragment is rejected.
    pub fn parse(origin: &str) -> Result<Self, OriginError> {
        let url = Url::parse(origin).map_err(|_| OriginError::InvalidUrl)?;
        let parsed = Self::of(&url)?;
        if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
            return Err(OriginError::NotAnOrigin);
        }
        Ok(parsed)
    }

    /// The origin of `url`, which may have any path.
    pub fn of(url: &Url) -> Result<Self, OriginError> {
        if url.scheme() != "https" {
            return Err(OriginError::Scheme);
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(OriginError::Credentials);
        }
        let host = match url.host() {
            Some(Host::Domain(domain)) if !domain.is_empty() => domain.to_owned(),
            Some(Host::Ipv4(address)) => address.to_string(),
            Some(Host::Ipv6(address)) => format!("[{address}]"),
            _ => return Err(OriginError::InvalidUrl),
        };
        let port = url.port_or_known_default().ok_or(OriginError::InvalidUrl)?;
        Ok(Self { host, port })
    }

    /// The canonical host: lowercase ASCII, punycode for international
    /// names, IPv6 addresses in brackets.
    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.port {
            443 => write!(f, "https://{}", self.host),
            port => write!(f, "https://{}:{port}", self.host),
        }
    }
}

impl fmt::Debug for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Origin({self})")
    }
}

impl std::str::FromStr for Origin {
    type Err = OriginError;

    fn from_str(origin: &str) -> Result<Self, OriginError> {
        Self::parse(origin)
    }
}

/// Why a navigation, popup, or initial URL was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BlockReason {
    /// Not a URL.
    InvalidUrl,
    /// `http`, `file`, `data`, `javascript`, `blob`, `about` (outside a
    /// subframe's blank document), or another browser scheme.
    Scheme,
    /// A user name or password in the URL.
    Credentials,
    /// An `https` origin missing from the navigation allowlist.
    OriginNotAllowed,
    /// A request to open a new window, denied by [`PopupPolicy::Deny`].
    Popup,
    /// A custom scheme the system would hand to another app. Never
    /// forwarded to the operating system.
    ExternalProtocol,
    /// A download, which v1 never starts.
    Download,
}

impl fmt::Display for BlockReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidUrl => "invalid URL",
            Self::Scheme => "scheme not allowed",
            Self::Credentials => "credentials in URL",
            Self::OriginNotAllowed => "origin not allowed",
            Self::Popup => "popup denied",
            Self::ExternalProtocol => "external protocol not opened",
            Self::Download => "download denied",
        })
    }
}

/// What a webview does with a request to open a new window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum PopupPolicy {
    /// Refuse it and report [`BlockReason::Popup`].
    #[default]
    Deny,
    /// Load an allowed URL in the current webview instead. The page loses
    /// `window.opener`, so flows that post back to their opener break.
    NavigateCurrent,
}

/// Where a proposed navigation would load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Constructed by the native backends.
pub(crate) enum Target {
    /// The top-level document: initial loads, links, forms, scripts,
    /// redirects, history, reloads, and response checks.
    MainFrame,
    /// A document in a frame.
    Subframe,
    /// A new window (`window.open`, `target=_blank`).
    NewWindow,
}

/// The policy's answer for one proposed navigation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decision {
    Allow,
    /// A popup to load in the current view instead
    /// ([`PopupPolicy::NavigateCurrent`]).
    LoadInCurrent(Url),
    Block(BlockReason),
}

/// The exact-origin navigation allowlist of one webview. Backends ask it
/// for every top-level and frame navigation, redirect, popup, and document
/// response before accepting it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NavigationPolicy {
    navigation: Vec<Origin>,
    evaluation: Vec<Origin>,
    popups: PopupPolicy,
}

impl NavigationPolicy {
    pub(crate) fn new(
        navigation: Vec<Origin>,
        evaluation: Vec<Origin>,
        popups: PopupPolicy,
    ) -> Self {
        Self {
            navigation,
            evaluation,
            popups,
        }
    }

    /// The origin `url` may load as a document, or why not.
    pub(crate) fn check(&self, url: &Url) -> Result<Origin, BlockReason> {
        match url.scheme() {
            "https" => {}
            scheme if is_browser_scheme(scheme) => return Err(BlockReason::Scheme),
            _ => return Err(BlockReason::ExternalProtocol),
        }
        let origin = Origin::of(url).map_err(|error| match error {
            OriginError::Credentials => BlockReason::Credentials,
            _ => BlockReason::InvalidUrl,
        })?;
        if self.navigation.contains(&origin) {
            Ok(origin)
        } else {
            Err(BlockReason::OriginNotAllowed)
        }
    }

    /// Decide a proposed navigation to `url`, a native engine's URL string.
    pub(crate) fn decide(&self, url: &str, target: Target) -> Decision {
        let Ok(parsed) = Url::parse(url) else {
            return Decision::Block(BlockReason::InvalidUrl);
        };
        // Frames commonly start from an empty document that inherits its
        // parent's origin; it fetches nothing and never runs app scripts.
        if target == Target::Subframe && matches!(url, "about:blank" | "about:srcdoc") {
            return Decision::Allow;
        }
        if let Err(reason) = self.check(&parsed) {
            return Decision::Block(reason);
        }
        match (target, self.popups) {
            (Target::NewWindow, PopupPolicy::Deny) => Decision::Block(BlockReason::Popup),
            (Target::NewWindow, PopupPolicy::NavigateCurrent) => Decision::LoadInCurrent(parsed),
            _ => Decision::Allow,
        }
    }

    /// Whether scripts may run in a document of `origin`.
    pub(crate) fn evaluation_allowed(&self, origin: &Origin) -> bool {
        self.evaluation.contains(origin)
    }
}

/// Schemes a browser handles itself, as opposed to ones the system would
/// pass to another app.
fn is_browser_scheme(scheme: &str) -> bool {
    matches!(
        scheme,
        "http"
            | "file"
            | "data"
            | "javascript"
            | "blob"
            | "about"
            | "ws"
            | "wss"
            | "ftp"
            | "filesystem"
            | "view-source"
    )
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn origins(list: &[&str]) -> Vec<Origin> {
        list.iter()
            .map(|origin| Origin::parse(origin).unwrap())
            .collect()
    }

    fn policy(popups: PopupPolicy) -> NavigationPolicy {
        NavigationPolicy::new(
            origins(&["https://example.com", "https://login.example.net:8443"]),
            origins(&["https://example.com"]),
            popups,
        )
    }

    fn decision(decision: Decision) -> String {
        match decision {
            Decision::Allow => "allow".to_owned(),
            Decision::LoadInCurrent(url) => format!("load {url}"),
            Decision::Block(reason) => format!("block {reason}"),
        }
    }

    #[test]
    fn origins_serialize_as_browsers_do() {
        for (input, canonical) in [
            ("https://EXAMPLE.com", "https://example.com"),
            ("https://example.com:443/", "https://example.com"),
            ("https://example.com:8443", "https://example.com:8443"),
            ("https://bücher.example", "https://xn--bcher-kva.example"),
            ("https://127.0.0.1:9000", "https://127.0.0.1:9000"),
            ("https://[::1]:9000", "https://[::1]:9000"),
            ("https://example.com.", "https://example.com."),
        ] {
            assert_eq!(
                Origin::parse(input).unwrap().to_string(),
                canonical,
                "{input}"
            );
        }
    }

    #[test]
    fn origin_strings_that_are_not_https_origins_are_rejected() {
        for (input, error) in [
            ("example.com", OriginError::InvalidUrl),
            ("https://example.com:99999", OriginError::InvalidUrl),
            ("http://example.com", OriginError::Scheme),
            ("data:text/html,hi", OriginError::Scheme),
            ("https://user:pass@example.com", OriginError::Credentials),
            ("https://example.com/login", OriginError::NotAnOrigin),
            ("https://example.com/?next=1", OriginError::NotAnOrigin),
            ("https://example.com/#top", OriginError::NotAnOrigin),
        ] {
            assert_eq!(Origin::parse(input), Err(error), "{input}");
        }
    }

    #[test]
    fn navigations_load_only_exact_allowed_origins() {
        let policy = policy(PopupPolicy::Deny);
        for (url, target, expected) in [
            ("https://example.com/app?x=1", Target::MainFrame, "allow"),
            ("https://EXAMPLE.com:443/", Target::MainFrame, "allow"),
            (
                "https://login.example.net:8443/sso",
                Target::Subframe,
                "allow",
            ),
            (
                "https://example.com.evil.test/",
                Target::MainFrame,
                "block origin not allowed",
            ),
            (
                "https://evil.test/https://example.com/",
                Target::MainFrame,
                "block origin not allowed",
            ),
            (
                "https://evilexample.com/",
                Target::MainFrame,
                "block origin not allowed",
            ),
            (
                "https://sub.example.com/",
                Target::MainFrame,
                "block origin not allowed",
            ),
            (
                "https://login.example.net/",
                Target::MainFrame,
                "block origin not allowed",
            ),
            (
                "https://example.com:8443/",
                Target::MainFrame,
                "block origin not allowed",
            ),
            (
                "https://example.com@evil.test/",
                Target::MainFrame,
                "block credentials in URL",
            ),
            (
                "https://evil.test@example.com/",
                Target::MainFrame,
                "block credentials in URL",
            ),
            (
                "http://example.com/",
                Target::MainFrame,
                "block scheme not allowed",
            ),
            (
                "javascript:alert(1)",
                Target::MainFrame,
                "block scheme not allowed",
            ),
            (
                "data:text/html,x",
                Target::Subframe,
                "block scheme not allowed",
            ),
            (
                "file:///etc/passwd",
                Target::MainFrame,
                "block scheme not allowed",
            ),
            ("about:blank", Target::MainFrame, "block scheme not allowed"),
            ("about:blank", Target::Subframe, "allow"),
            (
                "devin://callback?code=1",
                Target::MainFrame,
                "block external protocol not opened",
            ),
            ("not a url", Target::MainFrame, "block invalid URL"),
            (
                "https://example.com/new",
                Target::NewWindow,
                "block popup denied",
            ),
            (
                "https://evil.test/",
                Target::NewWindow,
                "block origin not allowed",
            ),
        ] {
            assert_eq!(
                decision(policy.decide(url, target)),
                expected,
                "{url} in {target:?}"
            );
        }
    }

    #[test]
    fn navigate_current_loads_only_allowed_popups_in_place() {
        let policy = policy(PopupPolicy::NavigateCurrent);
        assert_eq!(
            decision(policy.decide("https://example.com/next", Target::NewWindow)),
            "load https://example.com/next"
        );
        assert_eq!(
            decision(policy.decide("https://evil.test/", Target::NewWindow)),
            "block origin not allowed"
        );
    }

    proptest! {
        /// Parsing an origin's own serialization gives the same origin, so a
        /// stored or displayed origin never compares unequal to itself.
        #[test]
        fn origin_serialization_round_trips(
            labels in prop::collection::vec("[a-zA-Z0-9äöüß-]{1,12}", 1..4),
            port in 1u16..,
        ) {
            let input = format!("https://{}:{port}", labels.join("."));
            if let Ok(origin) = Origin::parse(&input) {
                prop_assert_eq!(Origin::parse(&origin.to_string()), Ok(origin));
            }
        }
    }
}
