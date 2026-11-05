package com.kokannadian.lillith;

import android.content.Context;
import android.util.Log;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.BufferedReader;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.io.RandomAccessFile;
import java.net.HttpURLConnection;
import java.net.URL;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Properties;
import java.util.zip.ZipEntry;
import java.util.zip.ZipInputStream;

/**
 * The clips that don't fit inside the APK (an APK must stay under ~1 GB to
 * publish). android/build-apk.sh moves some tease clips into one zip on the
 * same GitHub release and writes assets/clippack/{clippack.properties,
 * files.list, pieces.list}. On Ko's tap in setup this downloads that zip
 * (or its ~50 MiB pieces) once from the release (HTTPS, api.github.com +
 * GitHub's asset CDN only), resumes and retries each piece, checks every
 * SHA-256, joins the pieces, and unpacks into filesDir/generated next to
 * the bundled clips. Until then the engine simply leaves those clips out
 * (engine.rs drop_uninstalled_clips).
 *
 * A release that still publishes a single zip (pieces=0 / empty pieces.list)
 * uses the original one-file path. Piece assets are named
 * {@code <asset>.part1}, {@code <asset>.part2}, ... — the same layout
 * tools/check-release-zip.py accepts.
 *
 * The repo is private, so this needs a GitHub token. It is passed in from a
 * masked field, kept in memory for this one download, and never written
 * anywhere (no prefs, no file, no log).
 *
 * This is the app's only network use; nothing else opens a socket.
 */
final class ClipPack {
    private static final String TAG = "lillith_clippack";
    private static final String DIR = "clippack";
    private static final String API = "https://api.github.com";
    private static final int TRIES = 8;

    interface Progress {
        void onProgress(String what, long done, long total);
    }

    /** One release piece of the clip-pack zip (join order = list order). */
    static final class Piece {
        final String name;
        final long size;
        final String sha256;

        Piece(String name, long size, String sha256) {
            this.name = name;
            this.size = size;
            this.sha256 = sha256;
        }
    }

    static final class Info {
        final String repo, tag, asset, sha256;
        final long size;
        final int clips, totalClips, pieces;

        Info(Properties p) throws IOException {
            repo = need(p, "repo");
            tag = need(p, "tag");
            asset = need(p, "asset");
            sha256 = need(p, "sha256").toLowerCase(Locale.US);
            size = Long.parseLong(need(p, "size"));
            clips = Integer.parseInt(need(p, "clips"));
            totalClips = Integer.parseInt(need(p, "total_clips"));
            String pn = p.getProperty("pieces");
            pieces = (pn == null || pn.trim().isEmpty()) ? 0 : Integer.parseInt(pn.trim());
            if (!repo.matches("[A-Za-z0-9._-]+/[A-Za-z0-9._-]+") || sha256.length() != 64) {
                throw new IOException("bad clippack.properties");
            }
            if (pieces < 0) throw new IOException("bad pieces count");
        }

        private static String need(Properties p, String k) throws IOException {
            String v = p.getProperty(k);
            if (v == null || v.trim().isEmpty()) throw new IOException("clippack.properties has no " + k);
            return v.trim();
        }
    }

    private ClipPack() {}

    /** The pack this APK expects, or null when every clip is inside the APK. */
    static Info info(Context c) {
        Properties p = new Properties();
        try (InputStream in = c.getAssets().open(DIR + "/clippack.properties")) {
            p.load(in);
            return new Info(p);
        } catch (IOException | RuntimeException e) {
            return null;
        }
    }

    private static File stampFile(Context c) {
        return new File(c.getFilesDir(), "clippack.stamp");
    }

    /** True when this APK needs no pack, or the pack is unpacked and checked. */
    static boolean installed(Context c) {
        Info i = info(c);
        if (i == null) return true;
        if (!i.sha256.equals(readSmall(stampFile(c)))) return false;
        try {
            File root = new File(c.getFilesDir(), "generated");
            for (String rel : files(c).keySet()) {
                if (!new File(root, rel).isFile()) return false;
            }
            return true;
        } catch (IOException e) {
            return false;
        }
    }

    /** files.list: path (relative to generated/) -> sha256. */
    private static HashMap<String, String> files(Context c) throws IOException {
        HashMap<String, String> out = new HashMap<>();
        try (BufferedReader r = new BufferedReader(new InputStreamReader(
                c.getAssets().open(DIR + "/files.list"), "UTF-8"))) {
            String line;
            while ((line = r.readLine()) != null) {
                line = line.trim();
                if (line.isEmpty()) continue;
                if (line.indexOf("  ") != 64) throw new IOException("bad files.list line: " + line);
                String rel = line.substring(66);
                if (!safe(rel)) throw new IOException("bad path in files.list: " + rel);
                out.put(rel, line.substring(0, 64).toLowerCase(Locale.US));
            }
        }
        if (out.isEmpty()) throw new IOException("files.list is empty");
        return out;
    }

    /**
     * pieces.list: "<sha256>  <size>  <name>" per line, join order.
     * Empty / missing list means the release still has a single {@link Info#asset}.
     */
    static List<Piece> parsePiecesList(InputStream in) throws IOException {
        ArrayList<Piece> out = new ArrayList<>();
        try (BufferedReader r = new BufferedReader(new InputStreamReader(in, "UTF-8"))) {
            String line;
            int n = 0;
            while ((line = r.readLine()) != null) {
                line = line.trim();
                if (line.isEmpty() || line.startsWith("#")) continue;
                // sha256 (64) + two spaces + size + two spaces + name
                if (line.length() < 68 || line.indexOf("  ") != 64) {
                    throw new IOException("bad pieces.list line: " + line);
                }
                String sha = line.substring(0, 64).toLowerCase(Locale.US);
                String rest = line.substring(66);
                int sp = rest.indexOf("  ");
                if (sp < 1) throw new IOException("bad pieces.list line: " + line);
                long size = Long.parseLong(rest.substring(0, sp));
                String name = rest.substring(sp + 2).trim();
                if (size <= 0 || !pieceNameOk(name)) {
                    throw new IOException("bad piece in pieces.list: " + name);
                }
                out.add(new Piece(name, size, sha));
                n++;
            }
            if (n == 0) return Collections.emptyList();
        }
        return out;
    }

    static boolean pieceNameOk(String name) {
        // <asset>.partN — no path separators, matches check-release-zip.py
        return name != null && !name.isEmpty() && !name.contains("/") && !name.contains("\\")
                && !name.contains("..") && name.matches("[A-Za-z0-9._-]+\\.part[1-9][0-9]*");
    }

    private static List<Piece> pieces(Context c, Info i) throws IOException {
        List<Piece> list;
        try (InputStream in = c.getAssets().open(DIR + "/pieces.list")) {
            list = parsePiecesList(in);
        } catch (IOException e) {
            // Older single-file APKs have no pieces.list.
            if (i.pieces == 0) return Collections.emptyList();
            throw new IOException("pieces.list required when pieces=" + i.pieces + ": " + e.getMessage());
        }
        if (i.pieces > 0 && list.size() != i.pieces) {
            throw new IOException("pieces.list has " + list.size() + " entries, clippack.properties says "
                    + i.pieces);
        }
        if (i.pieces == 0 && !list.isEmpty()) {
            throw new IOException("pieces.list is present but clippack.properties has pieces=0");
        }
        long sum = 0;
        for (Piece p : list) sum += p.size;
        if (!list.isEmpty() && sum != i.size) {
            throw new IOException("piece sizes sum to " + sum + ", pack size is " + i.size);
        }
        return list;
    }

    /** SHA-256 of a file; used by download and tests. */
    static String fileSha256(File f, boolean[] cancel) throws IOException {
        return sha256(f, cancel);
    }

    static boolean safe(String rel) {
        return !rel.isEmpty() && !rel.startsWith("/") && !rel.contains("..") && !rel.contains("\\")
                && rel.matches("[A-Za-z0-9._/-]+");
    }

    /**
     * Blocking; call off the UI thread. {@code cancel[0]} set to true stops it
     * (partial piece downloads are kept for a resume).
     */
    static void download(Context c, String token, Progress p, boolean[] cancel) throws IOException {
        Info i = info(c);
        if (i == null) return;
        if (token == null || token.trim().isEmpty()) throw new IOException("enter a GitHub token first");
        token = token.trim();
        HashMap<String, String> want = files(c);
        List<Piece> pieceList = pieces(c, i);
        File zip = new File(c.getFilesDir(), "clippack.zip.part");
        long free = c.getFilesDir().getUsableSpace();
        long need = 2 * i.size - (zip.isFile() ? zip.length() : 0) + (64L << 20);
        if (free < need) {
            throw new IOException("not enough free storage: need " + (need >> 20) + " MB, have " + (free >> 20) + " MB");
        }
        if (!(zip.isFile() && zip.length() == i.size && i.sha256.equals(sha256(zip, cancel)))) {
            if (pieceList.isEmpty()) {
                long id = assetId(i, token);
                fetch(i.asset, i.size, id, i.repo, token, zip, p, cancel,
                        "Downloading the rest of her clips…");
            } else {
                downloadPieces(c, i, pieceList, token, zip, p, cancel);
            }
            p.onProgress("Checking the download…", 0, 1);
            String got = sha256(zip, cancel);
            if (!i.sha256.equals(got)) {
                //noinspection ResultOfMethodCallIgnored
                zip.delete();
                throw new IOException("download is corrupt (sha256 " + got + ", expected " + i.sha256
                        + "); tap Download to retry");
            }
        }
        finish(c, i, zip, want, p, cancel);
    }

    /**
     * The no-network way in: the same (joined) zip, downloaded by Ko in a
     * browser and picked in setup. Copied in, then checked exactly like a
     * download. For a multi-piece release, join the pieces on a computer
     * first (or use Download in-app).
     */
    static void importFrom(Context c, InputStream in, Progress p, boolean[] cancel) throws IOException {
        Info i = info(c);
        if (i == null) return;
        HashMap<String, String> want = files(c);
        File zip = new File(c.getFilesDir(), "clippack.zip.part");
        long free = c.getFilesDir().getUsableSpace() + (zip.isFile() ? zip.length() : 0);
        long need = 2 * i.size + (64L << 20);
        if (free < need) {
            throw new IOException("not enough free storage: need " + (need >> 20) + " MB, have " + (free >> 20) + " MB");
        }
        byte[] buf = new byte[1 << 16];
        try (OutputStream os = new FileOutputStream(zip, false)) {
            long done = 0, lastUi = 0;
            int n;
            while ((n = in.read(buf)) > 0) {
                os.write(buf, 0, n);
                done += n;
                if (done > i.size) throw new IOException("that file is bigger than the clip pack (wrong file?)");
                if (done - lastUi >= (4L << 20)) {
                    lastUi = done;
                    p.onProgress("Copying the clip pack…", done, i.size);
                }
                if (cancel[0]) throw new IOException("cancelled");
            }
        }
        p.onProgress("Checking the clip pack…", 0, 1);
        String got = zip.length() == i.size ? sha256(zip, cancel) : "size " + zip.length();
        if (!i.sha256.equals(got)) {
            //noinspection ResultOfMethodCallIgnored
            zip.delete();
            throw new IOException("that isn't " + i.asset + " from release " + i.tag + " (" + got + ")");
        }
        finish(c, i, zip, want, p, cancel);
    }

    private static void finish(Context c, Info i, File zip, HashMap<String, String> want, Progress p,
            boolean[] cancel) throws IOException {
        unpack(c, zip, want, p, cancel);
        try (OutputStream os = new FileOutputStream(stampFile(c))) {
            os.write(i.sha256.getBytes("UTF-8"));
        }
        //noinspection ResultOfMethodCallIgnored
        zip.delete();
        Log.i(TAG, "clip pack installed: " + want.size() + " files, " + i.clips + " clips");
    }

    /** Download every piece (resume + SHA-256), then join into {@code zip}. */
    private static void downloadPieces(Context c, Info i, List<Piece> pieceList, String token, File zip,
            Progress p, boolean[] cancel) throws IOException {
        Map<String, Long> ids = assetIds(i, token, pieceList);
        File dir = new File(c.getFilesDir(), "clippack-pieces");
        if (!dir.isDirectory() && !dir.mkdirs()) throw new IOException("can't create " + dir);
        long doneBytes = 0;
        ArrayList<File> got = new ArrayList<>(pieceList.size());
        for (int idx = 0; idx < pieceList.size(); idx++) {
            Piece piece = pieceList.get(idx);
            Long id = ids.get(piece.name);
            if (id == null) throw new Fatal(piece.name + " is not on release " + i.tag);
            File finalPiece = new File(dir, piece.name);
            File part = new File(dir, piece.name + ".partial");
            if (finalPiece.isFile() && finalPiece.length() == piece.size
                    && piece.sha256.equals(sha256(finalPiece, cancel))) {
                // already verified on a previous run
            } else {
                if (finalPiece.exists()) {
                    //noinspection ResultOfMethodCallIgnored
                    finalPiece.delete();
                }
                // keep a partial so resume works across runs
                if (part.isFile() && part.length() > piece.size) {
                    //noinspection ResultOfMethodCallIgnored
                    part.delete();
                }
                String label = String.format(Locale.US, "Downloading clip pack piece %d/%d…",
                        idx + 1, pieceList.size());
                fetch(piece.name, piece.size, id, i.repo, token, part, p, cancel, label);
                p.onProgress(String.format(Locale.US, "Checking piece %d/%d…", idx + 1, pieceList.size()),
                        0, 1);
                String gotSha = sha256(part, cancel);
                if (!piece.sha256.equals(gotSha)) {
                    //noinspection ResultOfMethodCallIgnored
                    part.delete();
                    throw new IOException(piece.name + " is corrupt (sha256 " + gotSha + ", expected "
                            + piece.sha256 + "); tap Download to retry");
                }
                if (finalPiece.exists() && !finalPiece.delete()) {
                    throw new IOException("can't replace " + finalPiece);
                }
                if (!part.renameTo(finalPiece)) throw new IOException("can't rename " + part);
            }
            got.add(finalPiece);
            doneBytes += piece.size;
            p.onProgress(String.format(Locale.US, "Piece %d/%d ready", idx + 1, pieceList.size()),
                    doneBytes, i.size);
        }
        p.onProgress("Joining the clip pack…", 0, 1);
        joinFiles(got, zip, i.size, cancel);
        // pieces are only needed until the join verifies; free the space
        for (File f : got) {
            //noinspection ResultOfMethodCallIgnored
            f.delete();
        }
        //noinspection ResultOfMethodCallIgnored
        dir.delete();
    }

    /** Concatenate {@code parts} into {@code dest}, replacing it. */
    static void joinFiles(List<File> parts, File dest, long expectSize, boolean[] cancel)
            throws IOException {
        File tmp = new File(dest.getPath() + ".joining");
        byte[] buf = new byte[1 << 16];
        long written = 0;
        try (OutputStream os = new FileOutputStream(tmp, false)) {
            for (File part : parts) {
                try (InputStream in = new FileInputStream(part)) {
                    int n;
                    while ((n = in.read(buf)) > 0) {
                        os.write(buf, 0, n);
                        written += n;
                        if (cancel[0]) throw new IOException("cancelled");
                    }
                }
            }
        }
        if (written != expectSize) {
            //noinspection ResultOfMethodCallIgnored
            tmp.delete();
            throw new IOException("joined pack is " + written + " bytes, expected " + expectSize);
        }
        if (dest.exists() && !dest.delete()) throw new IOException("can't replace " + dest);
        if (!tmp.renameTo(dest)) throw new IOException("can't rename " + tmp);
    }

    /**
     * Release asset id map for the pieces (and size check). One releases API
     * call, retried.
     */
    static Map<String, Long> assetIds(Info i, String token, List<Piece> want) throws IOException {
        HashMap<String, Long> need = new HashMap<>();
        HashMap<String, Long> wantSize = new HashMap<>();
        for (Piece p : want) {
            need.put(p.name, null);
            wantSize.put(p.name, p.size);
        }
        IOException last = null;
        for (int t = 0; t < TRIES; t++) {
            HttpURLConnection h = open(API + "/repos/" + i.repo + "/releases/tags/" + i.tag);
            try {
                if (!token.isEmpty()) h.setRequestProperty("Authorization", "Bearer " + token);
                h.setRequestProperty("Accept", "application/vnd.github+json");
                int code = h.getResponseCode();
                if (code == 401) throw new Fatal("GitHub refused the token (401). Check it and that it can read " + i.repo + ".");
                if (code == 404) throw new Fatal("release " + i.tag + " not found in " + i.repo
                        + " (404): the token needs read access to that repo's contents");
                if (code != 200) throw new IOException("GitHub answered " + code);
                JSONArray assets = new JSONObject(readAll(h.getInputStream())).getJSONArray("assets");
                HashMap<String, Long> found = new HashMap<>();
                for (int k = 0; k < assets.length(); k++) {
                    JSONObject a = assets.getJSONObject(k);
                    String name = a.getString("name");
                    if (!need.containsKey(name)) continue;
                    long size = a.getLong("size");
                    long expect = wantSize.get(name);
                    if (size != expect) {
                        throw new Fatal("the release's " + name + " is " + size
                                + " bytes, this app expects " + expect + " (wrong APK for this release?)");
                    }
                    found.put(name, a.getLong("id"));
                }
                for (String name : need.keySet()) {
                    if (!found.containsKey(name)) throw new Fatal(name + " is not on release " + i.tag);
                }
                return found;
            } catch (Fatal f) {
                throw f;
            } catch (IOException e) {
                last = e;
            } catch (org.json.JSONException e) {
                last = new IOException("unexpected answer from GitHub: " + e.getMessage());
            } finally {
                h.disconnect();
            }
            sleep(t);
        }
        throw last;
    }

    /** Release asset id of the pack, via the releases API (the token reads the private repo). */
    static long assetId(Info i, String token) throws IOException {
        Piece one = new Piece(i.asset, i.size, i.sha256);
        Long id = assetIds(i, token, Collections.singletonList(one)).get(i.asset);
        if (id == null) throw new Fatal(i.asset + " is not on release " + i.tag);
        return id;
    }

    /**
     * Downloads (resuming) into {@code dest} until it holds {@code size} bytes.
     * {@code label} is the progress prefix (piece or whole pack).
     */
    static void fetch(String assetName, long size, long id, String repo, String token, File dest,
            Progress p, boolean[] cancel, String label) throws IOException {
        IOException last = null;
        int fails = 0;
        byte[] buf = new byte[1 << 16];
        while (true) {
            long have = dest.isFile() ? dest.length() : 0;
            if (have > size) {
                //noinspection ResultOfMethodCallIgnored
                dest.delete();
                have = 0;
            }
            if (have == size) return;
            if (cancel[0]) throw new IOException("cancelled (" + (have >> 20) + " MB kept for next time)");
            if (fails >= TRIES) throw last;
            HttpURLConnection h = null;
            try {
                // 1) the API answers with a short-lived signed CDN link (asked fresh on every retry)
                HttpURLConnection a = open(API + "/repos/" + repo + "/releases/assets/" + id);
                String loc;
                try {
                    if (!token.isEmpty()) a.setRequestProperty("Authorization", "Bearer " + token);
                    a.setRequestProperty("Accept", "application/octet-stream");
                    int code = a.getResponseCode();
                    if (code == 401 || code == 403 || code == 404) {
                        throw new Fatal("GitHub refused the download (" + code + "); check the token");
                    }
                    loc = a.getHeaderField("Location");
                    if (code / 100 != 3 || loc == null) throw new IOException("GitHub answered " + code + " without a link");
                } finally {
                    a.disconnect();
                }
                URL u = new URL(loc);
                String host = u.getHost().toLowerCase(Locale.US);
                if (!"https".equals(u.getProtocol())
                        || !(host.endsWith(".githubusercontent.com") || host.equals("github.com"))) {
                    throw new Fatal("unexpected download host " + host);
                }
                // 2) the CDN link itself carries the authorisation: the token is NOT sent there
                h = open(loc);
                if (have > 0) h.setRequestProperty("Range", "bytes=" + have + "-");
                int code = h.getResponseCode();
                boolean append;
                if (code == 206 && have > 0) {
                    append = true;
                } else if (code == 200) {
                    append = false;
                    have = 0;
                } else if (code == 416) {
                    //noinspection ResultOfMethodCallIgnored
                    dest.delete();
                    throw new IOException("resume refused (416); starting over");
                } else {
                    throw new IOException("download answered " + code);
                }
                try (InputStream in = h.getInputStream();
                     OutputStream os = new FileOutputStream(dest, append)) {
                    int n;
                    long done = have;
                    long lastUi = 0;
                    while ((n = in.read(buf)) > 0) {
                        os.write(buf, 0, n);
                        done += n;
                        if (done > size) throw new IOException(assetName + " download is bigger than expected");
                        if (done - lastUi >= (1L << 20)) {
                            lastUi = done;
                            p.onProgress(label, done, size);
                        }
                        if (cancel[0]) break;
                    }
                }
                if (dest.length() > have) fails = 0; // progress made: reset the retry budget
            } catch (Fatal f) {
                throw f;
            } catch (IOException e) {
                last = e;
                fails++;
                Log.w(TAG, "download try " + fails + " failed (" + assetName + "): " + e.getMessage());
                p.onProgress("Connection trouble, retrying (" + fails + "/" + TRIES + ")…",
                        dest.isFile() ? dest.length() : 0, size);
                sleep(fails);
            } finally {
                if (h != null) h.disconnect();
            }
        }
    }

    /** @deprecated kept name for call sites; prefer {@link #fetch}. */
    static void fetch(Info i, long id, String token, File zip, Progress p, boolean[] cancel)
            throws IOException {
        fetch(i.asset, i.size, id, i.repo, token, zip, p, cancel, "Downloading the rest of her clips…");
    }

    static void unpack(Context c, File zip, HashMap<String, String> want, Progress p, boolean[] cancel)
            throws IOException {
        File root = new File(c.getFilesDir(), "generated");
        HashMap<String, String> left = new HashMap<>(want);
        byte[] buf = new byte[1 << 16];
        int done = 0;
        try (ZipInputStream z = new ZipInputStream(new FileInputStream(zip))) {
            ZipEntry e;
            while ((e = z.getNextEntry()) != null) {
                if (cancel[0]) throw new IOException("cancelled");
                if (e.isDirectory()) continue;
                String rel = e.getName();
                String sha = left.remove(rel);
                if (sha == null || !safe(rel)) throw new IOException("unexpected file in the clip pack: " + rel);
                File out = new File(root, rel);
                //noinspection ResultOfMethodCallIgnored
                out.getParentFile().mkdirs();
                File tmp = new File(out.getPath() + ".part");
                MessageDigest md = digest();
                try (OutputStream os = new FileOutputStream(tmp)) {
                    int n;
                    while ((n = z.read(buf)) > 0) {
                        os.write(buf, 0, n);
                        md.update(buf, 0, n);
                    }
                }
                if (!sha.equals(hex(md.digest()))) {
                    //noinspection ResultOfMethodCallIgnored
                    tmp.delete();
                    throw new IOException(rel + " in the clip pack is corrupt");
                }
                if (out.exists() && !out.delete()) throw new IOException("can't replace " + out);
                if (!tmp.renameTo(out)) throw new IOException("can't rename " + tmp);
                p.onProgress("Unpacking her clips…", ++done, want.size());
            }
        }
        if (!left.isEmpty()) throw new IOException("the clip pack is missing " + left.size() + " files");
    }

    /** A failure retrying won't fix (bad token, wrong release). */
    private static final class Fatal extends IOException {
        Fatal(String m) {
            super(m);
        }
    }

    private static HttpURLConnection open(String url) throws IOException {
        HttpURLConnection h = (HttpURLConnection) new URL(url).openConnection();
        h.setInstanceFollowRedirects(false);
        h.setConnectTimeout(20_000);
        h.setReadTimeout(60_000);
        h.setUseCaches(false);
        h.setRequestProperty("User-Agent", "Lillith-Android");
        return h;
    }

    private static void sleep(int attempt) {
        try {
            Thread.sleep(Math.min(30_000L, 1000L << Math.min(attempt, 5)));
        } catch (InterruptedException ignored) {
            Thread.currentThread().interrupt();
        }
    }

    private static String readAll(InputStream in) throws IOException {
        StringBuilder s = new StringBuilder();
        try (BufferedReader r = new BufferedReader(new InputStreamReader(in, "UTF-8"))) {
            char[] b = new char[8192];
            int n;
            while ((n = r.read(b)) > 0) s.append(b, 0, n);
        }
        return s.toString();
    }

    private static String sha256(File f, boolean[] cancel) throws IOException {
        MessageDigest md = digest();
        byte[] buf = new byte[1 << 16];
        try (InputStream in = new FileInputStream(f)) {
            int n;
            while ((n = in.read(buf)) > 0) {
                md.update(buf, 0, n);
                if (cancel[0]) throw new IOException("cancelled");
            }
        }
        return hex(md.digest());
    }

    private static MessageDigest digest() {
        try {
            return MessageDigest.getInstance("SHA-256");
        } catch (NoSuchAlgorithmException e) {
            throw new IllegalStateException(e);
        }
    }

    private static String hex(byte[] b) {
        StringBuilder s = new StringBuilder(b.length * 2);
        for (byte x : b) s.append(String.format("%02x", x));
        return s.toString();
    }

    private static String readSmall(File f) {
        try (RandomAccessFile r = new RandomAccessFile(f, "r")) {
            byte[] b = new byte[(int) Math.min(128, r.length())];
            r.readFully(b);
            return new String(b, "UTF-8").trim();
        } catch (IOException e) {
            return "";
        }
    }
}
