"""Privileged-action hook (intentionally refuses everything).

lillith-py bots may only read their own attachments/memory and write their own
memory. Anything that would write outside the bot's data dir, run a command or
launch a program must instead be proposed to Ko through Lillith's Soul
Contract (`lillith contract-card`, crates/lillith-core/src/soul_contract.rs)
and be recorded in Lillith's shared audit.log by the Rust side.

That bridge does not exist yet, so this stub is the single place a future
privileged tool would call - and today it always refuses. Do not add tools
that bypass it.
"""
from __future__ import annotations


class ContractRequired(PermissionError):
    pass


PRIVILEGED_KINDS = {"write_file", "run_command", "launch_program", "network"}


def request_privileged_action(kind: str, detail: str, bot: str | None = None):
    """Always raises ContractRequired. Future: hand the action to
    `lillith contract-card` and wait for Ko's Yes/No; the Rust side audits it."""
    raise ContractRequired(
        f"{kind!r} needs a Soul Contract (`lillith contract-card`); lillith-py cannot "
        f"perform privileged actions itself (bot={bot!r}, detail={detail[:120]!r})")
