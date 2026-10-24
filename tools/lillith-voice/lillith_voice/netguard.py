"""Hard offline guard for every lillith-voice process.

1. Sets the usual "offline" env switches (Hugging Face, transformers, pip, ORT)
   so no library even *tries* to fetch anything.
2. Monkey-patches the socket layer: AF_UNIX is allowed (PipeWire, our daemon
   socket); AF_INET/AF_INET6 may only reach loopback (llama-server on
   127.0.0.1:8080). Anything else raises NetworkBlocked. DNS for non-local names
   is refused too, and AF_INET/AF_INET6 sockets may only bind() to loopback
   (never 0.0.0.0 / ::), so no library can open a listener on the LAN.

Child processes (ffmpeg, pw-play) inherit the env; ffmpeg is additionally run
with `-protocol_whitelist pipe,file` and only ever gets pipes.
"""
import ipaddress
import os
import socket

OFFLINE_ENV = {
    "HF_HUB_OFFLINE": "1",
    "TRANSFORMERS_OFFLINE": "1",
    "HF_DATASETS_OFFLINE": "1",
    "HF_HUB_DISABLE_TELEMETRY": "1",
    "DO_NOT_TRACK": "1",
    "PIP_NO_INDEX": "1",
    "PIP_DISABLE_PIP_VERSION_CHECK": "1",
    "ORT_DISABLE_TELEMETRY": "1",
}
for _k, _v in OFFLINE_ENV.items():
    os.environ[_k] = _v


class NetworkBlocked(OSError):
    pass


_LOCAL_NAMES = {"localhost", "localhost.localdomain", "ip6-localhost", ""}


def _is_loopback(host) -> bool:
    if host is None:
        return False
    if isinstance(host, bytes):
        host = host.decode(errors="replace")
    host = str(host).strip("[]").split("%")[0]
    if host in _LOCAL_NAMES:
        return True
    try:
        return ipaddress.ip_address(host).is_loopback
    except ValueError:
        return False


_orig_connect = socket.socket.connect
_orig_connect_ex = socket.socket.connect_ex
_orig_sendto = socket.socket.sendto
_orig_getaddrinfo = socket.getaddrinfo
_orig_bind = socket.socket.bind


def _check(sock, address):
    fam = getattr(sock, "family", None)
    if fam == getattr(socket, "AF_UNIX", object()):
        return
    if isinstance(address, tuple) and address and _is_loopback(address[0]):
        return
    raise NetworkBlocked(f"lillith-voice is offline-only; refused connection to {address!r}")


def _connect(self, address):
    _check(self, address)
    return _orig_connect(self, address)


def _connect_ex(self, address):
    _check(self, address)
    return _orig_connect_ex(self, address)


def _sendto(self, data, *args):
    address = args[-1]
    _check(self, address)
    return _orig_sendto(self, data, *args)


def _bind(self, address):
    fam = getattr(self, "family", None)
    if fam in (socket.AF_INET, getattr(socket, "AF_INET6", None)):
        if not (isinstance(address, tuple) and address and _is_loopback(address[0]) and address[0] != ""):
            raise NetworkBlocked(f"lillith-voice is local-only; refused bind to {address!r} (loopback only)")
    return _orig_bind(self, address)


def _getaddrinfo(host, *args, **kw):
    if not _is_loopback(host):
        raise NetworkBlocked(f"lillith-voice is offline-only; refused DNS lookup of {host!r}")
    return _orig_getaddrinfo(host, *args, **kw)


socket.socket.connect = _connect
socket.socket.connect_ex = _connect_ex
socket.socket.sendto = _sendto
socket.socket.bind = _bind
socket.getaddrinfo = _getaddrinfo
ACTIVE = True
