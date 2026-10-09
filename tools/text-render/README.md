# Text render reports

`report.sh` renders the fixtures in `crates/quark-render/tests/native_text` (UI strings, code, CJK, color emoji, variable and static weights) at scales 1, 1.5, and 2 and writes `report.tsv` plus one PNG per row. Given a baseline directory, it also prints per-row deltas, pixel differences, and gate failures. The script header lists its environment variables.

```bash
tools/text-render/report.sh /tmp/candidate /path/to/baseline
```

The harness needs a GPU device. On a Linux machine without one, point Vulkan at Mesa's lavapipe with `VK_DRIVER_FILES=<mesa>/share/vulkan/icd.d/lvp_icd.<arch>.json`. Lavapipe frame times are CPU diagnostics, not GPU performance.

## Baseline

`baseline/report.tsv` records the master `8efa336` renderer (vendored glyphon and swash) with 1,000 repeated frames per row, in a release build on llvmpipe (aarch64). Three runs agreed on every glyph, upload, atlas, and repeated-frame allocation count; first-frame allocations varied by one. Warm frame medians varied by up to 15% between runs on this shared machine, so compare times only between alternating runs on one machine.

Observations from that baseline:

- Repeated frames rasterize and upload nothing at every scale. They make one more allocation than the same frame without text (92 against 91).
- The first frame rasterizes one bitmap per unique glyph and scale. The `weights` fixture uploads 41.6 KB at 1x, 88.4 KB at 1.5x, and 150.4 KB at 2x. The `emoji` fixture uploads 183.7 KB at 2x for 21 glyphs.
- The atlas starts as a 256-pixel mask texture and a 256-pixel color texture (320 KiB). Color emoji at 2x grow the color texture once (1.06 MiB in total). The `weights` fixture grows the mask texture once at 1.5x and 2x (512 KiB in total).
- Measured by the existing UI allocation tests on the same commit: workbench repeated frame 45 (52 with accessibility), streamed update 245, sidebar scroll 255 over a repeated frame for 3 entering rows, transcript scroll 54 for 2 entering rows; quark-app list repeated frame 0, transcript repeated frame 16, streamed transcript frame 116 (145 with accessibility).
