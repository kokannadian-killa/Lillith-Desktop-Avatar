#!/usr/bin/env python3
"""Refuses a release zip that carries files which must never ship.

Usage:
    python3 tools/check-release-zip.py Lillith-<ver>.zip

Release zips are packed by hand, so run this on every one before it is
uploaded. Exit 0 = clean, 1 = offending entries (each one printed), 2 = not
a readable zip / bad usage. Standard library only.

Refused entries:
  - anything under */tests/fixtures/tls/  (test-only TLS chain + private key)
  - *.key, *.key.der                       (key material)
  - *.pem holding a PRIVATE KEY block
  - *.gguf                                 (model weights)
  - master.zip                             (the repo-in-a-zip LFS blob)
  - anything under tools/lillith-py/       (dev-only Python sidecar)
"""
import sys
import zipfile
from pathlib import PurePosixPath

PRIVATE_PEM = b"PRIVATE KEY-----"


def has_seq(parts, seq):
    n = len(seq)
    return any(tuple(parts[i : i + n]) == seq for i in range(len(parts) - n + 1))


def why_refused(z, info):
    """The reason `info` must not ship, or None."""
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
    if low.endswith(".gguf"):
        return "model weights (.gguf)"
    if base == "master.zip":
        return "master.zip"
    if low.endswith(".pem"):
        with z.open(info) as f:
            if PRIVATE_PEM in f.read(1 << 20):
                return "private key (.pem)"
    return None


def main(argv):
    if len(argv) != 1:
        print("usage: check-release-zip.py ZIP", file=sys.stderr)
        return 2
    try:
        z = zipfile.ZipFile(argv[0])
    except (OSError, zipfile.BadZipFile) as e:
        print(f"error: {argv[0]}: {e}", file=sys.stderr)
        return 2
    with z:
        bad = [(i.filename, r) for i in z.infolist() if (r := why_refused(z, i))]
        n = len(z.infolist())
    if bad:
        print(f"REFUSED: {argv[0]} has {len(bad)} entr{'y' if len(bad) == 1 else 'ies'} that must not ship:")
        for name, why in bad:
            print(f"  {name}  ({why})")
        return 1
    print(f"ok: {argv[0]} ({n} entries, nothing forbidden)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
