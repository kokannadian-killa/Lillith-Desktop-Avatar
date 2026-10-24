"""Text normalisation so the voice reads LLM output like a person would.

numbers -> words, money/percent/time/ordinals, common abbreviations, hums ("mmm", "hmm"),
giggles (hehe/teehee -> "Hehe."), tildes and stretched punctuation, emoji/markdown (via core.clean_text).
"""
import re

_ONES = "zero one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen " \
        "sixteen seventeen eighteen nineteen".split()
_TENS = "_ _ twenty thirty forty fifty sixty seventy eighty ninety".split()
_ORD = {"one": "first", "two": "second", "three": "third", "five": "fifth", "eight": "eighth",
        "nine": "ninth", "twelve": "twelfth"}


def num2words(n: int) -> str:
    if n < 0:
        return "minus " + num2words(-n)
    if n < 20:
        return _ONES[n]
    if n < 100:
        return _TENS[n // 10] + ("" if n % 10 == 0 else "-" + _ONES[n % 10])
    if n < 1000:
        return _ONES[n // 100] + " hundred" + ("" if n % 100 == 0 else " " + num2words(n % 100))
    for div, word in ((10 ** 12, "trillion"), (10 ** 9, "billion"), (10 ** 6, "million"), (1000, "thousand")):
        if n >= div:
            return num2words(n // div) + " " + word + ("" if n % div == 0 else " " + num2words(n % div))
    return str(n)


def ordinal(n: int) -> str:
    w = num2words(n)
    head, _, last = w.rpartition(" ")
    pre, _, tail = last.rpartition("-")
    if tail in _ORD:
        tail = _ORD[tail]
    elif tail.endswith("y"):
        tail = tail[:-1] + "ieth"
    else:
        tail += "th"
    last = (pre + "-" if pre else "") + tail
    return (head + " " if head else "") + last


def _year(n: int) -> str:
    if 2000 <= n <= 2009:
        return num2words(n)
    hi, lo = divmod(n, 100)
    if lo == 0:
        return num2words(hi) + " hundred"
    return num2words(hi) + " " + ("oh " + num2words(lo) if lo < 10 else num2words(lo))


def _number(m):
    s = m.group(0).replace(",", "")
    if "." in s:
        a, b = s.split(".", 1)
        return num2words(int(a or 0)) + " point " + " ".join(_ONES[int(d)] for d in b)
    n = int(s)
    if 1100 <= n <= 2099 and len(s) == 4 and "," not in m.group(0):
        return _year(n)
    return num2words(n)


def _money(m):
    cur, whole, cents = m.group(1), m.group(2).replace(",", ""), m.group(3)
    unit = {"$": "dollar", "£": "pound", "€": "euro"}[cur]
    w = int(whole or 0)
    out = num2words(w) + " " + unit + ("" if w == 1 else "s")
    if cents and int(cents):
        c = int(cents.ljust(2, "0")[:2])
        out += " and " + num2words(c) + " cent" + ("" if c == 1 else "s")
    return out


def _time(m):
    h, mi, ap = int(m.group(1)), int(m.group(2)), (m.group(3) or "").lower().replace(".", "")
    words = num2words(h) + (" o'clock" if mi == 0 and not ap else "" if mi == 0 else
                            " oh " + num2words(mi) if mi < 10 else " " + num2words(mi))
    return words + (" " + " ".join(ap.upper()) if ap else "")


_ABBR = [
    (r"\bDr\.", "Doctor"), (r"\bMr\.", "Mister"), (r"\bMrs\.", "Missus"), (r"\bMs\.", "Miz"),
    (r"\bSt\.", "Saint"), (r"\bvs\.?", "versus"), (r"\be\.g\.", "for example"), (r"\bi\.e\.", "that is"),
    (r"\betc\.", "et cetera"), (r"\bapprox\.", "approximately"), (r"\bmins?\b", "minutes"),
    (r"\bhrs?\b", "hours"), (r"\bbtw\b", "by the way"), (r"\bidk\b", "I don't know"), (r"\bimo\b", "in my opinion"),
    (r"\bomg\b", "oh my god"), (r"\bOMG\b", "oh my god"), (r"\btbh\b", "to be honest"), (r"\bpls\b", "please"),
    (r"\bplz\b", "please"), (r"\bu\b", "you"), (r"\bur\b", "your"), (r"\br\b", "are"), (r"\bthx\b", "thanks"),
    (r"\bw/\s?", "with "), (r"&", " and "), (r"\bAI\b", "A.I."), (r"\bkm\b", "kilometers"), (r"\bkg\b", "kilos"),
]


def normalize(text: str) -> str:
    t = text
    t = t.replace("~", "")                                        # "hiii~" -> "hiii"
    t = re.sub(r"\b(?:te+he+|he(?:h+e+)+|hihi+|kek+)\b[.!]*", "Hehe.", t, flags=re.I)
    t = re.sub(r"\b(?:ha){3,}h?\b", "Hahaha.", t, flags=re.I)
    t = re.sub(r"\bm{3,}\b", "Mmm", t, flags=re.I)                # mmmmm -> Mmm (hum)
    t = re.sub(r"\bhm{2,}\b", "Hmm", t, flags=re.I)
    t = re.sub(r"\b(u+h+m+|u+m+)\b", "um", t, flags=re.I)
    t = re.sub(r"([!?])[!?1]+", r"\1", t)                         # "?!?!!" -> "?"
    t = re.sub(r"\.{4,}", "...", t)
    for pat, rep in _ABBR:
        t = re.sub(pat, rep, t)
    t = re.sub(r"([$£€])\s?(\d[\d,]*)(?:\.(\d{1,2}))?", _money, t)
    t = re.sub(r"\b(\d{1,2}):(\d{2})\s*([ap]\.?m\.?)?", _time, t, flags=re.I)
    t = re.sub(r"\b(\d+)\s*%", lambda m: num2words(int(m.group(1))) + " percent", t)
    t = re.sub(r"\b(\d+)(st|nd|rd|th)\b", lambda m: ordinal(int(m.group(1))), t)
    t = re.sub(r"\b(\d+)\s*(am|pm)\b", lambda m: num2words(int(m.group(1))) + " " + " ".join(m.group(2).upper()), t,
               flags=re.I)
    t = re.sub(r"\b(\d+)'s\b", lambda m: num2words(int(m.group(1))) + "s", t)
    t = re.sub(r"(?<![\w.])\d[\d,]*(?:\.\d+)?(?![\w])", _number, t)
    t = re.sub(r"\s+", " ", t).strip()
    return t
