#!/usr/bin/env python3
"""Downloads Lillith's real art into a GitHub "Download ZIP" (main.zip).

GitHub's Download ZIP leaves Git LFS files (her PNG sheets and MP4 poses) as
small text placeholders ("pointers") unless the repo has "Include Git LFS
objects in archives" switched on. This swaps each placeholder under
assets/generated for the real file, straight from this repo's own Git LFS
storage on github.com, and checks every file's sha256 before using it.

    python3 linux/fetch-art.py              # from anywhere; finds the folder itself
    python3 linux/fetch-art.py --check      # only count the placeholders (no network)

The repo is private, so it needs your GitHub login. It uses, in order:
$GH_TOKEN / $GITHUB_TOKEN, else the GitHub CLI (`gh auth login` once, then
`gh auth token`). Nothing else is contacted, and no token is ever saved.

Exit codes: 0 ok (or nothing to do), 1 error, 3 placeholders left (--check),
4 needs a GitHub login.
"""
import argparse
import base64
import hashlib
import json
import os
import shutil
import subprocess
import sys
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

REPO = os.environ.get("LILLITH_REPO", "kokannadian-killa/Lillith-Desktop-Avatar")
LFS_MAGIC = b"version https://git-lfs.github.com/spec/v1"
NEEDS_LOGIN = 4


def pointers(root: Path, includes: list[str]):
    """(path, oid, size) for every LFS pointer file under the include dirs."""
    for inc in includes:
        base = root / inc
        files = [base] if base.is_file() else sorted(p for p in base.rglob("*") if p.is_file())
        for p in files:
            try:
                if p.stat().st_size > 1024:
                    continue
                data = p.read_bytes()
            except OSError:
                continue
            if not data.startswith(LFS_MAGIC):
                continue
            fields = dict(l.split(" ", 1) for l in data.decode("ascii", "replace").splitlines() if " " in l)
            oid = fields.get("oid", "").removeprefix("sha256:")
            if len(oid) == 64 and fields.get("size", "").isdigit():
                yield p, oid, int(fields["size"])


def token() -> str | None:
    for k in ("GH_TOKEN", "GITHUB_TOKEN"):
        if os.environ.get(k):
            return os.environ[k]
    gh = shutil.which("gh")
    if gh:
        r = subprocess.run([gh, "auth", "token", "--hostname", "github.com"],
                           capture_output=True, text=True)
        if r.returncode == 0 and r.stdout.strip():
            return r.stdout.strip()
    return None


def batch(objs, tok):
    url = f"https://github.com/{REPO}.git/info/lfs/objects/batch"
    body = json.dumps({"operation": "download", "transfers": ["basic"],
                       "objects": [{"oid": o, "size": s} for _, o, s in objs]}).encode()
    req = urllib.request.Request(url, data=body, method="POST", headers={
        "Accept": "application/vnd.git-lfs+json",
        "Content-Type": "application/vnd.git-lfs+json",
        "User-Agent": "lillith-fetch-art"})
    if tok:
        req.add_header("Authorization", "Basic " + base64.b64encode(f"x-access-token:{tok}".encode()).decode())
    with urllib.request.urlopen(req, timeout=60) as r:
        return {o["oid"]: o for o in json.load(r)["objects"]}


def download(path: Path, oid: str, size: int, obj: dict) -> str | None:
    if "error" in obj:
        return f"{path.name}: {obj['error'].get('message', 'unavailable')}"
    act = obj.get("actions", {}).get("download")
    if not act:
        return f"{path.name}: no download link"
    req = urllib.request.Request(act["href"], headers={**act.get("header", {}), "User-Agent": "lillith-fetch-art"})
    tmp = path.with_name(f".{path.name}.part-{os.getpid()}")
    h = hashlib.sha256()
    try:
        with urllib.request.urlopen(req, timeout=120) as r, open(tmp, "wb") as o:
            while chunk := r.read(1 << 20):
                h.update(chunk)
                o.write(chunk)
        if h.hexdigest() != oid or tmp.stat().st_size != size:
            return f"{path.name}: checksum mismatch, not used"
        os.replace(tmp, path)
        return None
    except (OSError, urllib.error.URLError) as e:
        return f"{path.name}: {e}"
    finally:
        tmp.unlink(missing_ok=True)


def main() -> int:
    here = Path(__file__).resolve().parent
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--root", type=Path, default=here.parent, help="the unzipped Lillith folder")
    ap.add_argument("--include", action="append", help="folder or file to fill in (default: assets/generated)")
    ap.add_argument("--check", action="store_true", help="only count placeholders, no network")
    a = ap.parse_args()
    includes = a.include or ["assets/generated"]
    todo = list(pointers(a.root, includes))
    total = sum(s for _, _, s in todo)
    if not todo:
        print("  art: all real files, nothing to download")
        return 0
    print(f"  art: {len(todo)} placeholder files ({total / 1e6:.0f} MB) in {', '.join(includes)}")
    if a.check:
        return 3
    tok = token()
    errors, done = [], 0
    for i in range(0, len(todo), 100):
        chunk = todo[i:i + 100]
        try:
            objs = batch(chunk, tok)
        except urllib.error.HTTPError as e:
            if e.code in (401, 403, 404):
                print("\n  GitHub said no (the repo is private, so it needs your login).\n"
                      "  Log in once, then run the installer again:\n"
                      "      sudo pacman -S --needed github-cli\n"
                      "      gh auth login --web\n", file=sys.stderr)
                return NEEDS_LOGIN
            print(f"error: GitHub LFS: {e}", file=sys.stderr)
            return 1
        except urllib.error.URLError as e:
            print(f"error: can't reach github.com: {e.reason}", file=sys.stderr)
            return 1
        with ThreadPoolExecutor(4) as ex:
            for err in ex.map(lambda t: download(*t, objs.get(t[1], {"error": {"message": "not on GitHub"}})), chunk):
                if err:
                    errors.append(err)
                else:
                    done += 1
        print(f"  art: {done}/{len(todo)} files downloaded", flush=True)
    if errors:
        print(f"error: {len(errors)} files failed, e.g. {errors[0]} (re-run to retry)", file=sys.stderr)
        return 1
    print(f"  art: done, {total / 1e6:.0f} MB, every file sha256-checked")
    return 0


if __name__ == "__main__":
    sys.exit(main())
