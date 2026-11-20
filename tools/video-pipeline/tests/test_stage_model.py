"""tools/release/stage-matte-model.sh: copies isnet-anime.onnx into a release
payload only after the size + sha256 check, never downloads. Tiny dummy
files only (a fake matte_model.py carries their size and sum)."""
import hashlib
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
SCRIPT = REPO / 'tools' / 'release' / 'stage-matte-model.sh'
DATA = b'not really an onnx model\n'


class StageMatteModel(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix='stage-matte-'))
        (self.tmp / 'tools' / 'release').mkdir(parents=True)
        (self.tmp / 'tools' / 'video-pipeline').mkdir(parents=True)
        self.script = self.tmp / 'tools' / 'release' / SCRIPT.name
        shutil.copy2(SCRIPT, self.script)
        (self.tmp / 'tools' / 'video-pipeline' / 'matte_model.py').write_text(
            f"MODEL_SIZE = {len(DATA)}  # bytes\n"
            f"MODEL_SHA256 = '{hashlib.sha256(DATA).hexdigest()}'\n")
        self.src = self.tmp / 'dl' / 'isnet-anime.onnx'
        self.src.parent.mkdir()
        self.dest = self.tmp / 'payload' / 'models'
        self.out = self.dest / 'rembg' / 'isnet-anime.onnx'

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def run_it(self, *args, src=None):
        env = dict(os.environ, LILLITH_MATTE_MODEL=str(src or self.src))
        return subprocess.run(['bash', str(self.script), str(self.dest), *args],
                              env=env, capture_output=True, text=True)

    def test_good_file_is_staged(self):
        self.src.write_bytes(DATA)
        r = self.run_it()
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(self.out.read_bytes(), DATA)
        self.assertIn('sha256 OK', r.stdout)

    def test_bad_checksum_fails_and_stages_nothing(self):
        self.src.write_bytes(DATA.upper())   # same size, other bytes
        for args in ((), ('--optional',)):
            r = self.run_it(*args)
            self.assertEqual(r.returncode, 1, r.stdout)
            self.assertIn('sha256 mismatch', r.stderr)
            self.assertFalse(self.out.exists())

    def test_wrong_size_fails(self):
        self.src.write_bytes(DATA[:-1])
        r = self.run_it()
        self.assertEqual(r.returncode, 1)
        self.assertIn('bytes, needs', r.stderr)
        self.assertFalse(self.out.exists())

    def test_missing_source(self):
        r = self.run_it()
        self.assertEqual(r.returncode, 1)
        self.assertIn('LILLITH_MATTE_MODEL', r.stderr)
        r = self.run_it('--optional')
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn('warning', r.stderr)
        self.assertFalse(self.out.exists())

    def test_bad_usage(self):
        r = subprocess.run(['bash', str(self.script)], capture_output=True, text=True)
        self.assertEqual(r.returncode, 2)

    def test_real_constants_are_readable(self):
        # The real matte_model.py constants parse (the same sed the script uses).
        mm = (REPO / 'tools' / 'video-pipeline' / 'matte_model.py').read_text()
        self.assertIn("MODEL_SHA256 = 'f15622d853e8260172812b657053460e20806f04b9e05147d49af7bed31a6e99'", mm)
        self.assertRegex(mm, r'(?m)^MODEL_SIZE = 176069933\b')


if __name__ == '__main__':
    unittest.main()
