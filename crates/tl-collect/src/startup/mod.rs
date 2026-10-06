//! Startup entries: XDG autostart, systemd user units, Plasma scripts, impact, watches.
pub mod autostart;
pub mod impact;
pub mod plasma_scripts;
pub mod user_units;
pub mod watch;

use std::sync::Arc;

use smallvec::SmallVec;

pub use autostart::{
    DesktopEntry, MAX_DESKTOP_FILE, StartupEnv, find_on_path, locale_match_list,
    parse_desktop_entry,
};
pub use impact::{ImpactDetailRaw, ImpactRaw, UnitTiming, classify};
pub use user_units::{
    UnitFileListing, autostart_unit_name, is_generated_autostart_unit, systemd_escape,
};
pub use watch::StartupWatch;

use crate::CollectError;

/// One full scan. `units` comes from ListUnitFilesByPatterns on the user manager (E14).
/// Clears `out`, then appends XDG autostart rows, user unit rows and Plasma script rows,
/// in that order, and links desktop entries to their generated units.
///
/// The engine reads the generated unit's timing and state; this function does not check that
/// the unit exists.
pub fn scan_startup(
    env: &StartupEnv,
    units: &[UnitFileListing<'_>],
    out: &mut Vec<StartupRaw>,
) -> Result<(), CollectError> {
    out.clear();
    autostart::scan_autostart(env, out)?;
    for row in out.iter_mut().filter(|r| {
        matches!(
            r.kind,
            StartupKindRaw::AutostartUser
                | StartupKindRaw::AutostartSystem
                | StartupKindRaw::AutostartSystemOverridden
        )
    }) {
        row.generated_unit = Some(Arc::from(autostart_unit_name(&row.file_name)));
    }
    user_units::scan_user_units(units, env, out);
    plasma_scripts::scan_plasma_scripts(env, out)
}

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
