package com.kokannadian.lillith.voice;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.Map;

/**
 * SentencePiece unigram encoder (Pocket TTS's tokenizer.model), a pure-Java
 * port of sentencepiece's {@code Model::EncodeOptimized} + byte fallback, so
 * the APK needs no native sentencepiece. It reads the model protobuf itself.
 *
 * <p>Supported: model_type UNIGRAM, an identity normalizer (no precompiled
 * charsmap), add_dummy_prefix, remove_extra_whitespaces, escape_whitespaces,
 * byte_fallback. Anything else throws, so the voice falls back instead of
 * speaking with wrong tokens. Ids match sentencepiece 0.2.2 (the desktop
 * kit's version): see android/app/test.
 */
public final class SpTokenizer {
    private static final int NORMAL = 1, UNKNOWN = 2, CONTROL = 3, USER_DEFINED = 4, UNUSED = 5, BYTE = 6;
    private static final float UNK_PENALTY = 10.0f;
    private static final String WS = "\u2581";

    private final Map<String, Integer> pieces = new HashMap<>();
    private final int[] byteIds = new int[256];
    private final int unkId;
    private final float unkScore;
    private final float[] scores;
    private final int maxPieceCps;
    private final boolean addDummyPrefix, removeExtraWs, escapeWs, byteFallback;

    public static SpTokenizer load(File f) throws IOException {
        try (InputStream in = new FileInputStream(f)) {
            ByteArrayOutputStream bo = new ByteArrayOutputStream((int) Math.max(1024, f.length()));
            byte[] buf = new byte[1 << 16];
            int n;
            while ((n = in.read(buf)) > 0) bo.write(buf, 0, n);
            return new SpTokenizer(bo.toByteArray());
        }
    }

    public SpTokenizer(byte[] model) throws IOException {
        Pb top = new Pb(model, 0, model.length);
        java.util.List<String> ps = new java.util.ArrayList<>();
        java.util.List<Float> sc = new java.util.ArrayList<>();
        java.util.List<Integer> ty = new java.util.ArrayList<>();
        int modelType = 1; // proto default UNIGRAM
        boolean bf = false;
        boolean dummy = true, extra = true, esc = true;
        String normName = null;
        int charsmapLen = 0;
        boolean suffix = false;
        while (top.more()) {
            int key = top.varint32();
            int field = key >>> 3, wt = key & 7;
            if (field == 1 && wt == 2) { // repeated SentencePiece pieces
                Pb p = top.sub();
                String piece = "";
                float score = 0f;
                int type = NORMAL;
                while (p.more()) {
                    int k = p.varint32();
                    if ((k >>> 3) == 1 && (k & 7) == 2) piece = p.string();
                    else if ((k >>> 3) == 2 && (k & 7) == 5) score = p.fixed32f();
                    else if ((k >>> 3) == 3 && (k & 7) == 0) type = (int) p.varint();
                    else p.skip(k & 7);
                }
                ps.add(piece);
                sc.add(score);
                ty.add(type);
            } else if (field == 2 && wt == 2) { // TrainerSpec
                Pb t = top.sub();
                while (t.more()) {
                    int k = t.varint32();
                    if ((k >>> 3) == 3 && (k & 7) == 0) modelType = (int) t.varint();
                    else if ((k >>> 3) == 35 && (k & 7) == 0) bf = t.varint() != 0;
                    else if ((k >>> 3) == 24 && (k & 7) == 0) suffix = t.varint() != 0;
                    else t.skip(k & 7);
                }
            } else if (field == 3 && wt == 2) { // NormalizerSpec
                Pb nspec = top.sub();
                while (nspec.more()) {
                    int k = nspec.varint32();
                    int f = k >>> 3;
                    if (f == 1 && (k & 7) == 2) normName = nspec.string();
                    else if (f == 2 && (k & 7) == 2) charsmapLen = nspec.sub().len();
                    else if (f == 3 && (k & 7) == 0) dummy = nspec.varint() != 0;
                    else if (f == 4 && (k & 7) == 0) extra = nspec.varint() != 0;
                    else if (f == 5 && (k & 7) == 0) esc = nspec.varint() != 0;
                    else nspec.skip(k & 7);
                }
            } else {
                top.skip(wt);
            }
        }
        if (modelType != 1) throw new IOException("tokenizer: model_type " + modelType + " is not UNIGRAM");
        if (charsmapLen > 0) {
            throw new IOException("tokenizer: normalizer '" + normName + "' needs a precompiled charsmap (unsupported)");
        }
        if (suffix) throw new IOException("tokenizer: treat_whitespace_as_suffix is unsupported");
        if (ps.isEmpty()) throw new IOException("tokenizer: no pieces");
        addDummyPrefix = dummy;
        removeExtraWs = extra;
        escapeWs = esc;
        byteFallback = bf;
        scores = new float[ps.size()];
        float min = Float.MAX_VALUE;
        int unk = -1, maxLen = 1;
        java.util.Arrays.fill(byteIds, -1);
        for (int i = 0; i < ps.size(); i++) {
            String piece = ps.get(i);
            int type = ty.get(i);
            scores[i] = sc.get(i);
            if (type == NORMAL) min = Math.min(min, scores[i]);
            if (type == UNKNOWN) unk = i;
            if (type == BYTE && piece.length() == 6 && piece.startsWith("<0x") && piece.endsWith(">")) {
                byteIds[Integer.parseInt(piece.substring(3, 5), 16)] = i;
            }
            if (type == USER_DEFINED) {
                throw new IOException("tokenizer: user-defined pieces are unsupported");
            }
            if (!piece.isEmpty() && (type == NORMAL || type == UNUSED)) {
                pieces.put(piece, i);
                maxLen = Math.max(maxLen, piece.codePointCount(0, piece.length()));
            }
        }
        if (unk < 0) throw new IOException("tokenizer: no <unk> piece");
        if (byteFallback) {
            for (int b = 0; b < 256; b++) {
                if (byteIds[b] < 0) throw new IOException("tokenizer: byte_fallback without <0x" + b + ">");
            }
        }
        unkId = unk;
        unkScore = min - UNK_PENALTY;
        maxPieceCps = maxLen;
        // UNUSED pieces are in the trie but rejected in sentencepiece; this model has none.
        for (int i = 0; i < ty.size(); i++) {
            if (ty.get(i) == UNUSED) throw new IOException("tokenizer: unused pieces are unsupported");
        }
    }

    public int vocabSize() {
        return scores.length;
    }

    /** Identity normalizer: optional whitespace squeeze, dummy prefix, ' ' -> U+2581. */
    String normalize(String text) {
        if (text.isEmpty()) return "";
        String t = text;
        if (removeExtraWs) {
            StringBuilder b = new StringBuilder();
            boolean space = true; // drop leading
            for (int i = 0; i < t.length(); i++) {
                char c = t.charAt(i);
                if (c == ' ') {
                    if (!space) b.append(' ');
                    space = true;
                } else {
                    b.append(c);
                    space = false;
                }
            }
            int e = b.length();
            while (e > 0 && b.charAt(e - 1) == ' ') e--;
            t = b.substring(0, e);
            if (t.isEmpty()) return "";
        }
        if (addDummyPrefix) t = " " + t;
        return escapeWs ? t.replace(" ", WS) : t;
    }

    /** {@code SentencePieceProcessor.Encode(text)} (ids, no BOS/EOS). */
    public int[] encode(String text) {
        String norm = normalize(text);
        if (norm.isEmpty()) return new int[0];
        int[] cps = norm.codePoints().toArray();
        int n = cps.length;
        int[] charStart = new int[n + 1];
        for (int i = 0, c = 0; i < n; i++) {
            charStart[i] = c;
            c += Character.charCount(cps[i]);
        }
        charStart[n] = norm.length();
        int[] bestId = new int[n + 1];
        float[] bestScore = new float[n + 1];
        int[] startsAt = new int[n + 1];
        java.util.Arrays.fill(startsAt, -1);
        bestScore[0] = 0f;
        for (int s = 0; s < n; s++) {
            if (s > 0 && startsAt[s] == -1) continue; // unreachable (never for unigram + unk)
            float base = bestScore[s];
            boolean single = false;
            int lim = Math.min(n, s + maxPieceCps);
            for (int e = s + 1; e <= lim; e++) {
                Integer id = pieces.get(norm.substring(charStart[s], charStart[e]));
                if (id == null) continue;
                float cand = base + scores[id];
                if (startsAt[e] == -1 || cand > bestScore[e]) {
                    bestScore[e] = cand;
                    startsAt[e] = s;
                    bestId[e] = id;
                }
                if (e == s + 1) single = true;
            }
            if (!single) {
                int e = s + 1;
                float cand = base + unkScore;
                if (startsAt[e] == -1 || cand > bestScore[e]) {
                    bestScore[e] = cand;
                    startsAt[e] = s;
                    bestId[e] = unkId;
                }
            }
        }
        // backtrack
        java.util.ArrayList<int[]> rev = new java.util.ArrayList<>();
        for (int e = n; e > 0; e = startsAt[e]) rev.add(new int[] {startsAt[e], e, bestId[e]});
        java.util.ArrayList<Integer> out = new java.util.ArrayList<>();
        for (int i = rev.size() - 1; i >= 0; i--) {
            int[] r = rev.get(i);
            if (r[2] == unkId && byteFallback) {
                byte[] bytes = norm.substring(charStart[r[0]], charStart[r[1]]).getBytes(StandardCharsets.UTF_8);
                for (byte b : bytes) out.add(byteIds[b & 0xFF]);
            } else {
                out.add(r[2]);
            }
        }
        int[] ids = new int[out.size()];
        for (int i = 0; i < ids.length; i++) ids[i] = out.get(i);
        return ids;
    }

    // ------------------------------------------------------------ minimal protobuf reader
    private static final class Pb {
        private final byte[] b;
        private int p;
        private final int end;

        Pb(byte[] b, int p, int end) {
            this.b = b;
            this.p = p;
            this.end = end;
        }

        boolean more() {
            return p < end;
        }

        int len() {
            return end - p;
        }

        long varint() throws IOException {
            long r = 0;
            for (int shift = 0; shift < 64; shift += 7) {
                if (p >= end) throw new IOException("tokenizer: truncated varint");
                int x = b[p++] & 0xFF;
                r |= (long) (x & 0x7F) << shift;
                if ((x & 0x80) == 0) return r;
            }
            throw new IOException("tokenizer: bad varint");
        }

        int varint32() throws IOException {
            return (int) varint();
        }

        float fixed32f() throws IOException {
            if (p + 4 > end) throw new IOException("tokenizer: truncated fixed32");
            int v = (b[p] & 0xFF) | (b[p + 1] & 0xFF) << 8 | (b[p + 2] & 0xFF) << 16 | (b[p + 3] & 0xFF) << 24;
            p += 4;
            return Float.intBitsToFloat(v);
        }

        Pb sub() throws IOException {
            int n = (int) varint();
            if (n < 0 || p + n > end) throw new IOException("tokenizer: truncated message");
            Pb s = new Pb(b, p, p + n);
            p += n;
            return s;
        }

        String string() throws IOException {
            Pb s = sub();
            return new String(b, s.p, s.end - s.p, StandardCharsets.UTF_8);
        }

        void skip(int wt) throws IOException {
            switch (wt) {
                case 0: varint(); break;
                case 1: p += 8; break;
                case 2: sub(); break;
                case 5: p += 4; break;
                default: throw new IOException("tokenizer: wire type " + wt);
            }
            if (p > end) throw new IOException("tokenizer: truncated field");
        }
    }
}
