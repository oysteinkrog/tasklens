//! App grouping, classification, desktop entries and icons.
pub mod cgroup_app;
pub mod classify;
pub mod desktop;
pub mod icons;
pub mod launchers;
pub mod naming;

use std::ffi::OsString;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};
use smallvec::SmallVec;
use tl_model::ids::{GroupKey, ProcessKey};
use tl_model::process::{GroupKind, ProcessRecord};
use tl_model::time::TimeNs;

use crate::apps::cgroup_app::{
    AppSliceScan, AppUnit, UnitAppId, is_bwrap, parse_app_unit, scan_app_slice, unwrap_bwrap_targets,
};
use crate::apps::classify::{ClassifyInput, MatchInput, ProcClass, classify, match_app, qualifier};
use crate::apps::desktop::{DesktopIndex, DesktopWatch, data_dirs_from_env};
use crate::apps::icons::{
    GenericIcon, ICON_CACHE_SAVE_INTERVAL_S, INDEX_MAX, IconCache, ThemeDirs, generic_icon,
    kde_icon_theme, load_theme, theme_order,
};
use crate::apps::naming::exe_basename;
use crate::proc::cgroup::{CgroupRoot, app_slice_unit, unescape_unit};
use crate::proc::meta::{ExeLink, RawMeta, ReadBufs};

/// App rows draw their icon at this size, process rows at [`PROCESS_ICON_SIZE`].
const APP_ICON_SIZE: u32 = 24;
const PROCESS_ICON_SIZE: u32 = 16;

/// The directories the resolver reads. The engine fills it from the environment and
/// `tl-app/src/paths.rs`.
#[derive(Clone, Debug)]
pub struct AppPaths {
    pub home: PathBuf,
    pub xdg_data_home: Option<OsString>,
    pub xdg_data_dirs: Option<OsString>,
    pub xdg_config_home: Option<OsString>,
    /// `$XDG_CACHE_HOME/tasklens/icons.toml`.
    pub icon_cache: PathBuf,
}

/// What one process resolved to: its app, Type column class, group and icons.
#[derive(Clone, Debug)]
pub struct AppInfo {
    /// The matched desktop id. `Some` only for `ProcClass::App` rows.
    pub app_id: Option<Arc<str>>,
    pub app_name: Option<Arc<str>>,
    /// `Flatpak`, `Snap` or `Wine` (PROC-008), for app rows only.
    pub qualifier: Option<&'static str>,
    /// The desktop `Comment`, else `GenericName` (PROC-027).
    pub description: Option<Arc<str>>,
    /// The app icon at 24 px.
    pub icon_app: Option<Arc<str>>,
    /// The process icon at 16 px: the app's icon, else a generic one.
    pub icon_process: Option<Arc<str>>,
    pub desktop_path: Option<Arc<str>>,
    /// The unescaped name of the process's cgroup unit.
    pub unit: Option<Arc<str>>,
    pub class: ProcClass,
    pub group: GroupKind,
    pub group_key: GroupKey,
}

/// What `AppResolver::resolve` needs to know about one process.
pub struct ResolveInput<'a> {
    pub pid: u32,
    /// Real uid from status `Uid:`.
    pub ruid: u32,
    pub kernel_thread: bool,
    pub meta: &'a RawMeta,
    /// A KWin window maps to this pid (PROC-030, P2). Always false in P1.
    pub owns_window: bool,
    /// KWin `resourceClass` (P2). Always `None` in P1.
    pub wm_class: Option<&'a str>,
}

/// One cached result and what it was computed from.
struct Cached {
    /// The `comm` of the `RawMeta` used. `MetaCache` allocates a new `comm` on every read, so a
    /// different pointer means the metadata was refreshed.
    comm: Arc<str>,
    ruid: u32,
    owns_window: bool,
    wm_class: Option<Box<str>>,
    info: Arc<AppInfo>,
}

/// Groups, names and draws processes as apps (PROC-004 to PROC-008, ADR-016). The sweep calls
/// `begin_sweep`, then `resolve` per process, then `end_sweep`.
pub struct AppResolver {
    index: DesktopIndex,
    watch: Option<DesktopWatch>,
    themes: Vec<ThemeDirs>,
    icons: IconCache,
    scan: AppSliceScan,
    /// pid to its unit in `scan.units`.
    unit_of: FxHashMap<u32, usize>,
    cache: FxHashMap<ProcessKey, Cached>,
    bufs: ReadBufs,
    text: String,
    last_save: Option<TimeNs>,
}

impl AppResolver {
    /// Builds the desktop index, starts its watch, loads the icon themes and the icon cache.
    /// Missing directories or themes, and a failed watch, give fewer matches and no error.
    pub fn new(paths: &AppPaths) -> Self {
        let dirs = data_dirs_from_env(
            &paths.home,
            paths.xdg_data_home.as_deref(),
            paths.xdg_data_dirs.as_deref(),
        );
        let index = DesktopIndex::build(&dirs);
        let watch = match DesktopWatch::new(&dirs) {
            Ok(w) => Some(w),
            Err(e) => {
                tracing::warn!(error = %e, "desktop file watch not started; new apps need a restart");
                None
            }
        };

        let config = paths
            .xdg_config_home
            .as_deref()
            .map(Path::new)
            .filter(|p| p.is_absolute())
            .map_or_else(|| paths.home.join(".config"), Path::to_path_buf);
        let mut buf = Vec::new();
        let user_theme = File::open(config.join("kdeglobals"))
            .and_then(|f| f.take(INDEX_MAX).read_to_end(&mut buf))
            .ok()
            .and_then(|_| kde_icon_theme(&buf));
        // Each `<data dir>/applications` has its icons in `<data dir>/icons`.
        let icon_bases: Vec<PathBuf> = dirs.iter().map(|d| d.with_file_name("icons")).collect();
        let order = theme_order(user_theme.as_deref());
        let themes = order
            .iter()
            .filter_map(|name| load_theme(name, &icon_bases))
            .collect();
        let icons = IconCache::load(
            paths.icon_cache.clone(),
            order.first().map_or("", String::as_str),
        );

        Self {
            index,
            watch,
            themes,
            icons,
            scan: AppSliceScan::default(),
            unit_of: FxHashMap::default(),
            cache: FxHashMap::default(),
            bufs: ReadBufs::default(),
            text: String::new(),
            last_save: None,
        }
    }

    /// Rebuilds the desktop index when its watch says so, scans app.slice and maps its pids to
    /// their units. A new index generation clears every cached result.
    pub fn begin_sweep(&mut self, cg: &CgroupRoot, self_uid: u32, now: TimeNs) {
        let generation = self.index.generation;
        if let Some(watch) = &mut self.watch {
            self.index.rebuild_if(watch, now);
        }
        if self.index.generation != generation {
            self.cache.clear();
        }
        scan_app_slice(cg, self_uid, &mut self.bufs, &mut self.scan);
        self.unit_of.clear();
        for (i, unit) in self.scan.units.iter().enumerate() {
            for &pid in &unit.pids {
                self.unit_of.insert(pid, i);
            }
        }
    }

    /// The app units found by the last `begin_sweep`, sorted by cgroup path.
    pub fn app_units(&self) -> &[AppUnit] {
        &self.scan.units
    }

    /// The app data of one process. The result is cached per key and reused until the key's
    /// `RawMeta` is refreshed or the desktop index changes.
    pub fn resolve(
        &mut self,
        key: ProcessKey,
        inp: &ResolveInput<'_>,
        self_uid: u32,
    ) -> Arc<AppInfo> {
        if let Some(c) = self.cache.get(&key)
            && Arc::ptr_eq(&c.comm, &inp.meta.comm)
            && c.ruid == inp.ruid
            && c.owns_window == inp.owns_window
            && c.wm_class.as_deref() == inp.wm_class
        {
            return c.info.clone();
        }
        let info = Arc::new(self.compute(inp, self_uid));
        self.cache.insert(
            key,
            Cached {
                comm: inp.meta.comm.clone(),
                ruid: inp.ruid,
                owns_window: inp.owns_window,
                wm_class: inp.wm_class.map(Box::from),
                info: info.clone(),
            },
        );
        info
    }

    /// Drops the results of keys not in `live` and saves the icon cache when
    /// [`ICON_CACHE_SAVE_INTERVAL_S`] passed since the last save.
    pub fn end_sweep(&mut self, live: &FxHashSet<ProcessKey>, now: TimeNs) {
        self.cache.retain(|k, _| live.contains(k));
        let due = self.last_save.is_none_or(|last| {
            now.saturating_sub(last) >= ICON_CACHE_SAVE_INTERVAL_S * 1_000_000_000
        });
        if due {
            self.last_save = Some(now);
            if let Err(e) = self.icons.save() {
                tracing::debug!(error = %e, "icon cache not saved");
            }
        }
    }

    /// The app-level action targets for `roots`, with Flatpak `bwrap` wrappers replaced by the
    /// processes they run (PROC-060).
    pub fn unwrap_bwrap(
        &self,
        roots: &[ProcessKey],
        records: &[ProcessRecord],
    ) -> SmallVec<[ProcessKey; 4]> {
        unwrap_bwrap_targets(roots, records, |r| {
            is_bwrap(r.meta.exe.as_deref().map(exe_basename), &r.meta.comm)
        })
    }

    fn compute(&mut self, inp: &ResolveInput<'_>, self_uid: u32) -> AppInfo {
        let meta = inp.meta;
        let cgroup = meta.cgroup.as_ref();
        // The unit app: from this sweep's scan, else from the process's own cgroup path.
        let own_unit: Option<UnitAppId>;
        let unit_app = match self.unit_of.get(&inp.pid) {
            Some(&i) => Some(&self.scan.units[i].id),
            None => {
                own_unit = cgroup
                    .and_then(|c| app_slice_unit(c, self_uid))
                    .and_then(parse_app_unit);
                own_unit.as_ref()
            }
        };
        let exe = match &meta.exe {
            ExeLink::Path { path, .. } => Some(&**path),
            ExeLink::Restricted | ExeLink::None => None,
        };
        let m = MatchInput {
            unit_app,
            exe,
            argv0: meta.argv.first().map(|a| &**a),
            cmdline: meta.cmdline.as_deref(),
            wm_class: inp.wm_class,
        };
        let entry = if inp.kernel_thread {
            None
        } else {
            match_app(&m, &self.index)
        };
        let (class, group) = classify(
            &ClassifyInput {
                pid: inp.pid,
                ruid: inp.ruid,
                kernel_thread: inp.kernel_thread,
                cgroup,
                cgroup_denied: meta.cgroup_denied,
                app_id: entry.as_ref().map(|e| &e.id),
                owns_window: inp.owns_window,
            },
            self_uid,
        );
        // Only an app row carries app data; a System or Ungrouped row ignores the match.
        let entry = entry.filter(|_| class == ProcClass::App);

        let unit = cgroup
            .and_then(|c| c.unit.as_deref())
            .or_else(|| {
                let &i = self.unit_of.get(&inp.pid)?;
                self.scan.units[i].rel.rsplit('/').next()
            })
            .map(|u| {
                unescape_unit(u, &mut self.text);
                Arc::from(self.text.as_str())
            });

        let icon_name = entry.as_ref().and_then(|e| e.icon.clone());
        let icon_app = icon_name
            .as_deref()
            .and_then(|i| self.icons.lookup(i, APP_ICON_SIZE, &self.themes));
        let icon_process = match icon_name.as_deref() {
            Some(i) => self.icons.lookup(i, PROCESS_ICON_SIZE, &self.themes),
            None => {
                let kind = if inp.kernel_thread {
                    GenericIcon::KernelThread
                } else {
                    GenericIcon::Executable
                };
                self.icons
                    .lookup(generic_icon(kind), PROCESS_ICON_SIZE, &self.themes)
            }
        };

        let group_key = group.key();
        AppInfo {
            qualifier: if class == ProcClass::App {
                qualifier(&m, unit_app)
            } else {
                None
            },
            app_id: entry.as_ref().map(|e| e.id.clone()),
            app_name: entry.as_ref().map(|e| e.name.clone()),
            description: entry
                .as_ref()
                .and_then(|e| e.comment.clone().or_else(|| e.generic_name.clone())),
            desktop_path: entry.as_ref().map(|e| e.path.clone()),
            icon_app,
            icon_process,
            unit,
            class,
            group,
            group_key,
        }
    }
}

/// True when `pid` is an app pid and its parent is not: the app's primary process (PROC-008).
pub fn is_app_primary(pid: u32, ppid: u32, app_pids: &FxHashSet<u32>) -> bool {
    app_pids.contains(&pid) && !app_pids.contains(&ppid)
}
