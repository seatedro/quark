//! Which C library functions a program linking this crate calls.
//! libghostty-vt's archive bundles Zig's compiler-rt, whose slow copies of
//! `bcmp`, `memcpy`, `sin`, and others would otherwise replace the C
//! library's in the whole program (see build/compiler_rt.rs).

use std::ffi::{CStr, c_char, c_int, c_void};

unsafe extern "C" {
    #[cfg(unix)]
    fn bcmp(a: *const c_void, b: *const c_void, n: usize) -> c_int;
    fn memcmp(a: *const c_void, b: *const c_void, n: usize) -> c_int;
    fn memcpy(dest: *mut c_void, src: *const c_void, n: usize) -> *mut c_void;
    fn memmove(dest: *mut c_void, src: *const c_void, n: usize) -> *mut c_void;
    #[cfg(not(target_os = "macos"))]
    fn memset(dest: *mut c_void, c: c_int, n: usize) -> *mut c_void;
    fn strlen(s: *const c_char) -> usize;
    fn sin(x: f64) -> f64;
}

/// Each function and the address the link bound it to.
fn linked() -> Vec<(&'static CStr, usize)> {
    vec![
        #[cfg(unix)]
        (c"bcmp", bcmp as *const () as usize),
        (c"memcmp", memcmp as *const () as usize),
        (c"memcpy", memcpy as *const () as usize),
        (c"memmove", memmove as *const () as usize),
        // Ghostty's build leaves its own `memset` (src/quirks_memset.zig,
        // vectorized) in macOS archives, replacing libSystem's.
        #[cfg(not(target_os = "macos"))]
        (c"memset", memset as *const () as usize),
        (c"strlen", strlen as *const () as usize),
        (c"sin", sin as *const () as usize),
    ]
}

/// Whether `address`, where the link bound `name`, is the C library's.
#[cfg(unix)]
fn from_c_library(name: &CStr, address: usize) -> bool {
    // The dynamic linker finds only the C library's: compiler-rt's copies
    // are hidden, so they never reach the dynamic symbol table.
    // SAFETY: `name` is a C string.
    let c_library = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
    !c_library.is_null() && c_library as usize == address
}

/// Whether `address`, where the link bound a function, is in a DLL (the
/// C runtime's) rather than the executable, which holds compiler-rt.
#[cfg(windows)]
fn from_c_library(_name: &CStr, mut address: usize) -> bool {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
        fn GetModuleHandleExW(flags: u32, name: *const u16, module: *mut *mut c_void) -> i32;
    }
    const FROM_ADDRESS: u32 = 0x4;
    const UNCHANGED_REFCOUNT: u32 = 0x2;
    // An imported function's address is a stub in the executable, maybe
    // behind an incremental linking thunk (`jmp rel32`), that jumps through
    // the import table (`jmp [rip + disp32]`).
    // SAFETY: the addresses are of code and import table slots.
    unsafe {
        let code = address as *const u8;
        if *code == 0xe9 {
            let rel = code.add(1).cast::<i32>().read_unaligned();
            address = (address as isize + 5 + rel as isize) as usize;
        }
        let code = address as *const u8;
        if *code == 0xff && *code.add(1) == 0x25 {
            let disp = code.add(2).cast::<i32>().read_unaligned();
            address = *((address as isize + 6 + disp as isize) as *const usize);
        }
        let mut module = std::ptr::null_mut();
        let found = GetModuleHandleExW(
            FROM_ADDRESS | UNCHANGED_REFCOUNT,
            address as *const u16,
            &mut module,
        );
        found != 0 && module != GetModuleHandleW(std::ptr::null())
    }
}

#[test]
#[cfg(any(unix, not(target_feature = "crt-static")))]
fn c_library_functions_come_from_the_c_library() {
    // Keep libghostty-vt in the link, as any user of the crate does.
    drop(crate::TerminalState::headless(4, 2));
    let replaced: Vec<_> = linked()
        .into_iter()
        .filter(|&(name, address)| !from_c_library(name, address))
        .map(|(name, _)| name)
        .collect();
    assert!(
        replaced.is_empty(),
        "bound to compiler-rt's copies: {replaced:?}"
    );
}
