package com.kokannadian.lillith;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.content.res.Configuration;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.PixelFormat;
import android.graphics.Rect;
import android.graphics.drawable.GradientDrawable;
import android.os.Build;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.provider.Settings;
import android.util.DisplayMetrics;
import android.util.Log;
import android.view.Choreographer;
import android.view.Gravity;
import android.view.MotionEvent;
import android.view.View;
import android.view.WindowInsets;
import android.view.WindowManager;
import android.view.WindowMetrics;
import android.widget.TextView;
import android.widget.Toast;

import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;

/**
 * Foreground service that floats Lillith over other apps
 * (TYPE_APPLICATION_OVERLAY). Each display frame it ticks the native engine,
 * blits the frame when it changed and moves the window to where the engine
 * says she is (walks, drags, gravity).
 */
public final class OverlayService extends Service implements Choreographer.FrameCallback {
    static final String ACTION_STOP = "com.kokannadian.lillith.STOP";
    static final String ACTION_SAY = "com.kokannadian.lillith.SAY";
    static final String ACTION_REACT = "com.kokannadian.lillith.REACT";
    static final String EXTRA_TEXT = "text";
    static final String EXTRA_CLIP = "clip";
    private static final String CHANNEL = "overlay";
    private static final String TAG = "Lillith";

    static volatile boolean running;

    private WindowManager wm;
    private SpriteView view;
    private WindowManager.LayoutParams lp;
    private TextView bubble;
    private WindowManager.LayoutParams bubbleLp;
    private final Handler main = new Handler(Looper.getMainLooper());
    private final Runnable hideBubble = this::hideBubble;
    private long engine;
    private long lastFrameNs;
    private final int[] geo = new int[4];
    private Bitmap bitmap;
    private ByteBuffer pixels;

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        String action = intent == null ? null : intent.getAction();
        if (ACTION_STOP.equals(action)) {
            stopSelf();
            return START_NOT_STICKY;
        }
        if (engine == 0) {
            startForegroundNow();
            if (!Settings.canDrawOverlays(this)) {
                Toast.makeText(this, "Lillith needs the 'display over other apps' permission", Toast.LENGTH_LONG).show();
                stopSelf();
                return START_NOT_STICKY;
            }
            if (!start()) {
                stopSelf();
                return START_NOT_STICKY;
            }
        }
        if (ACTION_SAY.equals(action)) {
            String text = intent.getStringExtra(EXTRA_TEXT);
            if (text != null) showBubble(text);
            // talk_loop if the set has it, else a nod.
            if (!NativeEngine.nReact(engine, "talk_loop")) NativeEngine.nReact(engine, "nod");
        } else if (ACTION_REACT.equals(action)) {
            String clip = intent.getStringExtra(EXTRA_CLIP);
            if (clip != null) NativeEngine.nReact(engine, clip);
        }
        return START_STICKY;
    }

    private void startForegroundNow() {
        NotificationManager nm = getSystemService(NotificationManager.class);
        nm.createNotificationChannel(new NotificationChannel(CHANNEL, "Lillith on screen",
                NotificationManager.IMPORTANCE_LOW));
        int pif = PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT;
        PendingIntent stop = PendingIntent.getService(this, 1,
                new Intent(this, OverlayService.class).setAction(ACTION_STOP), pif);
        PendingIntent chat = PendingIntent.getActivity(this, 2,
                new Intent(this, ChatActivity.class).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK), pif);
        PendingIntent setup = PendingIntent.getActivity(this, 3,
                new Intent(this, SetupActivity.class).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK), pif);
        Notification n = new Notification.Builder(this, CHANNEL)
                .setContentTitle("Lillith is on screen")
                .setContentText("Tap her to play, hold for the menu, drag to move her")
                .setSmallIcon(android.R.drawable.star_on)
                .setContentIntent(setup)
                .setOngoing(true)
                .addAction(new Notification.Action.Builder(null, "Chat", chat).build())
                .addAction(new Notification.Action.Builder(null, "Hide", stop).build())
                .build();
        if (Build.VERSION.SDK_INT >= 34) {
            startForeground(1, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE);
        } else {
            startForeground(1, n);
        }
    }

    private boolean start() {
        if (!AssetInstaller.installed(this)) {
            Toast.makeText(this, "Open Lillith setup first (clips not unpacked yet)", Toast.LENGTH_LONG).show();
            return false;
        }
        DisplayMetrics dm = getResources().getDisplayMetrics();
        engine = NativeEngine.nCreate(AssetInstaller.manifest(this).getAbsolutePath(), dm.density,
                Prefs.size(this), null, System.nanoTime());
        if (engine == 0) {
            Log.e(TAG, "native engine failed to load the clip set");
            Toast.makeText(this, "Lillith couldn't load her clips", Toast.LENGTH_LONG).show();
            return false;
        }
        wm = getSystemService(WindowManager.class);
        pushScreen();
        NativeEngine.nTick(engine, 0f);
        NativeEngine.nGeometry(engine, geo);
        view = new SpriteView(this);
        lp = new WindowManager.LayoutParams(geo[0], geo[1],
                WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY,
                WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE
                        | WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS
                        | WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN,
                PixelFormat.TRANSLUCENT);
        lp.gravity = Gravity.TOP | Gravity.START;
        lp.x = geo[2];
        lp.y = geo[3];
        lp.setTitle("Lillith");
        wm.addView(view, lp);
        running = true;
        lastFrameNs = 0;
        Choreographer.getInstance().postFrameCallback(this);
        return true;
    }

    /** Usable screen in px: full display minus the navigation bar at the bottom. */
    private void pushScreen() {
        int w, h;
        if (Build.VERSION.SDK_INT >= 30) {
            WindowMetrics m = wm.getCurrentWindowMetrics();
            Rect b = m.getBounds();
            android.graphics.Insets nav = m.getWindowInsets()
                    .getInsetsIgnoringVisibility(WindowInsets.Type.navigationBars());
            w = b.width();
            h = b.height() - nav.bottom;
        } else {
            DisplayMetrics dm = new DisplayMetrics();
            //noinspection deprecation
            wm.getDefaultDisplay().getMetrics(dm);
            w = dm.widthPixels;
            h = dm.heightPixels;
        }
        NativeEngine.nSetScreen(engine, w, h);
    }

    @Override
    public void onConfigurationChanged(Configuration c) {
        super.onConfigurationChanged(c);
        if (engine != 0) pushScreen();
    }

    @Override
    public void doFrame(long frameTimeNanos) {
        if (engine == 0) return;
        float dt = lastFrameNs == 0 ? 1f / 60f : (frameTimeNanos - lastFrameNs) / 1e9f;
        lastFrameNs = frameTimeNanos;
        boolean changed = NativeEngine.nTick(engine, dt);
        NativeEngine.nGeometry(engine, geo);
        if (changed) {
            if (bitmap == null || bitmap.getWidth() != geo[0] || bitmap.getHeight() != geo[1]) {
                bitmap = Bitmap.createBitmap(geo[0], geo[1], Bitmap.Config.ARGB_8888);
                bitmap.setPremultiplied(true);
                pixels = ByteBuffer.allocateDirect(geo[0] * geo[1] * 4);
            }
            pixels.rewind();
            if (NativeEngine.nCopyPixels(engine, pixels)) {
                pixels.rewind();
                bitmap.copyPixelsFromBuffer(pixels);
                view.invalidate();
            }
        }
        if (lp.x != geo[2] || lp.y != geo[3] || lp.width != geo[0] || lp.height != geo[1]) {
            lp.x = geo[2];
            lp.y = geo[3];
            lp.width = geo[0];
            lp.height = geo[1];
            wm.updateViewLayout(view, lp);
            placeBubble();
        }
        handleOutputs();
        Choreographer.getInstance().postFrameCallback(this);
    }

    private void handleOutputs() {
        byte[] say = NativeEngine.nTakeSay(engine);
        if (say != null) showBubble(new String(say, StandardCharsets.UTF_8));
        int req = NativeEngine.nTakeRequests(engine);
        if ((req & NativeEngine.REQ_MENU) != 0) {
            startActivity(new Intent(this, MenuActivity.class).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
        }
        if ((req & NativeEngine.REQ_OPEN_CHAT) != 0) {
            startActivity(new Intent(this, ChatActivity.class).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
        }
        if ((req & NativeEngine.REQ_QUIT) != 0) stopSelf();
    }

    // ── Speech bubble (its own small overlay window above her head) ──────

    private void showBubble(String text) {
        if (wm == null) return;
        if (bubble == null) {
            bubble = new TextView(this);
            bubble.setTextColor(Color.WHITE);
            bubble.setTextSize(14f);
            int pad = (int) (10 * getResources().getDisplayMetrics().density);
            bubble.setPadding(pad, pad / 2, pad, pad / 2);
            GradientDrawable bg = new GradientDrawable();
            bg.setColor(0xE0301040);
            bg.setStroke(pad / 4, 0xFFE040A0);
            bg.setCornerRadius(pad * 1.5f);
            bubble.setBackground(bg);
            bubble.setMaxWidth((int) (260 * getResources().getDisplayMetrics().density));
            bubbleLp = new WindowManager.LayoutParams(
                    WindowManager.LayoutParams.WRAP_CONTENT, WindowManager.LayoutParams.WRAP_CONTENT,
                    WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY,
                    WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE
                            | WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE
                            | WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN,
                    PixelFormat.TRANSLUCENT);
            bubbleLp.gravity = Gravity.TOP | Gravity.START;
        }
        bubble.setText(text);
        placeBubble();
        if (bubble.getParent() == null) {
            wm.addView(bubble, bubbleLp);
        } else {
            wm.updateViewLayout(bubble, bubbleLp);
        }
        main.removeCallbacks(hideBubble);
        main.postDelayed(hideBubble, Math.min(12000, 2500 + 60L * text.length()));
    }

    private void placeBubble() {
        if (bubble == null || bubbleLp == null) return;
        bubbleLp.x = Math.max(0, lp.x + lp.width / 4);
        bubbleLp.y = Math.max(0, lp.y - (int) (40 * getResources().getDisplayMetrics().density));
        if (bubble.getParent() != null) wm.updateViewLayout(bubble, bubbleLp);
    }

    private void hideBubble() {
        if (bubble != null && bubble.getParent() != null) wm.removeView(bubble);
    }

    @Override
    public void onDestroy() {
        running = false;
        Choreographer.getInstance().removeFrameCallback(this);
        main.removeCallbacks(hideBubble);
        hideBubble();
        if (view != null && view.getParent() != null) wm.removeView(view);
        if (engine != 0) {
            NativeEngine.nDestroy(engine);
            engine = 0;
        }
        super.onDestroy();
    }

    /** Draws the current frame; forwards touches (screen px) to the engine. */
    private final class SpriteView extends View {
        private boolean tracking;

        SpriteView(Context c) {
            super(c);
        }

        @Override
        protected void onDraw(Canvas canvas) {
            if (bitmap != null) canvas.drawBitmap(bitmap, 0, 0, null);
        }

        @Override
        public boolean onTouchEvent(MotionEvent e) {
            if (engine == 0) return false;
            float x = e.getRawX(), y = e.getRawY();
            switch (e.getActionMasked()) {
                case MotionEvent.ACTION_DOWN:
                    tracking = NativeEngine.nTouch(engine, NativeEngine.TOUCH_DOWN, x, y);
                    return true;
                case MotionEvent.ACTION_MOVE:
                    if (tracking) NativeEngine.nTouch(engine, NativeEngine.TOUCH_MOVE, x, y);
                    return true;
                case MotionEvent.ACTION_UP:
                    if (tracking) NativeEngine.nTouch(engine, NativeEngine.TOUCH_UP, x, y);
                    tracking = false;
                    return true;
                case MotionEvent.ACTION_CANCEL:
                    if (tracking) NativeEngine.nTouch(engine, NativeEngine.TOUCH_CANCEL, x, y);
                    tracking = false;
                    return true;
                default:
                    return true;
            }
        }
    }
}
