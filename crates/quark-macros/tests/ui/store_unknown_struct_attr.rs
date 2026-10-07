use quark_macros::Store;

#[derive(Store)]
#[store(defaults)]
struct Pane {
    scroll: f32,
}

fn main() {}
