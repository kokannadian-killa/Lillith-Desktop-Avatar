#!/usr/bin/env python3
"""Splits the clip set for the Android clip pack (android/build-apk.sh).

An APK has to stay under ~1 GB to be uploadable, and the 232-clip set's
sheets alone are ~1.35 GB (PNG is already compressed; lossless WebP would
still be ~1 GB). So the APK carries the core clips and the rest ship as one
clip-pack zip on the same GitHub release, which the app downloads once
(SetupActivity -> ClipPack.java), verifies and unpacks. No clip is changed:
the pack holds the very same sheet files.

  clip-pack.py select <generated_dir> <pack_mb>
      Prints the clips (one per line) that go into the pack: only tease
      clips nothing else points at (tags seasonal/cleavage_zoom or *_long,
      never idle*/event_*/blend_*, never a base_clip / jiggle_of /
      jiggle_variant / blend from/to target), seasonal first, then the
      largest, until the pack holds at least <pack_mb> MB of sheets.
  clip-pack.py pack <generated_dir> <list> <out.zip> <out.list>
      Writes the pack zip (stored, paths relative to generated/) and
      <out.list> ("<sha256>  <path>" per file) for the APK.
"""
import hashlib
import os
import sys
import tomllib
import zipfile

SEASONAL = ("valentines_", "spring_", "summer_", "halloween_", "xmas_", "newyear_")


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


if __name__ == "__main__":
    a = sys.argv[1:]
    if len(a) == 3 and a[0] == "select":
        print("\n".join(select(a[1], int(a[2]))))
    elif len(a) == 5 and a[0] == "pack":
        pack(*a[1:])
    else:
        sys.exit(__doc__)
