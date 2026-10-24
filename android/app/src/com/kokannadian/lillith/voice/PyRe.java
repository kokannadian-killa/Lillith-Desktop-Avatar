package com.kokannadian.lillith.voice;

import java.util.ArrayList;
import java.util.List;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * Python-3 {@code re} semantics on top of java.util.regex, so the desktop
 * kit's patterns (lillith_voice/textnorm.py, core.py, stt.py) can be copied
 * verbatim and behave the same on the JVM and on Android (ICU regex).
 *
 * <p>Python's str patterns are Unicode-aware: {@code \w} is letters, numbers
 * and '_', {@code \d} is any decimal digit, {@code \s} is Unicode whitespace
 * and {@code \b} sits between \w and \W. Java (and Android's ICU) differ in
 * places, so {@link #compile} rewrites those escapes into explicit classes.
 * {@code .} matches anything but '\n' (UNIX_LINES), like Python.
 *
 * <p>Pure Java, no Android imports: unit-tested on the JVM.
 */
public final class PyRe {
    /** Python's \w (str.isalnum() or '_'), as the inside of a character class. */
    static final String W = "\\p{L}\\p{N}_";
    /** Python's \s for str (Py_UNICODE_ISSPACE), as the inside of a character class. */
    static final String S = "\\t\\n\\u000B\\f\\r\\u001C-\\u001F \\u0085\\u00A0\\u1680\\u2000-\\u200A"
            + "\\u2028\\u2029\\u202F\\u205F\\u3000";
    static final String D = "\\p{Nd}";
    private static final String B = "(?:(?<=[" + W + "])(?![" + W + "])|(?<![" + W + "])(?=[" + W + "]))";

    public static final int I = Pattern.CASE_INSENSITIVE | Pattern.UNICODE_CASE;
    public static final int DOTALL = Pattern.DOTALL;

    private PyRe() {}

    /** Compile a Python {@code re} pattern (subset used by the kit) with Python semantics. */
    public static Pattern compile(String py) {
        return compile(py, 0);
    }

    public static Pattern compile(String py, int flags) {
        return Pattern.compile(translate(py), flags | Pattern.UNIX_LINES);
    }

    static String translate(String py) {
        StringBuilder o = new StringBuilder(py.length() + 32);
        boolean inClass = false;
        for (int i = 0; i < py.length(); i++) {
            char c = py.charAt(i);
            if (c == '\\' && i + 1 < py.length()) {
                char e = py.charAt(++i);
                switch (e) {
                    case 'w': o.append(inClass ? W : "[" + W + "]"); break;
                    case 'W':
                        if (inClass) throw new IllegalArgumentException("\\W inside [] unsupported: " + py);
                        o.append("[^" + W + "]");
                        break;
                    case 'd': o.append(inClass ? D : "[" + D + "]"); break;
                    case 's': o.append(inClass ? S : "[" + S + "]"); break;
                    case 'S':
                        if (inClass) throw new IllegalArgumentException("\\S inside [] unsupported: " + py);
                        o.append("[^" + S + "]");
                        break;
                    case 'b':
                        if (inClass) throw new IllegalArgumentException("\\b inside [] unsupported: " + py);
                        o.append(B);
                        break;
                    case 'D': case 'B': case 'A': case 'Z':
                        throw new IllegalArgumentException("\\" + e + " unsupported: " + py);
                    default:
                        o.append('\\').append(e);
                }
                continue;
            }
            if (!inClass && c == '[') {
                inClass = true;
                o.append(c);
                // A ']' right after '[' or '[^' is a literal in Python.
                if (i + 1 < py.length() && py.charAt(i + 1) == '^') o.append(py.charAt(++i));
                if (i + 1 < py.length() && py.charAt(i + 1) == ']') {
                    o.append("\\]");
                    i++;
                }
                continue;
            }
            if (inClass && c == '[') {
                o.append("\\["); // Python: a plain '[' inside a class is literal
                continue;
            }
            if (inClass && c == ']') inClass = false;
            if (inClass && (c == '&' || c == '~') ) {
                o.append('\\').append(c); // Java class operators ("&&") are literals in Python
                continue;
            }
            o.append(c);
        }
        return o.toString();
    }

    /** Replacement callback for {@link #sub}. */
    public interface Repl {
        String apply(Matcher m);
    }

    /** {@code re.sub(pattern, fn, s)}. */
    public static String sub(Pattern p, Repl fn, String s) {
        Matcher m = p.matcher(s);
        if (!m.find()) return s;
        StringBuilder out = new StringBuilder(s.length() + 16);
        int last = 0;
        do {
            out.append(s, last, m.start()).append(fn.apply(m));
            last = m.end();
        } while (m.find());
        return out.append(s, last, s.length()).toString();
    }

    /** {@code re.sub(pattern, literal, s)}; {@code \1}-style refs are not interpreted here. */
    public static String sub(Pattern p, String literal, String s) {
        return sub(p, m -> literal, s);
    }

    /** {@code re.split(pattern, s)} for patterns without groups (empty pieces kept, as in Python). */
    public static List<String> split(Pattern p, String s) {
        List<String> out = new ArrayList<>();
        Matcher m = p.matcher(s);
        int last = 0;
        while (m.find()) {
            out.add(s.substring(last, m.start()));
            last = m.end();
        }
        out.add(s.substring(last));
        return out;
    }

    // ------------------------------------------------------------ Python str helpers

    /** {@code ch.isspace()} for a code point. */
    public static boolean isSpace(int c) {
        return (c >= 0x09 && c <= 0x0D) || (c >= 0x1C && c <= 0x20) || c == 0x85 || c == 0xA0 || c == 0x1680
                || (c >= 0x2000 && c <= 0x200A) || c == 0x2028 || c == 0x2029 || c == 0x202F || c == 0x205F
                || c == 0x3000;
    }

    /** {@code ch.isalnum()} for a code point (letters or any numeric category). */
    public static boolean isAlnum(int c) {
        if (Character.isLetter(c)) return true;
        int t = Character.getType(c);
        return t == Character.DECIMAL_DIGIT_NUMBER || t == Character.LETTER_NUMBER || t == Character.OTHER_NUMBER;
    }

    /** {@code s.strip()} (Unicode whitespace). */
    public static String strip(String s) {
        int a = 0, b = s.length();
        while (a < b) {
            int c = s.codePointAt(a);
            if (!isSpace(c)) break;
            a += Character.charCount(c);
        }
        while (b > a) {
            int c = s.codePointBefore(b);
            if (!isSpace(c)) break;
            b -= Character.charCount(c);
        }
        return s.substring(a, b);
    }

    /** {@code s.split()} with no separator (runs of Unicode whitespace, no empty pieces). */
    public static List<String> splitWs(String s) {
        List<String> out = new ArrayList<>();
        int i = 0, n = s.length(), start = -1;
        while (i < n) {
            int c = s.codePointAt(i);
            if (isSpace(c)) {
                if (start >= 0) {
                    out.add(s.substring(start, i));
                    start = -1;
                }
            } else if (start < 0) {
                start = i;
            }
            i += Character.charCount(c);
        }
        if (start >= 0) out.add(s.substring(start));
        return out;
    }

    /** {@code len(s)} in code points. */
    public static int len(String s) {
        return s.codePointCount(0, s.length());
    }

    /** {@code s[a:b]} in code points (0 <= a <= b <= len). */
    public static String slice(String s, int a, int b) {
        int n = len(s);
        a = Math.max(0, Math.min(a, n));
        b = Math.max(a, Math.min(b, n));
        int ca = s.offsetByCodePoints(0, a);
        return s.substring(ca, s.offsetByCodePoints(ca, b - a));
    }

    /** {@code s.rfind(sub, 0, end)} in code points. */
    public static int rfind(String s, String sub, int endCp) {
        int n = len(s);
        int endChar = s.offsetByCodePoints(0, Math.max(0, Math.min(endCp, n)));
        int from = endChar - sub.length();
        if (from < 0) return -1;
        int idx = s.lastIndexOf(sub, from);
        return idx < 0 ? -1 : s.codePointCount(0, idx);
    }
}
