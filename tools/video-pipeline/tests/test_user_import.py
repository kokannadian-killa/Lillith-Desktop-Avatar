"""Tests for tools/video-pipeline/user_import.py (the GUI's "add my own video" engine).

  python3 -m pytest -q -p no:cacheprovider tools/video-pipeline/tests      (or: python3 -m unittest discover)

Every test video is synthesised here (a few frames at 160x120); nothing big is committed. The importer runs
with --work-height 160 so a run takes ~20 s on one core. Needs ffmpeg/ffprobe with libx264, numpy, scipy,
opencv, Pillow; skipped when ffmpeg is missing.
"""
import json, os, shutil, signal, subprocess, sys, tempfile, time, unittest
from pathlib import Path

sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent
TOOL = HERE.parent / 'user_import.py'
REPO = HERE.parent.parent.parent
sys.path.insert(0, str(HERE))
import synth  # noqa: E402

FF = shutil.which('ffmpeg'); FP = shutil.which('ffprobe')
STAGES = {'probe', 'decode', 'split', 'matte', 'clean', 'zoom', 'encode', 'qa'}
TINY = ['--work-height', '160', '--min-seconds', '0.9', '--max-ram-mb', '900']


def env_for(tmp):
    e = dict(os.environ, PYTHONDONTWRITEBYTECODE='1', TMPDIR=str(tmp), TEMP=str(tmp), TMP=str(tmp))
    e.pop('LILLITH_FFMPEG', None); e.pop('LILLITH_FFPROBE', None)
    return e


def run(args, env, timeout=600):
    p = subprocess.run([sys.executable, str(TOOL)] + [str(a) for a in args], capture_output=True, text=True, env=env, timeout=timeout)
    evs = [json.loads(l) for l in p.stdout.splitlines() if l.strip()]
    return p, evs


def tree(d):
    return {str(p.relative_to(d)): p.stat().st_mtime_ns for p in Path(d).rglob('*')}


@unittest.skipUnless(FF and FP, 'ffmpeg/ffprobe not installed')
class FullRun(unittest.TestCase):
    """One real run shared by the schema / split / confinement checks."""

    @classmethod
    def setUpClass(cls):
        cls.tmp = Path(tempfile.mkdtemp(prefix='ui_test_'))
        cls.sandbox_tmp = cls.tmp / 'tmpdir'; cls.sandbox_tmp.mkdir()
        cls.video = cls.tmp / 'in' / 'my dance clip.mp4'; cls.video.parent.mkdir()
        # wave 1.2 s -> hold 0.6 s -> bounce 1.2 s -> scene cut -> sway 1.2 s
        synth.make(cls.video, FF, w=160, h=120, wave=1.2, hold=0.6, bounce=1.2, scene2=1.2)
        cls.out = cls.tmp / 'staging'
        cls.tools_before = tree(HERE.parent)
        cls.in_before = tree(cls.video.parent)
        cls.p, cls.evs = run(['--input', cls.video, '--out', cls.out, '--json-progress'] + TINY, env_for(cls.sandbox_tmp))

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.tmp, ignore_errors=True)

    def test_exit_and_done(self):
        self.assertEqual(self.p.returncode, 0, self.p.stderr[-2000:])
        self.assertEqual(self.evs[-1]['event'], 'done')
        cands = [e for e in self.evs if e['event'] == 'candidate']
        self.assertEqual(self.evs[-1]['candidates'], len(cands))

    def test_event_schema(self):
        last = -1
        for e in self.evs:
            self.assertIn(e['event'], {'progress', 'candidate', 'done', 'error'})
            if e['event'] == 'progress':
                self.assertEqual(set(e), {'event', 'stage', 'pct', 'msg'})
                self.assertIn(e['stage'], STAGES)
                self.assertIsInstance(e['pct'], int); self.assertTrue(0 <= e['pct'] <= 100)
                self.assertGreaterEqual(e['pct'], last); last = e['pct']
                self.assertIsInstance(e['msg'], str)
            elif e['event'] == 'candidate':
                self.assertEqual(set(e), {'event', 'id', 'mp4', 'preview_png', 'frames', 'fps', 'qa', 'suggested_name'})
                self.assertTrue(Path(e['mp4']).is_absolute() and Path(e['mp4']).is_file())
                self.assertTrue(Path(e['preview_png']).is_absolute() and Path(e['preview_png']).is_file())
                self.assertEqual(e['fps'], 12); self.assertGreater(e['frames'], 0)
                self.assertEqual(set(e['qa']), {'pass', 'white_edge', 'fringe'})
                self.assertIsInstance(e['qa']['pass'], bool)
                self.assertTrue(e['suggested_name'].startswith('user_my_dance_clip_'))
        self.assertEqual(sum(e['event'] == 'error' for e in self.evs), 0)
        self.assertNotIn('error', [e['event'] for e in self.evs])

    def test_splits_into_movements(self):
        cands = [e for e in self.evs if e['event'] == 'candidate']
        self.assertGreaterEqual(len(cands), 3, [e['msg'] for e in self.evs if e['event'] == 'progress' and e['stage'] == 'split'])

    def test_clip_format(self):
        import numpy as np
        from PIL import Image
        for e in (e for e in self.evs if e['event'] == 'candidate'):
            d = Path(e['mp4']).parent; n = d.name
            js = json.loads((d / f'{n}.json').read_text())
            self.assertEqual((js['frame_count'], js['fps'], js['anchor'], js['baseline_y']), (e['frames'], 12, [200, 392], 392))
            sh = np.asarray(Image.open(d / js['sheet']).convert('RGBA'))
            self.assertEqual(sh.shape, (400, 400 * e['frames'], 4))
            self.assertFalse(sh[393:, :, 3].any(), 'pixels below the baseline')
            pr = subprocess.run([FP, '-v', 'error', '-select_streams', 'v:0', '-count_frames', '-show_entries',
                                 'stream=width,height,nb_read_frames,codec_name', '-of', 'json', e['mp4']], capture_output=True, text=True)
            st = json.loads(pr.stdout)['streams'][0]
            self.assertEqual((st['codec_name'], st['width'], st['height'], int(st['nb_read_frames'])), ('h264', 800, 400, e['frames']))
            if e['frames'] >= 12:
                self.assertGreater(max(js['cleavage_zoom']['zoom_per_frame']), 1.5)
                self.assertEqual(js['cleavage_zoom']['zoom_per_frame'][0], 1.0)
                self.assertEqual(js['cleavage_zoom']['zoom_per_frame'][-1], 1.0)
        self.assertTrue(any(e['frames'] >= 12 for e in self.evs if e['event'] == 'candidate'), 'no movement long enough to zoom')

    def test_writes_only_inside_out(self):
        self.assertEqual(tree(HERE.parent), self.tools_before, 'files changed next to the tools')
        self.assertEqual(tree(self.video.parent), self.in_before, 'files changed next to the input')
        self.assertEqual(list(self.sandbox_tmp.iterdir()), [], 'temp files left in TMPDIR')
        self.assertEqual(sorted(p.name for p in self.tmp.iterdir()), ['in', 'staging', 'tmpdir'])
        self.assertFalse((self.out / '.user_import_work').exists(), 'work dir left behind')

    def test_ram_cap(self):
        peak = self.evs[-1].get('peak_rss_mb')
        self.assertIsInstance(peak, int)
        self.assertLess(peak, 900 - 150)


@unittest.skipUnless(FF and FP, 'ffmpeg/ffprobe not installed')
class Edges(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix='ui_edge_'))
        self.video = self.tmp / 'clip.mp4'
        synth.make(self.video, FF, w=160, h=120, wave=1.0, hold=0.4, bounce=1.0, scene2=1.0)

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_ram_cap_too_low_is_an_error(self):
        p, evs = run(['--input', self.video, '--out', self.tmp / 'o', '--json-progress', '--max-ram-mb', '60'], env_for(self.tmp))
        self.assertEqual(p.returncode, 1)
        self.assertEqual(evs[-1]['event'], 'error'); self.assertIn('max-ram-mb', evs[-1]['msg'])
        self.assertFalse((self.tmp / 'o').exists())

    def test_ram_stays_under_cap(self):
        """the whole child tree (importer + ffmpeg) stays under --max-ram-mb (Linux: ru_maxrss of children)."""
        try:
            import resource
        except ImportError:
            self.skipTest('no resource module')
        code = ('import resource,subprocess,sys;'
                f'r=subprocess.run([sys.executable,{str(TOOL)!r},"--input",{str(self.video)!r},"--out",{str(self.tmp / "o")!r},'
                '"--json-progress","--work-height","160","--min-seconds","0.9","--max-ram-mb","700"],capture_output=True);'
                'print(r.returncode, resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss)')
        p = subprocess.run([sys.executable, '-c', code], capture_output=True, text=True, env=env_for(self.tmp), timeout=600)
        rc, kb = p.stdout.split()
        self.assertEqual(rc, '0', p.stderr)
        self.assertLess(int(kb) / 1024, 700)

    def test_missing_ffmpeg(self):
        e = env_for(self.tmp); empty = self.tmp / 'empty'; empty.mkdir(); e['PATH'] = str(empty)
        p, evs = run(['--input', self.video, '--out', self.tmp / 'o', '--json-progress'], e)
        self.assertEqual(p.returncode, 1)
        self.assertEqual(evs[-1]['event'], 'error')
        self.assertIn('ffmpeg', evs[-1]['msg']); self.assertIn('--ffmpeg', evs[-1]['msg'])
        self.assertFalse((self.tmp / 'o').exists())

    def test_ffmpeg_flags(self):
        """--ffmpeg/--ffprobe as files (path with spaces, as in C:\\Program Files\\...) or as their bin folder."""
        b = self.tmp / 'Lillith Desktop' / 'bin'; b.mkdir(parents=True)
        for n, src in (('ffmpeg', FF), ('ffprobe', FP)):
            shutil.copy2(src, b / n)  # a real copy, like the bundled bin\ffmpeg.exe
        e = env_for(self.tmp); empty = self.tmp / 'empty'; empty.mkdir(); e['PATH'] = str(empty)
        sys.path.insert(0, str(TOOL.parent))
        import user_import as ui
        A = type('A', (), {})
        a = A(); a.ffmpeg, a.ffprobe = str(b / 'ffmpeg'), None
        old = os.environ.get('PATH'); os.environ['PATH'] = str(empty)
        try:
            self.assertEqual(ui.find_tools(a), (str((b / 'ffmpeg').resolve()), str((b / 'ffprobe').resolve())))  # ffprobe found next to ffmpeg
            a.ffmpeg, a.ffprobe = str(b), str(b)
            self.assertEqual(ui.find_tools(a), (str((b / 'ffmpeg').resolve()), str((b / 'ffprobe').resolve())))
            a.ffmpeg = str(self.tmp / 'nope' / 'ffmpeg.exe')
            with self.assertRaises(ui.Fail):
                ui.find_tools(a)
        finally:
            os.environ['PATH'] = old
        # and a real (tiny) probe through the flag with PATH empty: gets past probe/split
        p, evs = run(['--input', self.video, '--out', self.tmp / 'o', '--json-progress', '--ffmpeg', b / 'ffmpeg',
                      '--ffprobe', b / 'ffprobe', '--max-ram-mb', '60000', '--work-height', '160', '--max-candidates', '1',
                      '--min-seconds', '0.9'], e)
        self.assertEqual(p.returncode, 0, p.stderr[-1500:])
        self.assertEqual(evs[-1]['event'], 'done')

    def test_bad_input(self):
        bad = self.tmp / 'notes.txt'; bad.write_text('x')
        p, evs = run(['--input', bad, '--out', self.tmp / 'o', '--json-progress'], env_for(self.tmp))
        self.assertEqual((p.returncode, evs[-1]['event']), (1, 'error'))
        broken = self.tmp / 'broken.mp4'; broken.write_bytes(b'\0' * 2048)
        p, evs = run(['--input', broken, '--out', self.tmp / 'o', '--json-progress'], env_for(self.tmp))
        self.assertEqual((p.returncode, evs[-1]['event']), (1, 'error'))

    def _cancel(self, out):
        kw = {}
        if os.name == 'nt':
            kw['creationflags'] = subprocess.CREATE_NEW_PROCESS_GROUP
        p = subprocess.Popen([sys.executable, str(TOOL), '--input', str(self.video), '--out', str(out), '--json-progress'] + TINY,
                             stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, env=env_for(self.tmp), **kw)
        for line in p.stdout:
            ev = json.loads(line)
            if ev['event'] == 'progress' and ev['stage'] in ('matte', 'zoom', 'clean'):
                break
        else:
            self.fail('never reached the matte stage')
        t0 = time.monotonic()
        p.send_signal(signal.CTRL_BREAK_EVENT if os.name == 'nt' else signal.SIGTERM)
        rc = p.wait(timeout=10)
        dt = time.monotonic() - t0
        p.stdout.close()
        self.assertEqual(rc, 130)
        self.assertLess(dt, 1.5)
        return dt

    def test_cancel_removes_new_staging(self):
        out = self.tmp / 'o'
        self._cancel(out)
        self.assertFalse(out.exists())

    def test_cancel_keeps_existing_files(self):
        out = self.tmp / 'existing'; out.mkdir(); (out / 'keep.txt').write_text('mine')
        self._cancel(out)
        self.assertEqual(sorted(p.name for p in out.iterdir()), ['keep.txt'])


class Units(unittest.TestCase):
    def test_zoom_curve(self):
        sys.path.insert(0, str(TOOL.parent))
        import user_import as ui
        zs, us, ks = ui.zoom_curve(40, 1.8)
        self.assertEqual((zs[0], zs[-1], max(zs)), (1.0, 1.0, 1.8))
        self.assertEqual(ks, sorted(ks))
        self.assertEqual(ui.zoom_curve(8, 1.8)[2], None)
        self.assertEqual(ui.slug('My Dance (final).MOV'), 'my_dance_final_mov')

    def test_holefill_fills_enclosed_highlight(self):
        import numpy as np
        sys.path.insert(0, str(TOOL.parent))
        import cellfix
        c = np.zeros((400, 400, 4), np.uint8); c[100:300, 100:300] = (200, 50, 120, 255)
        c[190:196, 190:196] = 0  # punched highlight
        out, n = cellfix.holefill(c)
        self.assertEqual(n, 36); self.assertTrue((out[100:300, 100:300, 3] == 255).all())


if __name__ == '__main__':
    unittest.main()
