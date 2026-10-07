use quark_macros::{Props, view};

#[derive(Props)]
struct Card {
    title: String,
    #[prop(into)]
    on_close: String,
    #[prop(default)]
    selected: bool,
}

trait IntoAny {
    fn into_any(self);
}

impl IntoAny for Card {
    fn into_any(self) {}
}

fn main() {
    view! { <Card on:close="x" selected /> };
}
