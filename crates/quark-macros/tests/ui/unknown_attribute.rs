use quark_macros::view;

struct Div;

fn div() -> Div {
    Div
}

impl Div {
    fn color(self, _color: &str) -> Self {
        self
    }

    fn into_any(self) {}
}

fn main() {
    view! { <div colr="red" /> };
}
