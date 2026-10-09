//! Website data stores: a fresh `nonPersistentDataStore` per ephemeral
//! view, and `dataStoreForIdentifier:` (macOS 14+) for a named profile.

use std::cell::RefCell;

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_foundation::{NSError, NSOperatingSystemVersion, NSProcessInfo, NSTimer, NSUUID};
use objc2_web_kit::WKWebsiteDataStore;

use crate::backend::ProfileSink;
use crate::profile::{DataStore, ProfileError, ProfileId};
use crate::{PlatformError, ProfileMode};

/// Named data stores need macOS 14.
fn named_stores_supported() -> bool {
    NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(NSOperatingSystemVersion {
        majorVersion: 14,
        minorVersion: 0,
        patchVersion: 0,
    })
}

/// The data store for `store`. Never the shared default store.
pub(super) fn data_store(
    mtm: MainThreadMarker,
    store: &DataStore,
) -> Result<(Retained<WKWebsiteDataStore>, ProfileMode), ProfileError> {
    match store {
        DataStore::Persistent(profile) => {
            if !named_stores_supported() {
                return Err(ProfileError::Unsupported);
            }
            let identifier = NSUUID::from_bytes(profile_uuid(profile));
            let store = unsafe { WKWebsiteDataStore::dataStoreForIdentifier(&identifier, mtm) };
            Ok((store, ProfileMode::Persistent))
        }
        // Ephemeral, and any kind added later: isolated and in memory.
        _ => Ok((
            unsafe { WKWebsiteDataStore::nonPersistentDataStore(mtm) },
            ProfileMode::Ephemeral,
        )),
    }
}

/// WebKit refuses to remove a store some web view still holds.
const STORE_IN_USE: (&str, isize) = ("WKWebSiteDataStore", 1);
/// Retries while the store is in use, and the pause between them. A
/// closed view's web process lets go of the store asynchronously, shortly
/// after the view is released.
const CLEAR_ATTEMPTS: u32 = 100;
const CLEAR_RETRY_SECONDS: f64 = 0.05;

/// Remove `profile`'s store, answering `sink` when WebKit has.
pub(super) fn clear(mtm: MainThreadMarker, profile: &ProfileId, sink: ProfileSink) {
    if !named_stores_supported() {
        sink.finished(Err(ProfileError::Unsupported));
        return;
    }
    remove(mtm, profile_uuid(profile), sink, CLEAR_ATTEMPTS);
}

fn remove(mtm: MainThreadMarker, uuid: [u8; 16], sink: ProfileSink, attempts: u32) {
    let sink = RefCell::new(Some(sink));
    let completion = RcBlock::new(move |error: *mut NSError| {
        let Some(sink) = sink.borrow_mut().take() else {
            return;
        };
        // SAFETY: WebKit passes a valid error or nil.
        let Some(error) = (unsafe { error.as_ref() }) else {
            return sink.finished(Ok(()));
        };
        let code = (error.domain().to_string(), error.code());
        if (code.0.as_str(), code.1) == STORE_IN_USE && attempts > 1 {
            return retry_later(mtm, uuid, sink, attempts - 1);
        }
        sink.finished(Err(ProfileError::Platform(PlatformError::new(
            "remove the website data store",
            format!("{} {}", code.0, code.1),
        ))));
    });
    unsafe {
        WKWebsiteDataStore::removeDataStoreForIdentifier_completionHandler(
            &NSUUID::from_bytes(uuid),
            &completion,
            mtm,
        );
    }
}

fn retry_later(mtm: MainThreadMarker, uuid: [u8; 16], sink: ProfileSink, attempts: u32) {
    let sink = RefCell::new(Some(sink));
    let fire = RcBlock::new(move |_timer| {
        if let Some(sink) = sink.borrow_mut().take() {
            remove(mtm, uuid, sink, attempts);
        }
    });
    // The run loop retains a scheduled timer until it fires.
    let _ = unsafe {
        NSTimer::scheduledTimerWithTimeInterval_repeats_block(CLEAR_RETRY_SECONDS, false, &fire)
    };
}

/// A stable UUID for `profile`, so the same app and purpose reopen the same
/// store across launches without a stored mapping. Name-based (version 8,
/// RFC 9562): FNV-1a 128 over a quark prefix and both parts, which
/// [`ProfileId`] limits to characters that cannot contain the separator.
pub(super) fn profile_uuid(profile: &ProfileId) -> [u8; 16] {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
    let mut hash = OFFSET;
    let name = format!("quark-webview\0{}\0{}", profile.app(), profile.purpose());
    for byte in name.bytes() {
        hash ^= u128::from(byte);
        hash = hash.wrapping_mul(PRIME);
    }
    let mut bytes = hash.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reopening a named profile in a later release must find the same
    /// store, so the derivation is pinned.
    #[test]
    fn profile_uuids_are_stable_and_distinct() {
        let uuid = |app, purpose| {
            let bytes = profile_uuid(&ProfileId::new(app, purpose).unwrap());
            bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        // Computed independently of this code.
        assert_eq!(
            uuid("com.example.portal", "devin-sign-in"),
            "a859111588ac87678e4c2496bef560e0"
        );
        assert_ne!(
            uuid("com.example.portal", "a"),
            uuid("com.example.portal", "b")
        );
        assert_ne!(uuid("a", "bc"), uuid("ab", "c"));
    }
}
