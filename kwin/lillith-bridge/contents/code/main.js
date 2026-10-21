// Lillith KWin bridge (KWin 6 scripting API). TODO(M5): verify on Plasma 6.
//
// Pushes window events and the cursor position to org.lillith.Bridge so
// the companion can react to windows on Wayland, where clients cannot see
// other windows or the global pointer (PLAN.md §1.3). For gravity and
// perching it also sends snapshots:
//   Stack(s): visible normal windows, bottom → top, one per line,
//             "id\tclass\tx\ty\tw\th" (frame geometry, global logical px)
//   Area(s):  "sx sy sw sh ax ay aw ah": the screen under the cursor and its
//             usable area (panels excluded), for the floor she lands on.
// Snapshots go out 60 ms after any window change and once a second, so a
// Lillith started after KWin picks them up.
//
// Install:  kpackagetool6 --type=KWin/Script -i kwin/lillith-bridge
//           (update: kpackagetool6 --type=KWin/Script -u kwin/lillith-bridge)
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

// Connects `fn` to signal `name` of `obj` if this KWin has it.
function on(obj, name, fn) {
    if (obj && obj[name] && obj[name].connect) obj[name].connect(fn);
}

function onCurrentDesktop(w) {
    if (w.onAllDesktops) return true;
    try {
        const cur = workspace.currentDesktop;
        const ds = w.desktops || [];
        for (let i = 0; i < ds.length; i++) {
            if (ds[i] === cur || (ds[i] && cur && ds[i].id === cur.id)) return true;
        }
        return ds.length === 0;
    } catch (e) {
        return true;
    }
}

// Normal, visible windows she may land on: no panels/docks, desktop,
// fullscreen, minimized/hidden windows, other desktops or her own surface.
function perchable(w) {
    if (!w || !w.normalWindow || w.deleted) return false;
    if (w.minimized || w.fullScreen || w.dock || w.desktopWindow || w.hidden === true) return false;
    if (String(w.resourceClass || "").toLowerCase() === "lillith") return false;
    return onCurrentDesktop(w);
}

function stackSnapshot() {
    const lines = [];
    const order = workspace.stackingOrder || [];
    for (let i = 0; i < order.length; i++) {
        const w = order[i];
        if (!perchable(w)) continue;
        const g = w.frameGeometry;
        lines.push([String(w.internalId), String(w.resourceClass || ""),
                    Math.round(g.x), Math.round(g.y),
                    Math.round(g.width), Math.round(g.height)].join("\t"));
    }
    return lines.join("\n");
}

function areaSnapshot() {
    let out = null;
    try {
        if (workspace.screenAt) out = workspace.screenAt(workspace.cursorPos);
    } catch (e) {}
    if (!out) out = workspace.activeScreen;
    const full = workspace.clientArea(KWin.FullScreenArea, out, workspace.currentDesktop);
    const work = workspace.clientArea(KWin.MaximizeArea, out, workspace.currentDesktop);
    return [full.x, full.y, full.width, full.height,
            work.x, work.y, work.width, work.height].map(Math.round).join(" ");
}

function sendSnapshots() {
    try {
        send("Stack", stackSnapshot());
        send("Area", areaSnapshot());
    } catch (e) {
        print("lillith-bridge: snapshot failed: " + e);
    }
}

// Debounced: geometry changes fire per frame while a window is dragged.
const snapTimer = new QTimer();
snapTimer.singleShot = true;
snapTimer.interval = 60;
snapTimer.timeout.connect(sendSnapshots);
function changed() {
    snapTimer.start();
}

const heartbeat = new QTimer();
heartbeat.interval = 1000;
heartbeat.timeout.connect(sendSnapshots);
heartbeat.start();

function watch(w) {
    if (!w) return;
    // Every window: one may become normal/visible/fullscreen later.
    on(w, "frameGeometryChanged", changed);
    on(w, "minimizedChanged", changed);
    on(w, "fullScreenChanged", changed);
    on(w, "desktopsChanged", changed);
    on(w, "stackingOrderChanged", changed);
    if (!w.normalWindow) return;
    w.minimizedChanged.connect(() => {
        if (w.minimized) send("WindowMinimized", ...describe(w));
    });
    w.frameGeometryChanged.connect(() => send("WindowGeometry", ...describe(w)));
    w.captionChanged.connect(() => send("WindowCaption", ...describe(w)));
}

workspace.windowAdded.connect((w) => {
    watch(w);
    changed();
    if (!w.normalWindow) return;
    send("WindowAdded", ...describe(w));
});

workspace.windowRemoved.connect((w) => {
    changed();
    if (!w.normalWindow) return;
    send("WindowRemoved", ...describe(w));
});

workspace.windowActivated.connect((w) => {
    changed();
    send("WindowActivated", ...describe(w));
});

on(workspace, "stackingOrderChanged", changed);
on(workspace, "currentDesktopChanged", changed);
on(workspace, "screensChanged", changed);
on(workspace, "virtualScreenGeometryChanged", changed);

workspace.windowList().forEach(watch);
sendSnapshots();
print("lillith-bridge: loaded, sending window stack + work area to " + SERVICE);

// Cursor position for Follow mode and proximity wake (~10 Hz).
const cursorTimer = new QTimer();
cursorTimer.interval = 100;
cursorTimer.timeout.connect(() => {
    const p = workspace.cursorPos;
    send("Cursor", Math.round(p.x), Math.round(p.y));
});
cursorTimer.start();
