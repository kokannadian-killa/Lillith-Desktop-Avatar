package com.kokannadian.lillith;

/**
 * The non-cloned voice used when Ko's cloned voice can't run (not installed,
 * or the model failed to load). Owned by the polish bot (Android TextToSpeech,
 * see AssetInstaller.fallbackVoice); registered with {@link LillithVoice#setFallback}.
 * Speaking only: implementations must not record audio or use the network.
 */
public interface VoiceFallback {
    /** Speak a reply; should return promptly or block until done, either is fine. */
    void speak(String text);

    /** Stop talking at once (safeword) and drop anything it queued. */
    void stop();
}
