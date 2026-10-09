// examples/codex/src/sidebar.rs, line 116 (the nav rows, as a view of their own)
fn excerpt() {
    view! {
        <div class="absolute left-2 top-[52] flex-col gap-px">
            <nav_row(p, view! {
                <div><icon svg={icons::COMPOSE} size={16.0} color={p.sidebar_text} /></div>
            }, "New chat", Msg::NewChat) />
            <nav_row(p, view! {
                <div class="w-4 h-4 items-center justify-center">
                    <icon svg={icons::PET} size={15.0} color={p.accent} />
                </div>
            }, "Your dot", Msg::Noop) class="pr-1.5">
                <div class="flex-1" />
                <div class="flex-row items-center w-6 h-5 justify-center rounded-[10]"
                     bg={p.badge}>
                    <txt("1", SMALL, p.badge_text) />
                </div>
            </nav_row>
        </div>
    }
}
