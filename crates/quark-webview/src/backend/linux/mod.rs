//! The WebKitGTK backend. Placeholder until its stream lands: no engine, so
//! every open fails with `OpenError::Unsupported`.

use super::Backend;

pub(super) fn backend() -> Option<Box<dyn Backend>> {
    None
}
