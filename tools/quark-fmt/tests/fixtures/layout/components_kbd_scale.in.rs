// crates/quark-components/src/kbd.rs, line 13
fn excerpt() {
    view! { scale,
        <div class="shrink-0 items-center justify-center"
             px={Sp::SM} py={Sp::XXS}
             bg={tc.element_background}
             border={tc.border_variant}
             rounded={Rad::MD}
             shadow={(1.0, 1.0, tc.border_variant.with_alpha(Alpha::MEDIUM))}>
            <text class="text-xs font-mono text-center" color={tc.text}>{&label}</text>
        </div>
    }
}
