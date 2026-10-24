package com.kokannadian.lillith.voice;

import java.io.DataOutputStream;
import java.io.File;
import java.io.FileOutputStream;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;

/** Offline synthesis smoke test on the JVM: one line through PocketEngine.render at temperature 0. */
public final class SmokeTest {
    public static void main(String[] a) throws Exception {
        File dir = new File(a[0]);
        long t0 = System.currentTimeMillis();
        PocketTts tts = new PocketTts(dir, new File(dir, "voices/lillith.npy"), 4, 0.0);
        tts.warmUp();
        System.out.println("smoke: loaded " + tts.files() + " + voice in " + (System.currentTimeMillis() - t0) + " ms");
        PocketEngine eng = new PocketEngine(tts, new VoiceTests.FakeSink(), 0.0, 1234, System.out::println);
        long t1 = System.currentTimeMillis();
        Dsp.Polished p = eng.render(a[2], null, () -> false);
        long ms = System.currentTimeMillis() - t1;
        float[] y = p.audio;
        System.out.printf("smoke: %d samples (%.2f s) in %d ms, gain %.2f dB%n", y.length, y.length / 24000.0, ms, p.gainDb);
        ByteBuffer bb = ByteBuffer.allocate(y.length * 4).order(ByteOrder.LITTLE_ENDIAN);
        for (float v : y) bb.putFloat(v);
        try (DataOutputStream o = new DataOutputStream(new FileOutputStream(a[1]))) {
            o.write(bb.array());
        }
        tts.close();
    }
}
