// Synthetic: CRLF newlines and wide characters before the macro.
fn excerpt() {
    let 名前 = view! { <div class="p-2" title="標準" aria-label="これはとても長いラベルです" on:click={go} /> };
}
