// Synthetic: every match-arm body form keeps its braces and commas.
fn excerpt() {
    view! {
        <div>
            match tab {
                Tab::Short => <spacer/>
                Tab::Long if wide => <icon svg={icons::LOADER} size={13.0} color={p.sidebar_muted} class="x" />
                Tab::Block => { <a/> <b/> }


                Tab::Bare => label.clone(),
                Tab::Empty => {}
                Tab::Commas => <c/>,
                Tab::Paired => <div class="w-full flex-col items-center gap-2 overflow-hidden rounded-[11]">
                    <text>"hello"</text>
                </div>
            }
        </div>
    }
}
