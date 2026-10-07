//! Release tooling for update manifests.
//!
//! ```text
//! manifest_tool keygen
//!     Print a new private key (the 32-byte Ed25519 seed) and its public key,
//!     as hex. Store the private key as a CI secret; compile the public key
//!     into the app.
//! manifest_tool public-key
//!     Print the public key for $QUARK_UPDATE_SIGNING_KEY.
//! manifest_tool sign <app> <channel> <version> <base-url> <platform>=<format>=<file>...
//!     Hash each file, build the payload, sign it with
//!     $QUARK_UPDATE_SIGNING_KEY, and print the manifest. Each artifact's
//!     URL is <base-url>/<file name>.
//! ```
//!
//! The private key is read from the environment, never from arguments, so
//! it stays out of shell history and process listings.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use quark_update::manifest;
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::json;

const KEY_VAR: &str = "QUARK_UPDATE_SIGNING_KEY";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("manifest_tool: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<String, String> {
    match args.first().map(String::as_str) {
        Some("keygen") => {
            let mut seed = [0u8; 32];
            SystemRandom::new()
                .fill(&mut seed)
                .map_err(|_| "no system randomness")?;
            let public = manifest::public_key(&seed).map_err(|e| e.to_string())?;
            Ok(format!(
                "private (secret {KEY_VAR}): {}\npublic: {}",
                hex(&seed),
                public.to_hex()
            ))
        }
        Some("public-key") => Ok(manifest::public_key(&seed()?)
            .map_err(|e| e.to_string())?
            .to_hex()),
        Some("sign") if args.len() >= 6 => {
            let [app, channel, version, base_url] = [&args[1], &args[2], &args[3], &args[4]];
            quark_update::Version::parse(version).map_err(|e| format!("version {version:?}: {e}"))?;
            let mut platforms = BTreeMap::new();
            for spec in &args[5..] {
                let mut parts = spec.splitn(3, '=');
                let (Some(platform), Some(format), Some(file)) =
                    (parts.next(), parts.next(), parts.next())
                else {
                    return Err(format!("expected <platform>=<format>=<file>, got {spec:?}"));
                };
                let bytes = std::fs::read(file).map_err(|e| format!("{file}: {e}"))?;
                let name = Path::new(file)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| format!("{file}: no file name"))?;
                let digest = ring::digest::digest(&ring::digest::SHA256, &bytes);
                platforms.insert(
                    platform.to_owned(),
                    json!({
                        "format": format,
                        "url": format!("{}/{}", base_url.trim_end_matches('/'), name),
                        "sha256": hex(digest.as_ref()),
                        "size": bytes.len(),
                    }),
                );
            }
            let payload = json!({
                "app": app,
                "channel": channel,
                "version": version,
                "platforms": platforms,
            });
            manifest::sign(&seed()?, &payload).map_err(|e| e.to_string())
        }
        _ => Err("usage: manifest_tool keygen | public-key | sign <app> <channel> <version> <base-url> <platform>=<format>=<file>...".into()),
    }
}

fn seed() -> Result<[u8; 32], String> {
    let text = std::env::var(KEY_VAR).map_err(|_| format!("{KEY_VAR} is not set"))?;
    let text = text.trim();
    if text.len() != 64 {
        return Err(format!("{KEY_VAR} must be 64 hex digits"));
    }
    let mut seed = [0u8; 32];
    for (i, byte) in seed.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16)
            .map_err(|_| format!("{KEY_VAR} must be 64 hex digits"))?;
    }
    Ok(seed)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
