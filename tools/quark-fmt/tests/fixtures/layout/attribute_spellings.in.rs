// Synthetic: attribute spellings stay as written; only spacing changes.
fn excerpt() {
    view! {
        <div aria-label = "x" on:key:mod+s={save} on:click = {go} x=-1 y=-1.5 hidden
             w={@width} bg={if on { a }} class="p-4 px-2"   test-id="t">
            <.footer>"slot"</.footer>
            <widgets::Button label="b"></Button>
            <{self.input} focused={f}></>
            <fragment>{?maybe} {...rest}</fragment>
            <></>
        </div>
    }
}
