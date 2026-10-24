#!/usr/bin/env python3
"""Unit test for tools/check-release-zip.py (run: python3 tools/check_release_zip_test.py).
Also run by the cargo gate (crates/lillith-learn/tests/release_zip_check.rs)."""
import os
import subprocess
import sys
import tempfile
import unittest
import zipfile

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "check-release-zip.py")


def make_zip(d, name, entries):
    p = os.path.join(d, name)
    with zipfile.ZipFile(p, "w") as z:
        for arc, data in entries.items():
            z.writestr(arc, data)
    return p


def run(path):
    return subprocess.run([sys.executable, SCRIPT, path], capture_output=True, text=True)


BENIGN = {
    "Lillith/README.md": "hello",
    "Lillith/crates/lillith-learn/src/lib.rs": "// code",
    "Lillith/crates/lillith-learn/tests/online_research.rs": "// tests are allowed",
    "Lillith/certs/public.pem": "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n",
    "Lillith/docs/keyboard.md": "not a .key file",
}


class CheckReleaseZip(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.d = self.tmp.name

    def tearDown(self):
        self.tmp.cleanup()

    def test_clean_zip_passes(self):
        r = run(make_zip(self.d, "clean.zip", BENIGN))
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("ok:", r.stdout)

    def test_tls_fixture_key_is_refused_and_named(self):
        bad = dict(BENIGN)
        bad["foo/tests/fixtures/tls/srv.key.der"] = b"\x30\x82"
        r = run(make_zip(self.d, "bad.zip", bad))
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("foo/tests/fixtures/tls/srv.key.der", r.stdout)
        self.assertNotIn("README.md", r.stdout)

    def test_every_forbidden_kind_is_refused(self):
        cases = [
            "x/tests/fixtures/tls/ca.der",
            "x/secret.key",
            "x/a.key.der",
            "x/models/m.Q4_K_M.gguf",
            "x/master.zip",
            "x/tools/lillith-py/bot.py",
            "x/server.pem",
        ]
        for arc in cases:
            data = "-----BEGIN PRIVATE KEY-----\nAA\n-----END PRIVATE KEY-----\n" if arc.endswith(".pem") else "x"
            with self.subTest(arc=arc):
                p = make_zip(self.d, "one.zip", {"x/ok.txt": "ok", arc: data})
                r = run(p)
                self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
                self.assertIn(arc, r.stdout)

    def test_not_a_zip_fails(self):
        p = os.path.join(self.d, "nope.zip")
        with open(p, "w") as f:
            f.write("not a zip")
        self.assertEqual(run(p).returncode, 2)
        self.assertEqual(subprocess.run([sys.executable, SCRIPT]).returncode, 2)


if __name__ == "__main__":
    unittest.main()
