// Lillith KWin bridge (KWin 6 scripting API). TODO(M5): verify on Plasma 6.
//
// Pushes window events and the cursor position to org.lillith.Bridge so
// the companion can react to windows on Wayland, where clients cannot see
// other windows or the global pointer (PLAN.md §1.3).
//
// Install:  kpackagetool6 --type=KWin/Script -i kwin/lillith-bridge
// Enable:   System Settings → Window Management → KWin Scripts → Lillith Bridge

const SERVICE = "org.lillith.Bridge";
const PATH = "/org/lillith/Bridge";
const IFACE = "org.lillith.Bridge";

function send(method, ...args) {
    callDBus(SERVICE, PATH, IFACE, method, ...args);
}

function describe(w) {
    if (!w) return ["", "", "", 0, 0, 0, 0];
    const g = w.frameGeometry;
    return [String(w.internalId), w.resourceClass || "", w.caption || "",
            Math.round(g.x), Math.round(g.y), Math.round(g.width), Math.round(g.height)];
}

function watch(w) {
    if (!w || !w.normalWindow) return;
    w.minimizedChanged.connect(() => {
        if (w.minimized) send("WindowMinimized", ...describe(w));
    });
    w.frameGeometryChanged.connect(() => send("WindowGeometry", ...describe(w)));
    w.captionChanged.connect(() => send("WindowCaption", ...describe(w)));
}

workspace.windowAdded.connect((w) => {
    if (!w.normalWindow) return;
    watch(w);
    send("WindowAdded", ...describe(w));
});

workspace.windowRemoved.connect((w) => {
    if (!w.normalWindow) return;
    send("WindowRemoved", ...describe(w));
});

workspace.windowActivated.connect((w) => {
    send("WindowActivated", ...describe(w));
});

workspace.windowList().forEach(watch);

// Cursor position for Follow mode and proximity wake (~10 Hz).
const cursorTimer = new QTimer();
cursorTimer.interval = 100;
cursorTimer.timeout.connect(() => {
    const p = workspace.cursorPos;
    send("Cursor", Math.round(p.x), Math.round(p.y));
});
cursorTimer.start();
