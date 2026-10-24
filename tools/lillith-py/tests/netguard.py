"""Test-suite network guard: any attempt to open a non-loopback socket (or
resolve a non-local hostname) is refused AND recorded; conftest fails the whole
run if anything was recorded, even if the code under test swallowed the error."""
from __future__ import annotations

import ipaddress
import socket

VIOLATIONS: list = []
_ORIG = {}
LOCAL_NAMES = {"localhost", "localhost.localdomain", "ip6-localhost", ""}


class NetworkGuardError(OSError):
    pass


def _is_local(host) -> bool:
    if host is None:
        return True
    if isinstance(host, bytes):
        host = host.decode(errors="replace")
    host = str(host).strip("[]").lower()
    if host in LOCAL_NAMES:
        return True
    try:
        ip = ipaddress.ip_address(host.split("%")[0])
    except ValueError:
        return False
    return ip.is_loopback or (ip.version == 6 and ip.ipv4_mapped is not None
                              and ip.ipv4_mapped.is_loopback) or ip == ipaddress.ip_address("0.0.0.0")


def _check(what: str, address):
    if isinstance(address, (str, bytes)):  # AF_UNIX path
        return
    host = address[0] if isinstance(address, tuple) and address else address
    if not _is_local(host):
        VIOLATIONS.append(f"{what} -> {address!r}")
        raise NetworkGuardError(f"network guard: non-loopback {what} to {address!r} refused")


def install():
    if _ORIG:
        return
    _ORIG.update(connect=socket.socket.connect, connect_ex=socket.socket.connect_ex,
                 create_connection=socket.create_connection, getaddrinfo=socket.getaddrinfo,
                 sendto=socket.socket.sendto)

    def connect(self, address):
        if self.family != getattr(socket, "AF_UNIX", None):
            _check("connect", address)
        return _ORIG["connect"](self, address)

    def connect_ex(self, address):
        if self.family != getattr(socket, "AF_UNIX", None):
            _check("connect_ex", address)
        return _ORIG["connect_ex"](self, address)

    def sendto(self, data, *args):
        if args and self.family != getattr(socket, "AF_UNIX", None):
            _check("sendto", args[-1])
        return _ORIG["sendto"](self, data, *args)

    def create_connection(address, *a, **kw):
        _check("create_connection", address)
        return _ORIG["create_connection"](address, *a, **kw)

    def getaddrinfo(host, *a, **kw):
        _check("getaddrinfo", (host,))
        return _ORIG["getaddrinfo"](host, *a, **kw)

    socket.socket.connect = connect
    socket.socket.connect_ex = connect_ex
    socket.socket.sendto = sendto
    socket.create_connection = create_connection
    socket.getaddrinfo = getaddrinfo


def uninstall():
    if not _ORIG:
        return
    socket.socket.connect = _ORIG["connect"]
    socket.socket.connect_ex = _ORIG["connect_ex"]
    socket.socket.sendto = _ORIG["sendto"]
    socket.create_connection = _ORIG["create_connection"]
    socket.getaddrinfo = _ORIG["getaddrinfo"]
    _ORIG.clear()
