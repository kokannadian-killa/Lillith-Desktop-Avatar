"""Tests for tools/video-pipeline/matte_model.py (where isnet-anime.onnx lives, whether it is whole, running it
without rembg) and `user_import.py --preflight` (what is missing and how to fix it, as one JSON line).

  python3 -m pytest -q -p no:cacheprovider tools/video-pipeline/tests/test_matte_model.py

No network, nothing downloaded; fake model files are tiny (the size check is exercised by patching MODEL_SIZE).
"""
import json, os, shutil, site, subprocess, sys, tempfile, unittest
from pathlib import Path

sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent
PIPE = HERE.parent
sys.path.insert(0, str(PIPE))
import matte_model as mm  # noqa: E402


class Model(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix='matte_model_'))
        self.old_size = mm.MODEL_SIZE
        mm.MODEL_SIZE = 5           # fake model files are 5 bytes
        self.here = self.tmp / 'install' / 'tools' / 'video-pipeline'
        self.bund = self.tmp / 'install' / 'models' / 'rembg'
        self.home = self.tmp / 'home'
        self.kw = dict(here=self.here, home=self.home, posix=False)

    def tearDown(self):
        mm.MODEL_SIZE = self.old_size
        shutil.rmtree(self.tmp, ignore_errors=True)

    def put(self, p, n=5):
        p.parent.mkdir(parents=True, exist_ok=True); p.write_bytes(b'x' * n); return p

    def test_constants_match_the_published_model(self):
        self.assertEqual(self.old_size, 176069933)
        self.assertEqual(len(mm.MODEL_SHA256), 64)
        self.assertEqual(mm.MODEL_NAME, 'isnet-anime.onnx')

    def test_bundled_dir_is_beside_tools(self):
        self.assertEqual(mm.bundled_dir(self.here), self.tmp / 'install' / 'models' / 'rembg')
        self.assertEqual(mm.bundled_dir(Path('/opt/lillith/tools/video-pipeline')), Path(mm.LINUX_BUNDLE))

    def test_installer_folder_beats_rembgs_own(self):
        self.put(self.home / '.u2net' / 'isnet-anime.onnx')
        b = self.put(self.bund / 'isnet-anime.onnx')
        p, tried = mm.find_model(env={}, strict=True, **self.kw)
        self.assertEqual(p, b.resolve())
        self.assertEqual(mm.model_dir(env={}, home=self.home, here=self.here), str(b.resolve().parent))

    def test_u2net_home_is_first(self):
        self.put(self.bund / 'isnet-anime.onnx')
        u = self.put(self.tmp / 'u2' / 'isnet-anime.onnx')
        p, _ = mm.find_model(env={'U2NET_HOME': str(self.tmp / 'u2')}, strict=True, **self.kw)
        self.assertEqual(p, u.resolve())

    def test_half_downloaded_file_is_never_picked(self):
        bad = self.put(self.bund / 'isnet-anime.onnx', n=3)               # wrong size
        good = self.put(self.home / '.u2net' / 'isnet-anime.onnx')
        p, _ = mm.find_model(env={}, strict=True, **self.kw)
        self.assertEqual(p, good.resolve())
        os.remove(good)
        self.assertIsNone(mm.find_model(env={}, strict=True, **self.kw)[0])
        self.assertEqual(mm.find_model(env={}, strict=False, **self.kw)[0], bad.resolve())
        r = mm.check(env={}, modules=(), **self.kw)
        self.assertFalse(r['ok'])
        self.assertIn('wrong size', r['problems'][0]['detail'])
        self.assertIn('3 bytes', r['problems'][0]['detail'])

    def test_missing_model_says_where_it_looked_and_how_to_fix(self):
        r = mm.check(env={}, modules=(), **self.kw)
        self.assertFalse(r['ok'])
        d = r['problems'][0]['detail']
        self.assertIn('isnet-anime.onnx', d); self.assertIn('not found', d)
        self.assertIn(str(self.bund), d)
        self.assertRegex(d, r'installer|Setup')
        self.assertTrue(r['tried'])

    def test_missing_python_package_is_named(self):
        self.put(self.bund / 'isnet-anime.onnx')
        r = mm.check(env={}, modules=('definitely_not_installed_pkg',), **self.kw)
        self.assertFalse(r['ok'])
        self.assertEqual(r['problems'][0]['item'], 'definitely_not_installed_pkg')
        self.assertIn("can't be imported", r['problems'][0]['detail'])

    def test_all_present_is_ok_and_no_model_needed_for_key(self):
        self.put(self.bund / 'isnet-anime.onnx')
        r = mm.check(env={}, modules=('json',), **self.kw)
        self.assertTrue(r['ok']); self.assertEqual(r['problems'], [])
        self.assertEqual(r['model_dir'], str(self.bund.resolve()))
        r = mm.check(need_model=False, env={}, modules=('json',), **dict(self.kw, home=self.tmp / 'empty'))
        self.assertTrue(r['ok'])

    def test_check_file_and_require_raise_a_readable_error(self):
        with self.assertRaises(mm.MatteModelError) as c:
            mm.check_file(self.tmp / 'nope.onnx')
        self.assertIn('missing', str(c.exception))
        f = self.put(self.tmp / 'short.onnx', n=2)
        with self.assertRaises(mm.MatteModelError) as c:
            mm.check_file(f)
        self.assertIn('2 bytes', str(c.exception))
        mm.check_file(self.put(self.tmp / 'ok.onnx'))

    def test_verify_checksum(self):
        f = self.put(self.bund / 'isnet-anime.onnx')
        r = mm.check(env={}, modules=(), verify=True, **self.kw)
        self.assertFalse(r['ok']); self.assertIn('checksum', r['problems'][0]['detail'])
        import hashlib
        old = mm.MODEL_SHA256
        try:
            mm.MODEL_SHA256 = hashlib.sha256(f.read_bytes()).hexdigest()
            self.assertTrue(mm.check(env={}, modules=(), verify=True, **self.kw)['ok'])
        finally:
            mm.MODEL_SHA256 = old

    def test_configure_env_sets_u2net_home_to_the_model_folder(self):
        old = os.environ.get('U2NET_HOME')
        try:
            os.environ['U2NET_HOME'] = str(self.tmp / 'u2')
            self.put(self.tmp / 'u2' / 'isnet-anime.onnx')
            self.assertEqual(mm.configure_env(), str((self.tmp / 'u2').resolve()))
            self.assertEqual(os.environ['U2NET_HOME'], str((self.tmp / 'u2').resolve()))
        finally:
            if old is None: os.environ.pop('U2NET_HOME', None)
            else: os.environ['U2NET_HOME'] = old

    def test_paths_agree_with_matte_model(self):
        env = dict(os.environ, U2NET_HOME=str(self.tmp / 'u2'), PYTHONDONTWRITEBYTECODE='1', HOME=str(self.tmp), USERPROFILE=str(self.tmp), PYTHONUSERBASE=site.getuserbase())   # no model anywhere: U2NET_HOME stays as set
        out = subprocess.run([sys.executable, '-c', 'import paths, matte_v2, os; print(paths.U2NET); print(os.environ["U2NET_HOME"]); print(matte_v2.MODEL)'],
                             cwd=PIPE, env=env, capture_output=True, text=True, timeout=120)
        self.assertEqual(out.returncode, 0, out.stderr)
        a, b, c = out.stdout.split('\n')[:3]
        self.assertEqual(a, str(self.tmp / 'u2')); self.assertEqual(b, a)
        self.assertEqual(c, os.path.join(a, 'isnet-anime.onnx'))


class Preflight(unittest.TestCase):
    """user_import.py --preflight: one JSON line, never a crash."""
    def run_pf(self, *args, **envs):
        env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1', PYTHONUSERBASE=site.getuserbase(), **envs)
        p = subprocess.run([sys.executable, str(PIPE / 'user_import.py'), '--preflight', *args], capture_output=True, text=True, env=env, timeout=120)
        self.assertEqual(p.returncode, 0, p.stderr)
        return json.loads(p.stdout.strip().splitlines()[-1])

    def test_missing_model_is_reported_not_raised(self):
        tmp = tempfile.mkdtemp(prefix='pf_')
        try:
            r = self.run_pf('--matte', 'isnet', '--model', os.path.join(tmp, 'nope.onnx'), U2NET_HOME=os.path.join(tmp, 'u2'))
            # the model may still be found in this box's own ~/.u2net: only assert the shape when it is missing
            self.assertIn('ok', r); self.assertEqual(r['matte'], 'isnet'); self.assertTrue(r['fatal'])
            if not r['ok']:
                self.assertTrue(all({'item', 'detail'} <= set(p) for p in r['problems']))
        finally:
            shutil.rmtree(tmp, ignore_errors=True)

    def test_key_and_alpha_never_need_the_model(self):
        for m in ('key', 'alpha'):
            r = self.run_pf('--matte', m)
            self.assertFalse(r['fatal'])
            self.assertFalse(any(p['item'] in ('isnet-anime.onnx', 'onnxruntime') for p in r['problems']))

    def test_auto_missing_model_is_not_fatal(self):
        tmp = tempfile.mkdtemp(prefix='pf_')
        try:
            r = self.run_pf('--model', os.path.join(tmp, 'x.onnx'), HOME=tmp, USERPROFILE=tmp, U2NET_HOME=tmp)
            self.assertEqual(r['matte'], 'auto'); self.assertFalse(r['fatal'])
        finally:
            shutil.rmtree(tmp, ignore_errors=True)


class CommandLineTools(unittest.TestCase):
    def test_build_clip_exits_2_with_a_message_when_the_model_is_missing(self):
        tmp = tempfile.mkdtemp(prefix='bc_')
        try:
            env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1', U2NET_HOME=tmp, HOME=tmp, USERPROFILE=tmp, PYTHONUSERBASE=site.getuserbase())
            p = subprocess.run([sys.executable, str(PIPE / 'build_clip.py'), '--clip', 'x', '--video', 'nope.mp4', '--start', '1', '--stop', '3'],
                               capture_output=True, text=True, env=env, timeout=120)
            self.assertEqual(p.returncode, 2, p.stderr[-400:])
            self.assertIn("can't cut the figure out", p.stderr)
            self.assertNotIn('Traceback', p.stderr)
            self.assertIn('isnet-anime.onnx', p.stderr)
        finally:
            shutil.rmtree(tmp, ignore_errors=True)


@unittest.skipUnless(mm.find_model(strict=True)[0] and all(__import__('importlib').util.find_spec(m) for m in ('onnxruntime', 'rembg')),
                     'needs the real model + onnxruntime + rembg to compare against')
class SameMaskAsRembg(unittest.TestCase):
    def test_isnet_mask_equals_rembg_only_mask(self):
        import numpy as np
        from PIL import Image
        rng = np.random.RandomState(7)
        rgb = np.full((96, 80, 3), 255, np.uint8)
        rgb[20:80, 25:55] = rng.randint(0, 255, (60, 30, 3))
        d = str(mm.find_model(strict=True)[0].parent)
        old = os.environ.get('U2NET_HOME'); os.environ['U2NET_HOME'] = d
        try:
            from rembg import new_session, remove
            ref = np.asarray(remove(Image.fromarray(rgb), session=new_session('isnet-anime'), only_mask=True))
        finally:
            if old is None: os.environ.pop('U2NET_HOME', None)
            else: os.environ['U2NET_HOME'] = old
        got = mm.isnet_mask(rgb)
        self.assertEqual(got.shape, ref.shape)
        self.assertLessEqual(int(np.abs(got.astype(int) - ref.astype(int)).max()), 1)


if __name__ == '__main__':
    unittest.main()
