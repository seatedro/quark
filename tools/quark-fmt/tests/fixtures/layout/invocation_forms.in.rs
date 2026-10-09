// Synthetic: delimiters, paths, headers, and views that fit on one line.
fn excerpt() {
    let a = view!(<spacer/>);
    let b = quark::view! {   <div   class="p-2"/>   };
    let c = ::quark::view! { scale, -> Div,
        <div/> };
    row.child(view! { <text>"short"</text> }).child(view! { <icon svg={icons::LOADER} size={13.0} /> });
    let d = view! { <div class="flex-row items-center gap-2"><icon svg={icons::A} /><icon svg={icons::B} /></div> };
}
