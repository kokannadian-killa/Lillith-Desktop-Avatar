"""The isnet-anime matte model: ONE place that knows where it lives, whether it is whole, and how to run it.

Why this exists: `rembg.new_session('isnet-anime')` downloads the 176 MB model into ~/.u2net the first time
(a failure on a poor connection looked like a crash), and paths.py looked in ~/.rembg/models/isnet-anime while
rembg looked in ~/.u2net. Now the installers bundle the model and every script resolves it here. Nothing is ever
downloaded at run time (Lillith is loopback-only), and rembg itself is not needed: `isnet_mask()` runs the model
with onnxruntime and reproduces rembg's isnet-anime mask (same resize, normalisation and scaling).

Where the model is looked for, first usable file wins (`$U2NET_HOME` is set to the folder that holds it):
  1. an explicit --model (a file, or a folder holding isnet-anime.onnx)
  2. $U2NET_HOME/isnet-anime.onnx
  3. the installer's folder: <install>/models/rembg  (Arch: /opt/lillith/models/rembg, Windows:
     <install dir>\\models\\rembg), found relative to this script (tools/video-pipeline/../../models/rembg)
  4. /opt/lillith/models/rembg (Linux package)
  5. ~/.u2net/isnet-anime.onnx             (rembg's own folder: a model Ko copied by hand)
  6. ~/.rembg/models/isnet-anime/isnet-anime.onnx   (older Lillith default)
A file is "usable" only with the exact size below (a half-finished download is never picked).

python3 matte_model.py [--json] [--verify]   prints the check (exit 0 = ready, 1 = something is missing)
"""
import hashlib
import json
import os
import sys
from pathlib import Path

MODEL_NAME = 'isnet-anime.onnx'
MODEL_SIZE = 176069933
MODEL_SHA256 = 'f15622d853e8260172812b657053460e20806f04b9e05147d49af7bed31a6e99'
HERE = Path(__file__).resolve().parent
LINUX_BUNDLE = '/opt/lillith/models/rembg'
# modules the pose pipeline imports for the cut-out (rembg is NOT one of them any more)
REQUIRED_MODULES = ('numpy', 'PIL', 'scipy', 'cv2', 'onnxruntime')


def _model_file(c):
    c = Path(c)
    return c if c.suffix.lower() == '.onnx' else c / MODEL_NAME


def bundled_dir(here=None):
    """<install>/models/rembg, relative to tools/video-pipeline."""
    return Path(here or HERE).parent.parent / 'models' / 'rembg'


def model_candidates(flag=None, env=None, home=None, here=None, posix=None):
    """Every path tried, in order (see the module docstring)."""
    env = os.environ if env is None else env
    home = Path.home() if home is None else home
    posix = (os.name == 'posix') if posix is None else posix
    out = []
    for c in (flag, env.get('U2NET_HOME')):
        if c:
            out.append(_model_file(c))
    if here is not False:
        out.append(bundled_dir(here) / MODEL_NAME)
    if posix:
        out.append(Path(LINUX_BUNDLE) / MODEL_NAME)
    out.append(home / '.u2net' / MODEL_NAME)
    out.append(home / '.rembg' / 'models' / 'isnet-anime' / MODEL_NAME)
    seen, res = set(), []
    for p in out:
        if str(p) not in seen:
            seen.add(str(p)); res.append(p)
    return res


def _size(p):
    try:
        return Path(p).stat().st_size if Path(p).is_file() else None
    except OSError:
        return None


def find_model(flag=None, env=None, home=None, here=None, strict=False, posix=None):
    """(path or None, [every path tried]). strict: skip files whose size is not MODEL_SIZE."""
    tried = model_candidates(flag, env, home, here, posix)
    for c in tried:
        s = _size(c)
        if s is not None and (not strict or s == MODEL_SIZE):
            return Path(c).resolve(), tried
    return None, tried


def model_dir(env=None, home=None, here=None):
    """The one folder U2NET_HOME should point at: where a usable model is, else the installer's folder if it
    exists, else rembg's own ~/.u2net."""
    env = os.environ if env is None else env
    p, _ = find_model(None, env, home, here, strict=True)
    if p:
        return str(p.parent)
    if env.get('U2NET_HOME'):
        return env['U2NET_HOME']
    b = bundled_dir(here)
    if b.is_dir():
        return str(b)
    if os.name == 'posix' and Path(LINUX_BUNDLE).is_dir():
        return LINUX_BUNDLE
    return str(Path.home() / '.u2net' if home is None else Path(home) / '.u2net')


def configure_env():
    """Point U2NET_HOME at the model folder (so rembg, if anything still uses it, looks in the same place)."""
    d = model_dir()
    os.environ['U2NET_HOME'] = d
    return d


def fix_hint():
    if os.name == 'nt':
        return 'Run Lillith-Setup.exe again (it puts the model in <install folder>\\models\\rembg).'
    return ('Run the Lillith installer again (Lillith-Installer.AppImage, or bash Lillith-Install.sh); it puts '
            f'{MODEL_NAME} in {LINUX_BUNDLE}.')


class MatteModelError(RuntimeError):
    """Raised (with a message fit to show Ko) when the matte model or one of its Python packages is unusable."""


def check(need_model=True, flag=None, env=None, home=None, here=None, modules=None, verify=False, posix=None):
    """What stops the AI cut-out from running. {'ok', 'problems': [{'item', 'detail'}], 'model', 'model_dir',
    'tried'}. Pure: imports are really tried, the model is never fetched."""
    problems = []
    for m in (REQUIRED_MODULES if modules is None else modules):
        try:
            __import__(m)
        except Exception as e:  # noqa: BLE001 (a broken install can raise anything)
            pkg = {'cv2': 'python-opencv', 'PIL': 'python-pillow', 'onnxruntime': 'python-onnxruntime'}.get(m, 'python-' + m)
            how = (f'Fix: sudo pacman -S --needed {pkg} (or run the Lillith installer again, it installs it).' if os.name != 'nt'
                   else 'Fix: run Lillith-Setup.exe again (it installs the Python packages next to Lillith).')
            problems.append({'item': m, 'detail': f"the Python package '{m}' can't be imported ({type(e).__name__}: {e}). {how}"})
    model, tried = find_model(flag, env, home, here, strict=True, posix=posix)
    if need_model:
        if model is None:
            wrong = [(str(t), _size(t)) for t in tried if _size(t) not in (None, MODEL_SIZE)]
            why = ('found but the wrong size (a half-finished download?): ' + '; '.join(f'{p} is {s} bytes, needs {MODEL_SIZE}' for p, s in wrong)
                   if wrong else 'not found. Looked in: ' + '; '.join(str(t) for t in tried))
            problems.append({'item': MODEL_NAME, 'detail': f'the AI cut-out model {MODEL_NAME} is {why}. {fix_hint()}'})
        elif verify:
            h = hashlib.sha256()
            with open(model, 'rb') as f:
                for b in iter(lambda: f.read(1 << 22), b''):
                    h.update(b)
            if h.hexdigest() != MODEL_SHA256:
                problems.append({'item': MODEL_NAME, 'detail': f'{model} has the right size but the wrong checksum (damaged). {fix_hint()}'})
    return {'ok': not problems, 'problems': problems, 'model': str(model) if model else None,
            'model_dir': str(model.parent) if model else model_dir(env, home, here), 'tried': [str(t) for t in tried]}


def check_file(path):
    """Raise MatteModelError unless `path` is the whole model (right size)."""
    n = _size(path)
    if n is None:
        raise MatteModelError(f'the AI cut-out model {MODEL_NAME} is missing ({path}). {fix_hint()}')
    if n != MODEL_SIZE:
        raise MatteModelError(f'the AI cut-out model {path} is {n} bytes, needs {MODEL_SIZE} (half-finished download?). {fix_hint()}')


def preflight_or_exit(need_model=True):
    """For command-line tools: print every problem and exit 2 instead of a traceback."""
    r = check(need_model)
    if not r['ok']:
        sys.stderr.write("Pose Builder can't cut the figure out:\n  - " + '\n  - '.join(p['detail'] for p in r['problems']) + '\n')
        sys.exit(2)
    return r


def require(need_model=True):
    """check(), raising MatteModelError with every problem spelled out."""
    r = check(need_model)
    if not r['ok']:
        raise MatteModelError("Pose Builder can't cut the figure out:\n  - " + '\n  - '.join(p['detail'] for p in r['problems']))
    return r


# ------------------------------------------------------------------ running the model (no rembg)
_SESSION = {}


def session(threads=2, flag=None):
    import onnxruntime as ort
    r = require()
    key = (r['model'], threads)
    if key not in _SESSION:
        so = ort.SessionOptions(); so.intra_op_num_threads = threads; so.inter_op_num_threads = 1
        _SESSION.clear()
        _SESSION[key] = ort.InferenceSession(r['model'], so, providers=['CPUExecutionProvider'])
    return _SESSION[key]


def isnet_mask(rgb, threads=2):
    """uint8 HxW matte, identical in method to rembg's isnet-anime `remove(..., only_mask=True)`:
    LANCZOS resize to 1024x1024, /max, minus the ImageNet mean, min-max scale, LANCZOS back."""
    import numpy as np
    from PIL import Image
    s = session(threads)
    img = Image.fromarray(np.asarray(rgb)[..., :3].astype(np.uint8))
    a = np.array(img.resize((1024, 1024), Image.Resampling.LANCZOS))
    a = a / max(np.max(a), 1e-6)
    a = (a - np.array((0.485, 0.456, 0.406))) / np.array((1.0, 1.0, 1.0))
    x = a.transpose(2, 0, 1)[None].astype(np.float32)
    pred = s.run(None, {s.get_inputs()[0].name: x})[0][:, 0, :, :]
    ma, mi = np.max(pred), np.min(pred)
    pred = np.squeeze((pred - mi) / (ma - mi))
    m = Image.fromarray((pred * 255).astype('uint8'), mode='L')
    return np.asarray(m.resize(img.size, Image.Resampling.LANCZOS))


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    r = check(verify='--verify' in argv)
    if '--json' in argv:
        print(json.dumps(r))
    elif r['ok']:
        print(f"ready: {r['model']} (U2NET_HOME={r['model_dir']})")
    else:
        print("Pose Builder can't cut the figure out:\n  - " + '\n  - '.join(p['detail'] for p in r['problems']))
    return 0 if r['ok'] else 1


if __name__ == '__main__':
    sys.exit(main())
