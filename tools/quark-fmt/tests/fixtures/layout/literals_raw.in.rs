// Synthetic: literals that look like markup or comments are never touched.
fn excerpt() {
    view! {
        <div class="p-4
                    px-2"><text>r#"</div> // not a comment"#   "/* nor this */"</text></div>
    }
}
