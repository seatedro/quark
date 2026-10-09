//! Shared webview test support: the HTTPS fixture and its pages. Included
//! by `webview_smoke` and the `webview_demo` example through `#[path]`, so
//! each uses only part of it.
#![allow(dead_code)]

pub mod fixture;
#[cfg(all(target_os = "linux", feature = "webview"))]
pub mod linux;
pub mod pages;
