package com.kokannadian.lillith;

import android.content.Context;
import android.content.res.AssetManager;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;

/**
 * Copies the bundled clip set (assets/generated: manifest.toml + sheets) out
 * of the APK into app storage once per install/update, because the native
 * engine loads pages by file path. ~190 MB; local copy only.
 */
final class AssetInstaller {
    interface Progress {
        void onFile(int done, int total);
    }

    private AssetInstaller() {}

    static File manifest(Context c) {
        return new File(new File(c.getFilesDir(), "generated"), "manifest.toml");
    }

    private static String stamp(Context c) {
        try {
            long t = c.getPackageManager().getPackageInfo(c.getPackageName(), 0).lastUpdateTime;
            return Long.toString(t);
        } catch (Exception e) {
            return "0";
        }
    }

    static boolean installed(Context c) {
        File s = new File(c.getFilesDir(), "generated.stamp");
        return manifest(c).isFile() && s.isFile() && stamp(c).equals(read(s));
    }

    /** Blocking; call off the UI thread. */
    static void install(Context c, Progress p) throws IOException {
        if (installed(c)) return;
        AssetManager am = c.getAssets();
        File root = new File(c.getFilesDir(), "generated");
        String[] dirs = am.list("generated");
        if (dirs == null) throw new IOException("APK has no assets/generated");
        java.util.ArrayList<String> files = new java.util.ArrayList<>();
        for (String d : dirs) {
            String[] sub = am.list("generated/" + d);
            if (sub == null || sub.length == 0) {
                files.add(d);
            } else {
                for (String f : sub) files.add(d + "/" + f);
            }
        }
        byte[] buf = new byte[1 << 16];
        for (int i = 0; i < files.size(); i++) {
            String rel = files.get(i);
            File out = new File(root, rel);
            //noinspection ResultOfMethodCallIgnored
            out.getParentFile().mkdirs();
            try (InputStream in = am.open("generated/" + rel);
                 OutputStream os = new FileOutputStream(out)) {
                int n;
                while ((n = in.read(buf)) > 0) os.write(buf, 0, n);
            }
            if (p != null) p.onFile(i + 1, files.size());
        }
        try (OutputStream os = new FileOutputStream(new File(c.getFilesDir(), "generated.stamp"))) {
            os.write(stamp(c).getBytes("UTF-8"));
        }
    }

    private static String read(File f) {
        try (InputStream in = new java.io.FileInputStream(f)) {
            byte[] b = new byte[64];
            int n = in.read(b);
            return n <= 0 ? "" : new String(b, 0, n, "UTF-8");
        } catch (IOException e) {
            return "";
        }
    }
}
