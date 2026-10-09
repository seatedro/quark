// Views inside `vec![..]`, which is read as Rust. text_demo.rs line 363 sits
// outside any view; settings.rs lines 313, 334, and 422 sit in a `vec!` in
// a child expression, the last with a view inside a view inside the `vec!`.
// crates/quark-app/examples/text_demo.rs, line 363 (spacing roughed up)
impl TextDemo {
    fn toolbar(&self) -> Vec<AnyElement> {
        vec![
            Self::row(vec![view! {
                <text class="text-sm"  color={colors.text_muted}>{sample}</text>
            }]),
        ]
    }
}

// examples/codex/src/settings.rs, line 313 (spacing roughed up)
fn general() {
    view! {
        <div class="flex-col" w={w}>
            {group(
                p,
                vec![
                    setting_row(
                        p,
                        "Default permissions",
                        Some("By default, ChatGPT can read and edit files in its workspace."),
                        view! {<div class="opacity-60">{toggle(p, true)}</div>},
                        w,
                    ),
                ],
            )}
        </div>
    }
}

// examples/codex/src/settings.rs, line 334
fn general_folder() {
    view! {
        <div class="flex-col" w={w}>
            {group(
                p,
                vec![
                    setting_row_wrap(
                        p,
                        "Projectless task folder",
                        Some("The location where tasks started outside of projects store their data by default."),
                        view! {
                            <div class="flex-row items-center gap-3">
                                <text size={12.5} color={p.muted} class="font-mono whitespace-nowrap">
                                    "/Users/rohit/…uments/Codex"
                                </text>
                                <div class="flex-row items-center h-7 px-[10] rounded-[8]"
                                     bg={p.rail_tile.lerp(settings_card(p), 0.3)}>
                                    <txt("Change", SMALL, p.text) />
                                </div>
                            </div>
                        },
                        250.0,
                    ),
                ],
            )}
        </div>
    }
}

// examples/codex/src/settings.rs, line 422
fn appearance() {
    view! {
        <div class="flex-col" w={w}>
            {section_title(p, "Visual style")}
            {group(
                p,
                vec![view! {
                    <setting_row(p, "Mode", None, view! {
                        <div class="flex-row items-center gap-4">
                            {preview(ThemeChoice::System, white, black)}
                            {preview(ThemeChoice::Light, white, white)}
                            {preview(ThemeChoice::Dark, black, black)}
                        </div>
                    }, w) class="h-[76]" />
                }],
            )}
        </div>
    }
}
