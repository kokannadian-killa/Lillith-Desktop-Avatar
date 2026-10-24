"""Lillith core: local attachments + local bot deployment for the Lillith desktop avatar."""
__version__ = "0.1.0"

# Every external program goes through lillith_py.launch (one audit.log line per
# launch). The guard refuses and audits any subprocess started without it.
from . import launch as _launch  # noqa: E402

_launch.install_guard()
