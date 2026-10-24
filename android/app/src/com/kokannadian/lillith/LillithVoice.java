package com.kokannadian.lillith;

import android.content.Context;
import android.util.Log;

import com.kokannadian.lillith.voice.PocketEngine;
import com.kokannadian.lillith.voice.PocketTts;
import com.kokannadian.lillith.voice.Safeword;
import com.kokannadian.lillith.voice.SpeechQueue;

import java.io.File;

/**
 * Lillith's voice OUTPUT on Android (speaking only: no microphone, no STT).
 * Ko's cloned voice (Pocket TTS on ONNX Runtime, fully offline, models from the
 * APK assets installed by AssetInstaller) when {@link AssetInstaller#clonedVoiceReady}
 * is true and the model loads; otherwise the registered {@link VoiceFallback}, and
 * the reason is logged (tag "lillith_voice").
 *
 * <p>Typed safeword ("red" / "OOC: stop"): {@link #stop} cuts her off mid-word and
 * drops every queued line, including replies requested before it ({@link #ticket}).
 * It only stops her talking; the local AI keeps running.
 */
public final class LillithVoice {
    private static final String TAG = "lillith_voice";
    private static LillithVoice instance;

    /** Desktop "lillith" preset: temperature 0.7, seed 1234 (the same line sounds the same). */
    private static final double TEMPERATURE = 0.7;
    private static final int SEED = 1234;

    private final Context app;
    private final SpeechQueue queue;
    private final FallbackEngine fallback = new FallbackEngine();
    private volatile PocketTts tts;
    private volatile AudioTrackSink sink;
    private volatile boolean loading;

    public static synchronized LillithVoice get(Context c) {
        if (instance == null) instance = new LillithVoice(c.getApplicationContext());
        return instance;
    }

    private LillithVoice(Context app) {
        this.app = app;
        queue = new SpeechQueue(fallback, m -> Log.i(TAG, m));
        if (AssetInstaller.clonedVoiceReady(app)) {
            load();
        } else {
            Log.w(TAG, "voice: cloned voice not installed; using fallback " + AssetInstaller.fallbackVoice(app));
        }
    }

    /** The polish bot's fallback voice (Android TextToSpeech). */
    public void setFallback(VoiceFallback f) {
        fallback.impl = f;
    }

    /** True for any typed chat message that is the safeword. */
    public static boolean isSafeword(String typed) {
        return Safeword.isTyped(typed);
    }

    /** Safeword: stop talking now and forget everything queued. */
    public void stop(String reason) {
        queue.stop(reason);
        fallback.stopNow(); // it may still be talking if the clone finished loading mid-line
    }

    /** Take when a reply is requested; pass to {@link #sayIfCurrent}. */
    public long ticket() {
        return queue.ticket();
    }

    /** Speak a reply unless a stop happened since {@code ticket}. */
    public boolean sayIfCurrent(long ticket, String text) {
        return queue.sayIfCurrent(ticket, text);
    }

    public void say(String text) {
        queue.say(text);
    }

    private void load() {
        loading = true;
        Thread t = new Thread(() -> {
            File dir = new File(AssetInstaller.voiceDir(app), "pocket");
            try {
                int threads = Math.max(1, Math.min(4, Runtime.getRuntime().availableProcessors()));
                PocketTts p = new PocketTts(dir, new File(dir, "voices/lillith.npy"), threads, TEMPERATURE);
                p.warmUp();
                AudioTrackSink s = new AudioTrackSink();
                tts = p;
                sink = s;
                queue.setEngine(new PocketEngine(p, s, TEMPERATURE, SEED, m -> Log.i(TAG, m)));
                Log.i(TAG, "voice: cloned 'lillith' voice loaded (" + p.files() + ") in " + p.loadMillis + " ms");
            } catch (Throwable e) {
                Log.e(TAG, "voice: cloned voice failed to load (" + e + "); using fallback "
                        + AssetInstaller.fallbackVoice(app), e);
            } finally {
                loading = false;
            }
        }, "lillith-voice-load");
        t.setDaemon(true);
        t.start();
    }

    /** Wraps the polish bot's VoiceFallback; logs when none is registered yet. */
    private final class FallbackEngine implements SpeechQueue.Engine {
        volatile VoiceFallback impl;
        private boolean warned;

        @Override
        public void speak(String text, SpeechQueue.Cancel c) {
            VoiceFallback f = impl;
            if (f == null) {
                if (!warned) {
                    Log.w(TAG, "voice: no fallback voice registered (" + AssetInstaller.fallbackVoice(app)
                            + (loading ? "; cloned voice still loading" : "") + "); reply not spoken");
                    warned = true;
                }
                return;
            }
            if (!c.cancelled()) f.speak(text);
        }

        @Override
        public void stopNow() {
            VoiceFallback f = impl;
            if (f != null) f.stop();
        }

        @Override
        public String name() {
            return "fallback";
        }
    }
}
