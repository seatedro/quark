use quark_macros::view;

fn main() {
    let _ = view! { -> u8, <>"a" "b"</> };
}
