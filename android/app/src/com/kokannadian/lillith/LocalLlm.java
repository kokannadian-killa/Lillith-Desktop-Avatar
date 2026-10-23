package com.kokannadian.lillith;

import java.nio.charset.StandardCharsets;
import java.util.List;

/**
 * On-device llama.cpp (liblillith_llm.so, android/llm). Fully local: loads the
 * GGUF the user picked in setup from app storage. No network of any kind.
 */
final class LocalLlm {
    static {
        System.loadLibrary("lillith_llm");
    }

    private static native long nLoad(String path, int nCtx, int nThreads);
    private static native void nFree(long h);
    private static native byte[] nChat(long h, String[] roles, String[] contents, int maxTokens,
                                       float temp, float topP, int seed);

    private static LocalLlm instance;
    private static String instancePath;

    private final long handle;

    private LocalLlm(long handle) {
        this.handle = handle;
    }

    /** Loads (or reuses) the model at {@code path}; null if it fails. Blocking. */
    static synchronized LocalLlm get(String path) {
        if (instance != null && path.equals(instancePath)) return instance;
        if (instance != null) {
            nFree(instance.handle);
            instance = null;
        }
        int threads = Math.max(2, Math.min(6, Runtime.getRuntime().availableProcessors() - 2));
        long h = nLoad(path, 4096, threads);
        if (h == 0) return null;
        instance = new LocalLlm(h);
        instancePath = path;
        return instance;
    }

    /** One reply for the chat so far (role/content pairs, system first). Blocking. */
    synchronized String chat(List<String[]> turns, int maxTokens) {
        String[] roles = new String[turns.size()];
        String[] contents = new String[turns.size()];
        for (int i = 0; i < turns.size(); i++) {
            roles[i] = turns.get(i)[0];
            contents[i] = turns.get(i)[1];
        }
        byte[] out = nChat(handle, roles, contents, maxTokens, 0.8f, 0.9f,
                (int) (System.nanoTime() & 0x7fffffff));
        return out == null ? null : new String(out, StandardCharsets.UTF_8).trim();
    }
}
