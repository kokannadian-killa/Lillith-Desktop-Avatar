package com.kokannadian.lillith;

import android.app.Activity;
import android.content.Intent;
import android.os.Bundle;
import android.view.inputmethod.EditorInfo;
import android.widget.Button;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;

/**
 * Chat with Lily using the on-device model (llama.cpp via JNI). No server,
 * no sockets, no network: replies are generated in this process.
 */
public final class ChatActivity extends Activity {
    private static final int MAX_TURNS = 12;
    private static final List<String[]> history = new ArrayList<>();

    private TextView log;
    private ScrollView scroll;
    private EditText input;
    private Button send;
    private boolean thinking;

    @Override
    protected void onCreate(Bundle b) {
        super.onCreate(b);
        int pad = (int) (12 * getResources().getDisplayMetrics().density);
        LinearLayout col = new LinearLayout(this);
        col.setOrientation(LinearLayout.VERTICAL);
        col.setPadding(pad, pad, pad, pad);
        log = new TextView(this);
        log.setTextSize(16);
        log.setTextIsSelectable(true);
        scroll = new ScrollView(this);
        scroll.addView(log);
        col.addView(scroll, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, 0, 1f));
        LinearLayout row = new LinearLayout(this);
        input = new EditText(this);
        input.setHint("Say something to Lily…");
        input.setImeOptions(EditorInfo.IME_ACTION_SEND);
        input.setSingleLine(true);
        input.setOnEditorActionListener((v, id, e) -> {
            sendNow();
            return true;
        });
        send = new Button(this);
        send.setText("Send");
        send.setOnClickListener(v -> sendNow());
        row.addView(input, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f));
        row.addView(send);
        col.addView(row);
        setContentView(col);
        for (String[] t : history) append(t[0], t[1]);
        if (model() == null) {
            append("lily", "I don't have a mind on this phone yet. Pick a .gguf model in Lillith setup "
                    + "(it stays on your phone) and I'll talk your ear off.");
        }
    }

    private String model() {
        String p = Prefs.modelPath(this);
        return p != null && new File(p).isFile() ? p : null;
    }

    private void append(String who, String text) {
        log.append(("user".equals(who) ? "You: " : "Lily: ") + text + "\n\n");
        scroll.post(() -> scroll.fullScroll(ScrollView.FOCUS_DOWN));
    }

    private void sendNow() {
        String text = input.getText().toString().trim();
        String path = model();
        if (LillithVoice.isSafeword(text)) { // before the "thinking" guard: works mid-reply
            input.setText("");
            append("user", text);
            LillithVoice.get(this).stop("typed safeword");
            return;
        }
        if (text.isEmpty() || thinking) return;
        if (path == null) {
            append("lily", "No model picked yet. Open setup and choose a .gguf file.");
            return;
        }
        input.setText("");
        append("user", text);
        history.add(new String[]{"user", text});
        while (history.size() > MAX_TURNS) history.remove(0);
        thinking = true;
        send.setEnabled(false);
        send.setText("…");
        final long voiceTicket = LillithVoice.get(this).ticket();
        new Thread(() -> {
            LocalLlm llm = LocalLlm.get(path);
            String reply = null;
            if (llm != null) {
                List<String[]> turns = new ArrayList<>();
                turns.add(new String[]{"system", new String(NativeEngine.nSystemPrompt(), StandardCharsets.UTF_8)});
                turns.addAll(history);
                reply = llm.chat(turns, 256);
            }
            final String r = reply == null || reply.isEmpty()
                    ? (llm == null ? "That model wouldn't load. Is it a valid GGUF that fits in memory?" : "…")
                    : reply;
            runOnUiThread(() -> {
                thinking = false;
                send.setEnabled(true);
                send.setText("Send");
                append("lily", r);
                if (llm != null) {
                    history.add(new String[]{"assistant", r});
                    LillithVoice.get(this).sayIfCurrent(voiceTicket, r); // dropped if "red" came meanwhile
                    if (OverlayService.running) {
                        startService(new Intent(this, OverlayService.class)
                                .setAction(OverlayService.ACTION_SAY)
                                .putExtra(OverlayService.EXTRA_TEXT, r));
                    }
                }
            });
        }).start();
    }
}
