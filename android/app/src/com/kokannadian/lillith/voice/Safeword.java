package com.kokannadian.lillith.voice;

import java.util.Locale;
import java.util.regex.Pattern;

/**
 * Typed-chat safeword ("red" / "OOC: stop"): stops her voice at once and
 * drops every queued line. Union of the two desktop matchers:
 * <ul>
 *   <li>lillith-core {@code kill_switch::is_safeword} (desktop chat box): the
 *       whole message is "red", or it starts with "OOC: stop" / "OOC stop";</li>
 *   <li>lillith_voice/stt.py {@code _SAFE_RE} (the voice kit), verbatim.</li>
 * </ul>
 * stt.py's {@code fuzzy_red} ("Read.", "Rad.", "Right." as a lone short
 * utterance) is NOT used: it exists because whisper mis-hears a spoken "red",
 * and typed text has no such errors. A typed safeword is never consent.
 */
public final class Safeword {
    private Safeword() {}

    /** stt.py _SAFE_RE, copied verbatim (Python semantics via {@link PyRe}). */
    private static final Pattern SAFE_RE = PyRe.compile(
            "^\\W*(red\\W*){1,4}$|\\b(o\\.?\\s?o\\.?\\s?c\\.?|ooc|(oh?\\W*){1,2}(see|c|sea)|out of character)\\W*\\s*stop\\b"
            + "|^\\W*(oh?\\W+){1,2}(i'?ll\\W+)?(see|c|sea)\\W+(stop|start)\\W*$|^\\W*(o\\W*){1,2}c\\W+(stop|start)\\W*$",
            PyRe.I);

    /** stt.py: {@code bool(_SAFE_RE.search(text.strip()))}. */
    public static boolean voiceKitRegex(String text) {
        return SAFE_RE.matcher(PyRe.strip(text == null ? "" : text)).find();
    }

    /** lillith-core kill_switch::is_safeword. */
    public static boolean desktopChat(String text) {
        if (text == null) return false;
        String t = text.trim();
        // Rust str::trim is Unicode White_Space; PyRe.strip covers the same set for typed text.
        t = PyRe.strip(t);
        int a = 0, b = t.length();
        while (a < b) {
            int c = t.codePointAt(a);
            if (rustAlnum(c) || c == ':') break;
            a += Character.charCount(c);
        }
        while (b > a) {
            int c = t.codePointBefore(b);
            if (rustAlnum(c) || c == ':') break;
            b -= Character.charCount(c);
        }
        t = t.substring(a, b).toLowerCase(Locale.ROOT);
        if (t.equals("red")) return true;
        StringBuilder sq = new StringBuilder();
        for (int i = 0; i < t.length(); ) {
            int c = t.codePointAt(i);
            if (!PyRe.isSpace(c)) sq.appendCodePoint(c);
            i += Character.charCount(c);
        }
        String s = sq.toString();
        return s.startsWith("ooc:stop") || s.startsWith("oocstop");
    }

    private static boolean rustAlnum(int c) {
        return Character.isAlphabetic(c) || PyRe.isAlnum(c);
    }

    /** True when a typed chat message is the safeword. */
    public static boolean isTyped(String text) {
        return desktopChat(text) || voiceKitRegex(text);
    }
}
