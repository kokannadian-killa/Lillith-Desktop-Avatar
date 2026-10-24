"""Mock llama-server (OpenAI-compatible) for tests. Loopback only.

Run standalone:  python3 tests/mock_llama.py --port 8080
"""
from __future__ import annotations

import json
import threading
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
                if outer.script:
                    nxt = outer.script.pop(0)
                    msg = nxt if isinstance(nxt, dict) else {"role": "assistant", "content": nxt}
                else:
                    last = body["messages"][-1]["content"]
                    if isinstance(last, list):
                        last = " ".join(p.get("text", "") for p in last if p.get("type") == "text")
                    msg = {"role": "assistant", "content": f"MOCK REPLY ({len(last)} chars)"}
                self._json(200, {"id": "x", "object": "chat.completion", "model": body.get("model"),
                                 "choices": [{"index": 0, "message": msg, "finish_reason": "stop"}],
                                 "usage": {"prompt_tokens": 1, "completion_tokens": 1}})

        self.server = ThreadingHTTPServer(("127.0.0.1", port), H)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *a):
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
