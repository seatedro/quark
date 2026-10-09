// examples/codex/src/widgets.rs, line 177
fn excerpt() {
    view! { -> Div,
        <div class="w-8 h-5 shrink-0 rounded-[10] flex-row items-center px-0.5"
             bg={if on { p.toggle_on } else { p.menu_hi }} @when {on} { class="justify-end" }>
            <div class="w-4 h-4 rounded-[8] bg-white" />
        </div>
    }
}
