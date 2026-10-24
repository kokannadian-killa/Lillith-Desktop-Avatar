"""Every program launch goes through lillith_py.launch.run, which writes exactly one
audit.log line in the Rust format (path from crates/lillith-core/src/config.rs,
format from SoulContract::audit)."""
import os
import re
import shutil
import stat
import subprocess
import sys
from datetime import datetime
from pathlib import Path

import pytest

import procguard
from helpers import audit_lines, make_pdf, make_png
from lillith_py import launch
from lillith_py.attachments import AttachmentStore, Config
from lillith_py.attachments import extract
from lillith_py.bots import Lillith
from lillith_py.bots import engine as engine_mod

PKG = Path(launch.__file__).resolve().parent
LINE = re.compile(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ\t(EXEMPT|REFUSED)\tlillith-py:launch\t(\S+) \$ (.*)$")

FAKES = {
    "pdftotext": 'echo "fake pdf text from $3"',
    "pdfinfo": 'echo "Title: x"; echo "Pages: 3"',
    "magick": 'for a; do last=$a; done; printf thumb > "$last"',
    "tesseract": 'echo "INVOICE TOTAL 42"',
    "notify-send": 'exit 0',
    "exit3": 'exit 3',
    "sleeper": 'sleep 5',
    "echoargs": 'echo "$@"',
}


@pytest.fixture
def fakebin(tmp_path, monkeypatch):
    """Stand-ins for the exempt tools (the box/CI lack some), first on PATH."""
    d = tmp_path / "fakebin"
    d.mkdir()
    for name, body in FAKES.items():
        p = d / name
        p.write_text("#!/bin/sh\n" + body + "\n")
        p.chmod(p.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    monkeypatch.setenv("PATH", f"{d}{os.pathsep}{os.environ.get('PATH', '')}")
    return d


def parsed(tmp_path):
    out = []
    for line in audit_lines(tmp_path):
        m = LINE.match(line)
        assert m, f"not in the Rust audit format: {line!r}"
        out.append(m.groups())
    return out


# ---------- format and path ----------
def test_audit_path_is_the_rust_one(tmp_path, monkeypatch):
    monkeypatch.setenv("LILLITH_HOME", str(tmp_path / "elsewhere"))  # ignored on purpose
    assert launch.audit_log_path() == tmp_path / "xdg-data" / "lillith" / "audit.log"
    monkeypatch.delenv("XDG_DATA_HOME")
    monkeypatch.setenv("HOME", str(tmp_path / "h"))
    assert launch.audit_log_path() == tmp_path / "h" / ".local/share/lillith/audit.log"


def test_one_line_per_launch_with_outcome(tmp_path, fakebin):
    assert launch.run(["echoargs", "a", "b"], purpose="t", capture_output=True).stdout == b"a b\n"
    assert launch.run(["exit3"], capture_output=True).returncode == 3
    with pytest.raises(subprocess.TimeoutExpired):
        launch.run(["sleeper"], timeout=0.2, capture_output=True)
    with pytest.raises(FileNotFoundError):
        launch.run([str(tmp_path / "nope")], capture_output=True)
    rows = parsed(tmp_path)
    assert len(rows) == 4
    assert rows[0] == ("EXEMPT", "echoargs", "echoargs a b -> exit=0 [t]")
    assert rows[1][2].endswith("-> exit=3")
    assert rows[2][2].endswith("-> timeout")
    assert "-> error: FileNotFoundError" in rows[3][2]


def test_secrets_and_home_paths_are_masked(tmp_path, fakebin, monkeypatch):
    home = tmp_path / "home" / "ko"
    monkeypatch.setenv("HOME", str(home))
    secret = "sk-live-0123456789abcdefABCDEF"
    argv = ["echoargs", "--api-key", secret, f"--token={secret}", f"api_key={secret}",
            "Authorization: Bearer " + secret, secret, f"{home}/Documents/taxes.pdf",
            "Lillith says " * 15 + "hi", "--psm", "3"]
    r = launch.run(argv, capture_output=True)
    assert r.returncode == 0 and secret.encode() in r.stdout  # the program still got the real args
    [line] = audit_lines(tmp_path)
    assert secret not in line and str(home) not in line
    assert "--api-key <redacted>" in line and "--token=<redacted>" in line
    assert "api_key=<redacted>" in line and "Bearer <redacted>" in line
    assert "~/Documents/taxes.pdf" in line and "<197 chars>" in line and "--psm 3" in line


# ---------- each exempt program: unchanged behaviour + exactly one line ----------
def test_pdftotext_and_pdfinfo_one_line_each(tmp_path, fakebin):
    with AttachmentStore(Config(home=tmp_path / "a", ocr=False)) as s:
        f = tmp_path / "doc.pdf"
        f.write_bytes(make_pdf(["hello"]))
        a = s.add(f)
        assert a.text_source == "pdftotext" and a.pages == 3 and "fake pdf text" in s.text(a)
    rows = parsed(tmp_path)
    assert [r[1] for r in rows] == ["pdftotext", "pdfinfo"]
    assert all(r[0] == "EXEMPT" and r[2].endswith("]") and "exit=0" in r[2] for r in rows)


def test_imagemagick_thumbnail_one_line(tmp_path, fakebin, monkeypatch):
    monkeypatch.setattr(extract, "Image", None)  # no Pillow -> ImageMagick
    with AttachmentStore(Config(home=tmp_path / "a", ocr=False)) as s:
        f = tmp_path / "p.png"
        f.write_bytes(make_png(20, 10))
        a = s.add(f)
        assert a.thumbnail_path and Path(a.thumbnail_path).read_bytes() == b"thumb"
        assert (a.width, a.height) == (20, 10)
    rows = parsed(tmp_path)
    assert [(r[0], r[1]) for r in rows] == [("EXEMPT", "magick")]
    assert "-thumbnail 256x256" in rows[0][2]


def test_tesseract_ocr_one_line(tmp_path, fakebin, monkeypatch):
    monkeypatch.setattr(extract, "Image", None)
    monkeypatch.setattr(extract.launch, "which",
                        lambda n: None if n in ("magick", "convert") else shutil.which(n))
    with AttachmentStore(Config(home=tmp_path / "a", ocr=True)) as s:
        f = tmp_path / "scan.png"
        f.write_bytes(make_png(10, 10))
        a = s.add(f)
        assert a.text_source == "ocr" and s.search("invoice")[0]["id"] == a.id
    rows = parsed(tmp_path)
    assert [(r[0], r[1]) for r in rows] == [("EXEMPT", "tesseract")]


def test_pillow_needs_no_launch(tmp_path, fakebin):
    if extract.Image is None:
        pytest.skip("Pillow not installed")
    with AttachmentStore(Config(home=tmp_path / "a", ocr=False)) as s:
        f = tmp_path / "p.png"
        f.write_bytes(make_png(64, 32))
        a = s.add(f)
        assert a.thumbnail_path
    assert audit_lines(tmp_path) == []  # Pillow is in-process, not a program launch


def test_plain_text_needs_no_launch(tmp_path, fakebin):
    with AttachmentStore(Config(home=tmp_path / "a", ocr=True)) as s:
        for name, body in [("a.txt", "plain"), ("b.py", "print(1)"), ("c.json", '{"k": 1}'),
                           ("d.csv", "a,b\n1,2"), ("e.md", "# hi")]:
            (tmp_path / name).write_text(body)
            assert body.splitlines()[0] in s.text(s.add(tmp_path / name))
    assert audit_lines(tmp_path) == []


def test_notify_send_one_line_and_routine_saved(tmp_path, fakebin, mock_llama, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    app = Lillith(tmp_path / "lh", Config(home=tmp_path / "lh" / "attachments", ocr=False))
    try:
        bot = app.bots.create("Lily", endpoint=mock_llama.url)
        r = app.bots.add_routine(bot.name, "check-in", "*/5 * * * *", "How is Ko?",
                                 now=datetime(2026, 10, 23, 8, 2))
        res = app.run_due(now=datetime(2026, 10, 23, 8, 5))
        assert res[0]["status"] == "ok" and res[0]["reply"].startswith("MOCK REPLY")
        assert app.bots.history(bot.name)[-1]["source"] == "routine:check-in"
        assert app.tasks.job_log(res[0]["task"])[-1]["status"] == "done"
        assert app.bots.routine(r["id"])["last_status"] == "ok"
    finally:
        app.close()
    rows = parsed(tmp_path)
    assert [(r[0], r[1]) for r in rows] == [("EXEMPT", "notify-send")]
    assert "--app-name=Lillith" in rows[0][2] and rows[0][2].endswith("-> exit=0 [routine notification]")


def test_notify_send_missing_launches_nothing(tmp_path, monkeypatch):
    monkeypatch.setenv("PATH", str(tmp_path / "empty"))
    assert engine_mod.send_notification("t", "b") is False
    assert audit_lines(tmp_path) == []


@pytest.mark.skipif(not shutil.which("pdftotext"), reason="poppler not installed")
def test_real_pdftotext_through_wrapper(tmp_path):
    with AttachmentStore(Config(home=tmp_path / "a", ocr=False)) as s:
        f = tmp_path / "doc.pdf"
        f.write_bytes(make_pdf(["Lillith lives on the SSD"]))
        a = s.add(f)
        assert "Lillith lives on the SSD" in s.text(a)
    progs = [r[1] for r in parsed(tmp_path)]
    assert progs[0] == "pdftotext" and len(progs) == (2 if shutil.which("pdfinfo") else 1)


# ---------- nothing bypasses the wrapper ----------
def test_test_guard_catches_a_direct_launch(tmp_path):
    with procguard.expect_violation():
        with pytest.raises(procguard.BypassError):
            subprocess.run(["true"])
        with pytest.raises(procguard.BypassError):
            os.system("true")


def test_runtime_guard_refuses_and_audits_a_bypass(tmp_path):
    with procguard.suspended():
        with pytest.raises(launch.LaunchRefused):
            subprocess.run(["/bin/echo", "hi"], capture_output=True)
    rows = parsed(tmp_path)
    assert rows == [("REFUSED", "echo", "echo hi -> bypassed the lillith_py.launch wrapper")]


FORBIDDEN = re.compile(r"\b(import subprocess|from subprocess|os\.system|os\.popen|os\.spawn\w*|"
                       r"os\.exec\w*|os\.posix_spawn\w*|os\.fork\w*|os\.startfile|import pty|"
                       r"create_subprocess_\w+|import pytesseract|import webbrowser|import sh\b)")


def test_no_launch_code_outside_the_wrapper():
    bad = []
    for p in PKG.rglob("*.py"):
        if p.name == "launch.py" and p.parent == PKG:
            continue
        for n, line in enumerate(p.read_text(encoding="utf-8").splitlines(), 1):
            if FORBIDDEN.search(line):
                bad.append(f"{p.relative_to(PKG)}:{n}: {line.strip()}")
    assert bad == []
