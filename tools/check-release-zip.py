#!/usr/bin/env python3
"""Refuses a release zip that carries files which must never ship.

Usage:
    python3 tools/check-release-zip.py Lillith-<ver>.zip
    python3 tools/check-release-zip.py Lillith-<ver>.zip.part1      (split set)
    python3 tools/check-release-zip.py X.zip.part1 X.zip.part2 ...  (split set)

Release zips are packed by hand, so run this on every one before it is
uploaded. Exit 0 = clean, 1 = offending entries (each one printed), 2 = not
a readable zip / bad usage / broken split set. Standard library only.

Refused entries:
  - anything under */tests/fixtures/tls/  (test-only TLS chain + private key)
  - *.key, *.key.der                       (key material)
  - *.pem holding a PRIVATE KEY block
  - *.p12, *.pfx                           (PKCS#12 key bundles)
  - id_rsa*                                (SSH keys: id_rsa, id_rsa.pub, ...)
  - *.gguf, *.safetensors                  (model weights)
  - master.zip                             (the repo-in-a-zip LFS blob)
  - anything under tools/lillith-py/       (dev-only Python sidecar)

Nested zips: an entry that is itself a *.zip is opened straight from the
archive (nothing is extracted to disk) and every rule above is applied to
its contents, recursively. A hit is named outer -> inner, e.g.
    Lillith/extra.zip -> keys/id_rsa
A nested zip that cannot be read, or nesting deeper than MAX_DEPTH, is
refused too (fail closed).

Split archives: X.zip.part1, X.zip.part2, ... are the bytes of one zip cut
into pieces (e.g. `split`). Give part1 (the rest are found next to it) or
list every part in order; the parts are streamed as one zip, never joined
on disk. A missing part, a gap, a lone part or parts given out of order is
reported with exit 2.
"""
import io
import os
import re
import sys
import zipfile
from pathlib import PurePosixPath

PRIVATE_PEM = b"PRIVATE KEY-----"
MAX_DEPTH = 8  # nested zips deeper than this are refused, not scanned
ARROW = " -> "
PART_RE = re.compile(r"^(?P<base>.+)\.part(?P<n>[0-9]+)$", re.IGNORECASE)


class BadInput(Exception):
    """Input that is not a scannable zip (exit 2)."""


def has_seq(parts, seq):
    n = len(seq)
    return any(tuple(parts[i : i + n]) == seq for i in range(len(parts) - n + 1))


def why_refused(z, info):
    """The reason `info` must not ship, or None. (Nested zips: see scan.)"""
    name = info.filename.replace("\\", "/")
    p = PurePosixPath(name)
    parts = [x.lower() for x in p.parts]
    low = name.lower()
    base = p.name.lower()
    if has_seq(parts, ("tests", "fixtures", "tls")) and not info.is_dir():
        return "test TLS fixture"
    if has_seq(parts, ("tools", "lillith-py")):
        return "tools/lillith-py (dev-only)"
    if info.is_dir():
        return None
    if low.endswith(".key") or low.endswith(".key.der"):
        return "key file"
    if low.endswith(".p12") or low.endswith(".pfx"):
        return "PKCS#12 key bundle"
    if base.startswith("id_rsa"):
        return "SSH key (id_rsa*)"
    if low.endswith(".gguf"):
        return "model weights (.gguf)"
    if low.endswith(".safetensors"):
        return "model weights (.safetensors)"
    if base == "master.zip":
        return "master.zip"
    if low.endswith(".pem"):
        with z.open(info) as f:
            if PRIVATE_PEM in f.read(1 << 20):
                return "private key (.pem)"
    return None


def scan(z, prefix="", depth=0):
    """Yield (path, reason) for every refused entry in `z`, recursing into
    nested *.zip entries. `prefix` is the outer path, e.g. "a.zip -> "."""
    for info in z.infolist():
        path = prefix + info.filename
        reason = why_refused(z, info)
        if reason:
            yield path, reason
            continue
        if info.is_dir() or not info.filename.lower().endswith(".zip"):
            continue
        if depth + 1 > MAX_DEPTH:
            yield path, f"nested zip deeper than {MAX_DEPTH} levels (not scanned)"
            continue
        try:
            # ZipExtFile is seekable when the outer file is, so the inner
            # zip is read straight out of the outer one, never to disk.
            with z.open(info) as f, zipfile.ZipFile(f) as inner:
                yield from scan(inner, path + ARROW, depth + 1)
        except (zipfile.BadZipFile, zipfile.LargeZipFile, OSError, EOFError,
                NotImplementedError, RuntimeError, ValueError) as e:
            yield path, f"nested zip unreadable, cannot check it ({e})"


class ConcatReader(io.RawIOBase):
    """Read-only, seekable view of several files laid end to end."""

    def __init__(self, paths):
        super().__init__()
        self._files = []
        self._starts = []
        total = 0
        try:
            for p in paths:
                f = open(p, "rb")
                self._files.append(f)
                self._starts.append(total)
                total += os.fstat(f.fileno()).st_size
        except OSError:
            self.close()
            raise
        self._size = total
        self._pos = 0

    def readable(self):
        return True

    def seekable(self):
        return True

    def tell(self):
        return self._pos

    def seek(self, offset, whence=io.SEEK_SET):
        if whence == io.SEEK_SET:
            pos = offset
        elif whence == io.SEEK_CUR:
            pos = self._pos + offset
        elif whence == io.SEEK_END:
            pos = self._size + offset
        else:
            raise ValueError(f"bad whence {whence}")
        if pos < 0:
            raise OSError("negative seek")
        self._pos = pos
        return pos

    def readinto(self, buf):
        while self._pos < self._size:
            # the last part whose start is <= pos
            i = max(k for k, s in enumerate(self._starts) if s <= self._pos)
            f = self._files[i]
            end = self._starts[i + 1] if i + 1 < len(self._starts) else self._size
            f.seek(self._pos - self._starts[i])
            n = f.readinto(memoryview(buf)[: end - self._pos])
            if n:
                self._pos += n
                return n
            # empty (or shrunk) part: step to the next one
            if end <= self._pos:
                break
            self._pos = end
        return 0

    def close(self):
        for f in getattr(self, "_files", []):
            f.close()
        super().close()


def sibling_parts(base):
    """The N of every existing `base.partN` file."""
    d, stem = os.path.split(base)
    out = []
    for f in os.listdir(d or "."):
        m = PART_RE.match(f)
        if m and m.group("base") == stem and os.path.isfile(os.path.join(d, f)):
            out.append(int(m.group("n")))
    return out


def split_parts(args):
    """The ordered part list for a split set, None if `args` is one plain
    zip. Raises BadInput with a clear message for a broken set."""
    m = [PART_RE.match(a) for a in args]
    if len(args) == 1 and not m[0]:
        return None
    if not all(m):
        raise BadInput("give one zip, or the .partN files of one split set (not a mix)")
    bases = {x.group("base") for x in m}
    if len(bases) != 1:
        raise BadInput(f"parts of different archives given: {', '.join(sorted(bases))}")
    base = bases.pop()
    nums = [int(x.group("n")) for x in m]
    if len(args) == 1:
        # Find the whole set next to the one part we were given.
        nums = []
        n = 1
        while os.path.isfile(f"{base}.part{n}"):
            nums.append(n)
            n += 1
        if not nums:
            raise BadInput(f"{base}.part1 is missing; cannot read a split set without its first part")
        given = int(m[0].group("n"))
        found = sibling_parts(base)
        orphans = sorted(k for k in found if k > nums[-1])
        if orphans:
            have = "part1" if nums[-1] == 1 else f"part1..part{nums[-1]}"
            raise BadInput(f"split set is missing part{nums[-1] + 1} "
                           f"(found {have} and part{orphans[0]})")
        if given not in nums:
            raise BadInput(f"{args[0]}: part{given} is not in the contiguous set "
                           f"part1..part{nums[-1]} (part{nums[-1] + 1} is missing)")
    else:
        if len(set(nums)) != len(nums):
            raise BadInput("the same part was given twice")
        if nums != sorted(nums):
            raise BadInput(f"parts given out of order ({', '.join('part%d' % n for n in nums)}); "
                           "list them as part1, part2, ...")
        want = list(range(1, len(nums) + 1))
        if nums != want:
            missing = sorted(set(range(1, max(nums) + 1)) - set(nums))
            raise BadInput(f"split set is missing part{', part'.join(map(str, missing))}")
    if len(nums) < 2:
        raise BadInput(f"{base}.part1 is a lone part; a split set needs part2 and on")
    extra = f"{base}.part{nums[-1] + 1}"
    if os.path.isfile(extra):
        raise BadInput(f"{extra} exists but is not in the set given")
    paths = [f"{base}.part{n}" for n in nums]
    for p in paths:
        if not os.path.isfile(p):
            raise BadInput(f"{p}: no such file")
    return paths


def open_input(args):
    """(label, ZipFile) for the command-line input. Raises BadInput."""
    parts = split_parts(args)
    try:
        if parts is None:
            return args[0], zipfile.ZipFile(args[0])
        label = f"{parts[0][: -len('.part1')]} (split, {len(parts)} parts)"
        reader = ConcatReader(parts)
        try:
            return label, zipfile.ZipFile(io.BufferedReader(reader))
        except BaseException:
            reader.close()
            raise
    except (OSError, zipfile.BadZipFile, zipfile.LargeZipFile, EOFError, ValueError) as e:
        if parts is None:
            raise BadInput(f"{args[0]}: {e}") from e
        raise BadInput(f"{parts[0]}..{parts[-1]}: {e} "
                       "(is the last part missing or truncated?)") from e


def main(argv):
    if not argv:
        print("usage: check-release-zip.py ZIP | ZIP.part1 [ZIP.part2 ...]", file=sys.stderr)
        return 2
    try:
        label, z = open_input(argv)
    except BadInput as e:
        print(f"error: {e}", file=sys.stderr)
        return 2
    try:
        with z:
            bad = list(scan(z))
            n = len(z.infolist())
    except (OSError, zipfile.BadZipFile, EOFError, ValueError) as e:
        print(f"error: {label}: {e}", file=sys.stderr)
        return 2
    if bad:
        print(f"REFUSED: {label} has {len(bad)} entr{'y' if len(bad) == 1 else 'ies'} that must not ship:")
        for name, why in bad:
            print(f"  {name}  ({why})")
        return 1
    print(f"ok: {label} ({n} entries, nothing forbidden)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
