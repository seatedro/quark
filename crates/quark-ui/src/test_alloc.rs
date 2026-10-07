//! A global allocator that counts this thread's allocations, so
//! frame-budget tests can assert how much a frame allocates and attribute
//! allocations to their call sites. A test binary installs it with
//! `#[global_allocator] static A: Counting = Counting;`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

pub struct Counting;

thread_local! {
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
    /// Set while `profile` runs; cleared inside the hook so the hook's own
    /// allocations are not attributed.
    static PROFILING: Cell<bool> = const { Cell::new(false) };
    static SITES: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
}

fn record() {
    let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
    if PROFILING.try_with(|p| p.replace(false)) != Ok(true) {
        return;
    }
    let site = call_site(&std::backtrace::Backtrace::force_capture().to_string());
    SITES.with(|sites| *sites.borrow_mut().entry(site).or_default() += 1);
    PROFILING.with(|p| p.set(true));
}

/// The innermost quark frame of a backtrace, with its source line.
fn call_site(trace: &str) -> String {
    let mut lines = trace.lines().map(str::trim);
    while let Some(line) = lines.next() {
        let Some((_, symbol)) = line.split_once(": ") else {
            continue;
        };
        let ours = symbol.starts_with("quark") || symbol.starts_with("<quark");
        if ours && !symbol.contains("test_alloc") {
            let at = lines.next().unwrap_or_default();
            let at = at.rsplit("/src/").next().unwrap_or(at);
            return format!("{symbol} ({at})");
        }
    }
    "outside quark".to_owned()
}

// SAFETY: forwards to the system allocator unchanged; counting touches only
// thread locals, and the profiling hook disables itself while it runs.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record();
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record();
        // SAFETY: as above.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: as above.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record();
        // SAFETY: as above.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// Allocations this thread has made so far.
pub fn allocations() -> u64 {
    ALLOCATIONS.with(Cell::get)
}

/// Allocations `f` makes on this thread.
pub fn count<R>(f: impl FnOnce() -> R) -> (R, u64) {
    let before = allocations();
    let result = f();
    (result, allocations() - before)
}

/// Run `f` and return its allocations by call site, most first. Slow:
/// every allocation captures a backtrace.
#[allow(dead_code)]
pub fn profile<R>(f: impl FnOnce() -> R) -> (R, Vec<(String, u64)>) {
    SITES.with(|sites| sites.borrow_mut().clear());
    PROFILING.with(|p| p.set(true));
    let result = f();
    PROFILING.with(|p| p.set(false));
    let mut sites: Vec<_> = SITES.with(|sites| sites.borrow_mut().drain().collect());
    sites.sort_by_key(|site| std::cmp::Reverse(site.1));
    (result, sites)
}
