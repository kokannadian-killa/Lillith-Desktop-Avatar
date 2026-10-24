import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from mock_llama import MockLlama  # noqa: E402
from lillith_core.attachments import AttachmentStore, Config  # noqa: E402


@pytest.fixture(autouse=True)
def _isolate_env(monkeypatch, tmp_path):
    for v in ("LILLITH_HOME", "LILLITH_ATTACH_HOME", "LILLITH_ATTACH_CONFIG", "LILLITH_LLAMA_URL",
              "LILLITH_API_TOKEN"):
        monkeypatch.delenv(v, raising=False)
    monkeypatch.setenv("XDG_DATA_HOME", str(tmp_path / "xdg-data"))
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path / "xdg-config"))


@pytest.fixture
def mock_llama():
    with MockLlama() as m:
        yield m


@pytest.fixture
def store(tmp_path, mock_llama):
    cfg = Config(home=tmp_path / "att", llama_url=mock_llama.url, ocr=False)
    s = AttachmentStore(cfg)
    yield s
    s.close()


@pytest.fixture
def files(tmp_path):
    d = tmp_path / "in"
    d.mkdir()
    return d
