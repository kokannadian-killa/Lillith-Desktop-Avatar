package com.kokannadian.lillith;

import android.media.AudioAttributes;
import android.media.AudioFormat;
import android.media.AudioTrack;
import android.os.SystemClock;

import com.kokannadian.lillith.voice.PcmSink;
import com.kokannadian.lillith.voice.SpeechQueue;

/**
 * Lillith's cloned voice out of the speaker: AudioTrack, MODE_STREAM, 24 kHz mono float.
 * Writes are non-blocking under a lock that {@link #flushNow} also takes, so after a
 * stop (safeword) not one stale sample can reach the device buffer. Output only: this
 * class never opens a microphone.
 */
final class AudioTrackSink implements PcmSink {
    private static final int SR = 24000;
    private final Object lock = new Object();
    private final AudioTrack track;

    AudioTrackSink() {
        int min = AudioTrack.getMinBufferSize(SR, AudioFormat.CHANNEL_OUT_MONO, AudioFormat.ENCODING_PCM_FLOAT);
        int bytes = Math.max(min, SR / 5 * 4); // ~200 ms device buffer: short enough for a quick stop
        track = new AudioTrack.Builder()
                .setAudioAttributes(new AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_ASSISTANT)
                        .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                        .build())
                .setAudioFormat(new AudioFormat.Builder()
                        .setSampleRate(SR)
                        .setEncoding(AudioFormat.ENCODING_PCM_FLOAT)
                        .setChannelMask(AudioFormat.CHANNEL_OUT_MONO)
                        .build())
                .setTransferMode(AudioTrack.MODE_STREAM)
                .setBufferSizeInBytes(bytes)
                .build();
    }

    @Override
    public boolean write(float[] pcm, int off, int len, SpeechQueue.Cancel c) {
        while (len > 0) {
            int n;
            synchronized (lock) {
                if (c.cancelled()) return false;
                if (track.getPlayState() != AudioTrack.PLAYSTATE_PLAYING) track.play();
                n = track.write(pcm, off, len, AudioTrack.WRITE_NON_BLOCKING);
            }
            if (n < 0) return false; // device error: give up on this line
            off += n;
            len -= n;
            if (len > 0) SystemClock.sleep(5); // buffer full: let it play
        }
        return true;
    }

    @Override
    public void flushNow() {
        synchronized (lock) {
            try {
                track.pause();
                track.flush();
            } catch (IllegalStateException ignored) {
                // not initialised / released
            }
        }
    }

    @Override
    public void drain(SpeechQueue.Cancel c) {
        // MODE_STREAM keeps playing what was written; nothing to wait for here.
    }

    void release() {
        synchronized (lock) {
            track.release();
        }
    }
}
