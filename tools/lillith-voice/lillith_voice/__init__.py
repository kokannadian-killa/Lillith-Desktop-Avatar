"""Lillith's fully-offline voice (Kokoro ONNX + local DSP).

Importing this package installs the network guard first (see netguard.py).
"""
from . import netguard as _netguard  # noqa: F401  (side effect: offline env + socket guard)

__version__ = "1.0.0"
