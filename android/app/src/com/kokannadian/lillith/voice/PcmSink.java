package com.kokannadian.lillith.voice;

/**
 * Where 24 kHz mono float PCM goes (AudioTrack on Android, a recorder in tests).
 * Implementations must make "check cancelled + write" atomic with {@link #flushNow},
 * so once a stop has bumped the generation and flushed, no stale audio can be written.
 */
public interface PcmSink {
    /** Write (blocking while the device buffer is full); false = cancelled, nothing more written. */
    boolean write(float[] pcm, int off, int len, SpeechQueue.Cancel c);

    /** Drop whatever is buffered and silence the output now. Any thread. */
    void flushNow();

    /** Wait until written audio has played out (or cancelled). */
    void drain(SpeechQueue.Cancel c);
}
