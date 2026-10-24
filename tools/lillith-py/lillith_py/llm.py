"""Loopback-only client for llama-server's OpenAI-compatible API.

Proxies from the environment are ignored, and any non-loopback URL is
refused, so a request can never leave the machine. Supports streaming (SSE)
with a per-token callback so callers can show progress, detect stalls and
cancel mid-generation.
"""
from __future__ import annotations

import json
import threading
import socket
import urllib.request

from .lillith_config import assert_loopback

_OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))


class LLMError(RuntimeError):
    pass


class Cancelled(Exception):
    """Raised when the caller's callback asks to stop generation."""


def _request(base_url, body, api_key, timeout):
    assert_loopback(base_url)
    url = base_url.rstrip("/") + "/v1/chat/completions"
    headers = {"Content-Type": "application/json"}
    if api_key:
        headers["Authorization"] = f"Bearer {api_key}"
    req = urllib.request.Request(url, data=json.dumps(body).encode(), headers=headers, method="POST")
    try:
        return _OPENER.open(req, timeout=timeout)
    except OSError as e:
        raise LLMError(f"llama-server at {base_url} unreachable: {e}") from e


def chat_completion(base_url: str, messages: list, *, model: str = "local", api_key: str = "",
                    timeout: float = 300.0, **params) -> dict:
    body = {"model": model, "messages": messages, "stream": False}
    body.update({k: v for k, v in params.items() if v is not None})
    try:
        with _request(base_url, body, api_key, timeout) as r:
            return json.loads(r.read().decode("utf-8"))
    except (OSError, ValueError) as e:
        raise LLMError(f"llama-server error: {e}") from e


def stream_chat(base_url: str, messages: list, *, model: str = "local", api_key: str = "",
                timeout: float = 300.0, on_token=None, register=None, **params) -> dict:
    """Streaming completion. ``on_token(text)`` is called for every delta; it may
    raise Cancelled. ``timeout`` is the per-read idle timeout (stall guard).
    ``register(close_fn)`` receives a function that aborts the request from
    another thread. Returns an assistant message dict."""
    body = {"model": model, "messages": messages, "stream": True}
    body.update({k: v for k, v in params.items() if v is not None})
    resp = _request(base_url, body, api_key, timeout)
    aborted = threading.Event()

    def _abort():
        aborted.set()
        try:
            sock = resp.fp.raw._sock  # type: ignore[attr-defined]
            sock.shutdown(socket.SHUT_RDWR)
        except Exception:
            pass
        try:
            resp.close()
        except Exception:
            pass

    if register:
        register(_abort)
    parts, tool_calls = [], {}
    try:
        ctype = resp.headers.get("Content-Type", "")
        if "text/event-stream" not in ctype:  # server ignored stream=true
            data = json.loads(resp.read().decode("utf-8"))
            msg = data["choices"][0]["message"]
            if on_token and msg.get("content"):
                on_token(msg["content"])
            return msg
        for raw in resp:
            line = raw.decode("utf-8", errors="replace").strip()
            if not line.startswith("data:"):
                continue
            payload = line[5:].strip()
            if payload == "[DONE]":
                break
            try:
                chunk = json.loads(payload)
                delta = chunk["choices"][0].get("delta") or {}
            except (ValueError, KeyError, IndexError):
                continue
            text = delta.get("content")
            if text:
                parts.append(text)
                if on_token:
                    on_token(text)
            for tc in delta.get("tool_calls") or []:
                slot = tool_calls.setdefault(tc.get("index", 0), {"function": {"name": "", "arguments": ""}})
                fn = tc.get("function") or {}
                slot["function"]["name"] += fn.get("name") or ""
                slot["function"]["arguments"] += fn.get("arguments") or ""
            if on_token and not text:
                on_token("")  # heartbeat: progress without visible text
    except Cancelled:
        raise
    except Exception as e:
        if aborted.is_set():  # closed from another thread (stop / stop-all / stall)
            raise Cancelled() from e
        if isinstance(e, (OSError, ValueError)):
            raise LLMError(f"stream interrupted: {e}") from e
        raise
    finally:
        try:
            resp.close()
        except Exception:
            pass
    if aborted.is_set():
        raise Cancelled()
    msg = {"role": "assistant", "content": "".join(parts)}
    if tool_calls:
        msg["tool_calls"] = [tool_calls[k] for k in sorted(tool_calls)]
    return msg


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
