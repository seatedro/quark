"""Subset Noto Sans CJK SC to the common CJK repertoire.

Han: GB 2312 + JIS X 0208 + Big5 level 1 + KS X 1001 hanja. Hangul: all
11,172 syllables when --all-hangul, else the 2,350 of KS X 1001. Plus kana,
bopomofo, CJK punctuation, fullwidth forms, and Basic Latin.

    uv run --no-project --with fonttools python subset-cjk.py \
        NotoSansCJKsc-Regular.otf QuarkCJKFallback-Regular.otf --all-hangul
"""
import argparse
from fontTools import subset
from fontTools.ttLib import TTFont

def two_bytes(cp, codec):
    try:
        return len(chr(cp).encode(codec)) == 2
    except UnicodeEncodeError:
        return False

def big5_level1(cp):
    try:
        b = chr(cp).encode('big5')
    except UnicodeEncodeError:
        return False
    return len(b) == 2 and 0xA440 <= int.from_bytes(b, 'big') <= 0xC67E

def charset(all_hangul):
    cps = set(range(0x20, 0x7F))
    for lo, hi in [(0x2E80, 0x2EFF), (0x3000, 0x30FF), (0x3100, 0x312F), (0x3130, 0x318F),
                   (0x31A0, 0x31FF), (0xFE30, 0xFE4F), (0xFF00, 0xFFEF)]:
        cps.update(range(lo, hi + 1))
    for cp in range(0x4E00, 0xA000):
        if (two_bytes(cp, 'gb2312') or two_bytes(cp, 'euc_jp') or big5_level1(cp)
                or two_bytes(cp, 'euc_kr')):
            cps.add(cp)
    for cp in range(0xAC00, 0xD7A4):
        if all_hangul or two_bytes(cp, 'euc_kr'):
            cps.add(cp)
    return cps

ap = argparse.ArgumentParser()
ap.add_argument('src'); ap.add_argument('out'); ap.add_argument('--all-hangul', action='store_true')
a = ap.parse_args()
opts = subset.Options()
# No kern: CJK sets solid, one em per character, which keeps mono columns.
opts.layout_features = []
opts.drop_tables += ['vhea', 'vmtx', 'VORG', 'BASE', 'DSIG']
opts.hinting = False
opts.name_IDs = [0, 1, 2, 3, 4, 5, 6, 13, 14]
font = TTFont(a.src)
sub = subset.Subsetter(opts)
cps = charset(a.all_hangul)
sub.populate(unicodes=cps)
sub.subset(font)
# Renamed: a subset must not pass for the full font, which system fallback
# lists name ("Noto Sans CJK SC").
name = font['name']
for rec in list(name.names):
    if rec.nameID in (1, 3, 4, 6, 16, 17):
        name.removeNames(nameID=rec.nameID)
family = 'Quark CJK Fallback'
name.setName(family, 1, 3, 1, 0x409); name.setName(family, 1, 1, 0, 0)
name.setName(family + ' Regular', 4, 3, 1, 0x409)
name.setName('QuarkCJKFallback-Regular', 6, 3, 1, 0x409)
name.setName('QuarkCJKFallback-Regular;subset of Noto Sans CJK SC 2.004', 3, 3, 1, 0x409)
font['CFF '].cff.fontNames = ['QuarkCJKFallback-Regular']
font.save(a.out)
print(len(cps), 'codepoints', len(font.getGlyphOrder()), 'glyphs')
