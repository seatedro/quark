// crates/quark-components/src/modal.rs, line 129
fn excerpt() {
    let panel = view! { scale,
        <div class="flex-col overflow-hidden"
             w={panel_width} px={padding_x} py={padding_y} gap={gap}
             bg={tc.elevated_surface} rounded={radius}
             border_b={tc.border} shadow_preset={shadows.layers()}
             on:click={quark_ui::element::NoopAction}
             id={format!("modal:{accessibility_label}")}
             test-id="modal"
             role="dialog"
             focus_scope={accessibility_label.clone()}
             trap_focus={true}
             accessibility_id={format!("modal:{accessibility_label}")}
             aria-label={accessibility_label}
             @when {self.height.is_some()} { h={(self.height.unwrap() * scale).round().min(max_h)} }>
            {header}
            {...self.body}
            if !self.footer.is_empty() {
                <spacer />
                <div class="flex-row" gap={Sp::LG}>
                    {...self.footer}
                </div>
            }
        </div>
    };
}
