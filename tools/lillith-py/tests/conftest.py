import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import netguard  # noqa: E402
from mock_llama import MockLlama  # noqa: E402
from lillith_py.attachments import AttachmentStore, Config  # noqa: E402


@pytest.fixture(autouse=True)
def _isolate_env(monkeypatch, tmp_path):
    for v in ("LILLITH_HOME", "LILLITH_ATTACH_HOME", "LILLITH_CONFIG", "LILLITH_API_TOKEN"):
        monkeypatch.delenv(v, raising=False)
    monkeypatch.setenv("XDG_DATA_HOME", str(tmp_path / "xdg-data"))
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path / "xdg-config"))


def write_config(tmp_path, text: str) -> Path:
    """Write Lillith's config.toml where lillith_config looks for it."""
    d = tmp_path / "xdg-config" / "lillith"
    d.mkdir(parents=True, exist_ok=True)
    (d / "config.toml").write_text(text, encoding="utf-8")
    return d / "config.toml"


@pytest.fixture
def mock_llama(tmp_path):
    with MockLlama() as m:
        # point Lillith's config.toml [chat] at the mock (as the Rust app writes it)
        write_config(tmp_path, f'[chat]\nendpoint = "{m.url}"\nmodel = "mock-model"\n'
                               f'system_prompt = "You are Lillith (test persona)."\n')
        yield m


@pytest.fixture
def store(tmp_path, mock_llama):
    cfg = Config(home=tmp_path / "att", ocr=False)
    s = AttachmentStore(cfg)
    yield s
    s.close()


@pytest.fixture
def files(tmp_path):
    d = tmp_path / "in"
    d.mkdir()
    return d


# ---------- network guard (loopback only, enforced for the whole run) ----------
def pytest_configure(config):
    netguard.install()


def pytest_sessionfinish(session, exitstatus):
    if netguard.VIOLATIONS:
        print("\n\nNETWORK GUARD: non-loopback network access attempted:")
        for v in netguard.VIOLATIONS:
            print("  " + v)
        session.exitstatus = 1
    netguard.uninstall()
