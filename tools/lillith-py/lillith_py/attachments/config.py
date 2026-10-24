"""Attachment store settings. Model/endpoint settings are NOT here: they come
from Lillith's config.toml (see lillith_py.lillith_config). Store knobs can be
tuned in config.toml under [sidecar.attachments]."""
from __future__ import annotations

import os
from dataclasses import dataclass, field, asdict
from pathlib import Path

from ..lillith_config import (LOOPBACK_HOSTS, NonLocalURLError, assert_loopback,  # noqa: F401
                              data_dir, load_toml, llm_settings, sidecar_settings)


def default_home() -> Path:
    env = os.environ.get("LILLITH_ATTACH_HOME")
    if env:
        return Path(env).expanduser()
    return data_dir() / "attachments"


@dataclass
class Config:
    home: Path = field(default_factory=default_home)
    # Context budget (approx tokens) reserved for attachment content per request.
    token_budget: int = 2048
    chars_per_token: float = 4.0
    max_file_bytes: int = 200 * 1024 * 1024
    max_text_bytes: int = 5 * 1024 * 1024
    read_default_limit: int = 200
    max_line_chars: int = 2000
    thumb_size: int = 256
    ocr: bool = True
    vision: bool = False                 # send images to llama-server (needs --mmproj)
    vision_max_bytes: int = 4 * 1024 * 1024
    request_timeout: float = 300.0

    def __post_init__(self):
        self.home = Path(self.home).expanduser()

    @classmethod
    def load(cls, path: Path | None = None, **overrides) -> "Config":
        """Read [sidecar.attachments] from Lillith's config.toml (``path`` overrides
        the config.toml location)."""
        cfg = load_toml(path)
        data = dict(sidecar_settings(cfg).attachments or {})
        data.update({k: v for k, v in overrides.items() if v is not None})
        known = set(cls.__dataclass_fields__)
        return cls(**{k: v for k, v in data.items() if k in known})

    def to_dict(self) -> dict:
        d = asdict(self)
        d["home"] = str(self.home)
        return d
