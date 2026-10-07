#[cfg(ghostty_vt)]
#[allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    unsafe_op_in_unsafe_fn,
    clippy::all
)]
mod sys;

#[cfg(test)]
mod smoke {
    #[test]
    fn links() {
        let mut t = std::ptr::null_mut();
        let r = unsafe { super::sys::ghostty_terminal_new(std::ptr::null(), &mut t, 10, 5) };
        assert_eq!(r, super::sys::GHOSTTY_SUCCESS);
        unsafe { super::sys::ghostty_terminal_free(t) };
    }
}
