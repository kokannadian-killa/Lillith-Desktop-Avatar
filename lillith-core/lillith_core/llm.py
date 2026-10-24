"""Minimal loopback-only client for llama-server's OpenAI-compatible API.

Proxies from the environment are ignored on purpose, and any non-loopback URL
is refused, so a request can never leave the machine.
"""
from __future__ import annotations

import json
import urllib.request

from .attachments.config import assert_loopback

_OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))


class LLMError(RuntimeError):
    pass


def chat_completion(base_url: str, messages: list, *, model: str = "local",
                    timeout: float = 300.0, **params) -> dict:
    assert_loopback(base_url)
    url = base_url.rstrip("/") + "/v1/chat/completions"
    body = {"model": model, "messages": messages, "stream": False}
    body.update({k: v for k, v in params.items() if v is not None})
    req = urllib.request.Request(url, data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"}, method="POST")
    try:
        with _OPENER.open(req, timeout=timeout) as r:
            return json.loads(r.read().decode("utf-8"))
    except OSError as e:
        raise LLMError(f"llama-server at {base_url} unreachable: {e}") from e


def reply_text(resp: dict) -> str:
    try:
        return resp["choices"][0]["message"]["content"] or ""
    except (KeyError, IndexError, TypeError):
        raise LLMError(f"unexpected response: {str(resp)[:300]}")


def health(base_url: str, timeout: float = 3.0) -> bool:
    assert_loopback(base_url)
    try:
        with _OPENER.open(base_url.rstrip("/") + "/health", timeout=timeout) as r:
            return r.status == 200
    except OSError:
        return False
