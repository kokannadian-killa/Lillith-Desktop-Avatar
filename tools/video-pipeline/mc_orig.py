"""Locate the ORIGINAL (pre-matte_clean) sheet of a clip dir.

matte_clean stamps <clip>.json `matte_clean.source_sha256` = sha256 of the original sheet, which is also its Git LFS
oid, so the original is recoverable from (1) $MC_BACKUP, (2) the repo's LFS object store, or (3) for v1 stamps
without a source sha, the LFS pointer of the sheet at origin/main. An unstamped sheet is its own original."""
import hashlib, json, os, subprocess


def sha(p):
    return hashlib.sha256(open(p, 'rb').read()).hexdigest()


def _git(d, *a):
    try:
        return subprocess.run(['git', '-C', d, *a], capture_output=True, text=True, timeout=30).stdout
    except Exception:
        return ''


def lfs_object(d, oid):
    common = _git(d, 'rev-parse', '--path-format=absolute', '--git-common-dir').strip()
    p = os.path.join(common, 'lfs', 'objects', oid[:2], oid[2:4], oid) if common and oid else ''
    return p if p and os.path.exists(p) else None


def original_sheet(d, backup_dir=None):
    """-> (path, sha256 or None, how). path is None when a stamped sheet's original cannot be found."""
    d = os.path.abspath(d.rstrip('/')); name = os.path.basename(d)
    sp = os.path.join(d, f'{name}_sheet.png'); jp = os.path.join(d, f'{name}.json')
    mc = (json.load(open(jp)).get('matte_clean') or {}) if os.path.exists(jp) else {}
    if not mc or mc.get('sheet_sha256') != sha(sp):
        return sp, None, 'current'          # never cleaned (or replaced since): the sheet is the original
    want = mc.get('source_sha256')
    if not want:  # v1 stamp: the original is what origin/main ships
        rel = os.path.relpath(sp, _git(d, 'rev-parse', '--show-toplevel').strip() or d)
        ptr = _git(d, 'show', f'origin/main:{rel}')
        want = next((l.split(':', 1)[1].strip() for l in ptr.splitlines() if l.startswith('oid sha256:')), None)
    if backup_dir:
        bk = os.path.join(backup_dir, f'{name}_sheet.png')
        if want and os.path.exists(bk) and sha(bk) == want:
            return bk, want, 'backup'
    p = lfs_object(d, want)
    if p:
        return p, want, 'lfs'
    return None, want, 'missing'
