#!/usr/bin/env python3
"""Unit test for tools/check-release-zip.py (run: python3 tools/check_release_zip_test.py).
Also run by the cargo gate (crates/lillith-learn/tests/release_zip_check.rs)."""
import os
import subprocess
import sys
import io
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


def zip_bytes(entries):
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
        for arc, data in entries.items():
            z.writestr(arc, data)
    return buf.getvalue()


def write_parts(d, name, data, n):
    """Cut `data` into `n` pieces name.part1..name.partN (like `split`)."""
    step = -(-len(data) // n)
    paths = []
    for i in range(n):
        p = os.path.join(d, f"{name}.part{i + 1}")
        with open(p, "wb") as f:
            f.write(data[i * step : (i + 1) * step])
        paths.append(p)
    return paths


def run(*paths):
    return subprocess.run([sys.executable, SCRIPT, *paths], capture_output=True, text=True)


BENIGN = {
    "Lillith/README.md": "hello",
    "Lillith/crates/lillith-learn/src/lib.rs": "// code",
    "Lillith/crates/lillith-learn/tests/online_research.rs": "// tests are allowed",
    "Lillith/certs/public.pem": "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n",
    "Lillith/docs/keyboard.md": "not a .key file",
    "Lillith/docs/rsa_id_howto.md": "basename does not start with id_rsa",
    "Lillith/docs/my_id_rsa.txt": "nor does this one",
    "Lillith/assets/p12.png": "not a .p12",
    "Lillith/docs/safetensors.md": "not a .safetensors file",
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
            "x/certs/client.p12",
            "x/certs/CLIENT.PFX",
            "x/.ssh/id_rsa",
            "x/.ssh/id_rsa.pub",
            "x/backup/id_rsa_backup",
            "x/models/model.safetensors",
        ]
        for arc in cases:
            data = "-----BEGIN PRIVATE KEY-----\nAA\n-----END PRIVATE KEY-----\n" if arc.endswith(".pem") else "x"
            with self.subTest(arc=arc):
                p = make_zip(self.d, "one.zip", {"x/ok.txt": "ok", arc: data})
                r = run(p)
                self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
                self.assertIn(arc, r.stdout)

    def test_new_kinds_benign_lookalikes_pass(self):
        for arc in ["x/docs/ssh_id_rsa.md", "x/my_id_rsa.txt", "x/p12.png",
                    "x/pfx/readme.md", "x/safetensors.md", "x/id_rsa/"]:
            with self.subTest(arc=arc):
                r = run(make_zip(self.d, "ok.zip", {arc: "" if arc.endswith("/") else "x"}))
                self.assertEqual(r.returncode, 0, r.stdout + r.stderr)

    # ---- nested zips -------------------------------------------------

    def test_nested_zip_clean_passes(self):
        inner = zip_bytes({"a/readme.txt": "hi", "b/inner2.zip": zip_bytes({"c.txt": "ok"})})
        r = run(make_zip(self.d, "outer.zip", {**BENIGN, "Lillith/extra.zip": inner}))
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)

    def test_nested_zip_with_banned_entry_is_refused_and_named(self):
        for banned in ["keys/id_rsa", "w/model.safetensors", "c/x.p12",
                       "t/tests/fixtures/tls/srv.key.der", "tools/lillith-py/a.py"]:
            with self.subTest(banned=banned):
                inner = zip_bytes({"ok.txt": "ok", banned: "x"})
                r = run(make_zip(self.d, "outer.zip", {**BENIGN, "Lillith/extra.zip": inner}))
                self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
                self.assertIn(f"Lillith/extra.zip -> {banned}", r.stdout)
                self.assertNotIn("ok.txt", r.stdout)

    def test_nested_private_pem_is_read_from_inside(self):
        pem = "-----BEGIN RSA PRIVATE KEY-----\nAA\n-----END RSA PRIVATE KEY-----\n"
        inner = zip_bytes({"certs/public.pem": BENIGN["Lillith/certs/public.pem"], "certs/s.pem": pem})
        r = run(make_zip(self.d, "outer.zip", {"in.zip": inner}))
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertIn("in.zip -> certs/s.pem", r.stdout)
        self.assertNotIn("public.pem", r.stdout)

    def test_deeply_nested_banned_entry_names_full_path(self):
        z3 = zip_bytes({"deep/id_rsa.pub": "ssh-rsa AAAA"})
        z2 = zip_bytes({"two.zip": z3})
        z1 = zip_bytes({"one/three.ZIP": z2})
        r = run(make_zip(self.d, "outer.zip", {"top.zip": z1}))
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertIn("top.zip -> one/three.ZIP -> two.zip -> deep/id_rsa.pub", r.stdout)

    def test_nested_zip_bomb_depth_is_refused(self):
        z = zip_bytes({"leaf.txt": "x"})
        for i in range(12):
            z = zip_bytes({f"l{i}.zip": z})
        r = run(make_zip(self.d, "outer.zip", {"top.zip": z}))
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertIn("deeper than", r.stdout)

    def test_corrupt_nested_zip_fails_closed(self):
        r = run(make_zip(self.d, "outer.zip", {"x/broken.zip": "not really a zip"}))
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertIn("x/broken.zip", r.stdout)
        self.assertIn("unreadable", r.stdout)

    # ---- split .partN archives ---------------------------------------

    def test_split_clean_set_passes_via_part1_or_explicit_list(self):
        data = zip_bytes({**BENIGN, "Lillith/big.bin": os.urandom(50_000)})
        parts = write_parts(self.d, "rel.zip", data, 4)
        for args in ([parts[0]], parts):
            with self.subTest(args=len(args)):
                r = run(*args)
                self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
                self.assertIn("split, 4 parts", r.stdout)

    def test_split_set_with_banned_entry_is_refused(self):
        inner = zip_bytes({"k/id_rsa": "key"})
        data = zip_bytes({**BENIGN, "Lillith/big.bin": os.urandom(30_000),
                          "x/m.safetensors": "w", "Lillith/n.zip": inner})
        parts = write_parts(self.d, "rel.zip", data, 3)
        r = run(parts[0])
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertIn("x/m.safetensors", r.stdout)
        self.assertIn("Lillith/n.zip -> k/id_rsa", r.stdout)

    def test_split_missing_part_is_bad_input(self):
        data = zip_bytes({**BENIGN, "Lillith/big.bin": os.urandom(30_000)})
        parts = write_parts(self.d, "rel.zip", data, 3)
        os.remove(parts[1])
        for given in (parts[0], parts[2]):  # finds part1, sees part3 orphaned
            r = run(given)
            self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
            self.assertIn("missing part2", r.stderr)
            self.assertNotIn("Traceback", r.stderr)
        r = run(parts[0], parts[2])
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.assertIn("missing part2", r.stderr)
        self.assertNotIn("Traceback", r.stderr)

    def test_split_missing_first_part_is_bad_input(self):
        data = zip_bytes({**BENIGN, "Lillith/big.bin": os.urandom(30_000)})
        parts = write_parts(self.d, "rel.zip", data, 3)
        os.remove(parts[0])
        r = run(parts[1])
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.assertIn("part1 is missing", r.stderr)
        self.assertNotIn("Traceback", r.stderr)

    def test_split_missing_last_part_is_bad_input(self):
        data = zip_bytes({**BENIGN, "Lillith/big.bin": os.urandom(30_000)})
        parts = write_parts(self.d, "rel.zip", data, 3)
        os.remove(parts[2])
        r = run(parts[0])  # part1+part2 only: no central directory
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.assertNotIn("Traceback", r.stderr)

    def test_split_lone_part_is_bad_input(self):
        p = os.path.join(self.d, "solo.zip.part1")
        with open(p, "wb") as f:
            f.write(zip_bytes(BENIGN))
        r = run(p)
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.assertIn("lone part", r.stderr)
        self.assertNotIn("Traceback", r.stderr)

    def test_split_out_of_order_and_mixed_args_are_bad_input(self):
        data = zip_bytes({**BENIGN, "Lillith/big.bin": os.urandom(30_000)})
        parts = write_parts(self.d, "rel.zip", data, 3)
        other = write_parts(self.d, "other.zip", data, 2)
        plain = make_zip(self.d, "plain.zip", BENIGN)
        for args, msg in [
            ([parts[1], parts[0], parts[2]], "out of order"),
            ([parts[0], parts[0], parts[1], parts[2]], "given twice"),
            ([parts[0], other[1]], "different archives"),
            ([plain, parts[0]], "not a mix"),
            ([parts[0], parts[1]], "not in the set"),  # part3 left out
        ]:
            with self.subTest(msg=msg, n=len(args)):
                r = run(*args)
                self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
                self.assertIn(msg, r.stderr)
                self.assertNotIn("Traceback", r.stderr)

    def test_split_parts_that_are_not_a_zip_are_bad_input(self):
        parts = write_parts(self.d, "junk.zip", b"not a zip at all, just bytes" * 10, 2)
        r = run(parts[0])
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.assertNotIn("Traceback", r.stderr)

    def test_not_a_zip_fails(self):
        p = os.path.join(self.d, "nope.zip")
        with open(p, "w") as f:
            f.write("not a zip")
        self.assertEqual(run(p).returncode, 2)
        self.assertEqual(subprocess.run([sys.executable, SCRIPT]).returncode, 2)


if __name__ == "__main__":
    unittest.main()
