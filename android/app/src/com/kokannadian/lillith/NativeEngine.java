package com.kokannadian.lillith;

import java.nio.ByteBuffer;

/** JNI bridge to liblillith_android.so (android/rust): brain + animator + clips. */
final class NativeEngine {
    static {
        System.loadLibrary("lillith_android");
    }

    static final int TOUCH_DOWN = 0, TOUCH_UP = 1, TOUCH_MOVE = 2, TOUCH_CANCEL = 3;
    static final int REQ_OPEN_CHAT = 1, REQ_MENU = 2, REQ_QUIT = 4;

    private NativeEngine() {}

    static native long nCreate(String manifest, float density, float size, String configToml, long seed);
    static native void nDestroy(long h);
    static native void nSetScreen(long h, int w, int hgt);
    static native boolean nTick(long h, float dt);
    /** out = [width, height, left, top] in screen px. */
    static native void nGeometry(long h, int[] out);
    static native boolean nCopyPixels(long h, ByteBuffer directBuffer);
    static native boolean nTouch(long h, int phase, float screenX, float screenY);
    static native int nTakeRequests(long h);
    static native byte[] nTakeSay(long h);
    static native boolean nReact(long h, String clip);
    static native byte[] nSystemPrompt();
}
