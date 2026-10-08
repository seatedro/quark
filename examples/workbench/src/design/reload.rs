//! Live theme reloading (stream B). Placeholder with stream B's
//! signatures: no watcher, nothing rejected.

/// Call `changed` whenever a watched theme file changes. Returns whether
/// a watcher started.
pub fn watch(_changed: impl Fn() + Send + 'static) -> bool {
    false
}

/// Why the last changed theme file was rejected, once.
pub fn take_rejection() -> Option<String> {
    None
}
