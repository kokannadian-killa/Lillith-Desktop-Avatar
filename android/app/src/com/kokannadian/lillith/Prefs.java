package com.kokannadian.lillith;

import android.content.Context;
import android.content.SharedPreferences;

/** Setup choices (app-private SharedPreferences). */
final class Prefs {
    private static final String NAME = "lillith";
    static final String KEY_MODEL = "model_path";
    static final String KEY_SIZE = "size";
    static final String KEY_SETUP_DONE = "setup_done";

    private Prefs() {}

    static SharedPreferences get(Context c) {
        return c.getSharedPreferences(NAME, Context.MODE_PRIVATE);
    }

    static String modelPath(Context c) {
        return get(c).getString(KEY_MODEL, null);
    }

    /** Size multiplier on top of the desktop default scale (1.0 = default). */
    static float size(Context c) {
        return get(c).getFloat(KEY_SIZE, 1.0f);
    }
}
