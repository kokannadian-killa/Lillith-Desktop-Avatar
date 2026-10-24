package com.kokannadian.lillith.voice;

import ai.onnxruntime.OnnxJavaType;
import ai.onnxruntime.OnnxTensor;
import ai.onnxruntime.OnnxValue;
import ai.onnxruntime.OrtEnvironment;
import ai.onnxruntime.OrtException;
import ai.onnxruntime.OrtLoggingLevel;
import ai.onnxruntime.OrtSession;
import ai.onnxruntime.TensorInfo;

import java.io.File;
import java.io.IOException;
import java.io.RandomAccessFile;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.FloatBuffer;
import java.nio.LongBuffer;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Random;

/**
 * Pocket TTS (Kyutai, CC-BY-4.0 weights) on ONNX Runtime: Lillith's cloned
 * voice, a Java port of tools/lillith-voice/lillith_voice/pocket.py
 * (PocketVoice: prepare/split/voice_state/stream).
 *
 * <p>Fully offline: every file comes from the local model dir (installed out
 * of the APK by AssetInstaller); nothing is fetched. ORT telemetry is turned
 * off and its Android telemetry classes are not packaged (build-apk.sh).
 *
 * <p>Pipeline per text chunk: SentencePiece ids -> text_conditioner ->
 * flow_lm_main (KV state per bundle.json manifest, primed once from
 * lillith.npy and cached) -> flow_lm_flow (LSD steps) -> mimi_decoder,
 * 24 kHz, streamed 3 frames first then 12-frame blocks.
 *
 * <p>Sampling differs from the desktop only in the RNG (java.util.Random vs
 * numpy PCG64), so a line is a different "take" than on the desktop; with
 * temperature 0 the output is deterministic and matches the Python.
 * Not thread-safe: one synthesis at a time.
 */
public final class PocketTts implements AutoCloseable {
    public static final int SR = 24000;

    /** Receives decoded audio blocks; return false to stop synthesis early. */
    public interface BlockSink {
        boolean accept(float[] pcm);
    }

    private static final class StateEntry {
        final String inputName;
        final int index;
        final long[] shape;
        final String dtype;
        final String fill;

        StateEntry(Object j) throws IOException {
            try {
                Map<String, Object> o = MiniJson.obj(j);
                inputName = MiniJson.str(o, "input_name");
                index = (int) MiniJson.num(o, "index");
                List<Object> s = MiniJson.arr(o.get("shape"));
                shape = new long[s.size()];
                for (int i = 0; i < shape.length; i++) shape[i] = (long) (double) (Double) s.get(i);
                dtype = MiniJson.str(o, "dtype");
                fill = MiniJson.optStr(o, "fill") == null ? "zeros" : MiniJson.str(o, "fill");
            } catch (Exception e) {
                throw new IOException("bundle.json: bad state entry: " + e.getMessage(), e);
            }
        }
    }

    private final OrtEnvironment env;
    private final OrtSession textCond, main, flow, dec;
    private final List<String> mainOut, decOut;
    private final SpTokenizer tok;
    private final List<StateEntry> flowManifest = new ArrayList<>(), mimiManifest = new ArrayList<>();
    private final int latentDim, condDim, maxTok;
    private final double frameRate;
    private final boolean removeSemicolons;
    private final float[] bos; // [1, nb, condDim] flattened or null
    private final int bosFrames;
    private final float[][] st; // LSD (s, t) pairs
    public final double temperature;
    public final long loadMillis;
    private final File voiceNpy;
    private Map<String, Cached> voiceState; // primed + cached once
    private final List<String> files = new ArrayList<>();

    private static final class Cached {
        final ByteBuffer data; // direct, native order, read-only use
        final long[] shape;
        final OnnxJavaType type;

        Cached(ByteBuffer d, long[] s, OnnxJavaType t) {
            data = d;
            shape = s;
            type = t;
        }
    }

    /**
     * @param bundleDir models/pocket (bundle.json, tokenizer.model, *.onnx, voices/)
     * @param voiceNpy  the cloned voice embedding (voices/lillith.npy)
     */
    public PocketTts(File bundleDir, File voiceNpy, int threads, double temperature) throws IOException, OrtException {
        long t0 = System.currentTimeMillis();
        try {
            Map<String, Object> meta = MiniJson.obj(MiniJson.parse(readText(new File(bundleDir, "bundle.json"))));
            latentDim = (int) MiniJson.num(meta, "latent_dim");
            condDim = (int) MiniJson.num(meta, "conditioning_dim");
            frameRate = MiniJson.num(meta, "frame_rate");
            maxTok = MiniJson.optInt(meta, "max_token_per_chunk", 50);
            removeSemicolons = MiniJson.optBool(meta, "remove_semicolons", false);
            if ((int) MiniJson.num(meta, "sample_rate") != SR) throw new IOException("bundle sample_rate != 24000");
            for (Object e : MiniJson.arr(meta.get("flow_lm_state_manifest"))) flowManifest.add(new StateEntry(e));
            for (Object e : MiniJson.arr(meta.get("mimi_state_manifest"))) mimiManifest.add(new StateEntry(e));
            tok = SpTokenizer.load(new File(bundleDir, MiniJson.str(meta, "tokenizer_file")));
            String bosFile = MiniJson.optStr(meta, "bos_before_voice_file");
            if (bosFile != null && !bosFile.isEmpty() && MiniJson.optBool(meta, "insert_bos_before_voice", false)) {
                Npy b = Npy.read(new File(bundleDir, bosFile));
                bos = b.data;
                bosFrames = b.data.length / condDim;
                if (bosFrames * condDim != b.data.length) throw new IOException("bos_before_voice: bad shape");
            } else {
                bos = null;
                bosFrames = 0;
            }
        } catch (IOException e) {
            throw e;
        } catch (Exception e) {
            throw new IOException("bundle.json: " + e.getMessage(), e);
        }
        this.temperature = temperature;
        this.voiceNpy = voiceNpy;
        env = OrtEnvironment.getEnvironment(OrtLoggingLevel.ORT_LOGGING_LEVEL_ERROR, "lillith-voice");
        try {
            env.setTelemetry(false); // no-op off Windows; belt and braces (no INTERNET permission anyway)
        } catch (Throwable ignored) {
            // older/newer ORT without the call
        }
        OrtSession.SessionOptions so = new OrtSession.SessionOptions();
        so.setIntraOpNumThreads(threads);
        so.setInterOpNumThreads(1);
        so.setSessionLogLevel(OrtLoggingLevel.ORT_LOGGING_LEVEL_ERROR);
        so.setCPUArenaAllocator(false); // ~1 GB less RSS on desktop; ANDROID_ASSETS.md section 5
        so.setOptimizationLevel(OrtSession.SessionOptions.OptLevel.ALL_OPT);
        textCond = open(so, bundleDir, "text_conditioner", false);
        main = open(so, bundleDir, "flow_lm_main", false);
        flow = open(so, bundleDir, "flow_lm_flow", false);
        dec = open(so, bundleDir, "mimi_decoder", false);
        mainOut = new ArrayList<>(main.getOutputNames());
        decOut = new ArrayList<>(dec.getOutputNames());
        checkManifest(main, mainOut, flowManifest, 2, "flow_lm_main");
        checkManifest(dec, decOut, mimiManifest, 1, "mimi_decoder");
        int lsd = 1;
        st = new float[lsd][];
        for (int j = 0; j < lsd; j++) st[j] = new float[] {(float) j / lsd, (float) (j + 1) / lsd};
        loadMillis = System.currentTimeMillis() - t0;
    }

    private OrtSession open(OrtSession.SessionOptions so, File dir, String stem, boolean int8) throws OrtException, IOException {
        File f = new File(dir, int8 ? stem + "_int8.onnx" : stem + ".onnx");
        if (!f.isFile()) f = new File(dir, int8 ? stem + ".onnx" : stem + "_int8.onnx"); // bundle ships one precision
        if (!f.isFile()) throw new IOException("missing model " + stem + ".onnx in " + dir);
        files.add(f.getName());
        return env.createSession(f.getAbsolutePath(), so);
    }

    private static void checkManifest(OrtSession s, List<String> outs, List<StateEntry> m, int off, String what)
            throws IOException, OrtException {
        java.util.Set<String> ins = s.getInputNames();
        for (StateEntry e : m) {
            if (!ins.contains(e.inputName)) throw new IOException(what + ": no input " + e.inputName);
            if (off + e.index >= outs.size()) throw new IOException(what + ": no output for " + e.inputName);
        }
    }

    public List<String> files() {
        return files;
    }

    public SpTokenizer tokenizer() {
        return tok;
    }

    public int maxTokens() {
        return maxTok;
    }

    public boolean removeSemicolons() {
        return removeSemicolons;
    }

    // ------------------------------------------------------------ text (pocket.prepare / split)

    public List<String> split(String text) {
        return SpeechText.pocketSplit(text, removeSemicolons, maxTok, s -> tok.encode(s).length);
    }

    // ------------------------------------------------------------ tensors

    private static int elems(long[] shape) {
        long n = 1;
        for (long d : shape) n *= d;
        if (n > Integer.MAX_VALUE) throw new IllegalArgumentException("tensor too large");
        return (int) n;
    }

    private OnnxTensor floats(float[] data, long... shape) throws OrtException {
        FloatBuffer fb = ByteBuffer.allocateDirect(Math.max(0, data.length) * 4).order(ByteOrder.nativeOrder()).asFloatBuffer();
        fb.put(data).rewind();
        return OnnxTensor.createTensor(env, fb, shape);
    }

    private OnnxTensor fillTensor(StateEntry e) throws OrtException {
        int n = elems(e.shape);
        switch (e.dtype) {
            case "float32": {
                float[] a = new float[n];
                if ("nan".equals(e.fill)) java.util.Arrays.fill(a, Float.NaN);
                else if ("ones".equals(e.fill)) java.util.Arrays.fill(a, 1f);
                return floats(a, e.shape);
            }
            case "int64": {
                LongBuffer lb = ByteBuffer.allocateDirect(n * 8).order(ByteOrder.nativeOrder()).asLongBuffer();
                long v = "ones".equals(e.fill) ? 1 : 0;
                for (int i = 0; i < n; i++) lb.put(v);
                lb.rewind();
                return OnnxTensor.createTensor(env, lb, e.shape);
            }
            case "bool": {
                ByteBuffer bb = ByteBuffer.allocateDirect(n).order(ByteOrder.nativeOrder());
                byte v = (byte) ("ones".equals(e.fill) ? 1 : 0);
                for (int i = 0; i < n; i++) bb.put(v);
                bb.rewind();
                return OnnxTensor.createTensor(env, bb, e.shape, OnnxJavaType.BOOL);
            }
            default:
                throw new IllegalArgumentException("unsupported state dtype " + e.dtype);
        }
    }

    /** Live state: tensors owned either by us (created) or by the Result they came from. */
    private static final class State implements AutoCloseable {
        final Map<String, OnnxTensor> t = new HashMap<>();
        final List<AutoCloseable> owned = new ArrayList<>();

        @Override
        public void close() {
            for (AutoCloseable c : owned) {
                try {
                    c.close();
                } catch (Exception ignored) {
                    // best effort
                }
            }
            owned.clear();
            t.clear();
        }
    }

    private State initState(List<StateEntry> m) throws OrtException {
        State s = new State();
        for (StateEntry e : m) {
            OnnxTensor x = fillTensor(e);
            s.t.put(e.inputName, x);
            s.owned.add(x);
        }
        return s;
    }

    /** Replace the state with the outputs of {@code r}; the old state is closed. r is owned by the new state. */
    private static State update(State old, OrtSession.Result r, List<String> outs, List<StateEntry> m, int off) {
        State s = new State();
        for (StateEntry e : m) s.t.put(e.inputName, (OnnxTensor) r.get(outs.get(off + e.index)).orElseThrow(
                () -> new IllegalStateException("missing output")));
        s.owned.add(r);
        old.close();
        return s;
    }

    private static OnnxValue out(OrtSession.Result r, List<String> outs, int i) {
        return r.get(outs.get(i)).orElseThrow(() -> new IllegalStateException("missing output " + i));
    }

    private static Map<String, OnnxTensor> inputs(State s, Object... kv) {
        Map<String, OnnxTensor> m = new HashMap<>(s.t);
        for (int i = 0; i < kv.length; i += 2) m.put((String) kv[i], (OnnxTensor) kv[i + 1]);
        return m;
    }

    // ------------------------------------------------------------ voice (primed once, cached)

    private synchronized Map<String, Cached> voiceState() throws IOException, OrtException {
        if (voiceState != null) return voiceState;
        Npy v = Npy.read(voiceNpy);
        int frames = v.data.length / condDim;
        if (frames * condDim != v.data.length) throw new IOException("voice embedding: last dim != " + condDim);
        float[] emb = new float[(bosFrames + frames) * condDim];
        if (bos != null) System.arraycopy(bos, 0, emb, 0, bos.length);
        System.arraycopy(v.data, 0, emb, bosFrames * condDim, v.data.length);
        State s = initState(flowManifest);
        try (OnnxTensor seq = floats(new float[0], 1, 0, latentDim);
             OnnxTensor te = floats(emb, 1, bosFrames + frames, condDim)) {
            OrtSession.Result r = main.run(inputs(s, "sequence", seq, "text_embeddings", te));
            s = update(s, r, mainOut, flowManifest, 2);
            Map<String, Cached> c = new HashMap<>();
            for (StateEntry e : flowManifest) {
                OnnxTensor x = s.t.get(e.inputName);
                TensorInfo ti = x.getInfo();
                ByteBuffer raw = x.getByteBuffer();
                ByteBuffer d = ByteBuffer.allocateDirect(raw.remaining()).order(ByteOrder.nativeOrder());
                d.put(raw).rewind();
                c.put(e.inputName, new Cached(d, ti.getShape(), ti.type));
            }
            voiceState = c;
            return c;
        } finally {
            s.close();
        }
    }

    /** Prime + cache Ko's voice state now (else on the first line). */
    public void warmUp() throws IOException, OrtException {
        voiceState();
    }

    private State clonedVoice() throws IOException, OrtException {
        State s = new State();
        for (Map.Entry<String, Cached> e : voiceState().entrySet()) {
            Cached c = e.getValue();
            OnnxTensor x = OnnxTensor.createTensor(env, c.data.duplicate().order(ByteOrder.nativeOrder()), c.shape, c.type);
            s.t.put(e.getKey(), x);
            s.owned.add(x);
        }
        return s;
    }

    // ------------------------------------------------------------ synthesis

    /**
     * pocket.stream: audio blocks for ONE chunk of text (<= max tokens).
     * @param seed sampling seed (temperature > 0); null = random
     * @param cancel polled every frame; return true to stop
     */
    public void stream(String text, double temp, Long seed, BlockSink sink, java.util.concurrent.Callable<Boolean> cancel)
            throws Exception {
        Object[] prep = SpeechText.prepare(text, removeSemicolons);
        String t = (String) prep[0];
        int after = (Integer) prep[1];
        if (t.isEmpty()) return;
        Random rng = seed == null ? new Random() : new Random(seed);
        int[] ids = tok.encode(t);
        long[] lids = new long[ids.length];
        for (int i = 0; i < ids.length; i++) lids[i] = ids[i];
        State s = clonedVoice();
        State ms = null;
        try {
            OnnxTensor te3;
            float[] teData;
            long[] teShape;
            try (OnnxTensor idT = OnnxTensor.createTensor(env, LongBuffer.wrap(lids), new long[] {1, ids.length});
                 OrtSession.Result r = textCond.run(java.util.Collections.singletonMap("token_ids", idT))) {
                OnnxTensor te = (OnnxTensor) r.get(0);
                long[] sh = te.getInfo().getShape();
                FloatBuffer fb = te.getFloatBuffer();
                teData = new float[fb.remaining()];
                fb.get(teData);
                teShape = sh.length == 2 ? new long[] {1, sh[0], sh[1]} : sh;
            }
            te3 = floats(teData, teShape);
            try (OnnxTensor seq0 = floats(new float[0], 1, 0, latentDim)) {
                OrtSession.Result r = main.run(inputs(s, "sequence", seq0, "text_embeddings", te3));
                s = update(s, r, mainOut, flowManifest, 2);
            } finally {
                te3.close();
            }
            int limit = (int) Math.ceil((ids.length / 3.0 + 2.0) * frameRate);
            ms = initState(mimiManifest);
            int minEos = Math.max(4, (int) (1.5 * ids.length));
            float[] nan = new float[latentDim];
            java.util.Arrays.fill(nan, Float.NaN);
            float[] curr = nan;
            List<float[]> pend = new ArrayList<>();
            int eos = -1, emitted = 0;
            float dt = 1.0f / st.length;
            float sd = (float) Math.sqrt(temp);
            try (OnnxTensor empty = floats(new float[0], 1, 0, condDim)) {
                for (int step = 0; step < limit; step++) {
                    if (cancel != null && cancel.call()) return;
                    OrtSession.Result r;
                    try (OnnxTensor cur = floats(curr, 1, 1, latentDim)) {
                        r = main.run(inputs(s, "sequence", cur, "text_embeddings", empty));
                    }
                    OnnxTensor cond = (OnnxTensor) out(r, mainOut, 0);
                    float eosLogit = ((OnnxTensor) out(r, mainOut, 1)).getFloatBuffer().get(0);
                    s = update(s, r, mainOut, flowManifest, 2); // r (and cond) now owned by s
                    if (eos < 0 && step >= minEos && eosLogit > -4.0f) eos = step;
                    if (eos >= 0 && step >= eos + after) break;
                    float[] x = new float[latentDim];
                    if (temp > 0) for (int i = 0; i < latentDim; i++) x[i] = (float) rng.nextGaussian() * sd;
                    for (float[] pair : st) {
                        try (OnnxTensor sT = floats(new float[] {pair[0]}, 1, 1);
                             OnnxTensor tT = floats(new float[] {pair[1]}, 1, 1);
                             OnnxTensor xT = floats(x, 1, latentDim)) {
                            Map<String, OnnxTensor> in = new HashMap<>();
                            in.put("c", cond);
                            in.put("s", sT);
                            in.put("t", tT);
                            in.put("x", xT);
                            try (OrtSession.Result fr = flow.run(in)) {
                                FloatBuffer v = ((OnnxTensor) fr.get(0)).getFloatBuffer();
                                for (int i = 0; i < latentDim; i++) x[i] = x[i] + v.get(i) * dt;
                            }
                        }
                    }
                    pend.add(x);
                    curr = x;
                    int need = emitted == 0 ? 3 : 12;
                    if (pend.size() >= need) {
                        ms = decode(pend, ms, sink);
                        emitted += pend.size();
                        pend.clear();
                        if (ms == null) return; // sink asked to stop
                    }
                }
            }
            if (!pend.isEmpty() && (cancel == null || !cancel.call())) ms = decode(pend, ms, sink);
        } finally {
            s.close();
            if (ms != null) ms.close();
        }
    }

    /** mimi_decoder on the pending latents; returns the new state, or null (state closed) if the sink said stop. */
    private State decode(List<float[]> pend, State ms, BlockSink sink) throws OrtException {
        float[] lat = new float[pend.size() * latentDim];
        for (int i = 0; i < pend.size(); i++) System.arraycopy(pend.get(i), 0, lat, i * latentDim, latentDim);
        OrtSession.Result r;
        try (OnnxTensor l = floats(lat, 1, pend.size(), latentDim)) {
            r = dec.run(inputs(ms, "latent", l));
        }
        FloatBuffer a = ((OnnxTensor) out(r, decOut, 0)).getFloatBuffer();
        float[] pcm = new float[a.remaining()];
        a.get(pcm);
        State next = update(ms, r, decOut, mimiManifest, 1);
        if (!sink.accept(pcm)) {
            next.close();
            return null;
        }
        return next;
    }

    @Override
    public void close() {
        for (OrtSession s : new OrtSession[] {textCond, main, flow, dec}) {
            try {
                if (s != null) s.close();
            } catch (OrtException ignored) {
                // closing
            }
        }
    }

    // ------------------------------------------------------------ helpers

    static String readText(File f) throws IOException {
        try (RandomAccessFile r = new RandomAccessFile(f, "r")) {
            byte[] b = new byte[(int) r.length()];
            r.readFully(b);
            return new String(b, StandardCharsets.UTF_8);
        }
    }

    /** Minimal .npy reader: little-endian float32 (or float16/float64), C order. */
    static final class Npy {
        final long[] shape;
        final float[] data;

        private Npy(long[] s, float[] d) {
            shape = s;
            data = d;
        }

        static Npy read(File f) throws IOException {
            byte[] b;
            try (RandomAccessFile r = new RandomAccessFile(f, "r")) {
                b = new byte[(int) r.length()];
                r.readFully(b);
            }
            if (b.length < 10 || b[0] != (byte) 0x93 || b[1] != 'N' || b[2] != 'U' || b[3] != 'M' || b[4] != 'P'
                    || b[5] != 'Y') {
                throw new IOException(f.getName() + ": not a .npy file");
            }
            int major = b[6];
            int hlen, hoff;
            if (major == 1) {
                hlen = (b[8] & 0xFF) | (b[9] & 0xFF) << 8;
                hoff = 10;
            } else {
                hlen = (b[8] & 0xFF) | (b[9] & 0xFF) << 8 | (b[10] & 0xFF) << 16 | (b[11] & 0xFF) << 24;
                hoff = 12;
            }
            String h = new String(b, hoff, hlen, StandardCharsets.ISO_8859_1);
            if (h.contains("'fortran_order': True")) throw new IOException(f.getName() + ": fortran order");
            java.util.regex.Matcher dm = java.util.regex.Pattern.compile("'descr':\\s*'([<>|=]?)(f[248])'").matcher(h);
            if (!dm.find() || ">".equals(dm.group(1))) throw new IOException(f.getName() + ": unsupported dtype in " + h);
            java.util.regex.Matcher sm = java.util.regex.Pattern.compile("'shape':\\s*\\(([^)]*)\\)").matcher(h);
            if (!sm.find()) throw new IOException(f.getName() + ": no shape");
            List<Long> dims = new ArrayList<>();
            for (String p : sm.group(1).split(",")) {
                if (!p.trim().isEmpty()) dims.add(Long.parseLong(p.trim()));
            }
            long[] shape = new long[dims.size()];
            long n = 1;
            for (int i = 0; i < shape.length; i++) {
                shape[i] = dims.get(i);
                n *= shape[i];
            }
            int width = dm.group(2).charAt(1) - '0';
            int off = hoff + hlen;
            if (off + n * width > b.length) throw new IOException(f.getName() + ": truncated");
            ByteBuffer bb = ByteBuffer.wrap(b, off, (int) (n * width)).order(ByteOrder.LITTLE_ENDIAN);
            float[] d = new float[(int) n];
            for (int i = 0; i < n; i++) {
                if (width == 4) d[i] = bb.getFloat();
                else if (width == 8) d[i] = (float) bb.getDouble();
                else d[i] = halfToFloat(bb.getShort());
            }
            return new Npy(shape, d);
        }

        private static float halfToFloat(short h) {
            int s = (h >> 15) & 1, e = (h >> 10) & 0x1F, m = h & 0x3FF;
            float v;
            if (e == 0) v = (float) (m * Math.pow(2, -24));
            else if (e == 31) v = m == 0 ? Float.POSITIVE_INFINITY : Float.NaN;
            else v = (float) ((1 + m / 1024.0) * Math.pow(2, e - 15));
            return s == 1 ? -v : v;
        }
    }
}
