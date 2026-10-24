"""Safeword check, mirroring crates/lillith-memory/src/safeword.rs::is_safeword:
the whole message is "red" (case/punctuation-insensitive), or it carries
"OOC: stop" / "OOC stop" anywhere. A safeword stops the chat reply (no model
call, nothing stored); it does not cancel running or queued bot tasks."""


def is_safeword(text: str) -> bool:
    squashed = "".join(c for c in (text or "").lower() if c.isalnum())
    return squashed == "red" or "oocstop" in squashed
