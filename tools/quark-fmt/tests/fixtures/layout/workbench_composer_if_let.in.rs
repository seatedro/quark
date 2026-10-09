// examples/workbench/src/composer/mod.rs, line 329
fn excerpt() {
    view! {
        <div w={width} h={height} class="flex-col items-center" pt={OUTER_TOP * z}
             pb={OUTER_BOTTOM * z} bg={colors.background}>
            <div w={column} class="flex-col" gap={GAP * z}>
                if let Some(error) = error {
                    <div accessibility_role={Role::Alert} aria-label={error.clone()}
                         test_id="composer.error"
                         class="flex-row items-center gap-[6] px-2 rounded-[6]" h={ERROR_HEIGHT * z}>
                        {svg_icon(lucide::ALERT_CIRCLE, 14.0).color(colors.status_error)}
                        <text class="text-xs" color={colors.status_error}>{error}</text>
                    </div>
                }
                <div class="flex-col" rounded={12.0 * z} gap={GAP * z} p={CARD_PAD * z}
                     border={colors.border_variant} bg={colors.surface}>
                    if !state.attachments.items.is_empty() {
                        <div h={CHIPS_HEIGHT * z} class="flex-row overflow-hidden">
                            {attachments::chips(&state.attachments, theme)}
                        </div>
                    }
                    <div px={2.0 * z} py={TEXT_PAD * z}>{editor}</div>
                    {toolbar}
                </div>
            </div>
            if let Some(popup) = popup {
                {popup}
            }
        </div>
    }
}
