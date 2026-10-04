// tasklens KWin bridge (KWin 6 script, plugin "tasklens-bridge").
// Sends the full window list to tasklens's WindowBridge.Update(s json) on every window event.
// The engine replaces the bus name placeholder below with its connection's unique name as a
// JSON string. No timers and no other D-Bus calls.

const BUS_NAME = __TL_BUS_NAME__;
const BRIDGE_PATH = "/io/github/oysteinkrog/Tasklens/WindowBridge";
const BRIDGE_INTERFACE = "io.github.oysteinkrog.Tasklens.WindowBridge";
const BRIDGE_METHOD = "Update";

// KWin's MaximizeMode value for a window maximized in both directions (MaximizeFull).
const MAXIMIZE_FULL = 3;

function entry(w) {
    const g = w.frameGeometry;
    return {
        internalId: w.internalId.toString(),
        pid: w.pid,
        caption: w.caption,
        resourceClass: w.resourceClass,
        desktopFileName: w.desktopFileName,
        minimized: w.minimized,
        maximized: w.maximizeMode === MAXIMIZE_FULL,
        keepAbove: w.keepAbove,
        frameGeometry: {
            x: Math.round(g.x),
            y: Math.round(g.y),
            width: Math.round(g.width),
            height: Math.round(g.height),
        },
        windowType: w.windowType,
    };
}

function send() {
    // stackingOrder is bottom to top; tasklens wants topmost first. A closed window can stay in
    // stackingOrder during its close animation, so deleted windows are left out.
    const list = workspace.stackingOrder
        .filter(function (w) { return !w.deleted; })
        .reverse()
        .map(entry);
    callDBus(BUS_NAME, BRIDGE_PATH, BRIDGE_INTERFACE, BRIDGE_METHOD, JSON.stringify(list));
}

// Geometry is sent only when a move or resize ends, never per pixel.
function hook(w) {
    w.captionChanged.connect(send);
    w.minimizedChanged.connect(send);
    w.maximizedChanged.connect(send);
    w.keepAboveChanged.connect(send);
    w.interactiveMoveResizeFinished.connect(send);
}

workspace.stackingOrder.forEach(hook);

workspace.windowAdded.connect(function (w) {
    hook(w);
    send();
});
workspace.windowRemoved.connect(send);
workspace.windowActivated.connect(send);

send();
