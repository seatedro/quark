//! The WebView2 backend. `cdp` and `state` hold the protocol and ordering
//! logic and build everywhere for their tests; `native` is the COM glue.
#![cfg_attr(not(all(feature = "native", windows)), allow(dead_code))]

mod cdp;
mod state;

#[cfg(all(feature = "native", windows))]
mod native;

#[cfg(all(feature = "native", windows))]
pub(super) fn backend() -> Option<Box<dyn super::Backend>> {
    native::backend()
}
