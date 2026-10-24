package com.kokannadian.lillith.voice;

import java.util.concurrent.LinkedBlockingQueue;
import java.util.concurrent.atomic.AtomicLong;

/**
 * One-at-a-time speech queue with a hard stop. {@link #stop} (the safeword)
 * bumps a generation counter, drops every queued line and cuts the audio
 * that is already playing; anything rendered or requested under an older
 * generation is discarded, so nothing said before "red" can start after it.
 * Pure Java (no Android imports) so it is unit-tested on the JVM.
 */
public final class SpeechQueue {
    /** Cancellation token handed to engines; true once a stop happened after the line was queued. */
    public interface Cancel {
        boolean cancelled();
    }

    /** A voice that speaks a reply (blocking or not) and can be cut at once from any thread. */
    public interface Engine {
        /** Speak (render and/or hand to the player); return early when {@code c} is cancelled. */
        void speak(String text, Cancel c) throws Exception;

        /** Cut playback NOW (called after the generation is bumped). Any thread. */
        void stopNow();

        String name();
    }

    /** Log sink (android.util.Log on device, stderr on the JVM). */
    public interface Logger {
        void log(String msg);
    }

    private static final class Item {
        final long gen;
        final String text;

        Item(long gen, String text) {
            this.gen = gen;
            this.text = text;
        }
    }

    private static final Item POISON = new Item(-1, null);

    private final AtomicLong gen = new AtomicLong();
    private final LinkedBlockingQueue<Item> q = new LinkedBlockingQueue<>();
    private final Logger log;
    private volatile Engine engine;
    private final Thread worker;
    private volatile boolean closed;

    public SpeechQueue(Engine engine, Logger log) {
        this.engine = engine;
        this.log = log == null ? m -> { } : log;
        worker = new Thread(this::run, "lillith-voice");
        worker.setDaemon(true);
        worker.start();
    }

    /** Swap the voice for the next line (e.g. the cloned voice finished loading). */
    public void setEngine(Engine e) {
        engine = e;
    }

    public Engine engine() {
        return engine;
    }

    /** Current generation: take it when a reply is requested, pass it to {@link #sayIfCurrent}. */
    public long ticket() {
        return gen.get();
    }

    /** Queue a line now. */
    public void say(String text) {
        sayIfCurrent(gen.get(), text);
    }

    /**
     * Queue a line only if no stop happened since {@code ticket} was taken
     * (a reply that was being generated when the safeword arrived is dropped).
     * @return true if queued
     */
    public boolean sayIfCurrent(long ticket, String text) {
        if (closed || text == null || text.trim().isEmpty()) return false;
        synchronized (this) { // atomic with stop(): no enqueue can slip in after a stop cleared the queue
            if (ticket != gen.get()) {
                log.log("voice: dropped a line requested before a stop");
                return false;
            }
            q.add(new Item(ticket, text));
        }
        return true;
    }

    /** The safeword: stop talking at once and forget everything queued. */
    public void stop(String reason) {
        int dropped;
        synchronized (this) {
            gen.incrementAndGet();
            dropped = q.size();
            q.clear();
        }
        Engine e = engine;
        if (e != null) e.stopNow();
        log.log("voice: stop (" + reason + "), dropped " + dropped + " queued line(s)");
    }

    public boolean idle() {
        return q.isEmpty() && !busy;
    }

    private volatile boolean busy;

    private void run() {
        while (!closed) {
            Item it;
            try {
                it = q.take();
            } catch (InterruptedException e) {
                return;
            }
            if (it == POISON) return;
            final long g = it.gen;
            Cancel c = () -> gen.get() != g || closed;
            if (c.cancelled()) continue;
            Engine e = engine;
            if (e == null) continue;
            busy = true;
            try {
                e.speak(it.text, c);
            } catch (Throwable t) {
                log.log("voice: " + e.name() + " failed: " + t);
            } finally {
                busy = false;
            }
        }
    }

    public void close() {
        closed = true;
        stop("close");
        q.add(POISON);
    }
}
