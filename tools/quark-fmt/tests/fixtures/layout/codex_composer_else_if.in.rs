// examples/codex/src/composer.rs, line 249
fn excerpt() {
    view! {
        <div class="flex-row items-center h-7 pl-2 pr-2 gap-[5]">
            <icon_button(p, icons::PLUS, 28.0, 16.0, p.text_soft, "Add files and more",
                         Msg::Open(Menu::Add)) id="pill.add" />
            <pill(p, "pill.permissions", "Change permissions", Msg::Open(Menu::Permissions),
                  app.menu == Some(Menu::Permissions))>
                <icon svg={approval_icon} size={15.0} color={tint} />
                if full {
                    <txt(approval, SMALL, tint) />
                }
            </pill>
            <div class="flex-1" />
            <pill(p, "pill.model", &model_label, Msg::Open(Menu::Model), model_open)
                  @when {full && model_open} { class="w-[146] justify-center" }
                  @when {full && !model_open} { class="gap-1" }>
                if !full {
                    <icon svg={icons::BRAIN} size={15.0} color={p.muted} />
                } else if model_open {
                    <txt("Select effort", SMALL, p.muted) />
                    <div class="w-[18]" />
                    <icon svg={icons::CHEVRON_DOWN} size={11.0} color={p.muted} />
                } else {
                    <txt(crate::MODEL, SMALL, p.text_soft) />
                    <txt(crate::EFFORTS[app.effort], SMALL, p.muted) />
                    <div class="w-0.5" />
                    <icon svg={icons::CHEVRON_DOWN} size={11.0} color={p.muted} />
                }
            </pill>
            <icon_button(p, icons::MIC, 28.0, 15.0, p.text_soft, "Dictate", Msg::Noop) />
            <div class="w-2" />
            {send_button(app, p)}
        </div>
    }
}
