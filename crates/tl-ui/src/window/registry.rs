//! `WindowKey`: the identity of every secondary window (UI-FND-021, INDEX.md "Window keys",
//! R-176). E16-A2 adds `WindowRegistry` to this file.

use tl_model::ids::{DeviceIdx, GroupKey, ProcessKey};
use tl_model::table::{StartupRef, UnitRef};

/// One secondary window. Confirmations, Set Affinity, Set Priority and Send Signal have no key
/// (modal children of their parent); the low-memory window has none (own process).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum WindowKey {
    /// Process Properties, cap 8 (PROC-077).
    Properties(ProcessKey),
    /// App Properties, cap 8.
    AppProperties(GroupKey),
    /// Service Properties, cap 10.
    Service(UnitRef),
    /// Startup Properties, cap 10.
    StartupProperties(StartupRef),
    /// Connection Properties by socket cookie, cap 10.
    ConnectionProperties(u64),
    Run,
    HandleSearch,
    Columns,
    Settings,
    ThemeEditor,
    ShareTheme,
    About,
    CrashReport,
    AdditionalSetup,
    Shortcuts,
    PublicIpNotice,
    CollectorHealth,
    Smart(DeviceIdx),
    DiskInUse(DeviceIdx),
    DetachedGraph(u8),
    Mini,
    /// The main window (R-176). Never stored in the registry and has no cap class.
    Main,
}
