package com.kokannadian.lillith.voice;

import java.math.BigInteger;
import java.util.Locale;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * Port of tools/lillith-voice/lillith_voice/textnorm.py: makes LLM output
 * read like a person would say it (numbers, money, times, ordinals,
 * abbreviations, hums, giggles, tildes, stretched punctuation).
 *
 * <p>Output must equal the desktop kit's {@code normalize()} byte for byte,
 * quirks included (see android/VOICE-RUNTIME.md "known desktop quirks");
 * android/app/test checks this against fixtures generated from the Python.
 */
public final class TextNorm {
    private TextNorm() {}

    private static final String[] ONES = ("zero one two three four five six seven eight nine ten eleven "
            + "twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen").split(" ");
    private static final String[] TENS = "_ _ twenty thirty forty fifty sixty seventy eighty ninety".split(" ");
    private static final BigInteger[] DIVS = {BigInteger.TEN.pow(12), BigInteger.TEN.pow(9),
            BigInteger.TEN.pow(6), BigInteger.valueOf(1000)};
    private static final String[] DIV_WORDS = {"trillion", "billion", "million", "thousand"};

    public static String num2words(long n) {
        return num2words(BigInteger.valueOf(n));
    }

    public static String num2words(BigInteger n) {
        if (n.signum() < 0) return "minus " + num2words(n.negate());
        if (n.compareTo(BigInteger.valueOf(1000)) < 0) {
            int v = n.intValue();
            if (v < 20) return ONES[v];
            if (v < 100) return TENS[v / 10] + (v % 10 == 0 ? "" : "-" + ONES[v % 10]);
            return ONES[v / 100] + " hundred" + (v % 100 == 0 ? "" : " " + num2words(v % 100));
        }
        for (int i = 0; i < DIVS.length; i++) {
            if (n.compareTo(DIVS[i]) >= 0) {
                BigInteger[] qr = n.divideAndRemainder(DIVS[i]);
                return num2words(qr[0]) + " " + DIV_WORDS[i] + (qr[1].signum() == 0 ? "" : " " + num2words(qr[1]));
            }
        }
        return n.toString();
    }

    private static String ordTail(String t) {
        switch (t) {
            case "one": return "first";
            case "two": return "second";
            case "three": return "third";
            case "five": return "fifth";
            case "eight": return "eighth";
            case "nine": return "ninth";
            case "twelve": return "twelfth";
            default: return null;
        }
    }

    public static String ordinal(BigInteger n) {
        String w = num2words(n);
        int sp = w.lastIndexOf(' ');
        String head = sp < 0 ? "" : w.substring(0, sp);
        String last = sp < 0 ? w : w.substring(sp + 1);
        int dash = last.lastIndexOf('-');
        String pre = dash < 0 ? "" : last.substring(0, dash);
        String tail = dash < 0 ? last : last.substring(dash + 1);
        String o = ordTail(tail);
        if (o != null) tail = o;
        else if (tail.endsWith("y")) tail = tail.substring(0, tail.length() - 1) + "ieth";
        else tail += "th";
        last = (pre.isEmpty() ? "" : pre + "-") + tail;
        return (head.isEmpty() ? "" : head + " ") + last;
    }

    private static String year(int n) {
        if (n >= 2000 && n <= 2009) return num2words(n);
        int hi = n / 100, lo = n % 100;
        if (lo == 0) return num2words(hi) + " hundred";
        return num2words(hi) + " " + (lo < 10 ? "oh " + num2words(lo) : num2words(lo));
    }

    private static BigInteger bigint(String digits) {
        return new BigInteger(digits); // accepts any Unicode decimal digits, like Python int()
    }

    private static String number(Matcher m) {
        String g = m.group(0);
        String s = g.replace(",", "");
        int dot = s.indexOf('.');
        if (dot >= 0) {
            String a = s.substring(0, dot), b = s.substring(dot + 1);
            StringBuilder o = new StringBuilder(num2words(a.isEmpty() ? BigInteger.ZERO : bigint(a)))
                    .append(" point ");
            for (int i = 0, k = 0; i < b.length(); k++) {
                int cp = b.codePointAt(i);
                if (k > 0) o.append(' ');
                o.append(ONES[Character.digit(cp, 10)]);
                i += Character.charCount(cp);
            }
            return o.toString();
        }
        BigInteger n = bigint(s);
        if (n.compareTo(BigInteger.valueOf(1100)) >= 0 && n.compareTo(BigInteger.valueOf(2099)) <= 0
                && PyRe.len(s) == 4 && !g.contains(",")) {
            return year(n.intValue());
        }
        return num2words(n);
    }

    private static String money(Matcher m) {
        String cur = m.group(1), whole = m.group(2).replace(",", ""), cents = m.group(3);
        String unit = "$".equals(cur) ? "dollar" : "£".equals(cur) ? "pound" : "euro";
        BigInteger w = whole.isEmpty() ? BigInteger.ZERO : bigint(whole);
        String out = num2words(w) + " " + unit + (w.equals(BigInteger.ONE) ? "" : "s");
        if (cents != null && !cents.isEmpty() && bigint(cents).signum() != 0) {
            String c2 = cents;
            while (PyRe.len(c2) < 2) c2 += "0";
            int c = bigint(PyRe.slice(c2, 0, 2)).intValue();
            out += " and " + num2words(c) + " cent" + (c == 1 ? "" : "s");
        }
        return out;
    }

    private static String spacedUpper(String s) {
        String u = s.toUpperCase(Locale.ROOT);
        StringBuilder o = new StringBuilder();
        for (int i = 0; i < u.length(); ) {
            int cp = u.codePointAt(i);
            if (o.length() > 0) o.append(' ');
            o.appendCodePoint(cp);
            i += Character.charCount(cp);
        }
        return o.toString();
    }

    private static String time(Matcher m) {
        int h = bigint(m.group(1)).intValue(), mi = bigint(m.group(2)).intValue();
        String ap = m.group(3) == null ? "" : m.group(3).toLowerCase(Locale.ROOT).replace(".", "");
        String words = num2words(h) + (mi == 0 && ap.isEmpty() ? " o'clock" : mi == 0 ? ""
                : mi < 10 ? " oh " + num2words(mi) : " " + num2words(mi));
        return words + (ap.isEmpty() ? "" : " " + spacedUpper(ap));
    }

    private static final Pattern GIGGLE = PyRe.compile("\\b(?:te+he+|he(?:h+e+)+|hihi+|kek+)\\b[.!]*", PyRe.I);
    private static final Pattern HAHA = PyRe.compile("\\b(?:ha){3,}h?\\b", PyRe.I);
    private static final Pattern MMM = PyRe.compile("\\bm{3,}\\b", PyRe.I);
    private static final Pattern HMM = PyRe.compile("\\bhm{2,}\\b", PyRe.I);
    private static final Pattern UM = PyRe.compile("\\b(u+h+m+|u+m+)\\b", PyRe.I);
    private static final Pattern BANG = PyRe.compile("([!?])[!?1]+");
    private static final Pattern DOTS = PyRe.compile("\\.{4,}");
    private static final String[][] ABBR_SRC = {
        {"\\bDr\\.", "Doctor"}, {"\\bMr\\.", "Mister"}, {"\\bMrs\\.", "Missus"}, {"\\bMs\\.", "Miz"},
        {"\\bSt\\.", "Saint"}, {"\\bvs\\.?", "versus"}, {"\\be\\.g\\.", "for example"}, {"\\bi\\.e\\.", "that is"},
        {"\\betc\\.", "et cetera"}, {"\\bapprox\\.", "approximately"}, {"\\bmins?\\b", "minutes"},
        {"\\bhrs?\\b", "hours"}, {"\\bbtw\\b", "by the way"}, {"\\bidk\\b", "I don't know"},
        {"\\bimo\\b", "in my opinion"}, {"\\bomg\\b", "oh my god"}, {"\\bOMG\\b", "oh my god"},
        {"\\btbh\\b", "to be honest"}, {"\\bpls\\b", "please"}, {"\\bplz\\b", "please"}, {"\\bu\\b", "you"},
        {"\\bur\\b", "your"}, {"\\br\\b", "are"}, {"\\bthx\\b", "thanks"}, {"\\bw/\\s?", "with "},
        {"&", " and "}, {"\\bAI\\b", "A.I."}, {"\\bkm\\b", "kilometers"}, {"\\bkg\\b", "kilos"},
    };
    private static final Pattern[] ABBR = new Pattern[ABBR_SRC.length];
    static {
        for (int i = 0; i < ABBR_SRC.length; i++) ABBR[i] = PyRe.compile(ABBR_SRC[i][0]);
    }
    // an integer, with "," only as a thousands separator (1,234,567), never a trailing comma (textnorm._INT)
    private static final String INT = "(?:\\d{1,3}(?:,\\d{3})+(?!\\d)|\\d+)";
    private static final Pattern MONEY = PyRe.compile("([$£€])\\s?(" + INT + ")(?:\\.(\\d{1,2}))?");
    // am/pm only as its own word: "at 5:30 today" keeps the space before "today"
    private static final Pattern TIME = PyRe.compile("\\b(\\d{1,2}):(\\d{2})(?:\\s*([ap]\\.?m\\b\\.?))?", PyRe.I);
    private static final Pattern PCT = PyRe.compile("\\b(\\d+)\\s*%");
    private static final Pattern ORD = PyRe.compile("\\b(\\d+)(st|nd|rd|th)\\b");
    private static final Pattern AMPM = PyRe.compile("\\b(\\d+)\\s*(am|pm)\\b", PyRe.I);
    private static final Pattern DECADE = PyRe.compile("\\b(\\d+)'s\\b");
    // a comma is a thousands separator only before exactly 3 digits: "5, maybe 6" keeps its comma
    private static final Pattern NUM = PyRe.compile("(?<![\\w.])" + INT + "(?:\\.\\d+)?(?![\\w])");
    private static final Pattern WS = PyRe.compile("\\s+");

    public static String normalize(String text) {
        String t = text.replace("~", "");
        t = PyRe.sub(GIGGLE, "Hehe.", t);
        t = PyRe.sub(HAHA, "Hahaha.", t);
        t = PyRe.sub(MMM, "Mmm", t);
        t = PyRe.sub(HMM, "Hmm", t);
        t = PyRe.sub(UM, "um", t);
        t = PyRe.sub(BANG, m -> m.group(1), t);
        t = PyRe.sub(DOTS, "...", t);
        for (int i = 0; i < ABBR.length; i++) t = PyRe.sub(ABBR[i], ABBR_SRC[i][1], t);
        t = PyRe.sub(MONEY, TextNorm::money, t);
        t = PyRe.sub(TIME, TextNorm::time, t);
        t = PyRe.sub(PCT, m -> num2words(bigint(m.group(1))) + " percent", t);
        t = PyRe.sub(ORD, m -> ordinal(bigint(m.group(1))), t);
        t = PyRe.sub(AMPM, m -> num2words(bigint(m.group(1))) + " " + spacedUpper(m.group(2)), t);
        t = PyRe.sub(DECADE, m -> num2words(bigint(m.group(1))) + "s", t);
        t = PyRe.sub(NUM, TextNorm::number, t);
        t = PyRe.strip(PyRe.sub(WS, " ", t));
        return t;
    }
}
