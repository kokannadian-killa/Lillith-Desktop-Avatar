#!/usr/bin/env python3
"""Copies only what Lillith reads at runtime from assets/generated into a
staging dir: manifest.toml plus every atlas page it references (`page = ...`
anywhere in the manifest). Source files are only read, never changed.

Usage: python3 windows/stage-assets.py DEST_DIR   (DEST_DIR/assets/generated/...)
"""
import shutil
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GEN = ROOT / "assets" / "generated"


def pages(node):
    if isinstance(node, dict):
        for k, v in node.items():
            if k == "page" and isinstance(v, str) and v:
                yield v
            else:
                yield from pages(v)
    elif isinstance(node, list):
        for v in node:
            yield from pages(v)


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    out = Path(sys.argv[1]) / "assets" / "generated"
    manifest = GEN / "manifest.toml"
    wanted = sorted(set(pages(tomllib.loads(manifest.read_text(encoding="utf-8")))))
    missing = [p for p in wanted if not (GEN / p).is_file()]
    if missing:
        print("missing atlas pages (git lfs pull?):", *missing, sep="\n  ", file=sys.stderr)
        return 1
    out.mkdir(parents=True, exist_ok=True)
    shutil.copy2(manifest, out / "manifest.toml")
    total = 0
    for p in wanted:
        if Path(p).is_absolute() or ".." in Path(p).parts:
            print(f"refusing page outside assets/generated: {p}", file=sys.stderr)
            return 1
        dst = out / p
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(GEN / p, dst)
        total += dst.stat().st_size
    print(f"staged manifest + {len(wanted)} pages ({total / 1e6:.0f} MB) -> {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
