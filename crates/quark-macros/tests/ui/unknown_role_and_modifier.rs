use quark_macros::view;

fn main() {
    let _ = view! { <div role="buton" on:key:ctl+s={()} aria-lable="x" /> };
}
