#!/usr/bin/env python3
"""Compare CoreText reference sheets with quark's swash sheets.

    compare.py OUT_DIR [--ct DIR] [--report FILE]

OUT_DIR is what run.sh produced: manifest.json, swash/<sheet>.png, and the
CoreText sheets (ct/<sheet>.<mode>.png, or --ct DIR). Standard library only,
so it runs on a stock macOS python3.

Coverage is recovered from a CoreText sheet as (C - B) / (F - B) for the
mode's text gray F and background gray B, and read directly from a swash
sheet. For every line (case) it reports:

  ink      total coverage, in pixels; `ratio` is CoreText over swash
  stem     mean horizontal coverage across the `l` stems, in pixels, over
           rows between 10% and 45% of an em above the baseline
  rows     sum over rows of |row ink difference| / swash ink: where the
           vertical distribution of ink differs (a row flip or baseline
           error shows up here first)
  bbox     ink bounds (left, top, right, bottom) difference in pixels
  mad      mean absolute coverage difference, in 1/255 units, over pixels
           either side inked

The summary groups lines by font, scale, and mode; the sweep section shows
how CoreText's smoothed coverage depends on text and background gray.
"""

import argparse
import json
import math
import os
import struct
import sys
import zlib


def read_png(path):
    """(width, height, channels, rows of bytes) for 8-bit, non-interlaced PNGs."""
    with open(path, "rb") as f:
        data = f.read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"{path}: not a PNG")
    pos, idat, header = 8, [], None
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        kind = data[pos + 4 : pos + 8]
        body = data[pos + 8 : pos + 8 + length]
        pos += 12 + length
        if kind == b"IHDR":
            header = struct.unpack(">IIBBBBB", body)
        elif kind == b"IDAT":
            idat.append(body)
        elif kind == b"IEND":
            break
    width, height, depth, color, _, _, interlace = header
    if depth != 8 or interlace:
        raise ValueError(f"{path}: depth {depth} interlace {interlace} unsupported")
    channels = {0: 1, 2: 3, 4: 2, 6: 4}[color]
    raw = zlib.decompress(b"".join(idat))
    stride = width * channels
    rows, prev = [], bytearray(stride)
    for y in range(height):
        f = raw[y * (stride + 1)]
        line = bytearray(raw[y * (stride + 1) + 1 : (y + 1) * (stride + 1)])
        for i in range(stride):
            a = line[i - channels] if i >= channels else 0
            b = prev[i]
            c = prev[i - channels] if i >= channels else 0
            if f == 1:
                line[i] = (line[i] + a) & 255
            elif f == 2:
                line[i] = (line[i] + b) & 255
            elif f == 3:
                line[i] = (line[i] + (a + b) // 2) & 255
            elif f == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pred = a if pa <= pb and pa <= pc else (b if pb <= pc else c)
                line[i] = (line[i] + pred) & 255
        rows.append(bytes(line))
        prev = line
    return width, height, channels, rows


def gray_rows(path):
    w, h, ch, rows = read_png(path)
    if ch == 1:
        return w, h, rows
    # Luminance-free: color sheets are only ever gray here, take channel 0.
    return w, h, [bytes(r[i * ch] for i in range(w)) for r in rows]


def coverage(rows, top, height, fg, bg):
    """Band rows as lists of coverage floats, recovered for text gray fg and
    background gray bg (both 0..255)."""
    span = fg - bg
    out = []
    for y in range(top, top + height):
        out.append([min(1.0, max(0.0, (v - bg) / span)) for v in rows[y]])
    return out


def metrics(cov, case, scale):
    em = case["size"] * scale
    ink = sum(sum(r) for r in cov)
    inked = [(x, y) for y, r in enumerate(cov) for x, v in enumerate(r) if v > 0]
    if inked:
        xs = [p[0] for p in inked]
        ys = [p[1] for p in inked]
        bbox = (min(xs), min(ys), max(xs), max(ys))
    else:
        bbox = (0, 0, 0, 0)
    top = case["band"][0]
    stems = []
    for g in case["glyphs"]:
        if g[6] != "l":
            continue
        # The glyph's own advance cell: an `l` keeps its ink inside it, and
        # neighbors' stems stay out.
        x0 = g[2]
        x1 = g[2] + math.ceil(g[3] + g[5])
        base = g[4] - top
        lo, hi = base - int(0.45 * em), base - max(1, int(0.10 * em))
        widths = [sum(cov[y][x0:x1]) for y in range(lo, hi + 1)]
        if widths:
            stems.append(sum(widths) / len(widths))
    stem = sum(stems) / len(stems) if stems else 0.0
    return {"ink": ink, "bbox": bbox, "stem": stem, "rows": [sum(r) for r in cov]}


def diff(a_cov, b_cov):
    total, n = 0.0, 0
    for ra, rb in zip(a_cov, b_cov):
        for va, vb in zip(ra, rb):
            if va > 0 or vb > 0:
                total += abs(va - vb)
                n += 1
    return 255 * total / n if n else 0.0


MODES = {
    "light-smooth": (0, 255),
    "light-plain": (0, 255),
    "dark-smooth": (255, 0),
    "dark-plain": (255, 0),
    "light-smooth-auto": (0, 255),
    "light-plain-auto": (0, 255),
}


def sweep_colors(mode):
    parts = mode.split("-")
    fg = round(int(parts[0][2:]) * 255 / 100)
    bg = round(int(parts[1][2:]) * 255 / 100)
    return fg, bg


def panel(args, manifest, ct_dir):
    """Match each on-screen panel block against the offscreen modes."""
    sheet = next(s for s in manifest["sheets"] if s["id"] == args.panel_sheet)
    w, h, ch, rows = read_png(args.panel)
    ox, oy = (int(v) for v in args.panel_origin.split(","))
    out = ["# On-screen panel vs offscreen CoreText\n"]
    out.append(f"Capture {args.panel} ({w}x{h}, {ch} channels), view origin {ox},{oy}.\n")
    out.append("| block | mode | exact pixels | mad /255 | max diff | ink ratio screen/offscreen | stem screen | stem offscreen |")
    out.append("|---|---|---|---|---|---|---|---|")
    for line in open(args.panel_layout):
        column, tone, bx, by, bw, bh = line.split()
        bx, by, bw, bh = int(bx), int(by), int(bw), int(bh)
        # Green channel: the panel draws grays, which stay neutral.
        screen = [
            bytes(rows[oy + by + y][(ox + bx + x) * ch + min(1, ch - 1)] for x in range(bw)) for y in range(bh)
        ]
        fg, bg = (0, 255) if tone == "light" else (255, 0)
        for mode in (f"{tone}-plain", f"{tone}-smooth"):
            sweep_names = {"light": "fg0-bg100", "dark": "fg100-bg0"}
            name = f"{sweep_names[tone]}-{mode.split('-')[1]}" if sheet["kind"] == "sweep" else mode
            off = gray_rows(os.path.join(ct_dir, f"{sheet['id']}.{name}.png"))[2]
            exact = sum(1 for y in range(bh) for x in range(bw) if screen[y][x] == off[y][x])
            diffs = [abs(screen[y][x] - off[y][x]) for y in range(bh) for x in range(bw)]
            inked = [d for y in range(bh) for x, d in enumerate(diffs[y * bw : (y + 1) * bw]) if screen[y][x] != bg or off[y][x] != bg]
            ink_s = ink_o = stem_s = stem_o = 0.0
            for case in sheet["cases"]:
                top, height = case["band"]
                cs = coverage(screen, top, height, fg, bg)
                co = coverage(off, top, height, fg, bg)
                ms, mo = metrics(cs, case, sheet["scale"]), metrics(co, case, sheet["scale"])
                ink_s += ms["ink"]
                ink_o += mo["ink"]
                stem_s += ms["stem"] / len(sheet["cases"])
                stem_o += mo["stem"] / len(sheet["cases"])
            out.append(
                f"| {column} {tone} | {mode} | {exact / (bw * bh):.4f} | {sum(inked) / max(1, len(inked)):.2f}"
                f" | {max(diffs)} | {ink_s / ink_o:.3f} | {stem_s:.3f} | {stem_o:.3f} |"
            )
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--panel", help="a capture of `ct_ref panel` to match instead")
    ap.add_argument("--panel-layout", default="/tmp/ct_ref_panel_layout.txt")
    ap.add_argument("--panel-origin", help="X,Y of the panel view's top-left in the capture")
    ap.add_argument("--panel-sheet", default="panel@1x")
    ap.add_argument("out")
    ap.add_argument("--ct", help="CoreText sheet directory (default OUT/ct)")
    ap.add_argument("--report", help="write the markdown report here too")
    ap.add_argument("--sheets", help="comma-separated sheet ids to compare (default all)")
    args = ap.parse_args()
    ct_dir = args.ct or os.path.join(args.out, "ct")
    manifest = json.load(open(os.path.join(args.out, "manifest.json")))
    only = set(args.sheets.split(",")) if args.sheets else None
    if args.panel:
        text = "\n".join(panel(args, manifest, ct_dir)) + "\n"
        if args.report:
            with open(args.report, "w") as f:
                f.write(text)
        sys.stdout.write(text)
        return
    lines = []
    emit = lines.append

    summary = {}
    sweep = {}
    for sheet in manifest["sheets"]:
        if only and sheet["id"] not in only:
            continue
        scale = sheet["scale"]
        _, _, sw_rows = gray_rows(os.path.join(args.out, "swash", sheet["id"] + ".png"))
        modes = sorted(
            f[len(sheet["id"]) + 1 : -4]
            for f in os.listdir(ct_dir)
            if f.startswith(sheet["id"] + ".") and f.endswith(".png")
        )
        ct_rows = {m: gray_rows(os.path.join(ct_dir, f"{sheet['id']}.{m}.png"))[2] for m in modes}
        for case in sheet["cases"]:
            top, height = case["band"]
            sw = coverage(sw_rows, top, height, 255, 0)
            swm = metrics(sw, case, scale)
            for mode in modes:
                fg, bg = MODES[mode] if mode in MODES else sweep_colors(mode)
                ct = coverage(ct_rows[mode], top, height, fg, bg)
                ctm = metrics(ct, case, scale)
                ratio = ctm["ink"] / swm["ink"] if swm["ink"] else 0.0
                rows = sum(abs(a - b) for a, b in zip(ctm["rows"], swm["rows"])) / (swm["ink"] or 1)
                bbox = tuple(a - b for a, b in zip(ctm["bbox"], swm["bbox"]))
                mad = diff(ct, sw)
                row = {
                    "case": case["id"],
                    "mode": mode,
                    "ink_sw": swm["ink"],
                    "ink_ct": ctm["ink"],
                    "ratio": ratio,
                    "stem_sw": swm["stem"],
                    "stem_ct": ctm["stem"],
                    "rows": rows,
                    "bbox": bbox,
                    "mad": mad,
                }
                key = (sheet["id"], mode)
                summary.setdefault(key, []).append(row)
                if sheet["kind"] == "sweep":
                    sweep.setdefault(case["id"], {})[mode] = row

    emit("# CoreText references vs quark swash\n")
    emit("Per sheet and mode, means over its lines (worst line in brackets).\n")
    emit("| sheet | mode | ink ratio CT/swash | stem swash px | stem CT px | stem delta px | rows diff | max bbox delta px | mad /255 |")
    emit("|---|---|---|---|---|---|---|---|---|")
    for (sheet, mode), rows in summary.items():
        n = len(rows)
        mean = lambda k: sum(r[k] for r in rows) / n  # noqa: E731
        worst_rows = max(rows, key=lambda r: r["rows"])
        worst_bbox = max(max(abs(v) for v in r["bbox"]) for r in rows)
        emit(
            f"| {sheet} | {mode} | {mean('ratio'):.3f} [{min(r['ratio'] for r in rows):.3f}..{max(r['ratio'] for r in rows):.3f}]"
            f" | {mean('stem_sw'):.3f} | {mean('stem_ct'):.3f} | {mean('stem_ct') - mean('stem_sw'):+.3f}"
            f" | {mean('rows'):.3f} [{worst_rows['rows']:.3f}] | {worst_bbox} | {mean('mad'):.1f} |"
        )

    if sweep:
        emit("\n## Smoothing sweep\n")
        emit("CoreText coverage relative to the same line with smoothing off (black on white),")
        emit("per text gray (fg) and background gray (bg), 0 black to 100 white.\n")
        emit("| case | mode | ink / plain ink | stem px | stem - plain stem px |")
        emit("|---|---|---|---|---|")
        for case, modes in sweep.items():
            plain = modes.get("fg0-bg100-plain")
            if not plain:
                continue
            for mode, row in sorted(modes.items()):
                emit(
                    f"| {case} | {mode} | {row['ink_ct'] / plain['ink_ct']:.3f}"
                    f" | {row['stem_ct']:.3f} | {row['stem_ct'] - plain['stem_ct']:+.3f} |"
                )

    emit("\n## Lines\n")
    emit("| case | mode | ink swash | ink CT | ratio | stem swash | stem CT | rows | bbox delta | mad |")
    emit("|---|---|---|---|---|---|---|---|---|---|")
    for rows in summary.values():
        for r in rows:
            emit(
                f"| {r['case']} | {r['mode']} | {r['ink_sw']:.1f} | {r['ink_ct']:.1f} | {r['ratio']:.3f}"
                f" | {r['stem_sw']:.3f} | {r['stem_ct']:.3f} | {r['rows']:.3f} | {r['bbox']} | {r['mad']:.1f} |"
            )

    text = "\n".join(lines) + "\n"
    if args.report:
        with open(args.report, "w") as f:
            f.write(text)
    sys.stdout.write(text)


if __name__ == "__main__":
    main()
