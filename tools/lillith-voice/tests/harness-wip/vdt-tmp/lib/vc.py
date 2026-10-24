import socket, json, os
def conn(path=None):
    s = socket.socket(socket.AF_UNIX); s.connect(path or os.environ["LILLITH_VOICE_SOCKET"]); return s, s.makefile("rb")
def req(o, timeout=60, path=None):
    s, f = conn(path); s.settimeout(timeout); s.sendall((json.dumps(o) + "\n").encode()); r = f.readline(); s.close(); return json.loads(r)
