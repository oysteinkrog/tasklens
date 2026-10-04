// tasklens one-shot window command (KWin 6 script, plugin "tasklens-cmd-<n>").
// The engine replaces the args placeholder below with a JSON object literal built by serde_json:
// { plugin, id, op, on, x, y, width, height }. Values never enter this script any other way.
// A missing window does nothing. The script always unloads itself at the end.

const args = __TL_ARGS__;

const w = workspace.windowList().find(function (w) {
    return w.internalId.toString() === args.id;
});

if (w) {
    if (args.op === "activate") {
        w.minimized = false;
        workspace.activeWindow = w;
    } else if (args.op === "close") {
        w.closeWindow();
    } else if (args.op === "keep_above") {
        w.keepAbove = args.on;
    } else if (args.op === "geometry") {
        w.frameGeometry = { x: args.x, y: args.y, width: args.width, height: args.height };
    }
}

callDBus("org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting", "unloadScript", args.plugin);
