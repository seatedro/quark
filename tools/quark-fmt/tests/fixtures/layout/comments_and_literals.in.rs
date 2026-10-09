// Synthetic: comment attachment, literal boundaries, empty and self-closing elements.
fn f() {
    view! { <div bg={color} // surface
class="p-4 px-2">
// Keep these words together.
<text>" leading " r#"and trailing "#</text>

<div></div> /* intentional empty child */
<spacer/>
</div> }
}
