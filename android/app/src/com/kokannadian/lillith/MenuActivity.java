package com.kokannadian.lillith;

import android.app.Activity;
import android.content.Intent;
import android.os.Bundle;
import android.widget.ArrayAdapter;
import android.widget.ListView;

/** Long-press menu (the desktop right-click menu, trimmed for a phone). */
public final class MenuActivity extends Activity {
    private static final String[] ITEMS = {"Chat with Lily", "Blow a kiss", "Wave", "Dance", "Settings", "Hide Lillith"};
    private static final String[] CLIPS = {null, "blow_kiss", "wave", "sway_dance", null, null};

    @Override
    protected void onCreate(Bundle b) {
        super.onCreate(b);
        setTitle("Lillith");
        ListView list = new ListView(this);
        list.setAdapter(new ArrayAdapter<>(this, android.R.layout.simple_list_item_1, ITEMS));
        list.setOnItemClickListener((parent, view, pos, id) -> {
            if (pos == 0) {
                startActivity(new Intent(this, ChatActivity.class));
            } else if (pos == 4) {
                startActivity(new Intent(this, SetupActivity.class));
            } else if (pos == 5) {
                startService(new Intent(this, OverlayService.class).setAction(OverlayService.ACTION_STOP));
            } else {
                startService(new Intent(this, OverlayService.class)
                        .setAction(OverlayService.ACTION_REACT)
                        .putExtra(OverlayService.EXTRA_CLIP, CLIPS[pos]));
            }
            finish();
        });
        setContentView(list);
    }
}
