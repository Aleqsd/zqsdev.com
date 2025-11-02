#!/usr/bin/env python3

from __future__ import annotations

import argparse
import re
from pathlib import Path


COMMENT_RE = re.compile(r"/\*[^!][\s\S]*?\*/")
WHITESPACE_RE = re.compile(r"\s+")
PUNCTUATION_RE = re.compile(r"\s*([{};:,>])\s*")


def minify_css(source: str) -> str:
    """Perform a lightweight CSS minification suitable for build-time use."""
    stripped = COMMENT_RE.sub("", source)
    condensed = WHITESPACE_RE.sub(" ", stripped)
    tightened = PUNCTUATION_RE.sub(r"\1", condensed)
    return tightened.replace(";}", "}").strip()


def main() -> None:
    parser = argparse.ArgumentParser(description="Minify a CSS file.")
    parser.add_argument("input", type=Path, help="Path to the source CSS file")
    parser.add_argument(
        "-o",
        "--output",
        type=Path,
        default=None,
        help="Where to write the minified CSS (defaults to overwriting input)",
    )
    args = parser.parse_args()

    css = args.input.read_text(encoding="utf-8")
    minified = minify_css(css)

    output_path = args.output or args.input
    output_path.write_text(minified, encoding="utf-8")


if __name__ == "__main__":
    main()
