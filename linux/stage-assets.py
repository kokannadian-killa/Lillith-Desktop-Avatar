#!/usr/bin/env python3
"""Installs Lillith's animation set: manifest.toml plus only the atlas pages
(`page = ...`) and pose videos (`video = ...`) it references, not the loose
frames. The source is only read, never changed. A video that's missing or
still a Git LFS pointer is skipped (she plays that pose from its PNG page).

Usage:
    python3 linux/stage-assets.py --check SRC         # usable? (exit 0 / 3 = Git LFS pointers / 1)
    python3 linux/stage-assets.py SRC DEST            # copy into DEST (= .../lillith/generated)

SRC is a folder holding manifest.toml (assets/generated, or a folder that
contains assets/generated), or a .zip that does (e.g. the Windows portable
zip Lillith-<ver>-windows-x64.zip from the GitHub release). DEST is replaced
atomically (new copy first, then swap), and left alone if already identical.

Exit codes: 0 ok, 1 error, 3 the pages are Git LFS pointer stubs (a GitHub
"Download ZIP" / main.zip of a repo whose archives don't include LFS files).
"""
import os
import shutil
import sys
import tomllib
import zipfile
from pathlib import Path, PurePosixPath

PNG_MAGIC = b"\x89PNG\r\n\x1a\n"
LFS_MAGIC = b"version https://git-lfs"
LFS_EXIT = 3


def refs(node, key):
    if isinstance(node, dict):
        for k, v in node.items():
            if k == key and isinstance(v, str) and v:
                yield v
            else:
                yield from refs(v, key)
    elif isinstance(node, list):
        for v in node:
            yield from refs(v, key)


def pages(node):
    return refs(node, "page")


def safe(p: str) -> bool:
    return not (Path(p).is_absolute() or ".." in Path(p).parts)


class Source:
    """A folder or a zip with manifest.toml + pages, read-only."""

    def __init__(self, src: Path):
        self.zip = None
        if src.is_file() and zipfile.is_zipfile(src):
            self.zip = zipfile.ZipFile(src)
            names = [n for n in self.zip.namelist() if n.endswith("manifest.toml")]
            names.sort(key=lambda n: (not n.endswith("assets/generated/manifest.toml"), len(n)))
            if not names:
                raise SystemExit(f"error: no manifest.toml inside {src}")
            self.base = str(PurePosixPath(names[0]).parent)
            self.members = set(self.zip.namelist())
        else:
            for cand in (src, src / "assets" / "generated", src / "generated"):
                if (cand / "manifest.toml").is_file():
                    self.base = cand
                    break
            else:
                raise SystemExit(f"error: no manifest.toml in {src} (or its assets/generated)")

    def exists(self, rel: str) -> bool:
        if self.zip:
            return f"{self.base}/{rel}" in self.members
        return (self.base / rel).is_file()

    def head(self, rel: str, n: int = 64) -> bytes:
        if self.zip:
            with self.zip.open(f"{self.base}/{rel}") as f:
                return f.read(n)
        with open(self.base / rel, "rb") as f:
            return f.read(n)

    def size(self, rel: str) -> int:
        if self.zip:
            return self.zip.getinfo(f"{self.base}/{rel}").file_size
        return (self.base / rel).stat().st_size

    def copy(self, rel: str, dst: Path) -> None:
        dst.parent.mkdir(parents=True, exist_ok=True)
        if self.zip:
            with self.zip.open(f"{self.base}/{rel}") as f, open(dst, "wb") as o:
                shutil.copyfileobj(f, o, 1 << 20)
        else:
            shutil.copy2(self.base / rel, dst)

    def where(self) -> str:
        return f"{self.zip.filename}:{self.base}" if self.zip else str(self.base)


def check(src: Source) -> tuple[int, list[str]]:
    manifest = tomllib.loads(src.head("manifest.toml", 1 << 26).decode("utf-8"))
    wanted = sorted(set(pages(manifest)))
    if not wanted:
        print(f"error: {src.where()}/manifest.toml lists no atlas pages", file=sys.stderr)
        return 1, []
    bad = [p for p in wanted if Path(p).is_absolute() or ".." in Path(p).parts]
    if bad:
        print(f"error: pages outside the set: {bad[:3]}", file=sys.stderr)
        return 1, []
    missing = [p for p in wanted if not src.exists(p)]
    if missing:
        print(f"error: {len(missing)} atlas pages missing in {src.where()}, e.g. {missing[0]}",
              file=sys.stderr)
        return 1, []
    lfs = [p for p in wanted if src.head(p).startswith(LFS_MAGIC)]
    if lfs:
        print(f"{len(lfs)} of {len(wanted)} atlas pages in {src.where()} are Git LFS pointer "
              f"stubs, not images (e.g. {lfs[0]}).", file=sys.stderr)
        return LFS_EXIT, []
    notpng = [p for p in wanted if p.lower().endswith(".png") and not src.head(p).startswith(PNG_MAGIC)]
    if notpng:
        print(f"error: not a PNG: {notpng[0]}", file=sys.stderr)
        return 1, []
    vids = sorted(set(v for v in refs(manifest, "video") if safe(v)) - set(wanted))
    usable = [v for v in vids if src.exists(v) and not src.head(v).startswith(LFS_MAGIC)]
    if len(usable) < len(vids):
        print(f"  note: {len(vids) - len(usable)} of {len(vids)} pose videos missing or Git LFS "
              f"pointers; those poses play from their PNG pages", file=sys.stderr)
    return 0, wanted + usable


def up_to_date(src: Source, wanted: list[str], dest: Path) -> bool:
    m = dest / "manifest.toml"
    if not m.is_file() or m.read_bytes() != src.head("manifest.toml", 1 << 26):
        return False
    return all((dest / p).is_file() and (dest / p).stat().st_size == src.size(p) for p in wanted)


def main(argv: list[str]) -> int:
    if len(argv) == 2 and argv[0] == "--check":
        rc, wanted = check(Source(Path(argv[1])))
        if rc == 0:
            print(f"ok: manifest + {len(wanted)} pages and videos")
        return rc
    if len(argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    src, dest = Source(Path(argv[0])), Path(argv[1])
    rc, wanted = check(src)
    if rc:
        return rc
    if up_to_date(src, wanted, dest):
        print(f"animation set already up to date in {dest}")
        return 0
    tmp = dest.with_name(f".{dest.name}.new-{os.getpid()}")
    old = dest.with_name(f".{dest.name}.old-{os.getpid()}")
    shutil.rmtree(tmp, ignore_errors=True)
    total = 0
    try:
        src.copy("manifest.toml", tmp / "manifest.toml")
        for p in wanted:
            src.copy(p, tmp / p)
            total += (tmp / p).stat().st_size
        if dest.exists():
            dest.rename(old)
        tmp.rename(dest)
    except BaseException:
        shutil.rmtree(tmp, ignore_errors=True)
        if old.exists() and not dest.exists():
            old.rename(dest)
        raise
    shutil.rmtree(old, ignore_errors=True)
    print(f"installed manifest + {len(wanted)} pages and videos ({total / 1e6:.0f} MB) -> {dest}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
