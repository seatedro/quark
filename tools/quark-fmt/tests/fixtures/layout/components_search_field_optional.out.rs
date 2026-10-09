// crates/quark-components/src/search_field.rs, line 34
fn excerpt() {
    view! {
        <div
            class="w-full flex-row items-center"
            gap={m.spacing_sm}
            px={m.spacing_sm + Sp::XXS}
            py={m.spacing_xs}
            rounded={m.control_radius}
            border={tc.border_variant}
        >
            <icon svg={lucide::SEARCH} size={Ico::XS} color={tc.text_muted} />
            <div class="flex-1" min-w={0.0}>{input}</div>
            {?trailing}
        </div>
    }
}
