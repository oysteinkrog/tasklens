// tasklens window picker lookup (KWin 6 script, plugin "tasklens-pick").
// The engine replaces the two placeholders below: the bus name with its connection's unique name
// as a JSON string, and the args with a JSON object literal { plugin, uuid } built by serde_json.
// Calls WindowBridge.Picked(s uuid, u pid) with the window's pid, or 0 when no window has that
// uuid, then unloads itself.

const BUS_NAME = __TL_BUS_NAME__;
const args = __TL_ARGS__;

const w = workspace.windowList().find(function (w) {
    return w.internalId.toString() === args.uuid;
});

callDBus(
    BUS_NAME,
    "/io/github/oysteinkrog/Tasklens/WindowBridge",
    "io.github.oysteinkrog.Tasklens.WindowBridge",
    "Picked",
    args.uuid,
    w ? w.pid : 0
);

callDBus("org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting", "unloadScript", args.plugin);
