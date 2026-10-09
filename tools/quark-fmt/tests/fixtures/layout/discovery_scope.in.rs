// Synthetic: which invocations are formatted.
#[cfg(feature = "never")]
fn disabled() -> AnyElement {
    view! {<div   class="x"/>}
}

fn foreign() {
    ui::view! {<div   class="x"/>};
    qview! {<div   class="x"/>};
    let v = vec![view! {<div   class="x"/>}];
}

// quark-fmt: skip
fn skipped_item() {
    view! {<div   class="x"/>}
}

#[rustfmt::skip]
fn rustfmt_skipped() {
    view! {<div   class="x"/>}
}

fn skipped_statement() {
    // quark-fmt: skip
    let v = view! {<div   class="x"/>};
    let w = view! {<div   class="x"/>};
}
