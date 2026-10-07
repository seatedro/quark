#!/usr/bin/env python3
"""Writes each example of a CommonMark spec.txt as one file in OUT_DIR, so
the markdown fuzz target can replay them (`-runs=0`) through
MarkdownDoc::parse and verify_integrity.

This checks that every spec input parses into a document whose invariants
hold. It does not compare rendered output with the spec's expected HTML.

    commonmark_corpus.py spec.txt OUT_DIR
"""

import os
import sys

FENCE = "`" * 32


def examples(spec):
    lines = iter(spec.splitlines(keepends=True))
    for line in lines:
        if line.rstrip("\n") != f"{FENCE} example":
            continue
        markdown = []
        for body in lines:
            if body.rstrip("\n") == ".":
                break
            markdown.append(body)
        # Skip the expected HTML up to the closing fence.
        for body in lines:
            if body.rstrip("\n") == FENCE:
                break
        # The spec writes tabs as U+2192 so they stay visible.
        yield "".join(markdown).replace("→", "\t")


def main():
    spec_path, out = sys.argv[1], sys.argv[2]
    with open(spec_path, encoding="utf-8") as f:
        spec = f.read()
    os.makedirs(out, exist_ok=True)
    count = 0
    for count, markdown in enumerate(examples(spec), start=1):
        with open(os.path.join(out, f"example_{count:03}"), "w", encoding="utf-8") as f:
            f.write(markdown)
    if count == 0:
        sys.exit(f"no examples found in {spec_path}")
    print(f"wrote {count} examples to {out}")


if __name__ == "__main__":
    main()
