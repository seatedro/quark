// examples/codex/src/thread.rs, line 134
fn excerpt() {
    view! { -> Div,
        <div class="flex-col shrink-0 pt-2" w={column}>
            for (i, item) in items.iter().enumerate() {
                let key = format!("item-{i}");
                match item {
                    Item::User { text: body, .. } => {
                        <div class="w-full flex-col items-end pt-[33]" key={key}>
                            <div
                                class="px-4 py-[9] rounded-[16]"
                                max_w={(column * 0.7).floor()}
                                bg={p.bubble}
                                accessibility_role={Role::Group}
                                aria-label="You said:"
                            >
                                <text
                                    size={BODY}
                                    color={p.bubble_text}
                                    line_height_points={LINE_PT}
                                >
                                    {body.clone()}
                                </text>
                            </div>
                            <div class="h-12" />
                        </div>
                    }
                    Item::Starting => {
                        <div class="w-full h-10" key={key}>
                            <txt("Starting your task", BODY, p.faint) />
                        </div>
                    }
                    Item::Thinking => {
                        <div class="w-full h-10" key={key}>{shimmer("Thinking", p, BODY)}</div>
                    }
                    Item::Error(msg) => {
                        <div class="w-full flex-col" key={key}>
                            <div
                                class="flex-row items-center w-full px-[13] py-[10] gap-3 rounded-[11]"
                                bg={p.notice}
                                border={p.notice_border}
                                role="alert"
                            >
                                <div class="self-start pt-0.5">
                                    <icon svg={icons::ALERT} size={17.0} color={p.text} />
                                </div>
                                <div class="flex-1 min-w-0">
                                    <text size={BODY} color={p.text} line_height={1.5}>
                                        {msg.clone()}
                                    </text>
                                </div>
                            </div>
                            <div class="h-5" />
                        </div>
                    }
                    Item::ModelChanged { from, to } => {
                        <div class="w-full flex-col" key={key}>
                            <div class="flex-row items-center w-full h-[22] gap-2">
                                <div class="flex-1 h-px" bg={p.hairline} />
                                <icon svg={icons::CUBE} size={14.0} color={p.divider_text} />
                                <txt(
                                    format!("Model changed from {from} to {to}."),
                                    BODY,
                                    p.divider_text
                                )
                                />
                                <icon svg={icons::INFO} size={12.0} color={p.divider_text} />
                                <div class="flex-1 h-px" bg={p.hairline} />
                            </div>
                            <div class="h-[23]" />
                        </div>
                    }
                    Item::Work {
                        took,
                        running,
                        open,
                        steps,
                    } => {
                        <work(p, i, took, *running, *open, steps, embed) key={key} />
                    }
                    Item::Answer { blocks, .. } => {
                        <answer(p, blocks, column) key={key} />
                    }
                    Item::FileChange { file } => {
                        <file_change(p, file, embed.stats) key={key} />
                    }
                }
            }
        </div>
    }
}
