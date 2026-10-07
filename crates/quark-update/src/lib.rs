//! Signed self-updates for Quark apps.
//!
//! An app publishes one signed JSON manifest per channel (see [`manifest`]).
//! The [`Updater`] thread fetches it on a schedule, verifies the Ed25519
//! signature against keys compiled into the app, refuses other apps',
//! other channels', and older versions, downloads the platform artifact with
//! resume and SHA-256 verification, and reports each step as an
//! [`UpdateEvent`]. Nothing here draws UI.
//!
//! Installing happens after the app quits. The app hands the
//! [`StagedUpdate`] to a [`PendingRestart`] and exits its event loop
//! normally; `main` then calls [`PendingRestart::apply`], which starts a
//! helper that waits for the process to exit, replaces the app (see
//! [`install`] for each platform), and relaunches it. The updater never
//! calls `std::process::exit`, so window state and app data are flushed
//! through the usual shutdown path first.
//!
//! ```no_run
//! use quark_update::{PublicKey, UpdateConfig, UpdateEvent, Updater};
//!
//! // The public half of the release key, from `manifest_tool public-key`.
//! let key = PublicKey::from_hex("3b6a27bcceb6a42d62a3a8d02a6f0d73653215771de243a63ac048a18b59da29").unwrap();
//! let config = UpdateConfig::new(
//!     "dev.quark.hello",
//!     env!("CARGO_PKG_VERSION").parse().unwrap(),
//!     "https://example.com/updates/{channel}.json",
//!     vec![key],
//! );
//! let updater = Updater::start(config, |event| match event {
//!     UpdateEvent::Available(update) => println!("{} is out", update.release.version),
//!     UpdateEvent::Ready(staged) => println!("restart to install {}", staged.release.version),
//!     _ => {}
//! });
//! # drop(updater);
//! ```
//!
//! ## Why ring
//!
//! Verification uses `ring` for Ed25519 and SHA-256. The HTTP client's
//! rustls backend already builds ring, so it adds no second crypto
//! implementation to the binary, and its verify-only `UnparsedPublicKey`
//! API has no secret-key state to misuse. `ed25519-dalek` would be the
//! choice for a build without rustls.

pub mod download;
mod hex;
pub mod install;
pub mod manifest;
pub mod schedule;
mod updater;

pub use download::{DownloadError, Progress};
pub use install::{InstallEnv, InstallPlan, Unsupported};
pub use manifest::{Artifact, Format, Manifest, ManifestError, PublicKey, Release, Verdict};
pub use schedule::Schedule;
pub use semver::Version;
pub use updater::{
    AvailableUpdate, PendingRestart, StagedUpdate, UpdateConfig, UpdateError, UpdateEvent, Updater,
    default_staging_dir,
};
