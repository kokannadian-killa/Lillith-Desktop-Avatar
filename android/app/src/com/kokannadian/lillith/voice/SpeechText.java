package com.kokannadian.lillith.voice;

import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.regex.Pattern;

/**
 * Text handling around the voice, ported 1:1 from the desktop kit:
 * <ul>
 *   <li>{@link #cleanText} = core.clean_text (actions="drop")</li>
 *   <li>{@link #splitSentences} = core.split_sentences</li>
 *   <li>{@link #prepare} / {@link #pocketSplit} = pocket.PocketVoice.prepare / split</li>
 *   <li>{@link #seedFor} = the per-line seed in core.Voice.render</li>
 * </ul>
 * Pure Java (JVM-tested against fixtures generated from the Python).
 */
public final class SpeechText {
    private SpeechText() {}

    private static final Pattern CODE = PyRe.compile("```.*?```", PyRe.DOTALL);
    private static final Pattern URL = PyRe.compile("https?://\\S+|www\\.\\S+");
    private static final Pattern ACTION = PyRe.compile("\\*[^*\\n]{1,80}\\*");
    private static final Pattern THINK = PyRe.compile("<think>.*?</think>", PyRe.DOTALL);
    private static final Pattern MARKUP = PyRe.compile("[*_#`~>|\\[\\]{}]");
    private static final Pattern WS = PyRe.compile("\\s+");
    private static final Pattern SENT = PyRe.compile("(?<=[.!?…])[\"')\\]]*\\s+|\\n+");
    private static final Pattern SENT_END = PyRe.compile("(?<=[.!?…])\\s+");
    private static final Pattern CLAUSE_END = PyRe.compile("(?<=[,;:])\\s+");

    /** Python unicodedata categories So, Sk, Cs, Co, Cn (+ ZWJ, VS16) become spaces. */
    private static boolean emojiLike(int cp) {
        if (cp == 0x200D || cp == 0xFE0F) return true;
        int t = Character.getType(cp);
        return t == Character.OTHER_SYMBOL || t == Character.MODIFIER_SYMBOL || t == Character.SURROGATE
                || t == Character.PRIVATE_USE || t == Character.UNASSIGNED;
    }

    /** Make LLM output speakable: drop code blocks, URLs, emoji, markdown and *actions*. */
    public static String cleanText(String text) {
        String t = PyRe.sub(CODE, " ", text);
        t = PyRe.sub(URL, " link ", t);
        t = PyRe.sub(ACTION, " ", t);
        t = PyRe.sub(THINK, " ", t);
        StringBuilder b = new StringBuilder(t.length());
        for (int i = 0; i < t.length(); ) {
            int cp = t.codePointAt(i);
            if (emojiLike(cp)) b.append(' ');
            else b.appendCodePoint(cp);
            i += Character.charCount(cp);
        }
        t = PyRe.sub(MARKUP, " ", b.toString());
        return PyRe.strip(PyRe.sub(WS, " ", t));
    }

    public static List<String> splitSentences(String text) {
        return splitSentences(text, 120, 260);
    }

    /** Sentence chunks; the first chunk is kept short so audio starts sooner. */
    public static List<String> splitSentences(String text, int firstMax, int maxLen) {
        List<String> parts = new ArrayList<>();
        for (String p : PyRe.split(SENT, text)) {
            String s = PyRe.strip(p);
            if (!s.isEmpty()) parts.add(s);
        }
        List<String> chunks = new ArrayList<>();
        String cur = "";
        for (String p : parts) {
            int limit = chunks.isEmpty() ? firstMax : maxLen;
            if (cur.isEmpty()) {
                cur = p;
            } else if (PyRe.len(cur) + 1 + PyRe.len(p) <= limit) {
                cur += " " + p;
            } else {
                chunks.add(cur);
                cur = p;
            }
            if (chunks.isEmpty() && PyRe.len(cur) >= 25) {
                chunks.add(cur);
                cur = "";
            }
        }
        if (!cur.isEmpty()) chunks.add(cur);
        List<String> out = new ArrayList<>();
        for (String c : chunks) {
            while (PyRe.len(c) > maxLen * 2) {
                int cut = Math.max(PyRe.rfind(c, ", ", maxLen), PyRe.rfind(c, " ", maxLen));
                cut = cut > 0 ? cut : maxLen;
                out.add(PyRe.strip(PyRe.slice(c, 0, cut + 1)));
                c = PyRe.strip(PyRe.slice(c, cut + 1, PyRe.len(c)));
            }
            if (!c.isEmpty()) out.add(c);
        }
        return out;
    }

    /** pocket.prepare: returns {text, framesAfterEos}; text "" when there is nothing to say. */
    public static Object[] prepare(String text, boolean removeSemicolons) {
        String t = String.join(" ", PyRe.splitWs(text.replace("\r", " ")));
        if (t.isEmpty()) return new Object[] {"", 1};
        if (removeSemicolons) t = t.replace(";", ",");
        int after = PyRe.splitWs(t).size() <= 4 ? 3 : 1;
        int first = t.codePointAt(0);
        t = new String(Character.toChars(first)).toUpperCase(Locale.ROOT) + t.substring(Character.charCount(first));
        if (PyRe.isAlnum(t.codePointBefore(t.length()))) t += ".";
        return new Object[] {t, after};
    }

    /** Token counter used by {@link #pocketSplit} (the SentencePiece encoder). */
    public interface TokenCount {
        int count(String s);
    }

    /** pocket.split: chunks of <= maxTok tokens at sentence (then comma) boundaries. */
    public static List<String> pocketSplit(String text, boolean removeSemicolons, int maxTok, TokenCount tok) {
        String t = (String) prepare(text, removeSemicolons)[0];
        List<String> out = new ArrayList<>();
        if (t.isEmpty()) return out;
        for (String p : PyRe.split(SENT_END, t)) {
            if (p.isEmpty()) continue;
            if (tok.count(p) <= maxTok) {
                out.add(p);
                continue;
            }
            String cur = "";
            for (String q : PyRe.split(CLAUSE_END, p)) {
                String cand = PyRe.strip(cur + " " + q);
                if (!cur.isEmpty() && tok.count(cand) > maxTok) {
                    out.add(cur);
                    cur = q;
                } else {
                    cur = cand;
                }
            }
            if (!cur.isEmpty()) out.add(cur);
        }
        List<String> merged = new ArrayList<>();
        for (String p : out) {
            int n = merged.size();
            if (n > 0 && tok.count(merged.get(n - 1) + " " + p) <= maxTok && PyRe.len(merged.get(n - 1)) < 40) {
                merged.set(n - 1, merged.get(n - 1) + " " + p);
            } else {
                merged.add(p);
            }
        }
        return merged;
    }

    /** core.Voice.render: stable per-line seed, (seed + sum(ord(c)) * 31) % 2**31. */
    public static long seedFor(long seed, String normalized) {
        long sum = 0;
        for (int i = 0; i < normalized.length(); ) {
            int cp = normalized.codePointAt(i);
            sum += cp;
            i += Character.charCount(cp);
        }
        return Math.floorMod(seed + sum * 31L, 1L << 31);
    }

    /** speaker.pause_after: natural pause after a sentence by its final punctuation (seconds). */
    public static double pauseAfter(String chunk) {
        String c = chunk;
        int e = c.length();
        while (e > 0 && " \"')]".indexOf(c.charAt(e - 1)) >= 0) e--;
        c = c.substring(0, e);
        if (c.endsWith("...") || c.endsWith("…")) return 0.42;
        if (c.endsWith("?")) return 0.30;
        if (c.endsWith("!")) return 0.26;
        if (c.endsWith(".")) return 0.32;
        if (c.endsWith(",") || c.endsWith(";") || c.endsWith(":") || c.endsWith("-") || c.endsWith("—")) return 0.14;
        return 0.22;
    }
}
