use quark_macros::{Props, view};

#[derive(Props)]
struct Card {
    title: String,
}

trait IntoAny {
    fn into_any(self);
}

impl IntoAny for Card {
    fn into_any(self) {}
}

fn main() {
    view! { <Card title={5} /> };
}
