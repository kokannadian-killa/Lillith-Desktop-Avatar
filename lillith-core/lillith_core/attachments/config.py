"""Configuration for Lillith attachments (100% local)."""
from __future__ import annotations

import json
import os
from dataclasses import dataclass, field, asdict
from pathlib import Path
from urllib.parse import urlparse

LOOPBACK_HOSTS = {"127.0.0.1", "localhost", "::1", "[::1]"}


def _xdg(var: str, default: str) -> Path:
    return Path(os.environ.get(var) or (Path.home() / default))


def default_home() -> Path:
    env = os.environ.get("LILLITH_ATTACH_HOME")
    if env:
        return Path(env).expanduser()
    return _xdg("XDG_DATA_HOME", ".local/share") / "lillith" / "attachments"


def default_config_path() -> Path:
    env = os.environ.get("LILLITH_ATTACH_CONFIG")
    if env:
        return Path(env).expanduser()
    return _xdg("XDG_CONFIG_HOME", ".config") / "lillith" / "attachments.json"


class NonLocalURLError(ValueError):
    """Raised when a URL would leave the machine."""


def assert_loopback(url: str) -> str:
    p = urlparse(url)
    if p.scheme not in ("http", "https"):
        raise NonLocalURLError(f"unsupported scheme in {url!r}")
    host = (p.hostname or "").lower()
    if host not in LOOPBACK_HOSTS and not host.startswith("127."):
        raise NonLocalURLError(f"refusing non-loopback host {host!r} (Lillith is local-only)")
    return url


@dataclass
class Config:
    home: Path = field(default_factory=default_home)
    llama_url: str = "http://127.0.0.1:8080"
    model: str = "dolphin3.0-llama3.2-3b-q4_k_m"
    # Context budget (in approx tokens) reserved for attachment content per request.
    token_budget: int = 2048
    chars_per_token: float = 4.0
    max_file_bytes: int = 200 * 1024 * 1024       # refuse to ingest bigger files
    max_text_bytes: int = 5 * 1024 * 1024         # cap on extracted text kept on disk
    read_default_limit: int = 200                  # lines per page
    max_line_chars: int = 2000
    thumb_size: int = 256
    ocr: bool = True                               # use tesseract on images if available
    vision: bool = False                           # send images to llama-server (needs --mmproj)
    vision_max_bytes: int = 4 * 1024 * 1024
    request_timeout: float = 300.0
    system_prompt: str = "You are Lillith, a helpful local AI companion."

    def __post_init__(self):
        self.home = Path(self.home).expanduser()
        assert_loopback(self.llama_url)

    @classmethod
    def load(cls, path: Path | None = None, **overrides) -> "Config":
        path = Path(path) if path else default_config_path()
        data: dict = {}
        if path.is_file():
            data = json.loads(path.read_text(encoding="utf-8"))
        if os.environ.get("LILLITH_LLAMA_URL"):
            data["llama_url"] = os.environ["LILLITH_LLAMA_URL"]
        data.update({k: v for k, v in overrides.items() if v is not None})
        known = {f for f in cls.__dataclass_fields__}
        return cls(**{k: v for k, v in data.items() if k in known})

    def to_dict(self) -> dict:
        d = asdict(self)
        d["home"] = str(self.home)
        return d
