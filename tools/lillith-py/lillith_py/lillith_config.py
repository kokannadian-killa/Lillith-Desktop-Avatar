"""Read Lillith's own config.toml (owned by the Rust app) - read-only.

Location (same rules as crates/lillith-core/src/config.rs):
    $LILLITH_CONFIG (override, tests) else
    $XDG_CONFIG_HOME/lillith/config.toml else ~/.config/lillith/config.toml

Tables used by lillith-py (all optional; the Rust Config ignores unknown tables):

    [llm]              endpoint, model, persona, api_key      (preferred)
    [chat]             endpoint, model, system_prompt, api_key (Rust chat bubble; fallback)
    [sidecar]          port (8765), workers (1, max 2), stall_minutes (5)
    [sidecar.attachments]  token_budget, ocr, vision, max_file_bytes, ...

Missing file / tables fall back to http://127.0.0.1:8080. A non-loopback
endpoint is always refused.
"""
from __future__ import annotations

import os
import tomllib
from dataclasses import dataclass
from pathlib import Path
from urllib.parse import urlparse

DEFAULT_ENDPOINT = "http://127.0.0.1:8080"
DEFAULT_MODEL = "local"
DEFAULT_PORT = 8765
MAX_WORKERS = 2
DEFAULT_PERSONA = ("You are a helper bot working for Lillith, Ko's local desktop companion. "
                   "Be concise and accurate.")
LOOPBACK_HOSTS = {"127.0.0.1", "localhost", "::1", "[::1]"}


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


def config_path() -> Path:
    env = os.environ.get("LILLITH_CONFIG")
    if env:
        return Path(env).expanduser()
    base = os.environ.get("XDG_CONFIG_HOME") or str(Path.home() / ".config")
    return Path(base) / "lillith" / "config.toml"


def data_dir() -> Path:
    """Same as the Rust data_dir(): $XDG_DATA_HOME/lillith else ~/.local/share/lillith.
    $LILLITH_HOME overrides (tests / portable installs)."""
    env = os.environ.get("LILLITH_HOME")
    if env:
        return Path(env).expanduser()
    base = os.environ.get("XDG_DATA_HOME") or str(Path.home() / ".local/share")
    return Path(base) / "lillith"


def load_toml(path: Path | None = None) -> dict:
    p = Path(path) if path else config_path()
    if not p.is_file():
        return {}
    with open(p, "rb") as f:
        return tomllib.load(f)


@dataclass
class LLMSettings:
    endpoint: str = DEFAULT_ENDPOINT
    model: str = DEFAULT_MODEL
    persona: str = DEFAULT_PERSONA
    api_key: str = ""
    source: str = "default"


def llm_settings(cfg: dict | None = None) -> LLMSettings:
    cfg = load_toml() if cfg is None else cfg
    s = LLMSettings()
    chat = cfg.get("chat") if isinstance(cfg.get("chat"), dict) else {}
    llm = cfg.get("llm") if isinstance(cfg.get("llm"), dict) else {}
    if chat:
        s.endpoint = str(chat.get("endpoint") or s.endpoint)
        s.model = str(chat.get("model") or s.model)
        s.persona = str(chat.get("system_prompt") or s.persona)
        s.api_key = str(chat.get("api_key") or "")
        s.source = "[chat]"
    if llm:
        s.endpoint = str(llm.get("endpoint") or s.endpoint)
        s.model = str(llm.get("model") or s.model)
        s.persona = str(llm.get("persona") or s.persona)
        s.api_key = str(llm.get("api_key") or s.api_key)
        s.source = "[llm]"
    assert_loopback(s.endpoint)
    return s


@dataclass
class SidecarSettings:
    port: int = DEFAULT_PORT
    workers: int = 1
    stall_minutes: float = 5.0
    attachments: dict = None


def sidecar_settings(cfg: dict | None = None) -> SidecarSettings:
    cfg = load_toml() if cfg is None else cfg
    sc = cfg.get("sidecar") if isinstance(cfg.get("sidecar"), dict) else {}
    port = int(sc.get("port", DEFAULT_PORT))
    if not 1 <= port <= 65535:
        raise ValueError(f"[sidecar] port out of range: {port}")
    workers = max(1, min(int(sc.get("workers", 1)), MAX_WORKERS))
    stall = float(sc.get("stall_minutes", 5.0))
    if stall <= 0:
        raise ValueError("[sidecar] stall_minutes must be > 0")
    att = sc.get("attachments") if isinstance(sc.get("attachments"), dict) else {}
    return SidecarSettings(port=port, workers=workers, stall_minutes=stall, attachments=dict(att))
