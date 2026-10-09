// crates/quark-components/src/table.rs, line 964
fn excerpt() {
    view! {
        <div
            w={width}
            h={height}
            class="flex-col overflow-clip"
            track_focus={t.focus}
            @when {env.accessible} {
                accessibility_id={t.id}
                accessibility_role={Role::Table}
                aria-label={t.label}
                accessibility_table_size={(t.view.len() + 1, t.order.len())}
                aria-multiselectable={true}
            }
            @for &(binding, key) in KEYS { on_key={(binding, on_event(TableEvent::Key(key)))} }
        >
            <div class="w-full" h={t.header_height} class="shrink-0 overflow-clip">
                // The header stays put vertically and follows the body
                // horizontally.
                <div
                    w={t.total_width()}
                    h={t.header_height}
                    class="flex-row shrink-0"
                    translate={(-t.scroll_x, 0.0)}
                    bg={colors.header}
                    border_b={colors.border}
                    @when {env.accessible} {
                        accessibility_id={format!("{}.header", t.id)}
                        accessibility_role={Role::Row}
                        aria-rowindex={0}
                    }
                >
                    <div w={t.column_left(columns.start)} class="shrink-0" />
                    for pos in columns.clone() {
                        {header_cell(t, pos, reorder_target, colors, env, on_event)}
                    }
                </div>
            </div>
            <div
                class="w-full"
                h={t.body_height()}
                class="flex-col"
                scroll_y={t.scroll_y}
                scroll_total={window.total_extent}
                on:scroll={ScrollActionBuilder::new(move |lines| on_event(TableEvent::Scroll(lines)))
                    .with_to_px(move |px| on_event(TableEvent::ScrollTo(px as f32)))}
                scroll_x={t.scroll_x}
                scroll_total_x={t.total_width()}
                on:scroll_x={ScrollActionBuilder::new(move |lines| on_event(TableEvent::ScrollX(lines)))
                    .with_to_px(move |px| on_event(TableEvent::ScrollXTo(px as f32)))}
                @when {t.scrollbar_auto_hide} {
                    scrollbar_visibility={&t.scrollbar}
                    class="scrollbar-auto-hide"
                }
            >
                <div class="w-full shrink-0" h={window.top_spacer} />
                for display in window.range {
                    {table_row(t, data, display, columns.clone(), colors, env, on_event)}
                }
                <div class="w-full shrink-0" h={window.bottom_spacer} />
            </div>
        </div>
    }
}
