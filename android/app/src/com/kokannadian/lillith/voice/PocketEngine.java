package com.kokannadian.lillith.voice;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.LinkedBlockingQueue;

/**
 * Lillith's cloned voice as a {@link SpeechQueue.Engine}: the desktop
 * Speaker._play loop (lillith_voice/speaker.py) on Android. Each sentence is
 * rendered (Pocket TTS stream -> crossfades -> polish, same as Voice.render),
 * then played while the next sentence renders; the first sentence's gain is
 * reused for the rest of the reply, and natural pauses follow each sentence.
 */
public final class PocketEngine implements SpeechQueue.Engine {
    private final PocketTts tts;
    private final PcmSink sink;
    private final SpeechQueue.Logger log;
    private final double temperature;
    private final Integer seed;
    private final LinkedBlockingQueue<Object[]> play = new LinkedBlockingQueue<>(); // {Cancel, float[]}
    private final Object playLock = new Object();
    private final Thread player;

    /** @param seed desktop preset seed (1234) or null for a random take each time */
    public PocketEngine(PocketTts tts, PcmSink sink, double temperature, Integer seed, SpeechQueue.Logger log) {
        this.tts = tts;
        this.sink = sink;
        this.temperature = temperature;
        this.seed = seed;
        this.log = log == null ? m -> { } : log;
        player = new Thread(this::playLoop, "lillith-voice-play");
        player.setDaemon(true);
        player.start();
    }

    @Override
    public String name() {
        return "pocket(cloned)";
    }

    /** Voice.render(chunk) for the "lillith" preset: normalized text -> polished audio. */
    public Dsp.Polished render(String chunk, Double gainRef, SpeechQueue.Cancel c) throws Exception {
        String text = TextNorm.normalize(chunk);
        Long sd = seed == null ? null : SpeechText.seedFor(seed, text);
        List<float[]> parts = new ArrayList<>();
        List<String> split = tts.split(text);
        for (int i = 0; i < split.size(); i++) {
            List<float[]> blocks = new ArrayList<>();
            Long s = sd == null ? null : sd + 7919L * i;
            tts.stream(split.get(i), temperature, s, b -> {
                blocks.add(b);
                return !c.cancelled();
            }, c::cancelled);
            if (c.cancelled()) return null;
            parts.add(Dsp.crossfadeConcat(blocks, PocketTts.SR, 3.0));
        }
        float[] raw = Dsp.crossfadeConcat(parts, PocketTts.SR, 8.0);
        Dsp.Polished p = Dsp.polish(raw, PocketTts.SR, gainRef);
        // desktop: gref = st.get("gain_db", gref) and gain_db is round(g, 2)
        Double g = p.gainDb == null ? null : Math.round(p.gainDb * 100.0) / 100.0;
        return new Dsp.Polished(p.audio, g);
    }

    @Override
    public void speak(String text, SpeechQueue.Cancel c) throws Exception {
        List<String> chunks = SpeechText.splitSentences(SpeechText.cleanText(text), 120, 260);
        Double gref = null;
        long t0 = System.currentTimeMillis();
        for (int i = 0; i < chunks.size(); i++) {
            if (c.cancelled()) return;
            String ch = chunks.get(i);
            Dsp.Polished p = render(ch, gref, c);
            if (p == null || c.cancelled()) return;
            if (p.gainDb != null) gref = p.gainDb;
            float[] a = p.audio;
            if (i < chunks.size() - 1) { // natural pause is part of this chunk
                float[] f = Dsp.fade(a, PocketTts.SR, 0, 6);
                float[] gap = Dsp.gap(PocketTts.SR, SpeechText.pauseAfter(ch));
                a = new float[f.length + gap.length];
                System.arraycopy(f, 0, a, 0, f.length);
            }
            if (i == 0) log.log("voice: first audio after " + (System.currentTimeMillis() - t0) + " ms");
            synchronized (playLock) {
                if (c.cancelled()) return;
                play.add(new Object[] {c, a});
            }
        }
    }

    private void playLoop() {
        final int slice = PocketTts.SR / 50; // 20 ms slices: a stop lands within one slice
        while (true) {
            Object[] it;
            try {
                it = play.take();
            } catch (InterruptedException e) {
                return;
            }
            SpeechQueue.Cancel c = (SpeechQueue.Cancel) it[0];
            float[] a = (float[]) it[1];
            for (int off = 0; off < a.length && !c.cancelled(); off += slice) {
                if (!sink.write(a, off, Math.min(slice, a.length - off), c)) break;
            }
        }
    }

    @Override
    public void stopNow() {
        synchronized (playLock) {
            play.clear();
        }
        sink.flushNow();
    }
}
