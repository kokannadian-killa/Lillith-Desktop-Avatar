"""Minimal stand-in for llama-server's OpenAI-compatible streaming endpoint (127.0.0.1 only)."""
import http.server
import json
import sys
import time

REPLY = ("Oh hiii, you're back! Like, I was literally counting the seconds. "
         "Okay, so what are we doing tonight, babe? Hehe.")


class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", 0)))
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        for w in REPLY.split(" "):
            d = {"choices": [{"delta": {"content": w + " "}}]}
            self.wfile.write(f"data: {json.dumps(d)}\n\n".encode())
            self.wfile.flush()
            time.sleep(0.03)
        self.wfile.write(b"data: [DONE]\n\n")

    def log_message(self, *a):
        pass


http.server.HTTPServer(("127.0.0.1", int(sys.argv[1]) if len(sys.argv) > 1 else 8080), H).serve_forever()
