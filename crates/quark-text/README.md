# quark-text

Text for [Quark](../../README.md): fonts, shaping with cosmic-text, layout,
hit testing, and a layout cache. One `TextLayout` serves measurement,
hit testing, selection, and painting, so the caret a click lands on is the
caret that is drawn. Positions are `TextOffset`s, byte offsets on grapheme
boundaries.

## Features

| Feature | Default | Effect |
|---|---|---|
| `emoji-font` | yes | Bundles Noto Color Emoji (10.7 MB) as the emoji fallback |
| `cjk-font` | yes | Bundles a 3.7 MB subset of Noto Sans CJK SC as the CJK fallback |
| `profile` | no | A profiler scope and tracing span around shaping |

## License

The code is MIT. The fonts in [assets/fonts](assets/fonts) are under the
SIL Open Font License 1.1, each with its license file beside it.
