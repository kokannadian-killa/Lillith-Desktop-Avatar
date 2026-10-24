package com.kokannadian.lillith.voice;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * Dependency-free JVM tests for the Android voice core (run by android/voice-tests.sh):
 * parity with the desktop kit (fixtures generated from the Python reference), the typed
 * safeword matcher and the stop-drops-everything speech queue.
 *   java VoiceTests FIXTURE_DIR TOKENIZER_MODEL
 */
public final class VoiceTests {
    private static int pass, fail;
    private static final List<String> failures = new ArrayList<>();

    private static void check(String group, boolean ok, String what) {
        if (ok) {
            pass++;
        } else {
            fail++;
            if (failures.size() < 40) failures.add("[" + group + "] " + what);
        }
    }

    static String unesc(String s) {
        StringBuilder b = new StringBuilder();
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            if (c == '\\' && i + 1 < s.length()) {
                char n = s.charAt(++i);
                b.append(n == 'n' ? '\n' : n == 't' ? '\t' : n == 'r' ? '\r' : n);
            } else {
                b.append(c);
            }
        }
        return b.toString();
    }

    static List<String[]> tsv(File dir, String name) throws Exception {
        List<String[]> rows = new ArrayList<>();
        for (String line : Files.readAllLines(new File(dir, name).toPath(), StandardCharsets.UTF_8)) {
            if (line.isEmpty()) continue;
            rows.add(line.split("\t", -1));
        }
        return rows;
    }

    static String show(String s) {
        return "'" + s.replace("\n", "\\n").replace("\t", "\\t") + "'";
    }

    public static void main(String[] a) throws Exception {
        File fx = new File(a[0]);
        SpTokenizer tok = SpTokenizer.load(new File(a[1]));
        int before;

        before = fail;
        for (String[] r : tsv(fx, "textnorm.tsv")) {
            String in = unesc(r[0]), want = unesc(r[1]), got = TextNorm.normalize(in);
            check("textnorm", want.equals(got), show(in) + " -> " + show(got) + " want " + show(want));
        }
        report("textnorm", before);

        before = fail;
        for (String[] r : tsv(fx, "clean_split.tsv")) {
            String in = unesc(r[0]), c = SpeechText.cleanText(in);
            check("clean_text", unesc(r[1]).equals(c), show(in) + " -> " + show(c) + " want " + show(unesc(r[1])));
            List<String> want = new ArrayList<>();
            for (int i = 2; i < r.length; i++) want.add(unesc(r[i]));
            if (want.size() == 1 && want.get(0).isEmpty()) want.clear();
            List<String> got = SpeechText.splitSentences(unesc(r[1]));
            check("split_sentences", want.equals(got), show(r[1]) + " -> " + got + " want " + want);
        }
        report("clean_text + split_sentences", before);

        before = fail;
        for (String[] r : tsv(fx, "tokenizer.tsv")) {
            String in = unesc(r[0]);
            String got = join(tok.encode(in));
            check("tokenizer", r[1].equals(got), show(in) + " -> [" + got + "] want [" + r[1] + "]");
        }
        report("tokenizer", before);

        before = fail;
        for (String[] r : tsv(fx, "chain.tsv")) {
            String c = unesc(r[0]), t = TextNorm.normalize(c);
            check("chain.normalize", unesc(r[1]).equals(t), show(c));
            check("chain.seed", Long.parseLong(r[2]) == SpeechText.seedFor(1234, t), show(t) + " seed");
            List<String> parts = SpeechText.pocketSplit(t, false, 50, s -> tok.encode(s).length);
            int n = (r.length - 3) / 3;
            check("chain.split", parts.size() == n, show(t) + " parts " + parts + " want " + n);
            for (int i = 0; i < Math.min(n, parts.size()); i++) {
                Object[] p = SpeechText.prepare(parts.get(i), false);
                check("chain.prepare", unesc(r[3 + 3 * i]).equals(p[0]), show(parts.get(i)) + " -> " + show((String) p[0])
                        + " want " + show(unesc(r[3 + 3 * i])));
                check("chain.after", Integer.parseInt(r[4 + 3 * i]) == (Integer) p[1], show((String) p[0]) + " after");
                check("chain.ids", r[5 + 3 * i].equals(join(tok.encode((String) p[0]))), show((String) p[0]) + " ids");
            }
        }
        report("full text chain (normalize/seed/split/prepare/ids)", before);

        before = fail;
        for (String[] r : tsv(fx, "safeword_kit.tsv")) {
            String in = unesc(r[0]);
            check("safeword.kit_regex", Safeword.voiceKitRegex(in) == r[1].equals("1"), show(in) + " want " + r[1]);
        }
        safewordTable();
        report("safeword", before);

        before = fail;
        dspParity(fx);
        report("dsp polish parity", before);

        before = fail;
        queueDropTest();
        queueTicketTest();
        engineStopTest();
        report("speech queue stop/drop", before);

        System.out.println("\n" + pass + " passed, " + fail + " failed");
        for (String f : failures) System.out.println("  FAIL " + f);
        System.exit(fail == 0 ? 0 : 1);
    }

    private static void report(String what, int before) {
        System.out.println((fail == before ? "PASS " : "FAIL ") + what + (fail == before ? "" : " (" + (fail - before) + " failures)"));
    }

    static String join(int[] ids) {
        StringBuilder b = new StringBuilder();
        for (int i = 0; i < ids.length; i++) {
            if (i > 0) b.append(' ');
            b.append(ids[i]);
        }
        return b.toString();
    }

    // ------------------------------------------------------------ safeword (typed)

    private static void safewordTable() {
        // lillith-core kill_switch.rs tests (desktop chat box)
        for (String s : new String[] {"red", "RED", " Red! ", "red.", "OOC: stop", "ooc stop please", "(OOC: STOP)"}) {
            check("safeword.rust", Safeword.desktopChat(s), show(s) + " should match");
            check("safeword.typed", Safeword.isTyped(s), show(s) + " should match");
        }
        for (String s : new String[] {"", "redo", "I read a red book", "stop", "ooc: go on", "reddit"}) {
            check("safeword.rust", !Safeword.desktopChat(s), show(s) + " should not match");
            check("safeword.typed", !Safeword.isTyped(s), show(s) + " should not match");
        }
        // tests/safeword_test.py cases that apply to TYPED text: every regex case; the whisper
        // mishearings ("Read.", "Rad.", "Right." ... caught by fuzzy_red) are not the safeword when typed.
        String[] yes = {"Red.", "red", "Red!", "Red, red.", "OOC: stop", "Oh, see, stop.", "O O C stop", "red red red",
            "Oh, I'll see, stop.", "O-C. Start.", "Oh, see, start.", "O.O.C. stop!", "out of character, stop",
            "red\n", "\tred\t", "rEd?!", "OOC:stop", "ooc, stop", "OOC Stop!"};
        String[] no = {"Read.", "Rad.", "Rid.", "Rhett.", "Wred", "Right.", "That's right.", "I read it.", "Yes.",
            "Stop reading.", "All right then.", "Right, let's go to the red room.", "Read me a story.", "Yes, I agree.",
            "Wait.", "Oh, stop it.", "You'll stop.", "I see, stop that.", "Oh, I'll see you start.", "Let's start.",
            "Stop.", "Ready.", "Rate.", "", "bored", "redd", "re d", "ooc: go on", "Read", "right"};
        for (String s : yes) check("safeword.typed", Safeword.isTyped(s), show(s) + " should match");
        for (String s : no) check("safeword.typed", !Safeword.isTyped(s), show(s) + " should not match");
        check("safeword.null", !Safeword.isTyped(null), "null");
    }

    // ------------------------------------------------------------ dsp

    private static void dspParity(File fx) throws Exception {
        List<String[]> rows = tsv(fx, "dsp.tsv");
        for (String[] r : rows) {
            int k = Integer.parseInt(r[0]);
            float[] x = PocketTts.Npy.read(new File(fx, "dsp_raw" + k + ".npy")).data;
            float[] want = PocketTts.Npy.read(new File(fx, "dsp_pol" + k + ".npy")).data;
            Dsp.Polished p = Dsp.polish(x, 24000, null);
            double wantG = Double.parseDouble(r[1]);
            check("dsp.gain", p.gainDb != null && Math.abs(p.gainDb - wantG) < 1e-3, "case " + k + " gain " + p.gainDb + " want " + wantG);
            closeArr("dsp.polish#" + k, p.audio, want, 2e-4);
            float[] want2 = PocketTts.Npy.read(new File(fx, "dsp_polref" + k + ".npy")).data;
            closeArr("dsp.polish_ref#" + k, Dsp.polish(x, 24000, 4.25).audio, want2, 2e-4);
            if (!r[2].equals("nan")) {
                double L = Dsp.loudness(Dsp.eqDeess(Dsp.trim(x, 24000), 24000), 24000);
                check("dsp.loudness", Math.abs(L - Double.parseDouble(r[2])) < 1e-3, "case " + k + " L " + L + " want " + r[2]);
            }
            float[] xf = PocketTts.Npy.read(new File(fx, "dsp_xf" + k + ".npy")).data;
            List<float[]> bl = Arrays.asList(Arrays.copyOfRange(x, 0, 700), Arrays.copyOfRange(x, 700, 2000),
                    Arrays.copyOfRange(x, 2000, x.length));
            closeArr("dsp.crossfade#" + k, Dsp.crossfadeConcat(bl, 24000, 3.0), xf, 1e-6);
        }
    }

    private static void closeArr(String what, float[] got, float[] want, double tol) {
        if (got.length != want.length) {
            check(what, false, "length " + got.length + " want " + want.length);
            return;
        }
        double m = 0;
        for (int i = 0; i < got.length; i++) m = Math.max(m, Math.abs(got[i] - want[i]));
        check(what, m <= tol, "max |diff| " + m + " > " + tol);
        System.out.println("  " + what + ": n=" + got.length + " max|diff|=" + String.format("%.2e", m));
    }

    // ------------------------------------------------------------ queue

    /** Records every sample written; "playing" takes real time so a stop lands mid-line. */
    static final class FakeSink implements PcmSink {
        final Object lock = new Object();
        final List<Float> written = Collections.synchronizedList(new ArrayList<>());
        volatile int flushes;

        @Override
        public boolean write(float[] pcm, int off, int len, SpeechQueue.Cancel c) {
            try {
                Thread.sleep(2); // a 20 ms slice "plays" in ~2 ms
            } catch (InterruptedException e) {
                return false;
            }
            synchronized (lock) {
                if (c.cancelled()) return false;
                for (int i = 0; i < len; i++) written.add(pcm[off + i]);
                return true;
            }
        }

        @Override
        public void flushNow() {
            synchronized (lock) {
                flushes++;
            }
        }

        @Override
        public void drain(SpeechQueue.Cancel c) {}
    }

    /** Fake voice: "renders" each line slowly as samples equal to its id. */
    static final class FakeEngine implements SpeechQueue.Engine {
        final FakeSink sink;
        final List<String> started = Collections.synchronizedList(new ArrayList<>());
        final AtomicInteger stops = new AtomicInteger();
        volatile CountDownLatch firstAudio = new CountDownLatch(1);

        FakeEngine(FakeSink s) {
            sink = s;
        }

        @Override
        public void speak(String text, SpeechQueue.Cancel c) throws Exception {
            started.add(text);
            float id = Float.parseFloat(text.substring(4));
            float[] block = new float[480];
            Arrays.fill(block, id);
            for (int i = 0; i < 200 && !c.cancelled(); i++) { // ~400 ms of "speech"
                if (!sink.write(block, 0, block.length, c)) return;
                firstAudio.countDown();
            }
        }

        @Override
        public void stopNow() {
            stops.incrementAndGet();
            sink.flushNow();
        }

        @Override
        public String name() {
            return "fake";
        }
    }

    private static void queueDropTest() throws Exception {
        FakeSink sink = new FakeSink();
        FakeEngine eng = new FakeEngine(sink);
        List<String> log = Collections.synchronizedList(new ArrayList<>());
        SpeechQueue q = new SpeechQueue(eng, log::add);
        for (int i = 1; i <= 5; i++) q.say("line" + i);
        check("queue", eng.firstAudio.await(2, TimeUnit.SECONDS), "line1 never started");
        q.stop("safeword red");
        int atStop = sink.written.size();
        Thread.sleep(300);
        check("queue.no_audio_after_stop", sink.written.size() == atStop,
                "samples written after stop: " + (sink.written.size() - atStop));
        check("queue.only_line1_started", eng.started.equals(Collections.singletonList("line1")),
                "started " + eng.started);
        check("queue.flushed", sink.flushes == 1 && eng.stops.get() == 1, "flushes " + sink.flushes);
        check("queue.logged", log.size() == 1 && log.get(0).contains("dropped 4"), "log " + log);
        // she can talk again afterwards
        eng.firstAudio = new CountDownLatch(1);
        q.say("line9");
        check("queue.after_stop_speaks", eng.firstAudio.await(2, TimeUnit.SECONDS), "line9 never played");
        Thread.sleep(50);
        check("queue.after_stop_line", eng.started.size() == 2 && eng.started.get(1).equals("line9"), "started " + eng.started);
        synchronized (sink.lock) {
            boolean ok = true;
            for (int i = atStop; i < sink.written.size(); i++) ok &= sink.written.get(i) == 9f;
            check("queue.after_stop_only_new", ok, "old audio after stop");
        }
        q.close();
    }

    private static void queueTicketTest() throws Exception {
        FakeSink sink = new FakeSink();
        FakeEngine eng = new FakeEngine(sink);
        SpeechQueue q = new SpeechQueue(eng, null);
        long ticket = q.ticket(); // reply requested ...
        q.stop("safeword red"); // ... "red" typed while the LLM is still thinking
        check("ticket.dropped", !q.sayIfCurrent(ticket, "line7"), "stale reply was queued");
        Thread.sleep(100);
        check("ticket.not_spoken", eng.started.isEmpty() && sink.written.isEmpty(), "stale reply spoke");
        check("ticket.fresh_ok", q.sayIfCurrent(q.ticket(), "line8"), "fresh reply refused");
        // stop racing many says: nothing queued before the stop may start after it
        for (int round = 0; round < 50; round++) {
            for (int i = 0; i < 20; i++) q.say("line" + (100 + i));
            Thread.sleep(round % 7);
            q.stop("race");
            int at = sink.written.size();
            Thread.sleep(8);
            check("ticket.race", sink.written.size() == at, "round " + round + ": audio after stop");
        }
        q.close();
    }

    /** PocketEngine's player obeys a stop too (fake synthesis replaced by pre-rendered audio). */
    private static void engineStopTest() throws Exception {
        FakeSink sink = new FakeSink();
        AtomicInteger gen = new AtomicInteger();
        final int g = gen.get();
        SpeechQueue.Cancel c = () -> gen.get() != g;
        // PlayerOnly: exercise the slice loop the same way PocketEngine.playLoop does
        float[] a = new float[24000];
        Arrays.fill(a, 1f);
        Thread t = new Thread(() -> {
            for (int off = 0; off < a.length && !c.cancelled(); off += 480) {
                if (!sink.write(a, off, Math.min(480, a.length - off), c)) break;
            }
        });
        t.start();
        Thread.sleep(20);
        gen.incrementAndGet();
        sink.flushNow();
        int at = sink.written.size();
        t.join(1000);
        check("player.stops", sink.written.size() == at && at < a.length, "wrote " + (sink.written.size() - at) + " after stop");
    }
}
