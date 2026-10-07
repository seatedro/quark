//! The signed update manifest: one JSON document per channel, verified with
//! Ed25519 before any field in it is trusted.
//!
//! ```json
//! {
//!   "payload": {
//!     "app": "dev.quark.hello",
//!     "channel": "stable",
//!     "version": "1.4.0",
//!     "pub_date": "2026-10-01T12:00:00Z",
//!     "notes": "Faster startup.",
//!     "platforms": {
//!       "linux-x86_64": {
//!         "format": "appimage",
//!         "url": "https://example.com/hello_1.4.0_x86_64.AppImage",
//!         "sha256": "9f86d0…",
//!         "size": 48213504
//!       }
//!     }
//!   },
//!   "signature": "<hex Ed25519 signature of the canonical payload>"
//! }
//! ```
//!
//! The signature covers the payload's canonical JSON: object keys sorted by
//! code point (keys are ASCII in practice, where this agrees with
//! JavaScript's UTF-16 sort), no whitespace, strings escaped as `serde_json` does, and
//! integers only (floats format differently across languages). Verification
//! runs over the payload exactly as received, so fields this version does not
//! know about are still covered by the signature.
//!
//! The payload names its app and channel so that a validly signed manifest
//! cannot be replayed to another app that shares the key, or from the beta
//! feed to stable users.

use std::collections::BTreeMap;
use std::fmt;

use ring::signature::{ED25519, Ed25519KeyPair, UnparsedPublicKey};
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::hex;

/// A trusted Ed25519 public key. Ship more than one to rotate keys: a
/// manifest verifies if any key accepts it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PublicKey([u8; 32]);

impl PublicKey {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// 64 hex digits, as `manifest_tool keygen` prints them.
    pub fn from_hex(text: &str) -> Result<Self, ManifestError> {
        let bytes = hex::decode(text).ok_or(ManifestError::BadKey)?;
        Ok(Self(bytes.try_into().map_err(|_| ManifestError::BadKey)?))
    }

    pub fn to_hex(&self) -> String {
        hex::encode(&self.0)
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({})", self.to_hex())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    AppImage,
    Dmg,
    Nsis,
    Msi,
    /// Listed so a manifest can describe system packages; they update
    /// through the package manager, never in place.
    Deb,
    Rpm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub format: Format,
    pub url: String,
    /// Lowercase or uppercase hex SHA-256 of the file.
    pub sha256: String,
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Manifest {
    pub app: String,
    pub channel: String,
    pub version: Version,
    #[serde(default)]
    pub pub_date: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    /// Keyed by [`platform_key`].
    pub platforms: BTreeMap<String, Artifact>,
}

/// A newer release for this platform, from a verified manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub channel: String,
    pub notes: Option<String>,
    pub pub_date: Option<String>,
    pub artifact: Artifact,
}

/// What a verified manifest means for the running version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Update(Release),
    UpToDate,
    /// The manifest offers an older version, as a stale CDN copy or a
    /// replayed old manifest would. Never installed.
    Downgrade {
        offered: Version,
    },
}

/// What the running app expects of its feed.
#[derive(Debug, Clone, Copy)]
pub struct Expected<'a> {
    pub app: &'a str,
    pub channel: &'a str,
    pub current: &'a Version,
    pub platform: &'a str,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("update manifest is not valid JSON of the expected shape: {0}")]
    Malformed(String),
    #[error("no trusted update keys are configured")]
    NoTrustedKeys,
    #[error("an update key is not 32 bytes of hex")]
    BadKey,
    #[error("update manifest signature does not verify against any trusted key")]
    BadSignature,
    #[error("update manifest is for app {found:?}, expected {expected:?}")]
    WrongApp { expected: String, found: String },
    #[error("update manifest is for channel {found:?}, expected {expected:?}")]
    WrongChannel { expected: String, found: String },
    #[error("update {version} has no artifact for {platform}")]
    NoArtifact { version: Version, platform: String },
}

#[derive(Deserialize)]
struct Envelope {
    payload: Value,
    signature: String,
}

/// Parse `bytes` and check its signature against `keys`. Nothing in the
/// payload is interpreted until the signature verifies.
pub fn verify(bytes: &[u8], keys: &[PublicKey]) -> Result<Manifest, ManifestError> {
    if keys.is_empty() {
        return Err(ManifestError::NoTrustedKeys);
    }
    let envelope: Envelope =
        serde_json::from_slice(bytes).map_err(|e| ManifestError::Malformed(e.to_string()))?;
    let signature = hex::decode(&envelope.signature).ok_or(ManifestError::BadSignature)?;
    let message = canonical_json(&envelope.payload);
    let trusted = keys.iter().any(|key| {
        UnparsedPublicKey::new(&ED25519, &key.0)
            .verify(message.as_bytes(), &signature)
            .is_ok()
    });
    if !trusted {
        return Err(ManifestError::BadSignature);
    }
    serde_json::from_value(envelope.payload).map_err(|e| ManifestError::Malformed(e.to_string()))
}

/// Decide what a verified manifest offers the running app.
pub fn evaluate(manifest: Manifest, expected: Expected<'_>) -> Result<Verdict, ManifestError> {
    if manifest.app != expected.app {
        return Err(ManifestError::WrongApp {
            expected: expected.app.to_owned(),
            found: manifest.app,
        });
    }
    if manifest.channel != expected.channel {
        return Err(ManifestError::WrongChannel {
            expected: expected.channel.to_owned(),
            found: manifest.channel,
        });
    }
    if manifest.version == *expected.current {
        return Ok(Verdict::UpToDate);
    }
    if manifest.version < *expected.current {
        return Ok(Verdict::Downgrade {
            offered: manifest.version,
        });
    }
    let Manifest {
        channel,
        version,
        pub_date,
        notes,
        mut platforms,
        ..
    } = manifest;
    let artifact =
        platforms
            .remove(expected.platform)
            .ok_or_else(|| ManifestError::NoArtifact {
                version: version.clone(),
                platform: expected.platform.to_owned(),
            })?;
    Ok(Verdict::Update(Release {
        version,
        channel,
        notes,
        pub_date,
        artifact,
    }))
}

/// Sign `payload` with the Ed25519 private key `seed` and return the
/// manifest document. Release tooling only: apps never hold the seed.
pub fn sign(seed: &[u8; 32], payload: &Value) -> Result<String, ManifestError> {
    let pair = Ed25519KeyPair::from_seed_unchecked(seed).map_err(|_| ManifestError::BadKey)?;
    let signature = pair.sign(canonical_json(payload).as_bytes());
    let document = serde_json::json!({
        "payload": payload,
        "signature": hex::encode(signature.as_ref()),
    });
    serde_json::to_string_pretty(&document).map_err(|e| ManifestError::Malformed(e.to_string()))
}

/// The public key for an Ed25519 private key `seed`.
pub fn public_key(seed: &[u8; 32]) -> Result<PublicKey, ManifestError> {
    use ring::signature::KeyPair;
    let pair = Ed25519KeyPair::from_seed_unchecked(seed).map_err(|_| ManifestError::BadKey)?;
    let bytes = pair.public_key().as_ref();
    Ok(PublicKey(
        bytes.try_into().map_err(|_| ManifestError::BadKey)?,
    ))
}

/// `<os>-<arch>` for the running build: `linux-x86_64`, `macos-aarch64`,
/// `windows-x86_64`.
pub fn platform_key() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                write_canonical(&map[key], out);
            }
            out.push('}');
        }
        // Scalars print the same in compact serde_json and JavaScript's
        // JSON.stringify, given integer numbers.
        scalar => out.push_str(&scalar.to_string()),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    pub(crate) const SEED: [u8; 32] = [7; 32];
    const OTHER_SEED: [u8; 32] = [9; 32];

    pub(crate) fn payload(version: &str) -> Value {
        json!({
            "app": "dev.quark.hello",
            "channel": "stable",
            "version": version,
            "notes": "Faster startup.",
            "platforms": {
                "linux-x86_64": {
                    "format": "appimage",
                    "url": "https://example.com/hello.AppImage",
                    "sha256": "ab",
                    "size": 2
                }
            }
        })
    }

    /// Verify and evaluate as the updater does, reduced to one line.
    fn outcome(document: &str, keys: &[PublicKey], channel: &str, current: &str) -> String {
        let current = Version::parse(current).unwrap();
        let expected = Expected {
            app: "dev.quark.hello",
            channel,
            current: &current,
            platform: "linux-x86_64",
        };
        match verify(document.as_bytes(), keys).and_then(|m| evaluate(m, expected)) {
            Ok(Verdict::Update(release)) => {
                format!("update {} from {}", release.version, release.artifact.url)
            }
            Ok(Verdict::UpToDate) => "up to date".into(),
            Ok(Verdict::Downgrade { offered }) => format!("refused downgrade to {offered}"),
            Err(error) => format!("error: {error}"),
        }
    }

    #[test]
    fn verification_table() {
        let key = public_key(&SEED).unwrap();
        let other = public_key(&OTHER_SEED).unwrap();
        let signed = sign(&SEED, &payload("1.4.0")).unwrap();
        let tampered = signed.replace("hello.AppImage", "evil.AppImage");
        let old = sign(&SEED, &payload("1.2.0")).unwrap();
        let mut beta = payload("1.5.0-beta.1");
        beta["channel"] = json!("beta");
        let beta = sign(&SEED, &beta).unwrap();
        let mut foreign = payload("9.0.0");
        foreign["app"] = json!("com.other.app");
        let foreign = sign(&SEED, &foreign).unwrap();
        let mut no_linux = payload("1.4.0");
        no_linux["platforms"] = json!({});
        let no_linux = sign(&SEED, &no_linux).unwrap();
        let mut extra = payload("1.4.0");
        extra["future_field"] = json!({"z": 1, "a": [true, null]});
        let extra = sign(&SEED, &extra).unwrap();

        let cases: &[(&str, &str, &[PublicKey], &str, &str)] = &[
            (
                "valid",
                &signed,
                &[key],
                "1.3.0",
                "update 1.4.0 from https://example.com/hello.AppImage",
            ),
            (
                "rotated keys",
                &signed,
                &[other, key],
                "1.3.0",
                "update 1.4.0 from https://example.com/hello.AppImage",
            ),
            (
                "unknown fields stay signed",
                &extra,
                &[key],
                "1.3.0",
                "update 1.4.0 from https://example.com/hello.AppImage",
            ),
            (
                "tampered url",
                &tampered,
                &[key],
                "1.3.0",
                "error: update manifest signature does not verify against any trusted key",
            ),
            (
                "wrong key",
                &signed,
                &[other],
                "1.3.0",
                "error: update manifest signature does not verify against any trusted key",
            ),
            (
                "no keys",
                &signed,
                &[],
                "1.3.0",
                "error: no trusted update keys are configured",
            ),
            ("same version", &signed, &[key], "1.4.0", "up to date"),
            (
                "downgrade",
                &old,
                &[key],
                "1.3.0",
                "refused downgrade to 1.2.0",
            ),
            (
                "prerelease of current is older",
                &signed,
                &[key],
                "1.4.0-rc.1",
                "update 1.4.0 from https://example.com/hello.AppImage",
            ),
            (
                "beta feed on stable",
                &beta,
                &[key],
                "1.3.0",
                "error: update manifest is for channel \"beta\", expected \"stable\"",
            ),
            (
                "other app",
                &foreign,
                &[key],
                "1.3.0",
                "error: update manifest is for app \"com.other.app\", expected \"dev.quark.hello\"",
            ),
            (
                "no artifact",
                &no_linux,
                &[key],
                "1.3.0",
                "error: update 1.4.0 has no artifact for linux-x86_64",
            ),
        ];
        for &(name, document, keys, current, expected) in cases {
            assert_eq!(
                outcome(document, keys, "stable", current),
                expected,
                "{name}"
            );
        }
        assert_eq!(
            outcome(&beta, &[key], "beta", "1.4.0"),
            "update 1.5.0-beta.1 from https://example.com/hello.AppImage"
        );
    }

    // Any other manifest generator (a release script in another language)
    // must produce these exact bytes or every signature fails.
    #[test]
    fn canonical_json_matches_sorted_compact_form() {
        let value = json!({"b": [1, "x\"y"], "a": {"d": null, "c": true}, "é": "ü"});
        assert_eq!(
            canonical_json(&value),
            r#"{"a":{"c":true,"d":null},"b":[1,"x\"y"],"é":"ü"}"#
        );
    }
}
