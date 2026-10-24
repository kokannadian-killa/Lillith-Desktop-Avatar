"""Mock llama-server (OpenAI-compatible) for tests. Loopback only.

Run standalone:  python3 tests/mock_llama.py --port 8080
"""
from __future__ import annotations

import json
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class MockLlama:
    def __init__(self, port: int = 0):
        self.requests: list = []
        self.script: list = []   # queued replies: str or full message dict
        outer = self

        class H(BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def _json(self, code, obj):
                d = json.dumps(obj).encode()
                self.send_response(code)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(d)))
                self.end_headers()
                self.wfile.write(d)

            def _stream(self, body, msg, delay, hang):
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Connection", "close")
                self.end_headers()
                text = msg.get("content") or ""
                chunks = [w + " " for w in text.split(" ")] if text else []
                if chunks:
                    chunks[-1] = chunks[-1][:-1]
                try:
                    def send(delta):
                        d = {"choices": [{"index": 0, "delta": delta}]}
                        self.wfile.write(f"data: {json.dumps(d)}\n\n".encode())
                        self.wfile.flush()
                    send({"role": "assistant"})
                    for i, c in enumerate(chunks):
                        send({"content": c})
                        if i == 0 and hang:
                            outer._sleep(hang)
                        elif delay:
                            outer._sleep(delay)
                    for i, tc in enumerate(msg.get("tool_calls") or []):
                        send({"tool_calls": [{"index": i, **tc}]})
                    self.wfile.write(b"data: [DONE]\n\n")
                    self.wfile.flush()
                except (BrokenPipeError, ConnectionResetError):
                    pass
                self.close_connection = True

            def do_GET(self):
                if self.path == "/health":
                    return self._json(200, {"status": "ok"})
                if self.path == "/v1/models":
                    return self._json(200, {"data": [{"id": "mock"}]})
                self._json(404, {"error": "nf"})

            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                outer.requests.append(body)
                if self.path != "/v1/chat/completions":
                    return self._json(404, {"error": "nf"})
                delay, hang = 0.0, 0.0
                if outer.script:
                    nxt = outer.script.pop(0)
                    msg = dict(nxt) if isinstance(nxt, dict) else {"role": "assistant", "content": nxt}
                    delay = float(msg.pop("_chunk_delay", 0.0))  # seconds between streamed chunks
                    hang = float(msg.pop("_hang", 0.0))          # silence after first chunk
                    msg.setdefault("role", "assistant")
                else:
                    last = body["messages"][-1]["content"]
                    if isinstance(last, list):
                        last = " ".join(p.get("text", "") for p in last if p.get("type") == "text")
                    msg = {"role": "assistant", "content": f"MOCK REPLY ({len(last)} chars)"}
                if body.get("stream"):
                    return self._stream(body, msg, delay, hang)
                self._json(200, {"id": "x", "object": "chat.completion", "model": body.get("model"),
                                 "choices": [{"index": 0, "message": msg, "finish_reason": "stop"}],
                                 "usage": {"prompt_tokens": 1, "completion_tokens": 1}})

        self._closing = threading.Event()
        self.server = ThreadingHTTPServer(("127.0.0.1", port), H)
        self.server.daemon_threads = True
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def __enter__(self):
        self.thread.start()
        return self

    def _sleep(self, secs):
        self._closing.wait(secs)

    def __exit__(self, *a):
        self._closing.set()
        self.server.shutdown()
        self.server.server_close()


if __name__ == "__main__":
    import argparse
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8080)
    a = ap.parse_args()
    m = MockLlama(a.port)
    print(f"mock llama-server on {m.url}")
    m.server.serve_forever()
