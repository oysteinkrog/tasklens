//! Startup entries: XDG autostart, systemd user units, Plasma scripts, impact, watches.
pub mod autostart;
pub mod impact;
pub mod plasma_scripts;
pub mod user_units;
pub mod watch;

use std::sync::Arc;

use smallvec::SmallVec;

/// Where a startup row comes from (SYS-020 Type).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum StartupKindRaw {
    AutostartUser,
    AutostartSystem,
    AutostartSystemOverridden,
    UserUnit,
    PlasmaPreStartup,
    PlasmaLogout,
}

/// Effective state of a startup row (SYS-020 Status).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum StartupStatusRaw {
    Enabled,
    Disabled,
    Masked,
    NotInThisDesktop,
    MissingProgram,
}

/// One startup row as read from disk, before the engine maps it to the model type.
#[derive(Clone, Debug, PartialEq)]
pub struct StartupRaw {
    pub kind: StartupKindRaw,
    /// "org.kde.foo.desktop", "foo.service", "env.sh".
    pub file_name: Arc<str>,
    /// Absolute path of the file this row was read from. This is the row identity.
    pub source_path: Arc<str>,
    /// The directory of `source_path`.
    pub location: Arc<str>,
    /// Desktop `Name`, else the file name without its extension.
    pub name: Arc<str>,
    pub icon: Option<Arc<str>>,
    /// `Exec`, `ExecStart` or the script path.
    pub command: Arc<str>,
    /// Absolute path of the command's program, found on `PATH` when not absolute.
    pub resolved_path: Option<Arc<str>>,
    pub status: StartupStatusRaw,
    pub try_exec: Option<Arc<str>>,
    pub only_show_in: SmallVec<[Arc<str>; 2]>,
    pub not_show_in: SmallVec<[Arc<str>; 2]>,
    pub kde_autostart_phase: Option<Arc<str>>,
    /// The systemd unit generated for a desktop entry (E06-A2).
    pub generated_unit: Option<Arc<str>>,
    /// User row: the system path it shadows.
    pub overrides: Option<Arc<str>>,
    /// System row shadowed by a user file.
    pub overridden_by_user: bool,
    pub tasklens_override: bool,
    /// User units only (E06-010).
    pub unit_file_state: Option<Arc<str>>,
    /// User units only: `[Service] Type=`, for impact (E06-012).
    pub service_type: Option<Arc<str>>,
    pub can_toggle: bool,
    pub can_delete: bool,
    pub can_run: bool,
    pub file_exists: bool,
}
