"""Tiny cron expression parser (5 fields + @macros), stdlib only.

Fields: minute hour day-of-month month day-of-week. Supports *, a,b, a-b, */n, a-b/n,
month/day names (jan, mon) and @hourly @daily @midnight @weekly @monthly @yearly.
Day-of-month and day-of-week follow Vixie-cron OR semantics when both are restricted.
"""
from __future__ import annotations

from datetime import datetime, timedelta

MACROS = {
    "@yearly": "0 0 1 1 *", "@annually": "0 0 1 1 *", "@monthly": "0 0 1 * *",
    "@weekly": "0 0 * * 0", "@daily": "0 0 * * *", "@midnight": "0 0 * * *",
    "@hourly": "0 * * * *",
}
MONTHS = {m: i for i, m in enumerate(
    "jan feb mar apr may jun jul aug sep oct nov dec".split(), 1)}
DAYS = {d: i for i, d in enumerate("sun mon tue wed thu fri sat".split())}
RANGES = [(0, 59), (0, 23), (1, 31), (1, 12), (0, 7)]


class CronError(ValueError):
    pass


def _val(tok: str, idx: int) -> int:
    t = tok.lower()
    if idx == 3 and t in MONTHS:
        return MONTHS[t]
    if idx == 4 and t in DAYS:
        return DAYS[t]
    try:
        return int(t)
    except ValueError:
        raise CronError(f"bad value {tok!r}")


def _field(spec: str, idx: int) -> set[int]:
    lo, hi = RANGES[idx]
    out: set[int] = set()
    for part in spec.split(","):
        step = 1
        if "/" in part:
            part, s = part.split("/", 1)
            step = int(s)
            if step < 1:
                raise CronError("step must be >= 1")
        if part == "*":
            a, b = lo, hi
        elif "-" in part:
            x, y = part.split("-", 1)
            a, b = _val(x, idx), _val(y, idx)
        else:
            a = _val(part, idx)
            b = hi if step > 1 else a
        if not (lo <= a <= hi and lo <= b <= hi) or a > b:
            raise CronError(f"value out of range in {spec!r}")
        out.update(range(a, b + 1, step))
    if idx == 4 and 7 in out:
        out.discard(7)
        out.add(0)
    return out


class Cron:
    def __init__(self, expr: str):
        self.expr = expr.strip()
        e = MACROS.get(self.expr.lower(), self.expr)
        parts = e.split()
        if len(parts) != 5:
            raise CronError(f"expected 5 fields, got {len(parts)}: {expr!r}")
        self.minute, self.hour, self.dom, self.month, self.dow = (
            _field(p, i) for i, p in enumerate(parts))
        self.dom_star = parts[2] == "*" or parts[2].startswith("*/")
        self.dow_star = parts[4] == "*" or parts[4].startswith("*/")

    def _day_ok(self, d: datetime) -> bool:
        dom_ok = d.day in self.dom
        dow_ok = (d.isoweekday() % 7) in self.dow
        if self.dom_star and self.dow_star:
            return True
        if self.dom_star:
            return dow_ok
        if self.dow_star:
            return dom_ok
        return dom_ok or dow_ok

    def matches(self, d: datetime) -> bool:
        return (d.minute in self.minute and d.hour in self.hour and d.month in self.month
                and self._day_ok(d))

    def next_after(self, after: datetime) -> datetime:
        """First matching minute strictly after ``after`` (naive local time)."""
        t = after.replace(second=0, microsecond=0) + timedelta(minutes=1)
        limit = t + timedelta(days=366 * 5)
        while t < limit:
            if t.month not in self.month or not self._day_ok(t):
                t = (t + timedelta(days=1)).replace(hour=0, minute=0)
                continue
            if t.hour not in self.hour:
                t = (t + timedelta(hours=1)).replace(minute=0)
                continue
            if t.minute not in self.minute:
                t += timedelta(minutes=1)
                continue
            return t
        raise CronError(f"{self.expr!r} never fires")


def validate(expr: str) -> str:
    Cron(expr)
    return expr
