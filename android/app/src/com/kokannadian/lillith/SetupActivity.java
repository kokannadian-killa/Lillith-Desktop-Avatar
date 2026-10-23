package com.kokannadian.lillith;

import android.Manifest;
import android.app.Activity;
import android.content.Intent;
import android.content.pm.ShortcutInfo;
import android.content.pm.ShortcutManager;
import android.graphics.drawable.Icon;
import android.content.pm.PackageManager;
import android.database.Cursor;
import android.graphics.Typeface;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.provider.OpenableColumns;
import android.provider.Settings;
import android.view.View;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.ProgressBar;
import android.widget.ScrollView;
import android.widget.SeekBar;
import android.widget.TextView;

import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;

/**
 * First-run setup: overlay permission, notifications, the local GGUF model
 * (side-loaded by the user, copied into app storage; nothing is downloaded),
 * her size, then start/hide the overlay. Also offers a pinned home-screen
 * shortcut (once, after her first start; plus a button) where the launcher
 * supports it.
 */
public final class SetupActivity extends Activity {
    private static final int REQ_PICK_MODEL = 10;
    private static final int REQ_NOTIFY = 11;

    private TextView overlayStatus, notifyStatus, modelStatus, sizeLabel, startStatus;
    private Button overlayBtn, notifyBtn, pickBtn, removeBtn, startBtn;
    private ProgressBar progress;
    private boolean busy;

    @Override
    protected void onCreate(Bundle b) {
        super.onCreate(b);
        float dp = getResources().getDisplayMetrics().density;
        int pad = (int) (16 * dp);
        LinearLayout col = new LinearLayout(this);
        col.setOrientation(LinearLayout.VERTICAL);
        col.setPadding(pad, pad, pad, pad);

        col.addView(title("Lillith setup", 24));
        col.addView(text("Your desktop demon, now on your phone. Three quick steps."));

        col.addView(title("1. Float over other apps", 18));
        overlayStatus = text("");
        col.addView(overlayStatus);
        overlayBtn = button("Allow 'display over other apps'", v -> startActivity(new Intent(
                Settings.ACTION_MANAGE_OVERLAY_PERMISSION, Uri.parse("package:" + getPackageName()))));
        col.addView(overlayBtn);

        col.addView(title("2. Notification", 18));
        notifyStatus = text("");
        col.addView(notifyStatus);
        notifyBtn = button("Allow notification", v -> {
            if (Build.VERSION.SDK_INT >= 33) {
                requestPermissions(new String[]{Manifest.permission.POST_NOTIFICATIONS}, REQ_NOTIFY);
            }
        });
        col.addView(notifyBtn);

        col.addView(title("3. Local chat model (optional)", 18));
        col.addView(text("Pick a .gguf model you copied onto this phone (USB, file manager). "
                + "Lily runs it with llama.cpp right here on the device. Nothing is downloaded, "
                + "and nothing leaves the phone (this app has no internet permission). "
                + "A 1-4B model at Q4 is a good fit for most phones."));
        modelStatus = text("");
        col.addView(modelStatus);
        pickBtn = button("Pick GGUF file…", v -> {
            Intent i = new Intent(Intent.ACTION_OPEN_DOCUMENT);
            i.addCategory(Intent.CATEGORY_OPENABLE);
            i.setType("*/*");
            startActivityForResult(i, REQ_PICK_MODEL);
        });
        col.addView(pickBtn);
        removeBtn = button("Remove model", v -> {
            String p = Prefs.modelPath(this);
            if (p != null) //noinspection ResultOfMethodCallIgnored
                new File(p).delete();
            Prefs.get(this).edit().remove(Prefs.KEY_MODEL).apply();
            refresh();
        });
        col.addView(removeBtn);

        col.addView(title("4. Size", 18));
        sizeLabel = text("");
        col.addView(sizeLabel);
        SeekBar size = new SeekBar(this);
        size.setMax(150); // 50% .. 200%
        size.setProgress(Math.round(Prefs.size(this) * 100) - 50);
        size.setOnSeekBarChangeListener(new SeekBar.OnSeekBarChangeListener() {
            @Override
            public void onProgressChanged(SeekBar s, int p, boolean user) {
                Prefs.get(SetupActivity.this).edit().putFloat(Prefs.KEY_SIZE, (p + 50) / 100f).apply();
                refresh();
            }

            @Override
            public void onStartTrackingTouch(SeekBar s) {}

            @Override
            public void onStopTrackingTouch(SeekBar s) {}
        });
        col.addView(size);

        col.addView(title("5. Go", 18));
        progress = new ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal);
        progress.setVisibility(View.GONE);
        col.addView(progress);
        startStatus = text("");
        col.addView(startStatus);
        startBtn = button("Start Lillith", v -> toggle());
        col.addView(startBtn);
        col.addView(button("Chat with Lily", v -> startActivity(new Intent(this, ChatActivity.class))));
        if (canPinShortcut()) {
            col.addView(button("Add Lillith to the home screen", v -> requestPinShortcut()));
        }

        ScrollView sv = new ScrollView(this);
        sv.addView(col);
        setContentView(sv);
    }

    @Override
    protected void onResume() {
        super.onResume();
        refresh();
    }

    private void refresh() {
        boolean overlay = Settings.canDrawOverlays(this);
        overlayStatus.setText(overlay ? "✓ Allowed" : "Needed so she can sit on top of your apps.");
        overlayBtn.setEnabled(!overlay);
        boolean notify = Build.VERSION.SDK_INT < 33
                || checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED;
        notifyStatus.setText(notify ? "✓ Allowed" : "Android shows a small 'Lillith is on screen' notification while she's out.");
        notifyBtn.setEnabled(!notify);
        String model = Prefs.modelPath(this);
        boolean hasModel = model != null && new File(model).isFile();
        modelStatus.setText(hasModel ? "✓ " + new File(model).getName()
                + String.format(" (%.1f GB)", new File(model).length() / 1e9) : "No model yet: she'll still play, just no chat.");
        removeBtn.setEnabled(hasModel && !busy);
        pickBtn.setEnabled(!busy);
        sizeLabel.setText(String.format("%d%% of the desktop default size", Math.round(Prefs.size(this) * 100)));
        startBtn.setText(OverlayService.running ? "Hide Lillith" : "Start Lillith");
        startBtn.setEnabled(overlay && !busy);
    }

    /** The launcher can pin shortcuts (ShortcutManager, API 26+, launcher-dependent). */
    private boolean canPinShortcut() {
        try {
            ShortcutManager sm = getSystemService(ShortcutManager.class);
            return sm != null && sm.isRequestPinShortcutSupported();
        } catch (RuntimeException e) {
            return false;
        }
    }

    /** Asks the launcher to pin a "Lillith" home-screen shortcut (the system shows its own confirm dialog). */
    private void requestPinShortcut() {
        if (!canPinShortcut()) return;
        try {
            Intent open = new Intent(this, SetupActivity.class).setAction(Intent.ACTION_MAIN);
            ShortcutInfo info = new ShortcutInfo.Builder(this, "lillith_home")
                    .setShortLabel("Lillith")
                    .setLongLabel("Lillith setup")
                    .setIcon(Icon.createWithResource(this, getApplicationInfo().icon))
                    .setIntent(open)
                    .build();
            getSystemService(ShortcutManager.class).requestPinShortcut(info, null);
        } catch (RuntimeException e) {
            // Launcher refused (or no foreground); the app-drawer icon still works.
        }
    }

    /** First run only: offer the home-screen shortcut once after she first starts. */
    private void offerPinShortcutOnce() {
        if (Prefs.get(this).getBoolean(Prefs.KEY_PIN_OFFERED, false)) return;
        Prefs.get(this).edit().putBoolean(Prefs.KEY_PIN_OFFERED, true).apply();
        requestPinShortcut();
    }

    private void toggle() {
        if (OverlayService.running) {
            startService(new Intent(this, OverlayService.class).setAction(OverlayService.ACTION_STOP));
            OverlayService.running = false;
            refresh();
            return;
        }
        busy = true;
        progress.setVisibility(View.VISIBLE);
        refresh();
        new Thread(() -> {
            String err = null;
            try {
                AssetInstaller.install(this, (done, total) -> runOnUiThread(() -> {
                    progress.setMax(total);
                    progress.setProgress(done);
                    startStatus.setText("Unpacking her clips… " + done + "/" + total);
                }));
            } catch (Exception e) {
                err = e.getMessage();
            }
            final String fe = err;
            runOnUiThread(() -> {
                busy = false;
                progress.setVisibility(View.GONE);
                if (fe != null) {
                    startStatus.setText("Couldn't unpack clips: " + fe);
                } else {
                    startStatus.setText("Tap her to play, hold for the menu, drag to move her.");
                    Prefs.get(this).edit().putBoolean(Prefs.KEY_SETUP_DONE, true).apply();
                    startForegroundService(new Intent(this, OverlayService.class));
                    OverlayService.running = true;
                    offerPinShortcutOnce();
                }
                refresh();
            });
        }).start();
    }

    @Override
    protected void onActivityResult(int req, int res, Intent data) {
        super.onActivityResult(req, res, data);
        if (req != REQ_PICK_MODEL || res != RESULT_OK || data == null || data.getData() == null) return;
        Uri uri = data.getData();
        String name = displayName(uri);
        if (name == null || !name.toLowerCase().endsWith(".gguf")) {
            modelStatus.setText("That isn't a .gguf file.");
            return;
        }
        busy = true;
        progress.setVisibility(View.VISIBLE);
        progress.setMax(1000);
        refresh();
        new Thread(() -> {
            File dir = new File(getFilesDir(), "models");
            //noinspection ResultOfMethodCallIgnored
            dir.mkdirs();
            File out = new File(dir, name.replaceAll("[^A-Za-z0-9._-]", "_"));
            long total = size(uri);
            String err = null;
            try (InputStream in = getContentResolver().openInputStream(uri);
                 OutputStream os = new FileOutputStream(out)) {
                if (in == null) throw new java.io.IOException("can't open the file");
                byte[] buf = new byte[1 << 20];
                long done = 0;
                int n;
                while ((n = in.read(buf)) > 0) {
                    os.write(buf, 0, n);
                    done += n;
                    final long d = done;
                    if (total > 0) runOnUiThread(() -> {
                        progress.setProgress((int) (d * 1000 / total));
                        modelStatus.setText(String.format("Copying model into Lillith… %.1f / %.1f GB", d / 1e9, total / 1e9));
                    });
                }
            } catch (Exception e) {
                err = e.getMessage();
                //noinspection ResultOfMethodCallIgnored
                out.delete();
            }
            final String fe = err;
            runOnUiThread(() -> {
                busy = false;
                progress.setVisibility(View.GONE);
                if (fe == null) {
                    String old = Prefs.modelPath(this);
                    if (old != null && !old.equals(out.getAbsolutePath())) //noinspection ResultOfMethodCallIgnored
                        new File(old).delete();
                    Prefs.get(this).edit().putString(Prefs.KEY_MODEL, out.getAbsolutePath()).apply();
                }
                refresh();
                if (fe != null) modelStatus.setText("Copy failed: " + fe);
            });
        }).start();
    }

    private String displayName(Uri uri) {
        try (Cursor c = getContentResolver().query(uri, null, null, null, null)) {
            if (c != null && c.moveToFirst()) {
                int i = c.getColumnIndex(OpenableColumns.DISPLAY_NAME);
                if (i >= 0) return c.getString(i);
            }
        } catch (Exception ignored) {
        }
        return uri.getLastPathSegment();
    }

    private long size(Uri uri) {
        try (Cursor c = getContentResolver().query(uri, null, null, null, null)) {
            if (c != null && c.moveToFirst()) {
                int i = c.getColumnIndex(OpenableColumns.SIZE);
                if (i >= 0) return c.getLong(i);
            }
        } catch (Exception ignored) {
        }
        return -1;
    }

    @Override
    public void onRequestPermissionsResult(int req, String[] p, int[] r) {
        refresh();
    }

    private TextView title(String s, int sp) {
        TextView t = text(s);
        t.setTextSize(sp);
        t.setTypeface(Typeface.DEFAULT_BOLD);
        t.setPadding(0, (int) (14 * getResources().getDisplayMetrics().density), 0, 4);
        return t;
    }

    private TextView text(String s) {
        TextView t = new TextView(this);
        t.setText(s);
        t.setTextSize(15);
        return t;
    }

    private Button button(String s, View.OnClickListener l) {
        Button b = new Button(this);
        b.setText(s);
        b.setAllCaps(false);
        b.setOnClickListener(l);
        return b;
    }
}
