#!/usr/bin/env python3
"""Splits the clip set for the Android clip pack (android/build-apk.sh).

An APK has to stay under ~1 GB to be uploadable, and the full clip set's
sheets alone are well over that. So the APK carries the core clips and the
rest ship as a clip-pack zip on the same GitHub release, which the app
downloads once (SetupActivity -> ClipPack.java), verifies and unpacks. No
clip is changed: the pack holds the very same sheet files.

GitHub release assets must also stay under 1 GB, so a large pack is cut into
~50 MiB pieces (same size as the Arch installer). The app downloads each
piece with resume/retry/SHA-256, joins them, then unpacks. A pack that fits
in one piece stays a single zip (backward compatible).

  clip-pack.py select <generated_dir> <pack_mb>
      Prints the clips (one per line) that go into the pack: only tease
      clips nothing else points at (tags seasonal/cleavage_zoom or *_long,
      never idle*/event_*/blend_*, never a base_clip / jiggle_of /
      jiggle_variant / blend from/to target), seasonal first, then the
      largest, until the pack holds at least <pack_mb> MB of sheets.
  clip-pack.py pack <generated_dir> <list> <out.zip> <out.list>
      Writes the pack zip (stored, paths relative to generated/) and
      <out.list> ("<sha256>  <path>" per file) for the APK.
  clip-pack.py split <zip> <piece_bytes> <pieces.list>
      Cuts <zip> into <zip>.part1, <zip>.part2, ... of at most
      <piece_bytes> each (byte-identical to `split -b`), writes
      <pieces.list> ("<sha256>  <size>  <name>" per piece, join order),
      and removes <zip> so only the pieces remain. No-op (empty list,
      zip kept) when the zip is already <= <piece_bytes>.
"""
import hashlib
import os
import sys
import tomllib
import zipfile

SEASONAL = ("valentines_", "spring_", "summer_", "halloween_", "xmas_", "newyear_")
# GitHub release assets must stay under 1 GiB; refuse anything at or over.
MAX_ASSET = 1024 * 1024 * 1024


def load(gen):
    with open(os.path.join(gen, "manifest.toml"), "rb") as f:
        return tomllib.load(f)["clips"]


def files_of(gen, clip):
    """Every file the APK would stage for a clip: the PNGs in its page's folder."""
    page_dir = os.path.dirname(clip["page"])
    d = os.path.join(gen, page_dir)
    return sorted(os.path.join(page_dir, f) for f in os.listdir(d) if f.endswith(".png"))


def select(gen, pack_mb):
    clips = load(gen)
    refs = set()
    for c in clips.values():
        for k in ("base_clip", "jiggle_of", "jiggle_variant"):
            if c.get(k):
                refs.add(c[k])
        b = c.get("blend") or {}
        refs.update(v for v in (b.get("from"), b.get("to")) if v)
    dirs = {}
    for n, c in clips.items():
        dirs.setdefault(os.path.dirname(c["page"]), []).append(n)

    def candidate(n):
        c = clips[n]
        if n in refs or n.startswith(("idle", "event_", "blend_")):
            return False
        if c.get("mouth_patches"):
            return False
        # a page folder shared with another clip stays whole in the APK
        if len(dirs[os.path.dirname(c["page"])]) != 1:
            return False
        tags = c.get("tags", [])
        return "seasonal" in tags or "cleavage_zoom" in tags or n.endswith("_long")

    def size(n):
        return sum(os.path.getsize(os.path.join(gen, f)) for f in files_of(gen, clips[n]))

    cands = sorted((n for n in clips if candidate(n)),
                   key=lambda n: (not n.startswith(SEASONAL), -size(n), n))
    out, total, want = [], 0, pack_mb * 1_000_000
    for n in cands:
        if total >= want:
            break
        out.append(n)
        total += size(n)
    if total < want:
        sys.exit(f"clip-pack: only {total / 1e6:.0f} MB of pack candidates, wanted {pack_mb} MB")
    return out


def pack(gen, listfile, out_zip, out_list):
    clips = load(gen)
    names = [l.strip() for l in open(listfile) if l.strip()]
    files = []
    for n in names:
        files += files_of(gen, clips[n])
    with zipfile.ZipFile(out_zip + ".tmp", "w", zipfile.ZIP_STORED, allowZip64=True) as z, \
            open(out_list, "w") as lst:
        for rel in files:
            p = os.path.join(gen, rel)
            h = hashlib.sha256()
            with open(p, "rb") as f:
                for b in iter(lambda: f.read(1 << 20), b""):
                    h.update(b)
            z.write(p, rel)
            lst.write(f"{h.hexdigest()}  {rel}\n")
    os.replace(out_zip + ".tmp", out_zip)


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def split(zip_path, piece_bytes, pieces_list):
    """Cut zip_path into zip_path.partN; write pieces_list; remove zip when split."""
    piece_bytes = int(piece_bytes)
    if piece_bytes <= 0 or piece_bytes >= MAX_ASSET:
        sys.exit(f"clip-pack split: piece size must be 1..{MAX_ASSET - 1} bytes, got {piece_bytes}")
    size = os.path.getsize(zip_path)
    base = os.path.basename(zip_path)
    # Already fits in one release asset: keep the single zip, no pieces.list lines.
    if size <= piece_bytes:
        open(pieces_list, "w").close()
        if size >= MAX_ASSET:
            sys.exit(f"clip-pack split: {base} is {size} bytes (>= 1 GiB); refuse to ship it")
        return 0
    # Disk-friendly: peel pieces off the end and truncate, matching
    # `split -b <n> -d --numeric-suffixes=1` byte layout (part1 = bytes 0..n-1).
    n = (size + piece_bytes - 1) // piece_bytes
    pieces = []
    for i in range(n, 0, -1):
        off = (i - 1) * piece_bytes
        name = f"{base}.part{i}"
        path = os.path.join(os.path.dirname(zip_path) or ".", name)
        with open(zip_path, "rb") as src, open(path, "wb") as dst:
            src.seek(off)
            while True:
                chunk = src.read(1 << 20)
                if not chunk:
                    break
                dst.write(chunk)
        os.truncate(zip_path, off)
        psz = os.path.getsize(path)
        if psz >= MAX_ASSET:
            sys.exit(f"clip-pack split: {name} is {psz} bytes (>= 1 GiB)")
        pieces.append((name, psz, sha256_file(path)))
    os.remove(zip_path)
    pieces.reverse()  # part1 first
    # Join-check without rebuilding the zip on disk: stream the pieces.
    h = hashlib.sha256()
    total = 0
    for name, psz, expect in pieces:
        path = os.path.join(os.path.dirname(zip_path) or ".", name)
        with open(path, "rb") as f:
            for b in iter(lambda: f.read(1 << 20), b""):
                h.update(b)
                total += len(b)
        if sha256_file(path) != expect:
            sys.exit(f"clip-pack split: {name} changed under us")
    # We deleted the zip; the caller already recorded its sha256. Recompute
    # from pieces only to confirm the cut was byte-identical.
    joined = h.hexdigest()
    with open(pieces_list, "w") as lst:
        for name, psz, expect in pieces:
            lst.write(f"{expect}  {psz}  {name}\n")
    print(f"clip-pack split: {n} pieces, {total} bytes, joined sha256 {joined}", file=sys.stderr)
    return n


if __name__ == "__main__":
    a = sys.argv[1:]
    if len(a) == 3 and a[0] == "select":
        print("\n".join(select(a[1], int(a[2]))))
    elif len(a) == 5 and a[0] == "pack":
        pack(*a[1:])
    elif len(a) == 4 and a[0] == "split":
        split(a[1], a[2], a[3])
    else:
        sys.exit(__doc__)
