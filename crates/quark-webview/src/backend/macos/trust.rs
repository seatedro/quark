//! Test-only server trust (feature `test-trust`): accept exactly the
//! fixture's leaf certificate for exactly the listed loopback hosts.
//! Everything else, including another certificate on those hosts, gets
//! WebKit's default handling and so normal validation.

use std::ffi::c_void;

use objc2::encode::{Encoding, RefEncode};
use objc2::rc::Retained;
use objc2::{class, msg_send};
use objc2_foundation::{
    NSURLAuthenticationChallenge, NSURLAuthenticationMethodServerTrust, NSURLCredential,
    NSURLSessionAuthChallengeDisposition,
};

/// `SecTrustRef`'s target.
#[repr(C)]
struct SecTrust {
    _private: [u8; 0],
}

// SAFETY: matches the Objective-C type encoding of `SecTrustRef`.
unsafe impl RefEncode for SecTrust {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("__SecTrust", &[]));
}

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecTrustCopyCertificateChain(trust: *mut SecTrust) -> *const c_void;
    fn SecCertificateCopyData(certificate: *const c_void) -> *const c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFArrayGetCount(array: *const c_void) -> isize;
    fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
    fn CFDataGetLength(data: *const c_void) -> isize;
    fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
    fn CFRelease(object: *const c_void);
}

/// The certificate and hosts `quark_webview::testing::trust_leaf` set.
pub(super) struct TestTrust {
    der: &'static [u8],
    hosts: &'static [String],
}

impl TestTrust {
    pub(super) fn current() -> Option<Self> {
        crate::test_trust().map(|(der, hosts)| Self { der, hosts })
    }

    /// Answer `challenge` if it is the fixture's; `false` leaves it to
    /// WebKit's default handling.
    pub(super) fn answer(
        &self,
        challenge: &NSURLAuthenticationChallenge,
        completion: &wry::AuthChallengeCompletion,
    ) -> bool {
        let Some(credential) = self.credential(challenge) else {
            return false;
        };
        completion.call((
            NSURLSessionAuthChallengeDisposition::UseCredential,
            Retained::as_ptr(&credential).cast_mut(),
        ));
        true
    }

    fn credential(
        &self,
        challenge: &NSURLAuthenticationChallenge,
    ) -> Option<Retained<NSURLCredential>> {
        let space = challenge.protectionSpace();
        let method = space.authenticationMethod();
        if &*method != unsafe { NSURLAuthenticationMethodServerTrust } {
            return None;
        }
        let host = space.host().to_string();
        if !self.hosts.contains(&host) {
            return None;
        }
        // SAFETY: a server-trust protection space has a `SecTrustRef`.
        let trust: *mut SecTrust = unsafe { msg_send![&*space, serverTrust] };
        if trust.is_null() || leaf_der(trust).as_deref() != Some(self.der) {
            return None;
        }
        Some(unsafe { msg_send![class!(NSURLCredential), credentialForTrust: trust] })
    }
}

/// The DER of the leaf certificate `trust` presents.
fn leaf_der(trust: *mut SecTrust) -> Option<Vec<u8>> {
    // SAFETY: Core Foundation calls on objects this function owns or
    // borrows from `chain`, released before returning.
    unsafe {
        let chain = SecTrustCopyCertificateChain(trust);
        if chain.is_null() {
            return None;
        }
        let leaf = (CFArrayGetCount(chain) > 0).then(|| {
            let data = SecCertificateCopyData(CFArrayGetValueAtIndex(chain, 0));
            let bytes =
                std::slice::from_raw_parts(CFDataGetBytePtr(data), CFDataGetLength(data) as usize)
                    .to_vec();
            CFRelease(data);
            bytes
        });
        CFRelease(chain);
        leaf
    }
}
