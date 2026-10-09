// examples/codex/src/widgets.rs, icon_button
fn icon_button() -> Div {
    view! { -> Div,
        <div
            w={box_size}
            h={box_size}
            class="shrink-0 items-center justify-center rounded-[7]"
            hover_bg={p.row_hover.with_alpha(110)}
            role="button"
            aria-label={label.to_owned()}
            on:click={action}
        >
            <icon svg={svg} size={icon_size} color={color} />
        </div>
    }
}
