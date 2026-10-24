package com.kokannadian.lillith;

import android.content.Context;
import android.content.res.AssetFileDescriptor;
import android.content.res.AssetManager;
import android.util.Log;

import java.io.BufferedReader;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.ArrayList;
import java.util.Properties;

/**
 * Copies the bundled assets out of the APK into app storage once per
 * install/update, because the native code loads them by file path:
 * <ul>
 *   <li>assets/generated: the clip set (manifest.toml + sheets).</li>
 *   <li>assets/voice: Lillith's voice models (Ko's cloned voice, Pocket TTS;
 *       whisper tiny.en + silero VAD for the mic; Kokoro p1 only if the build
 *       asked for it). android/build-apk.sh writes voice/SHA256SUMS; every file
 *       listed there is copied and its SHA-256 checked, and a missing or corrupt
 *       file fails the install with its name. Nothing is ever downloaded.</li>
 * </ul>
 * Local copy only.
 */
final class AssetInstaller {
    private static final String TAG = "lillith_voice";
    private static final String VOICE = "voice";
    private static final String VOICE_SUMS = VOICE + "/SHA256SUMS";
    private static final String VOICE_PROPS = VOICE + "/voice.properties";

    interface Progress {
        void onFile(int done, int total);
    }

    private AssetInstaller() {}

    static File manifest(Context c) {
        return new File(new File(c.getFilesDir(), "generated"), "manifest.toml");
    }

    /** Where the voice models live after install (filesDir/voice/{pocket,stt,kokoro}). */
    static File voiceDir(Context c) {
        return new File(c.getFilesDir(), VOICE);
    }

    private static String stamp(Context c) {
        try {
            long t = c.getPackageManager().getPackageInfo(c.getPackageName(), 0).lastUpdateTime;
            return Long.toString(t);
        } catch (Exception e) {
            return "0";
        }
    }

    private static boolean clipsInstalled(Context c) {
        File s = new File(c.getFilesDir(), "generated.stamp");
        return manifest(c).isFile() && s.isFile() && stamp(c).equals(read(s));
    }

    /** True when this APK carries voice models (built with LILLITH_VOICE=1). */
    static boolean apkHasVoice(Context c) {
        try {
            c.getAssets().open(VOICE_SUMS).close();
            return true;
        } catch (IOException e) {
            return false;
        }
    }

    private static boolean voiceInstalled(Context c) {
        if (!apkHasVoice(c)) return true; // nothing to install
        File s = new File(c.getFilesDir(), "voice.stamp");
        return s.isFile() && stamp(c).equals(read(s));
    }

    static boolean installed(Context c) {
        return clipsInstalled(c) && voiceInstalled(c);
    }

    /**
     * True when Ko's cloned 'lillith' voice (Pocket TTS) is installed and
     * verified. The voice runtime must use the fallback (and say so) otherwise.
     */
    static boolean clonedVoiceReady(Context c) {
        File p = new File(voiceDir(c), "pocket");
        return voiceInstalled(c) && apkHasVoice(c)
                && new File(p, "voices/lillith.npy").isFile()
                && new File(p, "text_conditioner.onnx").isFile()
                && new File(p, "mimi_decoder.onnx").isFile()
                && (new File(p, "flow_lm_main.onnx").isFile()
                    || new File(p, "flow_lm_main_int8.onnx").isFile());
    }

    /** Name of the fallback voice this build uses when the clone can't run. */
    static String fallbackVoice(Context c) {
        Properties p = voiceProps(c);
        return "kokoro-p1".equals(p.getProperty("fallback"))
                ? "Kokoro p1 (stock voice, not Ko's clone)"
                : "Android system TextToSpeech (offline system voice, not Ko's clone)";
    }

    private static Properties voiceProps(Context c) {
        Properties p = new Properties();
        try (InputStream in = c.getAssets().open(VOICE_PROPS)) {
            p.load(in);
        } catch (IOException ignored) {
            // no voice in this APK: defaults
        }
        return p;
    }

    /** Blocking; call off the UI thread. */
    static void install(Context c, Progress p) throws IOException {
        if (installed(c)) {
            logVoiceStatus(c);
            return;
        }
        AssetManager am = c.getAssets();
        boolean doClips = !clipsInstalled(c);
        boolean doVoice = !voiceInstalled(c);

        ArrayList<String> clips = new ArrayList<>();
        if (doClips) {
            String[] dirs = am.list("generated");
            if (dirs == null || dirs.length == 0) throw new IOException("APK has no assets/generated");
            for (String d : dirs) {
                String[] sub = am.list("generated/" + d);
                if (sub == null || sub.length == 0) {
                    clips.add(d);
                } else {
                    for (String f : sub) clips.add(d + "/" + f);
                }
            }
        }
        // voice/SHA256SUMS lines: "<sha256>  <path relative to voice/>"
        ArrayList<String[]> voice = doVoice ? readSums(am) : new ArrayList<>();
        if (doVoice) checkVoicePresent(am, voice);
        if (doVoice) checkFreeSpace(c, am, voice);

        int total = clips.size() + voice.size();
        int done = 0;
        byte[] buf = new byte[1 << 16];
        File root = new File(c.getFilesDir(), "generated");
        for (String rel : clips) {
            copy(am, "generated/" + rel, new File(root, rel), null, buf);
            if (p != null) p.onFile(++done, total);
        }
        if (doClips) writeStamp(c, "generated.stamp");

        File vroot = voiceDir(c);
        for (String[] e : voice) {
            File out = new File(vroot, e[1]);
            if (out.isFile() && e[0].equals(sha256(out, buf))) {
                // already there from a previous install and intact
            } else {
                copy(am, VOICE + "/" + e[1], out, e[0], buf);
            }
            if (p != null) p.onFile(++done, total);
        }
        if (doVoice) {
            writeStamp(c, "voice.stamp");
            Log.i(TAG, "voice: installed " + voice.size() + " verified model files into " + vroot);
        }
        logVoiceStatus(c);
    }

    /** Log (and leave for the voice runtime / UI) which voice she will speak with. */
    private static void logVoiceStatus(Context c) {
        if (!apkHasVoice(c)) {
            Log.w(TAG, "voice: this APK was built without voice models (LILLITH_VOICE=0); fallback voice: "
                    + fallbackVoice(c));
        } else if (clonedVoiceReady(c)) {
            Log.i(TAG, "voice: cloned 'lillith' voice installed (flavor "
                    + voiceProps(c).getProperty("flavor", "?") + ")");
        } else {
            Log.w(TAG, "voice: using fallback voice " + fallbackVoice(c)
                    + " (cloned 'lillith' voice unavailable)");
        }
    }

    private static ArrayList<String[]> readSums(AssetManager am) throws IOException {
        ArrayList<String[]> out = new ArrayList<>();
        try (BufferedReader r = new BufferedReader(new InputStreamReader(am.open(VOICE_SUMS), "UTF-8"))) {
            String line;
            while ((line = r.readLine()) != null) {
                line = line.trim();
                if (line.isEmpty()) continue;
                int sp = line.indexOf("  ");
                if (sp != 64) throw new IOException("bad line in assets/" + VOICE_SUMS + ": " + line);
                String rel = line.substring(sp + 2);
                if (rel.startsWith("/") || rel.contains("..")) {
                    throw new IOException("bad path in assets/" + VOICE_SUMS + ": " + rel);
                }
                out.add(new String[] {line.substring(0, 64).toLowerCase(), rel});
            }
        }
        if (out.isEmpty()) throw new IOException("assets/" + VOICE_SUMS + " is empty");
        return out;
    }

    /** Fail up front, naming every voice model the APK is missing. */
    private static void checkVoicePresent(AssetManager am, ArrayList<String[]> voice) throws IOException {
        StringBuilder missing = new StringBuilder();
        for (String[] e : voice) {
            try {
                am.open(VOICE + "/" + e[1]).close();
            } catch (IOException ex) {
                missing.append(missing.length() == 0 ? "" : ", ").append(e[1]);
            }
        }
        if (missing.length() > 0) {
            String msg = "voice models missing from the APK: " + missing
                    + " (rebuild with android/build-apk.sh and the voice kit)";
            Log.e(TAG, "voice: " + msg);
            throw new IOException(msg);
        }
    }

    private static void checkFreeSpace(Context c, AssetManager am, ArrayList<String[]> voice) throws IOException {
        long need = 0;
        for (String[] e : voice) {
            try (AssetFileDescriptor fd = am.openFd(VOICE + "/" + e[1])) {
                need += fd.getLength(); // works because build-apk.sh stores models uncompressed
            } catch (IOException ex) {
                // compressed small file (txt/json): negligible
            }
        }
        long free = c.getFilesDir().getUsableSpace();
        long margin = 64L << 20;
        if (free < need + margin) {
            throw new IOException("not enough free storage for her voice: need "
                    + ((need + margin) >> 20) + " MB, have " + (free >> 20) + " MB");
        }
    }

    /** Copy one asset; when expectSha is set, verify it and keep the old file name only on a match. */
    private static void copy(AssetManager am, String asset, File out, String expectSha, byte[] buf)
            throws IOException {
        //noinspection ResultOfMethodCallIgnored
        out.getParentFile().mkdirs();
        File tmp = expectSha == null ? out : new File(out.getPath() + ".part");
        MessageDigest md = expectSha == null ? null : digest();
        try (InputStream in = am.open(asset);
             OutputStream os = new FileOutputStream(tmp)) {
            int n;
            while ((n = in.read(buf)) > 0) {
                os.write(buf, 0, n);
                if (md != null) md.update(buf, 0, n);
            }
        }
        if (md == null) return;
        String got = hex(md.digest());
        if (!got.equals(expectSha)) {
            //noinspection ResultOfMethodCallIgnored
            tmp.delete();
            String msg = "voice model " + asset + " is corrupt (sha256 " + got + ", expected " + expectSha + ")";
            Log.e(TAG, "voice: " + msg);
            throw new IOException(msg);
        }
        if (out.exists() && !out.delete()) throw new IOException("can't replace " + out);
        if (!tmp.renameTo(out)) throw new IOException("can't rename " + tmp + " to " + out);
    }

    private static String sha256(File f, byte[] buf) {
        MessageDigest md = digest();
        try (InputStream in = new FileInputStream(f)) {
            int n;
            while ((n = in.read(buf)) > 0) md.update(buf, 0, n);
        } catch (IOException e) {
            return "";
        }
        return hex(md.digest());
    }

    private static MessageDigest digest() {
        try {
            return MessageDigest.getInstance("SHA-256");
        } catch (NoSuchAlgorithmException e) {
            throw new IllegalStateException(e); // every Android has SHA-256
        }
    }

    private static String hex(byte[] b) {
        StringBuilder s = new StringBuilder(b.length * 2);
        for (byte x : b) s.append(String.format("%02x", x));
        return s.toString();
    }

    private static void writeStamp(Context c, String name) throws IOException {
        try (OutputStream os = new FileOutputStream(new File(c.getFilesDir(), name))) {
            os.write(stamp(c).getBytes("UTF-8"));
        }
    }

    private static String read(File f) {
        try (InputStream in = new FileInputStream(f)) {
            byte[] b = new byte[64];
            int n = in.read(b);
            return n <= 0 ? "" : new String(b, 0, n, "UTF-8");
        } catch (IOException e) {
            return "";
        }
    }
}
