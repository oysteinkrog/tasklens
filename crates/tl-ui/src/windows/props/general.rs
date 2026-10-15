//! The General tab of Process Properties: three key/value column panels, the command line block,
//! the value formatters and the loading, restricted, exited and filtered looks (UI-PWN-006,
//! PROC-078, R-050, R-096, R-008, D-004, canonical table (d), UI-FND-025, R-127).
//!
//! Static fields come from `TabPayload::General(Arc<GeneralDetail>)`; live numbers from this
//! key's `ProcessRecord`. The owner feeds both with [`GeneralTab::set_payload`] and
//! [`GeneralTab::set_record`]. The tab rebuilds its three `Arc<[KvField]>` columns only when a
//! source changed, and each value string is cached by `(FieldId, value bits, validity)`, so a
//! sweep that changes only live numbers re-formats at most 25 values.
//!
//! Hashes and Verify against package (UI-PWN-008): the hex strings are formatted once per
//! `HashState`; the Verify button sits in its own row under `Package status:` and sends
//! `Command::VerifyPackage`.
//!
//! Copy and links (E23-011, UI-PWN-007): a click on a value copies it (hashes as the full hex);
//! PPID, Executable and Affinity are links. A right-click, Shift+F10 or the Menu key opens the
//! field menu: Copy value, Copy row, Copy section (one column panel) and Copy all fields. The
//! three panels are one focus stop (context `PropsWindow > KvGrid`) with a field cursor: Up and
//! Down move in a column, Left and Right to the same row of the next column, Home and End to the
//! ends, Enter runs a link or copies, Ctrl+C copies. After the process exited, links are plain
//! text and copying still works. The Unit link comes with E23-016.
//!
//! The window block (UI-PWN-009) ends column 3 under a divider: the topmost KWin window of this
//! pid from `Snapshot.windows`, fed by [`GeneralTab::set_windows`]. Captions and classes are
//! untrusted text and are shown with control characters escaped.
//!
//! The command line is untrusted text (ADR-015). It is shown with control characters escaped and
//! is never logged.

use std::cell::Cell;
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::input::Copy;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    Action, AnyElement, App, AppContext as _, BorrowAppContext as _, Bounds, ClipboardItem,
    Context, Entity, FocusHandle, FontWeight, Global, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Point, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, WeakEntity, Window, canvas, div, point, prelude::FluentBuilder as _,
    px,
};
use rustc_hash::{FxHashMap, FxHasher};
use smallvec::{SmallVec, smallvec};
use tl_model::actions::{ActionId, ActionKind, ActionRequest, ActionTarget, SchedPolicy};
use tl_model::command::{Command, Tab};
use tl_model::details::{
    ElfArch, ExeLocation, GeneralDetail, GpuEngine, HashError, HashState, IdTriple, PackageCheck,
    Relro, Seccomp, TabPayload,
};
use tl_model::ids::ProcessKey;
use tl_model::process::{ProcFlags, ProcessRecord};
use tl_model::settings::{NetworkUnit, UnitsSettings};
use tl_model::snapshot::{FrameGeometry, WindowEntry, WindowsSnapshot};
use tl_model::system::CpuSet;
use tl_model::table::Total;
use tl_model::units::{self, LocalDateTime, NetUnit};
use tl_model::validity::{ProcessField, Validity};
use tracing::warn;

use super::filter::{FilterChanged, PropsFilter, lowercase, matches};
use crate::action_tracker::{GlobalActionTracker, ToastTexts};
use crate::app::EngineGlobal;
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::icons::TlIcon;
use crate::components::kv_grid::{
    KvLayout, KvWords, TlKeyValueGrid, TlKvBlock, TlKvRow, TlKvSection, TlValue, UnsupportedWord,
    ValueTone, kv_words, shown_text_in, value_copy_form,
};
use crate::components::menu::{TlMenu, open_context_menu_at};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::hub::GlobalHub;
use crate::i18n::{Strings, tr, tr_args};
use crate::pages::performance::details::Template;
use crate::pages::processes::ProcessesPage;
use crate::pages::processes::jump::JumpTexts;
use crate::settings_global::TlSettings;
use crate::text::metrics::TextMetricsCache;
use crate::text::style::{label_style, t_sm};
use crate::theme::{UiMetrics, current, metrics};
use crate::window::main::{ShellState, main_window_view};
use crate::windows::actions::{
    KV_GRID_CONTEXT, PropsActivateField, PropsCopyAllFields, PropsCopyValue, PropsFieldDown,
    PropsFieldEnd, PropsFieldHome, PropsFieldLeft, PropsFieldMenu, PropsFieldRight, PropsFieldUp,
    PropsVerifyPackage,
};

gpui_kit::actions!(
    tasklens_general,
    [
        GeneralGoToParent,
        GeneralOpenFileLocation,
        GeneralSetAffinity,
        GeneralCopyRow,
        GeneralCopySection
    ]
);

/// The en-US tooltip of a Restricted value (R-050, P3 text with the shield sentence), for
/// tests. The tab shows `pwn-general-restricted-tooltip`.
pub const RESTRICTED_TOOLTIP: &str = "Needs administrator access. Click the shield to grant it.";

/// Body width from which the three panels sit side by side, × ui_scale.
pub const THREE_COLUMN_MIN: f32 = 840.;
/// Field row height, × ui_scale. Not affected by `row_height_pct`.
const ROW_HEIGHT: f32 = 26.;
/// Label column clamp, × ui_scale.
const LABEL_MIN: f32 = 96.;
const LABEL_MAX: f32 = 150.;
/// Command line body line height, × ui_scale.
const CMDLINE_LINE_HEIGHT: f32 = 18.;
/// Copy icon size, × ui_scale.
const COPY_ICON: f32 = 12.;
/// Space above the window block's divider, × ui_scale. The divider sits inside the panel's
/// `sp4` padding, so it is inset `sp4` on both sides.
const WINDOW_GAP: f32 = 8.;
/// Hex digits a hash value shows before "…".
const HASH_SHOWN_DIGITS: usize = 16;

const KEY_COMPUTING: &str = "pwn-props-hash-computing";
const KEY_HASH_FAILED: &str = "pwn-props-hash-failed";
const KEY_HASH_NO_EXE: &str = "pwn-props-hash-no-exe";
const KEY_NOT_VERIFIED: &str = "pwn-props-package-not-verified";
const KEY_VERIFYING: &str = "pwn-props-package-verifying";
const KEY_MATCHES: &str = "pwn-props-package-matches";
const KEY_MISMATCH: &str = "pwn-props-package-mismatch";
const KEY_NOT_PACKAGED: &str = "pwn-props-package-not-packaged";
const KEY_NO_CHECKSUM: &str = "pwn-props-package-no-checksum";
const KEY_VERIFY: &str = "pwn-props-verify-button";
const KEY_TOAST_MATCHES: &str = "pwn-props-verify-toast-matches";
const KEY_TOAST_MISMATCH: &str = "pwn-props-verify-toast-mismatch";
const KEY_TOAST_FAILED: &str = "pwn-props-verify-toast-failed";
const KEY_NOT_RUNNING: &str = "pwn-props-not-running";
const KEY_WINDOW_NONE: &str = "pwn-props-window-none";
const KEY_WINDOW_NO_BRIDGE: &str = "pwn-props-window-no-bridge";
const KEY_WINDOW_MORE: &str = "pwn-props-window-more";
/// Tab order inside the tab: the fields, the command line Copy button, then Verify. The buttons
/// set their own focus handle's index to 0, so each sits in a tab group with its index.
const TAB_FIELDS: isize = 0;
const TAB_CMDLINE_COPY: isize = 1;
const TAB_VERIFY: isize = 2;
const KEY_LINK_PARENT: &str = "pwn-general-link-parent";
const KEY_LINK_EXE: &str = "pwn-general-link-exe";
const KEY_LINK_AFFINITY: &str = "pwn-general-link-affinity";
const KEY_COPY_VALUE: &str = "pwn-general-copy-value";
const KEY_COPY_ROW: &str = "pwn-general-copy-row";
const KEY_COPY_SECTION: &str = "pwn-general-copy-section";
const KEY_COPY_ALL: &str = "pwn-general-copy-all";
const KEY_TOAST_VALUE_COPIED: &str = "pwn-general-toast-value-copied";
const KEY_TOAST_COPIED: &str = "pwn-general-toast-copied";
const KEY_TOAST_OPENING: &str = "pwn-general-toast-opening";
const KEY_TOAST_OPEN_FAILED: &str = "pwn-general-toast-open-failed";
const KEY_CMDLINE_LABEL: &str = "pwn-general-cmdline-label";
/// The marker the kernel puts after the path of a deleted executable.
const KERNEL_DELETED: &str = " (deleted)";
/// Captions the "And N more" tooltip lists at most.
const WINDOW_MORE_LISTED: usize = 10;

// ---- Words (R-132) ----

/// The hash and package texts, translated once per strings version by [`general_words`].
pub struct GeneralWords {
    /// Hash value while hashing: "Computing…".
    pub computing: SharedString,
    /// Hash tooltip after a read error; the errno text follows on a second line.
    pub hash_failed: SharedString,
    /// Hash tooltip when there is no readable executable.
    pub hash_no_exe: SharedString,
    pub not_verified: SharedString,
    pub verifying: SharedString,
    pub matches: SharedString,
    pub mismatch: SharedString,
    pub not_packaged: SharedString,
    /// Also the tooltip of the disabled Verify button.
    pub no_checksum: SharedString,
    /// The Verify button label.
    pub verify: SharedString,
    pub toast_matches: SharedString,
    pub toast_mismatch: SharedString,
    pub toast_failed: SharedString,
    /// Tooltip of the Verify button after the process exited.
    pub not_running: SharedString,
    /// `Window:` value when the process has no top-level window.
    pub window_none: SharedString,
    /// `Window:` tooltip without the KWin bridge.
    pub window_no_bridge: SharedString,
    /// "And {n} more".
    window_more: Template,
    /// Link tooltips.
    pub link_parent: SharedString,
    pub link_exe: SharedString,
    pub link_affinity: SharedString,
    /// Field menu items.
    pub copy_value: SharedString,
    pub copy_row: SharedString,
    pub copy_section: SharedString,
    pub copy_all: SharedString,
    /// "Process property value copied.": a value copy.
    pub toast_value_copied: SharedString,
    /// "Copied to clipboard": a row, section or all fields.
    pub toast_copied: SharedString,
    pub toast_opening: SharedString,
    pub toast_open_failed: SharedString,
    /// The label of the command line in Copy all fields.
    pub cmdline_label: SharedString,
    /// Field labels, in [`FieldId::ALL`] order.
    labels: Vec<SharedString>,
    /// A value not read yet: "Collecting…".
    pub collecting: SharedString,
    /// Tooltip of a Restricted value.
    pub restricted_tooltip: SharedString,
    pub cmdline_title: SharedString,
    pub cmdline_copy: SharedString,
    priority_nice: Template,
    priority_rt: Template,
    affinity_all: Template,
    affinity_one: Template,
    affinity_many: Template,
    threads_differ: SharedString,
    user_setuid: Template,
    aslr: [SharedString; 3],
    aslr_unknown: Template,
    aslr_disabled: SharedString,
    seccomp_disabled: SharedString,
    seccomp_strict: SharedString,
    seccomp_one: SharedString,
    seccomp_many: Template,
    /// PIE, NX stack, Full RELRO, Shadow stack, NoNewPrivs, Seccomp.
    hardening: [SharedString; 6],
    none: SharedString,
    net_io: Template,
    arch: [SharedString; 3],
    unknown: SharedString,
    /// In [`location_index`] order.
    location: [SharedString; 11],
    /// 3D, Compute, Video decode, Video encode.
    gpu_engine: [SharedString; 4],
    /// Minimized, Maximized, Normal.
    window_state: [SharedString; 3],
    yes: SharedString,
    no: SharedString,
    ppid_none: SharedString,
    autogroup: Template,
    nx_stack_no: SharedString,
    relro_full: SharedString,
    relro_partial: SharedString,
    enabled: SharedString,
    disabled: SharedString,
    /// "(deleted)", after a deleted executable's path.
    pub exe_deleted: SharedString,
}

impl GeneralWords {
    fn with(t: impl Fn(&str) -> SharedString, tmpl: impl Fn(&str, &[&str]) -> Template) -> Self {
        Self {
            computing: t(KEY_COMPUTING),
            hash_failed: t(KEY_HASH_FAILED),
            hash_no_exe: t(KEY_HASH_NO_EXE),
            not_verified: t(KEY_NOT_VERIFIED),
            verifying: t(KEY_VERIFYING),
            matches: t(KEY_MATCHES),
            mismatch: t(KEY_MISMATCH),
            not_packaged: t(KEY_NOT_PACKAGED),
            no_checksum: t(KEY_NO_CHECKSUM),
            verify: t(KEY_VERIFY),
            toast_matches: t(KEY_TOAST_MATCHES),
            toast_mismatch: t(KEY_TOAST_MISMATCH),
            toast_failed: t(KEY_TOAST_FAILED),
            not_running: t(KEY_NOT_RUNNING),
            window_none: t(KEY_WINDOW_NONE),
            window_no_bridge: t(KEY_WINDOW_NO_BRIDGE),
            window_more: tmpl(KEY_WINDOW_MORE, &["n"]),
            link_parent: t(KEY_LINK_PARENT),
            link_exe: t(KEY_LINK_EXE),
            link_affinity: t(KEY_LINK_AFFINITY),
            copy_value: t(KEY_COPY_VALUE),
            copy_row: t(KEY_COPY_ROW),
            copy_section: t(KEY_COPY_SECTION),
            copy_all: t(KEY_COPY_ALL),
            toast_value_copied: t(KEY_TOAST_VALUE_COPIED),
            toast_copied: t(KEY_TOAST_COPIED),
            toast_opening: t(KEY_TOAST_OPENING),
            toast_open_failed: t(KEY_TOAST_OPEN_FAILED),
            cmdline_label: t(KEY_CMDLINE_LABEL),
            labels: FieldId::ALL
                .iter()
                .map(|&id| match id {
                    FieldId::WindowMore => SharedString::default(),
                    id => t(id.label_key()),
                })
                .collect(),
            collecting: t("pwn-general-collecting"),
            restricted_tooltip: t("pwn-general-restricted-tooltip"),
            cmdline_title: t("pwn-general-cmdline-title"),
            cmdline_copy: t("pwn-general-cmdline-copy"),
            priority_nice: tmpl("pwn-general-priority-nice", &["nice", "policy", "kernel"]),
            priority_rt: tmpl("pwn-general-priority-rt", &["rt", "policy", "kernel"]),
            affinity_all: tmpl("pwn-general-affinity-all", &["cpus"]),
            affinity_one: tmpl("pwn-general-affinity-one", &["cpus"]),
            affinity_many: tmpl("pwn-general-affinity-many", &["cpus", "n"]),
            threads_differ: t("pwn-general-affinity-threads-differ"),
            user_setuid: tmpl(
                "pwn-general-user-setuid",
                &["euser", "euid", "suser", "suid"],
            ),
            aslr: [
                t("pwn-general-aslr-off"),
                t("pwn-general-aslr-partial"),
                t("pwn-general-aslr-full"),
            ],
            aslr_unknown: tmpl("pwn-general-aslr-unknown", &["n"]),
            aslr_disabled: t("pwn-general-aslr-disabled-for-process"),
            seccomp_disabled: t("pwn-general-seccomp-disabled"),
            seccomp_strict: t("pwn-general-seccomp-strict"),
            seccomp_one: t("pwn-general-seccomp-filter-one"),
            seccomp_many: tmpl("pwn-general-seccomp-filter-many", &["n"]),
            hardening: [
                t("pwn-general-hardening-pie"),
                t("pwn-general-hardening-nx-stack"),
                t("pwn-general-hardening-full-relro"),
                t("pwn-general-hardening-shadow-stack"),
                t("pwn-general-hardening-no-new-privs"),
                t("pwn-general-hardening-seccomp"),
            ],
            none: t("pwn-general-none"),
            net_io: tmpl("pwn-general-net-io", &["rx", "tx"]),
            arch: [
                t("pwn-general-arch-x86-64"),
                t("pwn-general-arch-x86"),
                t("pwn-general-arch-aarch64"),
            ],
            unknown: t("pwn-general-unknown"),
            location: [
                t("pwn-general-location-temp"),
                t("pwn-general-location-downloads"),
                t("pwn-general-location-user-data"),
                t("pwn-general-location-user-profile"),
                t("pwn-general-location-system"),
                t("pwn-general-location-optional"),
                t("pwn-general-location-flatpak"),
                t("pwn-general-location-appimage"),
                t("pwn-general-location-wine-prefix"),
                t("pwn-general-location-deleted"),
                t("pwn-general-location-other"),
            ],
            gpu_engine: [
                t("pwn-general-gpu-engine-3d"),
                t("pwn-general-gpu-engine-compute"),
                t("pwn-general-gpu-engine-video-decode"),
                t("pwn-general-gpu-engine-video-encode"),
            ],
            window_state: [
                t("pwn-general-window-minimized"),
                t("pwn-general-window-maximized"),
                t("pwn-general-window-normal"),
            ],
            yes: t("pwn-general-yes"),
            no: t("pwn-general-no"),
            ppid_none: t("pwn-general-ppid-none"),
            autogroup: tmpl("pwn-general-autogroup", &["name", "nice"]),
            nx_stack_no: t("pwn-general-nx-stack-no"),
            relro_full: t("pwn-general-relro-full"),
            relro_partial: t("pwn-general-relro-partial"),
            enabled: t("pwn-general-enabled"),
            disabled: t("pwn-general-disabled"),
            exe_deleted: t("pwn-general-exe-deleted"),
        }
    }

    /// The en-US words, for code that runs without the strings.
    pub fn en() -> Self {
        Self::with(|key| crate::i18n::tr_en(key, &[]), Template::en)
    }

    /// The label of `id`, such as "Process:". Empty for [`FieldId::WindowMore`].
    pub fn label(&self, id: FieldId) -> &SharedString {
        &self.labels[id as usize]
    }

    fn yes_no(&self, yes: bool) -> &str {
        if yes { &self.yes } else { &self.no }
    }

    /// "And {n} more".
    pub fn window_more(&self, n: usize) -> SharedString {
        let mut out = String::new();
        let _ = self.window_more.write(&mut out, &[&n]);
        out.into()
    }
}

struct GeneralWordsCache {
    version: u64,
    words: Rc<GeneralWords>,
}

impl Global for GeneralWordsCache {}

/// The texts for the current strings. Translated only when the strings version changed; without
/// strings they are in en-US.
pub fn general_words(cx: &mut App) -> Rc<GeneralWords> {
    let Some(version) = cx.try_global::<Strings>().map(|s| s.strings_version) else {
        return Rc::new(GeneralWords::en());
    };
    if let Some(cache) = cx.try_global::<GeneralWordsCache>()
        && cache.version == version
    {
        return cache.words.clone();
    }
    let words = Rc::new(GeneralWords::with(
        |key| crate::i18n::tr(cx, key),
        |key, args| Template::new(cx, key, args),
    ));
    cx.set_global(GeneralWordsCache {
        version,
        words: words.clone(),
    });
    words
}

/// What a link field opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LinkTarget {
    /// PPID: Go to parent process.
    Parent,
    /// Affinity: Set affinity.
    Affinity,
    /// Executable: Open file location.
    Executable,
    /// Unit: Go to service.
    Unit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KvKind {
    Text,
    Link(LinkTarget),
    Hash,
    Status,
}

/// One field of the General tab, in column order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FieldId {
    // Column 1.
    Process,
    Pid,
    Started,
    Ppid,
    User,
    Priority,
    Autogroup,
    Affinity,
    Hardening,
    Pie,
    NxStack,
    Relro,
    ShadowStack,
    Aslr,
    NoNewPrivs,
    Seccomp,
    Architecture,
    Executable,
    Location,
    Package,
    Description,
    WorkingDir,
    Root,
    Cgroup,
    Unit,
    AppId,
    // Column 2.
    Sha256,
    Md5,
    PackageStatus,
    Packager,
    Threads,
    Handles,
    Memory,
    WorkingSetTotal,
    WorkingSetShared,
    Pss,
    Uss,
    Swap,
    Commit,
    MemoryPct,
    CpuPct,
    CpuTime,
    IoReadBytes,
    IoWriteBytes,
    IoReadOps,
    IoWriteOps,
    // Column 3.
    DiskReadBytes,
    DiskWriteBytes,
    CancelledWriteBytes,
    DiskReadRate,
    DiskWriteRate,
    DiskTotalRate,
    NetIo,
    GpuPct,
    Vram,
    GpuMemShared,
    GpuEngine,
    // Column 3, the window block (UI-PWN-009).
    WindowTitle,
    WindowId,
    WindowState,
    WindowClass,
    WindowRect,
    WindowSize,
    /// "And N more": a plain row with no label.
    WindowMore,
    /// The one row shown instead of the six fields: no window, no bridge or collecting.
    Window,
}

impl FieldId {
    /// Every field, in declaration order: `ALL[id as usize] == id`.
    pub const ALL: [FieldId; 65] = [
        FieldId::Process,
        FieldId::Pid,
        FieldId::Started,
        FieldId::Ppid,
        FieldId::User,
        FieldId::Priority,
        FieldId::Autogroup,
        FieldId::Affinity,
        FieldId::Hardening,
        FieldId::Pie,
        FieldId::NxStack,
        FieldId::Relro,
        FieldId::ShadowStack,
        FieldId::Aslr,
        FieldId::NoNewPrivs,
        FieldId::Seccomp,
        FieldId::Architecture,
        FieldId::Executable,
        FieldId::Location,
        FieldId::Package,
        FieldId::Description,
        FieldId::WorkingDir,
        FieldId::Root,
        FieldId::Cgroup,
        FieldId::Unit,
        FieldId::AppId,
        FieldId::Sha256,
        FieldId::Md5,
        FieldId::PackageStatus,
        FieldId::Packager,
        FieldId::Threads,
        FieldId::Handles,
        FieldId::Memory,
        FieldId::WorkingSetTotal,
        FieldId::WorkingSetShared,
        FieldId::Pss,
        FieldId::Uss,
        FieldId::Swap,
        FieldId::Commit,
        FieldId::MemoryPct,
        FieldId::CpuPct,
        FieldId::CpuTime,
        FieldId::IoReadBytes,
        FieldId::IoWriteBytes,
        FieldId::IoReadOps,
        FieldId::IoWriteOps,
        FieldId::DiskReadBytes,
        FieldId::DiskWriteBytes,
        FieldId::CancelledWriteBytes,
        FieldId::DiskReadRate,
        FieldId::DiskWriteRate,
        FieldId::DiskTotalRate,
        FieldId::NetIo,
        FieldId::GpuPct,
        FieldId::Vram,
        FieldId::GpuMemShared,
        FieldId::GpuEngine,
        FieldId::WindowTitle,
        FieldId::WindowId,
        FieldId::WindowState,
        FieldId::WindowClass,
        FieldId::WindowRect,
        FieldId::WindowSize,
        FieldId::WindowMore,
        FieldId::Window,
    ];

    /// The Fluent key of the label.
    pub const fn label_key(self) -> &'static str {
        use FieldId::*;
        match self {
            Process => "pwn-general-label-process",
            Pid => "pwn-general-label-pid",
            Started => "pwn-general-label-started",
            Ppid => "pwn-general-label-ppid",
            User => "pwn-general-label-user",
            Priority => "pwn-general-label-priority",
            Autogroup => "pwn-general-label-autogroup",
            Affinity => "pwn-general-label-affinity",
            Hardening => "pwn-general-label-hardening",
            Pie => "pwn-general-label-pie",
            NxStack => "pwn-general-label-nx-stack",
            Relro => "pwn-general-label-relro",
            ShadowStack => "pwn-general-label-shadow-stack",
            Aslr => "pwn-general-label-aslr",
            NoNewPrivs => "pwn-general-label-no-new-privs",
            Seccomp => "pwn-general-label-seccomp",
            Architecture => "pwn-general-label-architecture",
            Executable => "pwn-general-label-executable",
            Location => "pwn-general-label-location",
            Package => "pwn-general-label-package",
            Description => "pwn-general-label-description",
            WorkingDir => "pwn-general-label-working-dir",
            Root => "pwn-general-label-root",
            Cgroup => "pwn-general-label-cgroup",
            Unit => "pwn-general-label-unit",
            AppId => "pwn-general-label-app-id",
            Sha256 => "pwn-general-label-sha256",
            Md5 => "pwn-general-label-md5",
            PackageStatus => "pwn-general-label-package-status",
            Packager => "pwn-general-label-packager",
            Threads => "pwn-general-label-threads",
            Handles => "pwn-general-label-handles",
            Memory => "pwn-general-label-memory",
            WorkingSetTotal => "pwn-general-label-working-set-total",
            WorkingSetShared => "pwn-general-label-working-set-shared",
            Pss => "pwn-general-label-pss",
            Uss => "pwn-general-label-uss",
            Swap => "pwn-general-label-swap",
            Commit => "pwn-general-label-commit",
            MemoryPct => "pwn-general-label-memory-pct",
            CpuPct => "pwn-general-label-cpu-pct",
            CpuTime => "pwn-general-label-cpu-time",
            IoReadBytes => "pwn-general-label-io-read-bytes",
            IoWriteBytes => "pwn-general-label-io-write-bytes",
            IoReadOps => "pwn-general-label-io-read-ops",
            IoWriteOps => "pwn-general-label-io-write-ops",
            DiskReadBytes => "pwn-general-label-disk-read-bytes",
            DiskWriteBytes => "pwn-general-label-disk-write-bytes",
            CancelledWriteBytes => "pwn-general-label-cancelled-write-bytes",
            DiskReadRate => "pwn-general-label-disk-read-rate",
            DiskWriteRate => "pwn-general-label-disk-write-rate",
            DiskTotalRate => "pwn-general-label-disk-total-rate",
            NetIo => "pwn-general-label-net-io",
            GpuPct => "pwn-general-label-gpu-pct",
            Vram => "pwn-general-label-vram",
            GpuMemShared => "pwn-general-label-gpu-mem-shared",
            GpuEngine => "pwn-general-label-gpu-engine",
            WindowTitle => "pwn-general-label-window-title",
            WindowId => "pwn-general-label-window-id",
            WindowState => "pwn-general-label-window-state",
            WindowClass => "pwn-general-label-window-class",
            WindowRect => "pwn-general-label-window-rect",
            WindowSize => "pwn-general-label-window-size",
            // No label: the row shows "And N more" only.
            WindowMore => "",
            Window => "pwn-general-label-window",
        }
    }

    /// The fields of the window block, which sits under a divider.
    pub const fn is_window(self) -> bool {
        matches!(
            self,
            FieldId::WindowTitle
                | FieldId::WindowId
                | FieldId::WindowState
                | FieldId::WindowClass
                | FieldId::WindowRect
                | FieldId::WindowSize
                | FieldId::WindowMore
                | FieldId::Window
        )
    }

    /// Path values are cut in the middle instead of at the end.
    const fn is_path(self) -> bool {
        matches!(
            self,
            FieldId::Executable | FieldId::WorkingDir | FieldId::Root | FieldId::Cgroup
        )
    }
}

/// The fields of each column, in order. Fields marked hidden in UI-PWN-006 are left out of the
/// built column when they do not apply.
pub const COLUMNS: [&[FieldId]; 3] = [
    &[
        FieldId::Process,
        FieldId::Pid,
        FieldId::Started,
        FieldId::Ppid,
        FieldId::User,
        FieldId::Priority,
        FieldId::Autogroup,
        FieldId::Affinity,
        FieldId::Hardening,
        FieldId::Pie,
        FieldId::NxStack,
        FieldId::Relro,
        FieldId::ShadowStack,
        FieldId::Aslr,
        FieldId::NoNewPrivs,
        FieldId::Seccomp,
        FieldId::Architecture,
        FieldId::Executable,
        FieldId::Location,
        FieldId::Package,
        FieldId::Description,
        FieldId::WorkingDir,
        FieldId::Root,
        FieldId::Cgroup,
        FieldId::Unit,
        FieldId::AppId,
    ],
    &[
        FieldId::Sha256,
        FieldId::Md5,
        FieldId::PackageStatus,
        FieldId::Packager,
        FieldId::Threads,
        FieldId::Handles,
        FieldId::Memory,
        FieldId::WorkingSetTotal,
        FieldId::WorkingSetShared,
        FieldId::Pss,
        FieldId::Uss,
        FieldId::Swap,
        FieldId::Commit,
        FieldId::MemoryPct,
        FieldId::CpuPct,
        FieldId::CpuTime,
        FieldId::IoReadBytes,
        FieldId::IoWriteBytes,
        FieldId::IoReadOps,
        FieldId::IoWriteOps,
    ],
    &[
        FieldId::DiskReadBytes,
        FieldId::DiskWriteBytes,
        FieldId::CancelledWriteBytes,
        FieldId::DiskReadRate,
        FieldId::DiskWriteRate,
        FieldId::DiskTotalRate,
        FieldId::NetIo,
        FieldId::GpuPct,
        FieldId::Vram,
        FieldId::GpuMemShared,
        FieldId::GpuEngine,
        FieldId::WindowTitle,
        FieldId::WindowId,
        FieldId::WindowState,
        FieldId::WindowClass,
        FieldId::WindowRect,
        FieldId::WindowSize,
        FieldId::WindowMore,
        FieldId::Window,
    ],
];

/// One built field. `value` is the shown text: the formatted value, or the R-050 word.
#[derive(Clone, Debug, PartialEq)]
pub struct KvField {
    pub id: FieldId,
    pub label: SharedString,
    pub value: SharedString,
    pub kind: KvKind,
    pub validity: Validity,
    /// The untruncated text where the value shows a shortened form (hashes).
    pub full: Option<SharedString>,
    /// Why an Unsupported value has none.
    pub word: Option<UnsupportedWord>,
    /// The value tooltip: the full hash, or why the hashes failed.
    pub tooltip: Option<SharedString>,
    /// Status colour of a Valid value; the text always says the same in words.
    pub tone: Option<ValueTone>,
}

// ---- Formatters (pure) ----

/// `Nice 0, SCHED_OTHER, kernel prio 120`; realtime `RT 50, SCHED_FIFO, kernel prio 49`.
pub fn fmt_priority(
    out: &mut String,
    w: &GeneralWords,
    policy: SchedPolicy,
    nice: i8,
    rt_prio: u8,
    kernel: i32,
) {
    let name = match policy {
        SchedPolicy::Other => "SCHED_OTHER",
        SchedPolicy::Batch => "SCHED_BATCH",
        SchedPolicy::Idle => "SCHED_IDLE",
        SchedPolicy::Fifo => "SCHED_FIFO",
        SchedPolicy::Rr => "SCHED_RR",
        SchedPolicy::Deadline => "SCHED_DEADLINE",
    };
    let _ = match policy {
        SchedPolicy::Fifo | SchedPolicy::Rr => {
            w.priority_rt.write(out, &[&rt_prio, &name, &kernel])
        }
        _ => w.priority_nice.write(out, &[&nice, &name, &kernel]),
    };
}

/// CPU ids as ranges: "0-7,16-23", "3", "0,2".
fn write_ranges(out: &mut String, set: &CpuSet) {
    let mut run: Option<(u16, u16)> = None;
    let mut first = true;
    let mut flush = |out: &mut String, (a, b): (u16, u16)| {
        if !first {
            out.push(',');
        }
        first = false;
        let _ = if a == b {
            write!(out, "{a}")
        } else {
            write!(out, "{a}-{b}")
        };
    };
    for cpu in set.iter() {
        run = match run {
            Some((a, b)) if cpu == b + 1 => Some((a, cpu)),
            Some(r) => {
                flush(out, r);
                Some((cpu, cpu))
            }
            None => Some((cpu, cpu)),
        };
    }
    if let Some(r) = run {
        flush(out, r);
    }
}

/// `All (0-31)` when `set` holds every online CPU, else `0-7,16-23 (16 CPUs)`; plus
/// ` (threads differ)`. An empty `online` set (not known yet) never reads as All.
pub fn fmt_affinity(
    out: &mut String,
    w: &GeneralWords,
    set: &CpuSet,
    online: &CpuSet,
    threads_differ: bool,
) {
    if !online.is_empty() && set.intersection(online) == *online {
        w.affinity_all
            .write_each(out, |_, out| write_ranges(out, online));
    } else {
        let n = set.count();
        let template = if n == 1 {
            &w.affinity_one
        } else {
            &w.affinity_many
        };
        template.write_each(out, |i, out| match i {
            0 => write_ranges(out, set),
            _ => {
                let _ = write!(out, "{n}");
            }
        });
    }
    if threads_differ {
        out.push(' ');
        out.push_str(&w.threads_differ);
    }
}

/// `oystein (1000)`; when the effective or saved uid differs from the real one:
/// `oystein (1000), euid root (0), suid root (0)`. `names` are the real, effective and saved
/// user names.
pub fn fmt_user(out: &mut String, w: &GeneralWords, ids: &IdTriple, names: (&str, &str, &str)) {
    let (real, effective, saved) = names;
    let _ = write!(out, "{real} ({})", ids.real);
    if ids.effective != ids.real || ids.saved != ids.real {
        out.push_str(", ");
        let _ = w
            .user_setuid
            .write(out, &[&effective, &ids.effective, &saved, &ids.saved]);
    }
}

/// `Full (2)`, `Partial (1)` or `Off (0)` from randomize_va_space, plus
/// `, disabled for this process` with ADDR_NO_RANDOMIZE.
pub fn fmt_aslr(out: &mut String, w: &GeneralWords, system: u8, disabled_for_process: bool) {
    match w.aslr.get(usize::from(system)) {
        Some(word) => out.push_str(word),
        None => {
            let _ = w.aslr_unknown.write(out, &[&system]);
        }
    }
    if disabled_for_process {
        out.push_str(", ");
        out.push_str(&w.aslr_disabled);
    }
}

/// `Disabled`, `Strict` or `Filter (3 filters)`.
pub fn fmt_seccomp(out: &mut String, w: &GeneralWords, seccomp: Seccomp) {
    match seccomp {
        Seccomp::Disabled => out.push_str(&w.seccomp_disabled),
        Seccomp::Strict => out.push_str(&w.seccomp_strict),
        Seccomp::Filter { filters: 1 } => out.push_str(&w.seccomp_one),
        Seccomp::Filter { filters } => {
            let _ = w.seccomp_many.write(out, &[&filters]);
        }
    }
}

/// The hardening items that apply.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Hardening {
    pub pie: bool,
    pub nx_stack: bool,
    pub full_relro: bool,
    pub shadow_stack: bool,
    pub no_new_privs: bool,
    pub seccomp: bool,
}

/// The enabled items in this order, comma separated:
/// `PIE, NX stack, Full RELRO, Shadow stack, NoNewPrivs, Seccomp`; `None` when none apply.
pub fn fmt_hardening(out: &mut String, w: &GeneralWords, h: Hardening) {
    let on = [
        h.pie,
        h.nx_stack,
        h.full_relro,
        h.shadow_stack,
        h.no_new_privs,
        h.seccomp,
    ];
    let items = on.into_iter().zip(w.hardening.iter());
    let mut any = false;
    for (on, name) in items {
        if on {
            if any {
                out.push_str(", ");
            }
            out.push_str(name);
            any = true;
        }
    }
    if !any {
        out.push_str(&w.none);
    }
}

/// `Receive 1.2 Mbps, send 0.3 Mbps` in the `units.network` unit. Rates in bytes per second.
pub fn fmt_net_io(
    out: &mut String,
    w: &GeneralWords,
    rx_bps: f64,
    tx_bps: f64,
    unit: NetUnit,
    auto_scale: bool,
) {
    w.net_io.write_each(out, |i, out| {
        let bps = if i == 0 { rx_bps } else { tx_bps };
        units::net_rate(out, bps, unit, auto_scale);
    });
}

fn arch_word(w: &GeneralWords, arch: ElfArch) -> &str {
    match arch {
        ElfArch::X86_64 => &w.arch[0],
        ElfArch::X86 => &w.arch[1],
        ElfArch::AArch64 => &w.arch[2],
        ElfArch::Unknown => &w.unknown,
    }
}

/// The position of `location`'s word in [`GeneralWords`].
fn location_index(location: ExeLocation) -> usize {
    match location {
        ExeLocation::TempDirectory => 0,
        ExeLocation::UserDownloads => 1,
        ExeLocation::UserData => 2,
        ExeLocation::UserProfile => 3,
        ExeLocation::System => 4,
        ExeLocation::OptionalSoftware => 5,
        ExeLocation::Flatpak => 6,
        ExeLocation::AppImage => 7,
        ExeLocation::WinePrefix => 8,
        ExeLocation::Deleted => 9,
        ExeLocation::Other => 10,
    }
}

fn location_word(w: &GeneralWords, location: ExeLocation) -> &str {
    &w.location[location_index(location)]
}

fn gpu_engine_word(w: &GeneralWords, engine: Option<GpuEngine>) -> &str {
    match engine {
        Some(GpuEngine::ThreeD) => &w.gpu_engine[0],
        Some(GpuEngine::Compute) => &w.gpu_engine[1],
        Some(GpuEngine::VideoDecode) => &w.gpu_engine[2],
        Some(GpuEngine::VideoEncode) => &w.gpu_engine[3],
        None => &w.none,
    }
}

/// `L:0 T:32 R:1280 B:832`: the frame geometry's edges in logical px.
pub fn fmt_window_rect(out: &mut String, g: FrameGeometry) {
    let right = i64::from(g.x) + i64::from(g.width);
    let bottom = i64::from(g.y) + i64::from(g.height);
    let _ = write!(out, "L:{} T:{} R:{right} B:{bottom}", g.x, g.y);
}

/// `1280 x 800`.
pub fn fmt_window_size(out: &mut String, g: FrameGeometry) {
    let _ = write!(out, "{} x {}", g.width, g.height);
}

fn window_state_word<'a>(words: &'a GeneralWords, w: &WindowEntry) -> &'a str {
    if w.minimized {
        &words.window_state[0]
    } else if w.maximized {
        &words.window_state[1]
    } else {
        &words.window_state[2]
    }
}

/// Characters the untrusted-text rules never show raw (UI-FND-053).
fn is_control(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{200B}'
                | '\u{200C}'
                | '\u{200E}'
                | '\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
                | '\u{2028}'
                | '\u{2029}'
        )
}

/// Appends `text` with each control character written as `\u{XXXX}`.
pub fn escape_controls(out: &mut String, text: &str) {
    for c in text.chars() {
        if is_control(c) {
            let _ = write!(out, "\\u{{{:04X}}}", u32::from(c));
        } else {
            out.push(c);
        }
    }
}

/// "YYYY-MM-DD HH:MM:SS" in the system time zone.
fn write_local(out: &mut String, w: &GeneralWords, unix_s: i64) {
    match jiff::Timestamp::from_second(unix_s) {
        Ok(ts) => {
            let z = ts.to_zoned(jiff::tz::TimeZone::system());
            units::date_time(
                out,
                LocalDateTime {
                    year: i32::from(z.year()),
                    month: z.month() as u8,
                    day: z.day() as u8,
                    hour: z.hour() as u8,
                    minute: z.minute() as u8,
                    second: z.second() as u8,
                },
            );
        }
        Err(_) => out.push_str(&w.unknown),
    }
}

/// Lowercase hex of `bytes`.
pub fn hex(out: &mut String, bytes: &[u8]) {
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
}

/// The shown form of a full hex hash: its first 16 digits plus "…".
pub fn short_hash(full: &str) -> String {
    let cut = full.len().min(HASH_SHOWN_DIGITS);
    let mut out = String::with_capacity(cut + '…'.len_utf8());
    out.push_str(&full[..cut]);
    out.push('…');
    out
}

/// The hash fields' texts for one `HashState`, built once when the state or the words change.
struct HashView {
    state: HashState,
    words: Rc<GeneralWords>,
    /// SHA-256 then MD5: the shown text and the full hex (Done only).
    shown: [SharedString; 2],
    full: [Option<SharedString>; 2],
    /// Why the hashes failed (Failed only).
    error: Option<SharedString>,
}

impl HashView {
    fn new(state: &HashState, words: Rc<GeneralWords>) -> Self {
        let mut view = Self {
            state: state.clone(),
            shown: [words.computing.clone(), words.computing.clone()],
            full: [None, None],
            error: None,
            words,
        };
        match state {
            HashState::Pending => {}
            HashState::Done { sha256, md5 } => {
                for (i, bytes) in [&sha256[..], &md5[..]].into_iter().enumerate() {
                    let mut full = String::with_capacity(bytes.len() * 2);
                    hex(&mut full, bytes);
                    view.shown[i] = short_hash(&full).into();
                    view.full[i] = Some(full.into());
                }
            }
            HashState::Failed(HashError::ReadFailed { errno }) => {
                let reason = std::io::Error::from_raw_os_error(*errno);
                view.error = Some(format!("{}\n{reason}", view.words.hash_failed).into());
            }
            HashState::Failed(HashError::ExePathUnavailable) => {
                view.error = Some(view.words.hash_no_exe.clone());
            }
        }
        view
    }

    fn is_for(&self, state: &HashState, words: &Rc<GeneralWords>) -> bool {
        self.state == *state && Rc::ptr_eq(&self.words, words)
    }
}

/// What the Verify against package button shows. `None` from [`GeneralTab::verify_button`]
/// hides it.
#[derive(Clone, Debug, PartialEq)]
pub struct VerifyButton {
    /// "Verify against package", or "Verifying…" while loading.
    pub label: SharedString,
    pub loading: bool,
    pub disabled: bool,
    /// Why the button is disabled.
    pub tooltip: Option<SharedString>,
}

/// The package check the status shows: a pressed button reads as Verifying until the payload
/// moves on, and a payload without a package reads as Not packaged.
fn shown_check(d: &GeneralDetail, requested: bool) -> PackageCheck {
    if d.package.validity.is_valid() && d.package.value.is_none() {
        return PackageCheck::NotPackaged;
    }
    match d.package_check {
        PackageCheck::NotVerified if requested => PackageCheck::Verifying,
        check => check,
    }
}

fn hash_of(value: impl Hash) -> u64 {
    let mut h = FxHasher::default();
    value.hash(&mut h);
    h.finish()
}

fn ptr_bits(text: &Arc<str>) -> u64 {
    Arc::as_ptr(text).cast::<u8>() as usize as u64
}

/// The first non-valid of two validities, else Valid.
fn worst(a: Validity, b: Validity) -> Validity {
    if a.is_valid() { b } else { a }
}

/// This pid's entries of `Snapshot.windows`, topmost first, and the "And N more" texts, built
/// once per change of the entries or the words.
struct WindowsView {
    validity: Validity,
    entries: Vec<WindowEntry>,
    words: Rc<GeneralWords>,
    /// "And N more" and its tooltip, when there is more than one window.
    more: Option<(SharedString, SharedString)>,
}

impl WindowsView {
    fn new(validity: Validity, entries: Vec<WindowEntry>, words: Rc<GeneralWords>) -> Self {
        let more = entries.split_first().and_then(|(_, rest)| {
            if rest.is_empty() {
                return None;
            }
            let mut tip = String::new();
            for (i, w) in rest.iter().take(WINDOW_MORE_LISTED).enumerate() {
                if i > 0 {
                    tip.push('\n');
                }
                escape_controls(&mut tip, &w.caption);
            }
            Some((words.window_more(rest.len()), tip.into()))
        });
        Self {
            validity,
            entries,
            words,
            more,
        }
    }
}

// ---- Value cache ----

/// Formatted value strings, kept per field until the value bits or validity change.
#[derive(Default)]
struct ValueCache {
    map: FxHashMap<FieldId, (u64, Validity, SharedString)>,
    scratch: String,
    formats: u64,
}

impl ValueCache {
    fn get(
        &mut self,
        id: FieldId,
        bits: u64,
        validity: Validity,
        format: impl FnOnce(&mut String),
    ) -> SharedString {
        if let Some((b, v, text)) = self.map.get(&id)
            && *b == bits
            && *v == validity
        {
            return text.clone();
        }
        self.scratch.clear();
        format(&mut self.scratch);
        self.formats += 1;
        let text = SharedString::from(self.scratch.clone());
        self.map.insert(id, (bits, validity, text.clone()));
        text
    }
}

/// Builds one column's fields.
struct Builder<'a> {
    cache: &'a mut ValueCache,
    words: &'a KvWords,
    gw: &'a GeneralWords,
    out: Vec<KvField>,
}

impl Builder<'_> {
    /// Adds a field. Valid and Stale values are formatted (or taken from the cache); other
    /// states show their word; Missing leaves the field out.
    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        id: FieldId,
        kind: KvKind,
        validity: Validity,
        unsupported: UnsupportedWord,
        bits: u64,
        format: impl FnOnce(&mut String),
    ) {
        let value: SharedString = match validity {
            Validity::Valid | Validity::Stale => self.cache.get(id, bits, validity, format),
            Validity::WarmingUp => self.gw.collecting.clone(),
            Validity::Restricted => self.words.restricted.clone(),
            Validity::Unsupported => self.words.unsupported(unsupported).clone(),
            Validity::Failed => self.words.unavailable.clone(),
            Validity::Missing => return,
        };
        self.out.push(KvField {
            id,
            label: self.gw.label(id).clone(),
            value,
            kind,
            validity,
            full: None,
            word: (validity == Validity::Unsupported).then_some(unsupported),
            tooltip: None,
            tone: None,
        });
    }

    /// A field with a fixed text.
    fn word(&mut self, id: FieldId, kind: KvKind, validity: Validity, text: SharedString) {
        self.out.push(KvField {
            id,
            label: self.gw.label(id).clone(),
            value: text,
            kind,
            validity,
            full: None,
            word: None,
            tooltip: None,
            tone: None,
        });
    }

    /// A field not read yet: "Collecting…".
    fn collecting(&mut self, id: FieldId, kind: KvKind) {
        let text = self.gw.collecting.clone();
        self.word(id, kind, Validity::WarmingUp, text);
    }

    /// A field with a prepared text, tooltip and tone.
    #[allow(clippy::too_many_arguments)]
    fn shown(
        &mut self,
        id: FieldId,
        kind: KvKind,
        validity: Validity,
        value: SharedString,
        full: Option<SharedString>,
        tooltip: Option<SharedString>,
        tone: Option<ValueTone>,
    ) {
        self.out.push(KvField {
            id,
            label: self.gw.label(id).clone(),
            value,
            kind,
            validity,
            full,
            word: None,
            tooltip,
            tone,
        });
    }

    /// An Unsupported field with no payload value, showing `word`.
    fn unsupported(&mut self, id: FieldId, kind: KvKind, word: UnsupportedWord) {
        self.out.push(KvField {
            id,
            label: self.gw.label(id).clone(),
            value: self.words.unsupported(word).clone(),
            kind,
            validity: Validity::Unsupported,
            full: None,
            word: Some(word),
            tooltip: None,
            tone: None,
        });
    }

    /// A payload field: WarmingUp without a payload, else the field's own validity.
    fn total<T>(
        &mut self,
        id: FieldId,
        kind: KvKind,
        total: Option<&Total<T>>,
        bits: impl FnOnce(&T) -> u64,
        format: impl FnOnce(&T, &mut String),
    ) {
        self.total_word(id, kind, total, UnsupportedWord::NotSupported, bits, format);
    }

    fn total_word<T>(
        &mut self,
        id: FieldId,
        kind: KvKind,
        total: Option<&Total<T>>,
        unsupported: UnsupportedWord,
        bits: impl FnOnce(&T) -> u64,
        format: impl FnOnce(&T, &mut String),
    ) {
        match total {
            None => self.collecting(id, kind),
            Some(t) => {
                let b = bits(&t.value);
                self.add(id, kind, t.validity, unsupported, b, |s| {
                    format(&t.value, s)
                });
            }
        }
    }

    fn size(&mut self, id: FieldId, total: Option<&Total<u64>>) {
        self.total(id, KvKind::Text, total, |v| *v, |v, s| units::bytes(s, *v));
    }

    fn count(&mut self, id: FieldId, total: Option<&Total<u64>>) {
        self.total(id, KvKind::Text, total, |v| *v, |v, s| units::count(s, *v));
    }

    fn text(&mut self, id: FieldId, kind: KvKind, total: Option<&Total<Arc<str>>>) {
        self.total(id, kind, total, ptr_bits, |v, s| s.push_str(v));
    }

    fn finish(self) -> Arc<[KvField]> {
        self.out.into()
    }
}

/// The record values this tab shows. Compared with the last rendered copy.
#[derive(Clone, Debug, PartialEq)]
struct Live {
    name: Arc<str>,
    ppid: u32,
    nice: i8,
    threads: u32,
    cpu_pct: f32,
    rss_anon_bytes: u64,
    rss_bytes: u64,
    pss_bytes: u64,
    uss_bytes: u64,
    swap_bytes: u64,
    io_read_bps: f32,
    io_write_bps: f32,
    net_rx_bps: f32,
    net_tx_bps: f32,
    gpu_util_pct: f32,
    gpu_mem_bytes: u64,
    validity: tl_model::validity::FieldValidity,
}

impl Live {
    fn from_record(r: &ProcessRecord) -> Self {
        Self {
            name: r.meta.name.clone(),
            ppid: r.ppid,
            nice: r.nice,
            threads: r.threads,
            cpu_pct: r.cpu_pct,
            rss_anon_bytes: r.rss_anon_bytes,
            rss_bytes: r.rss_bytes,
            pss_bytes: r.pss_bytes,
            uss_bytes: r.uss_bytes,
            swap_bytes: r.swap_bytes,
            io_read_bps: r.io_read_bps,
            io_write_bps: r.io_write_bps,
            net_rx_bps: r.net_rx_bps,
            net_tx_bps: r.net_tx_bps,
            gpu_util_pct: r.gpu_util_pct,
            gpu_mem_bytes: r.gpu_mem_bytes,
            validity: r.validity,
        }
    }

    fn get(&self, f: ProcessField) -> Validity {
        self.validity.get(f)
    }
}

fn net_unit(units: &UnitsSettings) -> NetUnit {
    match units.network {
        NetworkUnit::Bits => NetUnit::Bits,
        NetworkUnit::Bytes => NetUnit::Bytes,
    }
}

// ---- Command line block ----

/// The command line block body. A child view, so the 32 KiB text is rebuilt and re-shaped only
/// when the command line changes.
pub struct CmdLineBlock {
    /// The Copy button's focus, second in the tab's tab order.
    copy_focus: FocusHandle,
    source: Option<Arc<str>>,
    validity: Validity,
    words: Rc<KvWords>,
    text: SharedString,
    shape_count: u64,
}

impl CmdLineBlock {
    fn new(words: Rc<KvWords>, copy_focus: FocusHandle) -> Self {
        Self {
            copy_focus,
            source: None,
            validity: Validity::WarmingUp,
            text: words.collecting.clone(),
            words,
            shape_count: 0,
        }
    }

    /// Takes the payload's command line and the state words. Returns true when the shown text
    /// changed.
    fn set(&mut self, cmdline: Option<&Total<Arc<str>>>, words: Rc<KvWords>) -> bool {
        let (source, validity) = match cmdline {
            Some(t) => (Some(&t.value), t.validity),
            None => (None, Validity::WarmingUp),
        };
        let same_source = match (&self.source, source) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b) || **a == **b,
            (None, None) => true,
            _ => false,
        };
        if same_source && validity == self.validity && Rc::ptr_eq(&words, &self.words) {
            return false;
        }
        self.source = source.cloned();
        self.validity = validity;
        self.words = words;
        let words = &self.words;
        self.text = match (validity, source) {
            (Validity::Valid | Validity::Stale, Some(text)) => {
                let mut out = String::with_capacity(text.len());
                escape_controls(&mut out, text);
                out.into()
            }
            (Validity::Restricted, _) => words.restricted.clone(),
            (Validity::Unsupported, _) => words.not_reported.clone(),
            (Validity::Failed, _) => words.unavailable.clone(),
            _ => words.collecting.clone(),
        };
        self.shape_count += 1;
        true
    }

    /// The Copy button's focus.
    pub fn copy_focus(&self) -> &FocusHandle {
        &self.copy_focus
    }

    /// How often the text was rebuilt, for tests and the bench.
    pub fn shape_count(&self) -> u64 {
        self.shape_count
    }

    /// The shown text.
    pub fn text(&self) -> &SharedString {
        &self.text
    }
}

impl Render for CmdLineBlock {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *metrics(cx);
        let words = general_words(cx);
        let copied = words.toast_value_copied.clone();
        let theme = current(cx);
        let mut title = div().flex_1().min_w_0();
        *title.text_style() = label_style(&m, m.text.base, FontWeight::MEDIUM);
        let title = title
            .truncate()
            .text_color(theme.text_secondary)
            .child(words.cmdline_title.clone());
        let valid = self.validity.is_valid() || self.validity == Validity::Stale;
        let text = self.text.clone();
        let copy = TlButton::new("general-cmdline-copy", TlButtonKind::Ghost)
            .size(TlButtonSize::Sm)
            .icon(TlIcon::Copy)
            .icon_size(px(COPY_ICON * m.ui_scale))
            .label(words.cmdline_copy.clone())
            .focus_handle(self.copy_focus.clone())
            .disabled(!valid)
            .on_click(move |_, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
                window_toast(window, cx, ToastKind::Success, copied.clone());
            });
        let mut body = div().debug_selector(|| "GeneralTab-cmdline-text".into());
        *body.text_style() = t_sm(&m);
        let body = body
            .w_full()
            .whitespace_normal()
            .line_height(px(CMDLINE_LINE_HEIGHT * m.ui_scale))
            .text_color(if valid {
                theme.text_primary
            } else {
                theme.text_secondary
            })
            .child(self.text.clone());
        v_flex()
            .debug_selector(|| "GeneralTab-cmdline".into())
            .w_full()
            .p(m.sp[4])
            .bg(theme.list_bg)
            .rounded(m.radius_small)
            .child(
                h_flex()
                    .h(px(ROW_HEIGHT * m.ui_scale))
                    .items_center()
                    .gap(m.sp[4])
                    .child(title)
                    .child(tab_slot(TAB_CMDLINE_COPY, copy)),
            )
            .child(body)
    }
}

// ---- The tab ----

/// Copy texts of one grid, by its flat row index.
type CopyTexts = Rc<[(usize, SharedString)]>;

/// The General tab view. `PropsWindow` builds it with the process key and the tab's filter.
pub struct GeneralTab {
    key: ProcessKey,
    payload: Option<Arc<GeneralDetail>>,
    live: Option<Live>,
    mem_total_bytes: u64,
    online: CpuSet,
    units: UnitsSettings,
    exited: bool,
    needle: SharedString,
    cache: ValueCache,
    /// The state words the fields were built with.
    words: Rc<KvWords>,
    general_words: Rc<GeneralWords>,
    hashes: Option<HashView>,
    /// `None` until the first `Snapshot.windows`.
    windows: Option<WindowsView>,
    /// Verify was pressed and the payload has not reached a final package check yet.
    verify_requested: bool,
    /// The window's toast stack, for the Verify result toasts.
    toasts: Option<WeakEntity<ToastQueue>>,
    columns: [Arc<[KvField]>; 3],
    /// Grid sections per column, after the filter. Column 2 ends at `Package status:` while the
    /// Verify row shows; `col2_tail` holds the rows under it.
    sections: [Rc<[TlKvSection]>; 3],
    col2_tail: Rc<[TlKvSection]>,
    /// Column 3's window block, drawn under a divider.
    col3_tail: Rc<[TlKvSection]>,
    /// The Verify button row is drawn under `Package status:`.
    verify_row: bool,
    /// Label column widths per column, with the `style_version` they were measured for.
    label_widths: Option<(u64, [Pixels; 3])>,
    /// Body width from the last paint (inside the padding), zero before the first.
    width: Rc<Cell<Pixels>>,
    /// The fields each column shows after the filter, in order; the field cursor indexes them.
    visible: [Vec<FieldId>; 3],
    /// How many of `visible[i]` the column's first grid draws; the rest are its tail grid.
    main_len: [usize; 3],
    /// Copy texts per column and grid (first, tail), by the grid's flat row index.
    copies: [[CopyTexts; 2]; 3],
    /// The field cursor: (column, index into `visible`).
    cursor: Option<(usize, usize)>,
    /// The panels' focus stop.
    focus: FocusHandle,
    /// The Verify button's focus, last in the tab order.
    verify_focus: FocusHandle,
    /// Panel bounds from the last paint, to place the field menu opened from the keyboard.
    panel_bounds: Rc<[Cell<Bounds<Pixels>>; 3]>,
    cmdline: Entity<CmdLineBlock>,
    filter: Entity<PropsFilter>,
    _subs: Vec<Subscription>,
}

impl GeneralTab {
    pub fn new(key: ProcessKey, filter: Entity<PropsFilter>, cx: &mut Context<Self>) -> Self {
        let sub = cx.subscribe(&filter, |this, filter, event: &FilterChanged, cx| {
            if filter.update(cx, |f, _| f.accept(event.generation)) {
                this.needle = event.needle.clone();
                this.apply_filter();
                cx.notify();
            }
        });
        let units = if cx.has_global::<TlSettings>() {
            TlSettings::settings(cx).units.clone()
        } else {
            UnitsSettings::default()
        };
        let mut tab = Self {
            key,
            payload: None,
            live: None,
            mem_total_bytes: 0,
            online: CpuSet::empty(),
            units,
            exited: false,
            needle: SharedString::default(),
            cache: ValueCache::default(),
            words: kv_words(cx),
            general_words: general_words(cx),
            hashes: None,
            windows: None,
            verify_requested: false,
            toasts: None,
            columns: [Arc::from([]), Arc::from([]), Arc::from([])],
            sections: [Rc::from([]), Rc::from([]), Rc::from([])],
            col2_tail: Rc::from([]),
            col3_tail: Rc::from([]),
            verify_row: false,
            label_widths: None,
            width: Rc::default(),
            visible: Default::default(),
            main_len: [0; 3],
            copies: Default::default(),
            cursor: None,
            focus: cx.focus_handle().tab_stop(true).tab_index(TAB_FIELDS),
            panel_bounds: Rc::default(),
            cmdline: cx.new(|cx| {
                let focus = cx.focus_handle();
                CmdLineBlock::new(kv_words(cx), focus)
            }),
            verify_focus: cx.focus_handle(),
            filter,
            _subs: vec![sub],
        };
        tab.rebuild(cx);
        tab
    }

    pub fn key(&self) -> ProcessKey {
        self.key
    }

    pub fn filter(&self) -> &Entity<PropsFilter> {
        &self.filter
    }

    /// Takes the General payload. Does nothing when the `Arc` is the one already shown.
    pub fn set_payload(&mut self, payload: Option<&Arc<TabPayload>>, cx: &mut Context<Self>) {
        let detail = payload.and_then(|p| match &**p {
            TabPayload::General(d) => Some(d.clone()),
            _ => None,
        });
        let same = match (&self.payload, &detail) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if same || self.exited {
            return;
        }
        let before = self.payload.as_ref().map(|d| d.package_check);
        if let Some(after) = detail.as_ref().map(|d| d.package_check) {
            self.verify_finished(before, after, cx);
        }
        self.payload = detail;
        let cmdline = self.payload.as_ref().map(|d| &d.cmdline);
        let words = kv_words(cx);
        self.cmdline.update(cx, |c, cx| {
            if c.set(cmdline, words) {
                cx.notify();
            }
        });
        self.rebuild(cx);
        cx.notify();
    }

    /// Shows the result toast when a verify this tab saw running reached its result.
    fn verify_finished(
        &mut self,
        before: Option<PackageCheck>,
        after: PackageCheck,
        cx: &mut Context<Self>,
    ) {
        if matches!(after, PackageCheck::NotVerified | PackageCheck::Verifying) {
            return;
        }
        let running = self.verify_requested || before == Some(PackageCheck::Verifying);
        self.verify_requested = false;
        if !running {
            return;
        }
        let words = &self.general_words;
        let (kind, text) = match after {
            PackageCheck::Matches => (ToastKind::Success, words.toast_matches.clone()),
            PackageCheck::DoesNotMatch => (ToastKind::Warning, words.toast_mismatch.clone()),
            PackageCheck::Failed => (ToastKind::Error, words.toast_failed.clone()),
            _ => return,
        };
        match self.toasts.as_ref().and_then(WeakEntity::upgrade) {
            Some(queue) => queue.update(cx, |q, cx| q.push(kind, text, None, None, cx)),
            None => warn!(target: "tasklens::ui", "no toast stack for the verify result"),
        }
    }

    /// The toast stack the Verify result toasts go to. Without one the tab finds its window's
    /// stack when it first renders.
    pub fn set_toasts(&mut self, toasts: WeakEntity<ToastQueue>) {
        self.toasts = Some(toasts);
    }

    /// The Verify against package button, or `None` while it is hidden: before the payload and
    /// for an executable that is not packaged.
    pub fn verify_button(&self) -> Option<VerifyButton> {
        let d = self.payload.as_deref()?;
        let words = &self.general_words;
        let check = shown_check(d, self.verify_requested);
        let loading = check == PackageCheck::Verifying;
        let tooltip = match check {
            PackageCheck::NotPackaged => return None,
            _ if self.exited => Some(words.not_running.clone()),
            PackageCheck::NoChecksum => Some(words.no_checksum.clone()),
            _ => None,
        };
        Some(VerifyButton {
            label: if loading {
                words.verifying.clone()
            } else {
                words.verify.clone()
            },
            loading: loading && !self.exited,
            disabled: tooltip.is_some(),
            tooltip,
        })
    }

    /// Click, Enter or Space on the Verify button: sends `Command::VerifyPackage(key)`. Does
    /// nothing while the button is hidden, disabled or loading.
    pub fn verify_package(&mut self, cx: &mut Context<Self>) {
        let Some(button) = self.verify_button() else {
            return;
        };
        if button.disabled || button.loading {
            return;
        }
        let Some(engine) = cx.try_global::<EngineGlobal>() else {
            warn!(target: "tasklens::ui", "no engine, verify not sent");
            return;
        };
        if let Err(err) = engine.0.send(Command::VerifyPackage(self.key)) {
            warn!(target: "tasklens::ui", ?err, "verify against package not sent");
            return;
        }
        self.verify_requested = true;
        self.rebuild(cx);
        cx.notify();
    }

    /// Whether the Verify button row is drawn (`Package status:` passes the filter and the
    /// button is not hidden).
    pub fn verify_row_shown(&self) -> bool {
        self.verify_row
    }

    /// Takes this key's record from a sweep and MemTotal for `Memory %`. Re-formats only when a
    /// shown value changed.
    pub fn set_record(
        &mut self,
        record: Option<&ProcessRecord>,
        mem_total_bytes: u64,
        cx: &mut Context<Self>,
    ) {
        if self.exited {
            return;
        }
        let live = record.map(Live::from_record);
        if live == self.live && mem_total_bytes == self.mem_total_bytes {
            return;
        }
        self.live = live;
        self.mem_total_bytes = mem_total_bytes;
        self.rebuild(cx);
        cx.notify();
    }

    /// The online CPUs, for the `All (0-31)` affinity form.
    pub fn set_online_cpus(&mut self, online: CpuSet, cx: &mut Context<Self>) {
        if online != self.online {
            self.online = online;
            self.rebuild(cx);
            cx.notify();
        }
    }

    /// The `[units]` settings, for Net I/O.
    pub fn set_units(&mut self, units: UnitsSettings, cx: &mut Context<Self>) {
        if units != self.units {
            self.units = units;
            self.rebuild(cx);
            cx.notify();
        }
    }

    /// Takes `Snapshot.windows` (bit `WINDOWS`). Rebuilds only when this pid's entries or the
    /// component's validity changed. Frozen once exited.
    pub fn set_windows(&mut self, windows: &WindowsSnapshot, cx: &mut Context<Self>) {
        if self.exited {
            return;
        }
        let pid = self.key.pid;
        let validity = windows.header.validity;
        let mine = windows.for_pid(pid);
        if let Some(view) = &self.windows
            && view.validity == validity
            && view.entries.len() == mine.len()
            && view.entries.iter().zip(mine).all(|(a, b)| a == b)
        {
            return;
        }
        let entries = mine.iter().filter(|w| w.pid == pid).cloned().collect();
        self.windows = Some(WindowsView::new(validity, entries, general_words(cx)));
        self.rebuild(cx);
        cx.notify();
    }

    /// Freezes the values. Link fields then draw as plain `text_secondary` text.
    pub fn set_exited(&mut self, exited: bool, cx: &mut Context<Self>) {
        if exited != self.exited {
            self.exited = exited;
            self.apply_filter();
            cx.notify();
        }
    }

    pub fn exited(&self) -> bool {
        self.exited
    }

    /// The built fields of column `i` (0..=2), before the filter.
    pub fn column(&self, i: usize) -> &Arc<[KvField]> {
        &self.columns[i]
    }

    /// The field `id`, if it is built.
    pub fn field(&self, id: FieldId) -> Option<&KvField> {
        self.columns
            .iter()
            .flat_map(|c| c.iter())
            .find(|f| f.id == id)
    }

    /// The grid rows of column `i` after the filter.
    pub fn rows(&self, i: usize) -> Vec<TlKvRow> {
        let tail: &[TlKvSection] = match i {
            1 => &self.col2_tail,
            2 => &self.col3_tail,
            _ => &[],
        };
        self.sections[i]
            .iter()
            .chain(tail)
            .flat_map(|s| s.blocks.iter())
            .flat_map(|b| b.rows.iter().cloned())
            .collect()
    }

    /// The ids of the fields the filter leaves, in column order.
    pub fn visible_ids(&self) -> Vec<FieldId> {
        self.columns
            .iter()
            .flat_map(|c| c.iter())
            .filter(|f| self.passes(f))
            .map(|f| f.id)
            .collect()
    }

    /// Values formatted so far, for tests and the bench.
    pub fn format_count(&self) -> u64 {
        self.cache.formats
    }

    pub fn cmdline(&self) -> &Entity<CmdLineBlock> {
        &self.cmdline
    }

    /// The field cursor: (column, index into the column's shown fields).
    pub fn cursor(&self) -> Option<(usize, usize)> {
        self.cursor
    }

    /// The field under the cursor.
    pub fn cursor_field(&self) -> Option<&KvField> {
        let (col, row) = self.cursor?;
        let id = *self.visible[col].get(row)?;
        self.field(id)
    }

    /// The label column width of column `col` from the last frame; the values start after it.
    pub fn label_width(&self, col: usize) -> Option<Pixels> {
        self.label_widths.map(|(_, w)| w[col])
    }

    /// The panels' focus stop.
    pub fn grid_focus(&self) -> &FocusHandle {
        &self.focus
    }

    /// The Verify button's focus.
    pub fn verify_focus(&self) -> &FocusHandle {
        &self.verify_focus
    }

    /// Moves the cursor to (`col`, `row`), clamped to the column's last field. Does nothing for an
    /// empty column.
    pub fn set_cursor(&mut self, col: usize, row: usize, cx: &mut Context<Self>) {
        let len = self.visible[col].len();
        if len == 0 {
            return;
        }
        let next = Some((col, row.min(len - 1)));
        if next != self.cursor {
            self.cursor = next;
            cx.notify();
        }
    }

    /// The first column with a field, for a cursor key with no cursor yet.
    fn first_cursor(&mut self, cx: &mut Context<Self>) -> bool {
        if self.cursor.is_some() {
            return false;
        }
        if let Some(col) = (0..3).find(|&c| !self.visible[c].is_empty()) {
            self.set_cursor(col, 0, cx);
        }
        true
    }

    fn step_row(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.first_cursor(cx) {
            return;
        }
        let Some((col, row)) = self.cursor else {
            return;
        };
        let row = row.saturating_add_signed(delta);
        self.set_cursor(col, row, cx);
    }

    /// Left or Right: the same row index in the next column with fields, clamped to its last row.
    fn step_column(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.first_cursor(cx) {
            return;
        }
        let Some((col, row)) = self.cursor else {
            return;
        };
        let next = if forward {
            (col + 1..3).find(|&c| !self.visible[c].is_empty())
        } else {
            (0..col).rev().find(|&c| !self.visible[c].is_empty())
        };
        if let Some(next) = next {
            self.set_cursor(next, row, cx);
        }
    }

    fn row_end(&mut self, last: bool, cx: &mut Context<Self>) {
        self.first_cursor(cx);
        if let Some((col, _)) = self.cursor {
            let row = if last { usize::MAX } else { 0 };
            self.set_cursor(col, row, cx);
        }
    }

    /// The copy form of a field: the full text for a shortened value, else the shown text.
    fn copy_text(&self, field: &KvField) -> String {
        if let Some(full) = &field.full {
            return full.to_string();
        }
        let row = to_row(field, self.exited, &self.general_words);
        let shown = shown_text_in(&row.value, &self.words).unwrap_or_default();
        value_copy_form(&shown).into_owned()
    }

    fn row_line(&self, field: &KvField) -> String {
        line(&field.label, &self.copy_text(field))
    }

    fn column_lines(&self, col: usize) -> impl Iterator<Item = String> + '_ {
        self.visible[col]
            .iter()
            .filter_map(|id| self.field(*id))
            .map(|f| self.row_line(f))
    }

    /// Copy all fields: every shown field of the three columns as `Label: value` lines in column
    /// order, then the command line.
    pub fn all_fields_text(&self, cx: &App) -> String {
        let mut lines: Vec<String> = (0..3).flat_map(|c| self.column_lines(c)).collect();
        let cmdline = self.cmdline.read(cx).text();
        lines.push(line(&self.general_words.cmdline_label, cmdline));
        lines.join("\n")
    }

    fn copy(&self, text: String, kind_value: bool, window: &mut Window, cx: &mut App) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        let words = &self.general_words;
        let toast = if kind_value {
            words.toast_value_copied.clone()
        } else {
            words.toast_copied.clone()
        };
        window_toast(window, cx, ToastKind::Success, toast);
    }

    fn copy_value(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.cursor_field().map(|f| self.copy_text(f)) {
            self.copy(text, true, window, cx);
        }
    }

    fn copy_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.cursor_field().map(|f| self.row_line(f)) {
            self.copy(text, false, window, cx);
        }
    }

    fn copy_section(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((col, _)) = self.cursor {
            let text = self.column_lines(col).collect::<Vec<_>>().join("\n");
            self.copy(text, false, window, cx);
        }
    }

    pub fn copy_all_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.all_fields_text(cx);
        self.copy(text, false, window, cx);
    }

    /// Enter: runs the cursor field's link, or copies its value.
    fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = self.cursor_field() else {
            return;
        };
        let link = (!self.exited && field.validity == Validity::Valid)
            .then(|| link_of(field, &self.general_words))
            .flatten();
        match link.map(|_| field.kind) {
            Some(KvKind::Link(LinkTarget::Parent)) => self.go_to_parent(window, cx),
            Some(KvKind::Link(LinkTarget::Executable)) => self.open_file_location(window, cx),
            Some(KvKind::Link(LinkTarget::Affinity)) => self.set_affinity(window, cx),
            _ => self.copy_value(window, cx),
        }
    }

    /// The PPID link: the Processes page selects the live parent through its jump target
    /// (PROC-071), and the main window comes to the front.
    pub fn go_to_parent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ppid) = self.live.as_ref().map(|l| l.ppid).filter(|p| *p != 0) else {
            return;
        };
        let parent = cx.try_global::<GlobalHub>().and_then(|hub| {
            hub.0
                .read(cx)
                .current()
                .processes
                .records
                .iter()
                .find(|r| r.key.pid == ppid && !r.flags.contains(ProcFlags::EXITED))
                .map(|r| r.key)
        });
        let not_found = tr(cx, "processes-toast-parent-not-found");
        let Some(parent) = parent else {
            window_toast(window, cx, ToastKind::Warning, not_found);
            return;
        };
        let texts = JumpTexts {
            success: Some(tr(cx, "processes-toast-switched-to-parent")),
            not_found: not_found.clone(),
            hidden: Some(not_found),
            switched_tab: None,
        };
        let main = cx.try_global::<ShellState>().and_then(|s| s.main_window);
        let page = main_window_view(cx)
            .and_then(|v| v.read(cx).page_entity::<ProcessesPage>(Tab::Processes));
        let (Some(main), Some(page)) = (main, page) else {
            warn!(target: "tasklens::ui", "no Processes page for Go to parent process");
            return;
        };
        let _ = main.update(cx, |_, window, cx| {
            window.activate_window();
            page.update(cx, |p, cx| p.select_by_key(parent, texts, window, cx));
        });
    }

    /// The Executable link: the file manager shows the executable (PROC-073).
    pub fn open_file_location(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self
            .payload
            .as_ref()
            .filter(|d| !d.exe_deleted && d.exe.validity.is_valid())
            .map(|d| d.exe.value.clone())
        else {
            return;
        };
        let Some(tracker) = cx.try_global::<GlobalActionTracker>().map(|t| t.0.clone()) else {
            warn!(target: "tasklens::ui", "no action tracker, file location not opened");
            return;
        };
        let Some(queue) = ToastQueue::of_window(window, cx) else {
            return;
        };
        let request = ActionRequest {
            id: ActionId(0),
            kind: ActionKind::ShowInFileManager {
                path,
                properties: false,
            },
            targets: SmallVec::new(),
            privileged: false,
        };
        let words = self.general_words.clone();
        let texts = ToastTexts::new(SharedString::default(), words.toast_open_failed.clone());
        let sent = tracker.update(cx, |t, cx| {
            t.track_or_toast(request, queue.downgrade(), texts, cx)
        });
        if sent.is_some() {
            window_toast(window, cx, ToastKind::Info, words.toast_opening.clone());
        }
    }

    /// The Affinity link: the Set Affinity dialog for this process.
    pub fn set_affinity(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self
            .live
            .as_ref()
            .map(|l| l.name.to_string())
            .unwrap_or_default();
        let subtitle = tr_args(
            cx,
            "processes-affinity-subtitle",
            &[
                ("name", name.into()),
                ("pid", self.key.pid.to_string().into()),
            ],
        );
        let targets = smallvec![ActionTarget::Process(self.key)];
        if let Err(err) =
            crate::windows::affinity::open(targets, subtitle, window.window_handle(), cx)
        {
            warn!(target: "tasklens::ui", %err, "the Set Affinity dialog did not open");
        }
    }

    /// Opens the field menu on the cursor field (`col`, `row`): at `position` for a right-click,
    /// else under the row.
    pub fn open_field_menu(
        &mut self,
        col: usize,
        row: usize,
        position: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.visible[col].get(row).is_none() {
            return;
        }
        self.set_cursor(col, row, cx);
        let position = position.unwrap_or_else(|| self.row_anchor(col, row, cx));
        let words = self.general_words.clone();
        let menu = TlMenu::build(window, cx, self.focus.clone(), |menu, _, _| {
            menu.item(words.copy_value.clone(), Box::new(PropsCopyValue))
                .item(words.copy_row.clone(), Box::new(GeneralCopyRow))
                .item(words.copy_section.clone(), Box::new(GeneralCopySection))
                .separator()
                .item(words.copy_all.clone(), Box::new(PropsCopyAllFields))
        });
        open_context_menu_at(position, window, cx, menu);
    }

    /// Where Shift+F10 opens the menu: the row's left edge plus `sp4`, at the row's bottom.
    fn row_anchor(&self, col: usize, row: usize, cx: &App) -> Point<Pixels> {
        let m = metrics(cx);
        let row_h = px(ROW_HEIGHT * m.ui_scale);
        let bounds = self.panel_bounds[col].get();
        let mut y = bounds.top() + m.sp[3] + row_h * (row + 1) as f32;
        if row >= self.main_len[col] {
            y += match col {
                1 => row_h,
                2 => px(WINDOW_GAP * m.ui_scale) + px(1.),
                _ => px(0.),
            };
        }
        point(bounds.left() + m.sp[4] * 2., y)
    }

    fn on_menu_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.first_cursor(cx);
        if let Some((col, row)) = self.cursor {
            self.open_field_menu(col, row, None, window, cx);
        }
    }

    /// The cursor row in grid `part` (0 the first, 1 the tail) of column `col`.
    fn cursor_in(&self, col: usize, part: usize, focused: bool) -> Option<usize> {
        let (c, row) = self.cursor?;
        if c != col || !focused {
            return None;
        }
        let main = self.main_len[col];
        match part {
            0 => (row < main).then_some(row),
            _ => row.checked_sub(main),
        }
    }

    fn passes(&self, field: &KvField) -> bool {
        if field.validity == Validity::Missing {
            return false;
        }
        if self.needle.is_empty() {
            return true;
        }
        let mut haystack = lowercase(&field.label);
        haystack.push(' ');
        haystack.push_str(&lowercase(&field.value));
        matches(&haystack, &self.needle)
    }

    /// Converts the built fields into grid rows, dropping the ones the filter hides. Column 2
    /// is cut under `Package status:` when the Verify row shows.
    fn apply_filter(&mut self) {
        let button = self.verify_button().is_some();
        for i in 0..3 {
            let mut rows = Vec::with_capacity(self.columns[i].len());
            let mut split = None;
            let shown: Vec<&KvField> = self.columns[i].iter().filter(|f| self.passes(f)).collect();
            self.visible[i] = shown.iter().map(|f| f.id).collect();
            for f in &shown {
                rows.push(to_row(f, self.exited, &self.general_words));
                if f.id == FieldId::PackageStatus && button {
                    split = Some(rows.len());
                }
            }
            if i == 1 {
                self.verify_row = split.is_some();
                self.col2_tail = match split {
                    Some(at) => section(rows.split_off(at)),
                    None => Rc::from([]),
                };
            }
            if i == 2 {
                let shown = self.columns[2].iter().filter(|f| self.passes(f));
                let at = shown
                    .clone()
                    .position(|f| f.id.is_window())
                    .unwrap_or(rows.len());
                let tail = rows.split_off(at);
                self.col3_tail = if tail.is_empty() {
                    Rc::from([])
                } else {
                    section(tail)
                };
            }
            self.main_len[i] = rows.len();
            let copies = |range: std::ops::Range<usize>| -> CopyTexts {
                shown[range]
                    .iter()
                    .enumerate()
                    .filter_map(|(n, f)| f.full.clone().map(|full| (n, full)))
                    .collect()
            };
            self.copies[i] = [copies(0..rows.len()), copies(rows.len()..shown.len())];
            self.sections[i] = section(rows);
        }
        self.cursor = self.cursor.and_then(|(col, row)| {
            let len = self.visible[col].len();
            (len > 0).then(|| (col, row.min(len - 1)))
        });
    }

    fn rebuild(&mut self, cx: &mut App) {
        self.words = kv_words(cx);
        let gw = general_words(cx);
        if !Rc::ptr_eq(&gw, &self.general_words) {
            // New strings: format every value and measure the labels again.
            self.cache = ValueCache::default();
            self.label_widths = None;
        }
        self.general_words = gw.clone();
        let d = self.payload.clone();
        let d = d.as_deref();
        if let Some(d) = d
            && !self
                .hashes
                .as_ref()
                .is_some_and(|h| h.is_for(&d.hashes, &self.general_words))
        {
            self.hashes = Some(HashView::new(&d.hashes, self.general_words.clone()));
        }
        if let Some(view) = &self.windows
            && !Rc::ptr_eq(&view.words, &self.general_words)
        {
            let view = self.windows.take().expect("checked above");
            self.windows = Some(WindowsView::new(
                view.validity,
                view.entries,
                self.general_words.clone(),
            ));
        }
        let live = self.live.clone();
        let live = live.as_ref();

        // Column 1.
        let mut b = Builder {
            cache: &mut self.cache,
            words: &self.words,
            gw: &gw,
            out: Vec::with_capacity(COLUMNS[0].len()),
        };
        match live {
            Some(l) => {
                b.add(
                    FieldId::Process,
                    KvKind::Text,
                    Validity::Valid,
                    UnsupportedWord::NotSupported,
                    ptr_bits(&l.name),
                    |s| s.push_str(&l.name),
                );
            }
            None => b.collecting(FieldId::Process, KvKind::Text),
        }
        let pid = self.key.pid;
        b.add(
            FieldId::Pid,
            KvKind::Text,
            Validity::Valid,
            UnsupportedWord::NotSupported,
            u64::from(pid),
            |s| {
                let _ = write!(s, "{pid}");
            },
        );
        b.total(
            FieldId::Started,
            KvKind::Text,
            d.map(|d| &d.started_s),
            |v| *v as u64,
            |v, s| write_local(s, &gw, *v),
        );
        match live {
            Some(l) if l.ppid == 0 => b.word(
                FieldId::Ppid,
                KvKind::Text,
                Validity::Valid,
                gw.ppid_none.clone(),
            ),
            Some(l) => {
                let ppid = l.ppid;
                b.add(
                    FieldId::Ppid,
                    KvKind::Link(LinkTarget::Parent),
                    Validity::Valid,
                    UnsupportedWord::NotSupported,
                    u64::from(ppid),
                    |s| {
                        let _ = write!(s, "{ppid}");
                    },
                );
            }
            None => b.collecting(FieldId::Ppid, KvKind::Text),
        }
        match d {
            Some(d) => {
                let validity = worst(d.uids.validity, d.uid_names.validity);
                let (r, e, s) = &d.uid_names.value;
                let ids = &d.uids.value;
                let bits = hash_of((
                    ids.real,
                    ids.effective,
                    ids.saved,
                    ptr_bits(r),
                    ptr_bits(e),
                    ptr_bits(s),
                ));
                b.add(
                    FieldId::User,
                    KvKind::Text,
                    validity,
                    UnsupportedWord::NotSupported,
                    bits,
                    |out| fmt_user(out, &gw, ids, (r, e, s)),
                );
            }
            None => b.collecting(FieldId::User, KvKind::Text),
        }
        match d {
            Some(d) => {
                let nice = live.map_or(0, |l| l.nice);
                let validity = worst(d.policy.validity, d.kernel_prio.validity);
                let validity = if live.is_none() && validity.is_valid() {
                    Validity::WarmingUp
                } else {
                    validity
                };
                let (policy, rt, kernel) = (d.policy.value, d.rt_prio, d.kernel_prio.value);
                b.add(
                    FieldId::Priority,
                    KvKind::Text,
                    validity,
                    UnsupportedWord::NotSupported,
                    hash_of((policy as u8, nice, rt, kernel)),
                    |s| fmt_priority(s, &gw, policy, nice, rt, kernel),
                );
            }
            None => b.collecting(FieldId::Priority, KvKind::Text),
        }
        b.total(
            FieldId::Autogroup,
            KvKind::Text,
            d.map(|d| &d.autogroup),
            |(name, nice)| hash_of((ptr_bits(name), *nice)),
            |(name, nice), s| {
                let _ = gw.autogroup.write(s, &[name, nice]);
            },
        );
        {
            let online = self.online;
            let differ = d.is_some_and(|d| d.affinity_threads_differ);
            b.total(
                FieldId::Affinity,
                KvKind::Link(LinkTarget::Affinity),
                d.map(|d| &d.affinity),
                |set| hash_of((set, online, differ)),
                |set, s| fmt_affinity(s, &gw, set, &online, differ),
            );
        }
        match d {
            Some(d) => {
                let parts = [
                    d.pie.validity,
                    d.nx_stack.validity,
                    d.relro.validity,
                    d.no_new_privs.validity,
                    d.seccomp.validity,
                ];
                let validity = parts
                    .into_iter()
                    .find(|v| !v.is_valid())
                    .unwrap_or(Validity::Valid);
                let h = Hardening {
                    pie: d.pie.value,
                    nx_stack: d.nx_stack.value,
                    full_relro: d.relro.value == Relro::Full,
                    shadow_stack: d.shadow_stack.validity.is_valid() && d.shadow_stack.value,
                    no_new_privs: d.no_new_privs.value,
                    seccomp: d.seccomp.value != Seccomp::Disabled,
                };
                b.add(
                    FieldId::Hardening,
                    KvKind::Text,
                    validity,
                    UnsupportedWord::NotSupported,
                    hash_of(h),
                    |s| fmt_hardening(s, &gw, h),
                );
            }
            None => b.collecting(FieldId::Hardening, KvKind::Text),
        }
        b.total(
            FieldId::Pie,
            KvKind::Text,
            d.map(|d| &d.pie),
            |v| u64::from(*v),
            |v, s| s.push_str(gw.yes_no(*v)),
        );
        b.total(
            FieldId::NxStack,
            KvKind::Text,
            d.map(|d| &d.nx_stack),
            |v| u64::from(*v),
            |v, s| s.push_str(if *v { &gw.yes } else { &gw.nx_stack_no }),
        );
        b.total(
            FieldId::Relro,
            KvKind::Text,
            d.map(|d| &d.relro),
            |v| *v as u64,
            |v, s| {
                s.push_str(match v {
                    Relro::Full => &gw.relro_full,
                    Relro::Partial => &gw.relro_partial,
                    Relro::None => &gw.none,
                })
            },
        );
        b.total(
            FieldId::ShadowStack,
            KvKind::Text,
            d.map(|d| &d.shadow_stack),
            |v| u64::from(*v),
            |v, s| s.push_str(if *v { &gw.enabled } else { &gw.disabled }),
        );
        match d {
            Some(d) => {
                let validity = worst(d.aslr_system.validity, d.aslr_disabled_for_process.validity);
                let (sys, off) = (d.aslr_system.value, d.aslr_disabled_for_process.value);
                b.add(
                    FieldId::Aslr,
                    KvKind::Text,
                    validity,
                    UnsupportedWord::NotSupported,
                    hash_of((sys, off)),
                    |s| fmt_aslr(s, &gw, sys, off),
                );
            }
            None => b.collecting(FieldId::Aslr, KvKind::Text),
        }
        b.total(
            FieldId::NoNewPrivs,
            KvKind::Text,
            d.map(|d| &d.no_new_privs),
            |v| u64::from(*v),
            |v, s| s.push_str(gw.yes_no(*v)),
        );
        b.total(
            FieldId::Seccomp,
            KvKind::Text,
            d.map(|d| &d.seccomp),
            |v| match v {
                Seccomp::Disabled => 0,
                Seccomp::Strict => 1,
                Seccomp::Filter { filters } => 2 + u64::from(*filters),
            },
            |v, s| fmt_seccomp(s, &gw, *v),
        );
        b.total(
            FieldId::Architecture,
            KvKind::Text,
            d.map(|d| &d.architecture),
            |v| *v as u64,
            |v, s| s.push_str(arch_word(&gw, *v)),
        );
        match d {
            Some(d) if d.exe_deleted && d.exe.validity.is_valid() => {
                // Plain text: a deleted file has no location to open.
                b.total(
                    FieldId::Executable,
                    KvKind::Text,
                    Some(&d.exe),
                    ptr_bits,
                    |v, s| {
                        // The kernel's own marker is replaced by the translated one.
                        s.push_str(v.strip_suffix(KERNEL_DELETED).unwrap_or(v));
                        s.push(' ');
                        s.push_str(&gw.exe_deleted);
                    },
                );
            }
            _ => b.text(
                FieldId::Executable,
                KvKind::Link(LinkTarget::Executable),
                d.map(|d| &d.exe),
            ),
        }
        b.total(
            FieldId::Location,
            KvKind::Text,
            d.map(|d| &d.location),
            |v| *v as u64,
            |v, s| s.push_str(location_word(&gw, *v)),
        );
        b.total(
            FieldId::Package,
            KvKind::Text,
            d.map(|d| &d.package),
            |p| {
                hash_of(
                    p.as_ref()
                        .map(|p| (ptr_bits(&p.name), ptr_bits(&p.version))),
                )
            },
            |p, s| match p {
                Some(p) => {
                    let _ = write!(s, "{} {}", p.name, p.version);
                }
                None => s.push_str(&gw.not_packaged),
            },
        );
        b.total_word(
            FieldId::Description,
            KvKind::Text,
            d.map(|d| &d.description),
            UnsupportedWord::NotReported,
            ptr_bits,
            |v, s| s.push_str(v),
        );
        b.text(FieldId::WorkingDir, KvKind::Text, d.map(|d| &d.cwd));
        match d.map(|d| &d.root) {
            Some(root) if root.validity.is_valid() && &*root.value == "/" => {}
            root => b.text(FieldId::Root, KvKind::Text, root),
        }
        match d {
            Some(d) => match &d.cgroup {
                Some(path) => b.add(
                    FieldId::Cgroup,
                    KvKind::Text,
                    Validity::Valid,
                    UnsupportedWord::NotReported,
                    ptr_bits(path),
                    |s| {
                        if !path.starts_with('/') {
                            s.push('/');
                        }
                        s.push_str(path);
                    },
                ),
                None => b.unsupported(FieldId::Cgroup, KvKind::Text, UnsupportedWord::NotReported),
            },
            None => b.collecting(FieldId::Cgroup, KvKind::Text),
        }
        if let Some(unit) = d.and_then(|d| d.unit.as_ref()) {
            b.add(
                FieldId::Unit,
                KvKind::Link(LinkTarget::Unit),
                Validity::Valid,
                UnsupportedWord::NotSupported,
                ptr_bits(&unit.name),
                |s| s.push_str(&unit.name),
            );
        }
        if let Some(app_id) = d.and_then(|d| d.app_id.as_ref()) {
            b.add(
                FieldId::AppId,
                KvKind::Text,
                Validity::Valid,
                UnsupportedWord::NotSupported,
                ptr_bits(app_id),
                |s| s.push_str(app_id),
            );
        }
        let col1 = b.finish();

        // Column 2.
        let mut b = Builder {
            cache: &mut self.cache,
            words: &self.words,
            gw: &gw,
            out: Vec::with_capacity(COLUMNS[1].len()),
        };
        match (d, &self.hashes) {
            (Some(d), Some(view)) => {
                let validity = match d.hashes {
                    // `Computing…` draws in text_secondary (see `to_row`).
                    HashState::Pending => Validity::WarmingUp,
                    HashState::Done { .. } => Validity::Valid,
                    HashState::Failed(_) => Validity::Failed,
                };
                for (i, id) in [FieldId::Sha256, FieldId::Md5].into_iter().enumerate() {
                    let value = match validity {
                        Validity::Failed => self.words.unavailable.clone(),
                        _ => view.shown[i].clone(),
                    };
                    let tooltip = view.full[i].clone().or_else(|| view.error.clone());
                    b.shown(
                        id,
                        KvKind::Hash,
                        validity,
                        value,
                        view.full[i].clone(),
                        tooltip,
                        None,
                    );
                }
            }
            _ => {
                b.collecting(FieldId::Sha256, KvKind::Hash);
                b.collecting(FieldId::Md5, KvKind::Hash);
            }
        }
        match d {
            Some(d) => {
                let words = &self.general_words;
                let (text, tone) = match shown_check(d, self.verify_requested) {
                    PackageCheck::NotVerified | PackageCheck::Failed => {
                        (words.not_verified.clone(), None)
                    }
                    PackageCheck::Verifying => (words.verifying.clone(), None),
                    PackageCheck::Matches => (words.matches.clone(), Some(ValueTone::Success)),
                    PackageCheck::DoesNotMatch => (words.mismatch.clone(), Some(ValueTone::Danger)),
                    PackageCheck::NotPackaged => (words.not_packaged.clone(), None),
                    PackageCheck::NoChecksum => (words.no_checksum.clone(), None),
                };
                b.shown(
                    FieldId::PackageStatus,
                    KvKind::Status,
                    Validity::Valid,
                    text,
                    None,
                    None,
                    tone,
                );
            }
            None => b.collecting(FieldId::PackageStatus, KvKind::Status),
        }
        match d.map(|d| &d.package) {
            Some(p) if p.validity.is_valid() => {
                if let Some(pkg) = &p.value {
                    match &pkg.packager {
                        Some(packager) => b.add(
                            FieldId::Packager,
                            KvKind::Text,
                            Validity::Valid,
                            UnsupportedWord::NotReported,
                            ptr_bits(packager),
                            |s| s.push_str(packager),
                        ),
                        None => b.unsupported(
                            FieldId::Packager,
                            KvKind::Text,
                            UnsupportedWord::NotReported,
                        ),
                    }
                }
            }
            Some(p) => b.add(
                FieldId::Packager,
                KvKind::Text,
                p.validity,
                UnsupportedWord::NotReported,
                0,
                |_| {},
            ),
            None => b.collecting(FieldId::Packager, KvKind::Text),
        }
        let live_count = |b: &mut Builder, id, f: ProcessField, v: Option<u64>| match (live, v) {
            (Some(l), Some(v)) => b.add(
                id,
                KvKind::Text,
                l.get(f),
                UnsupportedWord::NotSupported,
                v,
                |s| units::count(s, v),
            ),
            _ => b.collecting(id, KvKind::Text),
        };
        let live_size = |b: &mut Builder, id, f: ProcessField, v: Option<u64>| match (live, v) {
            (Some(l), Some(v)) => b.add(
                id,
                KvKind::Text,
                l.get(f),
                UnsupportedWord::NotSupported,
                v,
                |s| units::bytes(s, v),
            ),
            _ => b.collecting(id, KvKind::Text),
        };
        live_count(
            &mut b,
            FieldId::Threads,
            ProcessField::Threads,
            live.map(|l| u64::from(l.threads)),
        );
        match d {
            Some(d) => b.total(
                FieldId::Handles,
                KvKind::Text,
                Some(&d.handles),
                |v| u64::from(*v),
                |v, s| units::count(s, u64::from(*v)),
            ),
            None => b.collecting(FieldId::Handles, KvKind::Text),
        }
        live_size(
            &mut b,
            FieldId::Memory,
            ProcessField::RssAnon,
            live.map(|l| l.rss_anon_bytes),
        );
        live_size(
            &mut b,
            FieldId::WorkingSetTotal,
            ProcessField::Rss,
            live.map(|l| l.rss_bytes),
        );
        b.size(FieldId::WorkingSetShared, d.map(|d| &d.rss_shared_bytes));
        live_size(
            &mut b,
            FieldId::Pss,
            ProcessField::Pss,
            live.map(|l| l.pss_bytes),
        );
        live_size(
            &mut b,
            FieldId::Uss,
            ProcessField::Uss,
            live.map(|l| l.uss_bytes),
        );
        live_size(
            &mut b,
            FieldId::Swap,
            ProcessField::Swap,
            live.map(|l| l.swap_bytes),
        );
        b.size(FieldId::Commit, d.map(|d| &d.commit_bytes));
        match live {
            Some(l) if self.mem_total_bytes > 0 => {
                let pct = (l.rss_anon_bytes as f64 * 100. / self.mem_total_bytes as f64) as f32;
                b.add(
                    FieldId::MemoryPct,
                    KvKind::Text,
                    l.get(ProcessField::RssAnon),
                    UnsupportedWord::NotSupported,
                    u64::from(pct.to_bits()),
                    |s| units::percent(s, pct),
                );
            }
            _ => b.collecting(FieldId::MemoryPct, KvKind::Text),
        }
        match live {
            Some(l) => {
                let pct = l.cpu_pct;
                b.add(
                    FieldId::CpuPct,
                    KvKind::Text,
                    l.get(ProcessField::Cpu),
                    UnsupportedWord::NotSupported,
                    u64::from(pct.to_bits()),
                    |s| units::percent(s, pct),
                );
            }
            None => b.collecting(FieldId::CpuPct, KvKind::Text),
        }
        b.total(
            FieldId::CpuTime,
            KvKind::Text,
            d.map(|d| &d.cpu_time_ms),
            |v| *v / 1000,
            |v, s| units::cpu_time(s, *v / 1000),
        );
        b.size(FieldId::IoReadBytes, d.map(|d| &d.io_rchar));
        b.size(FieldId::IoWriteBytes, d.map(|d| &d.io_wchar));
        b.count(FieldId::IoReadOps, d.map(|d| &d.io_syscr));
        b.count(FieldId::IoWriteOps, d.map(|d| &d.io_syscw));
        let col2 = b.finish();

        // Column 3.
        let mut b = Builder {
            cache: &mut self.cache,
            words: &self.words,
            gw: &gw,
            out: Vec::with_capacity(COLUMNS[2].len()),
        };
        b.size(FieldId::DiskReadBytes, d.map(|d| &d.disk_read_bytes));
        b.size(FieldId::DiskWriteBytes, d.map(|d| &d.disk_write_bytes));
        b.size(
            FieldId::CancelledWriteBytes,
            d.map(|d| &d.cancelled_write_bytes),
        );
        let rate = |b: &mut Builder, id, validity: Option<Validity>, bps: f64| match validity {
            Some(v) => b.add(
                id,
                KvKind::Text,
                v,
                UnsupportedWord::NotSupported,
                bps.to_bits(),
                |s| units::rate_mib_fixed(s, bps),
            ),
            None => b.collecting(id, KvKind::Text),
        };
        let read = live.map(|l| (l.get(ProcessField::IoRead), f64::from(l.io_read_bps)));
        let write = live.map(|l| (l.get(ProcessField::IoWrite), f64::from(l.io_write_bps)));
        rate(
            &mut b,
            FieldId::DiskReadRate,
            read.map(|r| r.0),
            read.map_or(0., |r| r.1),
        );
        rate(
            &mut b,
            FieldId::DiskWriteRate,
            write.map(|w| w.0),
            write.map_or(0., |w| w.1),
        );
        rate(
            &mut b,
            FieldId::DiskTotalRate,
            read.zip(write).map(|(r, w)| worst(r.0, w.0)),
            read.map_or(0., |r| r.1) + write.map_or(0., |w| w.1),
        );
        match live {
            Some(l) => {
                let validity = worst(l.get(ProcessField::NetRx), l.get(ProcessField::NetTx));
                let (rx, tx) = (f64::from(l.net_rx_bps), f64::from(l.net_tx_bps));
                let unit = net_unit(&self.units);
                let auto = self.units.network_auto_scale;
                b.add(
                    FieldId::NetIo,
                    KvKind::Text,
                    validity,
                    UnsupportedWord::NotSupported,
                    hash_of((rx.to_bits(), tx.to_bits(), unit == NetUnit::Bits, auto)),
                    |s| fmt_net_io(s, &gw, rx, tx, unit, auto),
                );
            }
            None => b.collecting(FieldId::NetIo, KvKind::Text),
        }
        match live {
            Some(l) => {
                let pct = l.gpu_util_pct;
                b.add(
                    FieldId::GpuPct,
                    KvKind::Text,
                    l.get(ProcessField::GpuUtil),
                    UnsupportedWord::NotSupported,
                    u64::from(pct.to_bits()),
                    |s| units::percent(s, pct),
                );
            }
            None => b.collecting(FieldId::GpuPct, KvKind::Text),
        }
        live_size(
            &mut b,
            FieldId::Vram,
            ProcessField::GpuMem,
            live.map(|l| l.gpu_mem_bytes),
        );
        b.size(FieldId::GpuMemShared, d.map(|d| &d.gpu_mem_shared_bytes));
        b.total(
            FieldId::GpuEngine,
            KvKind::Text,
            d.map(|d| &d.gpu_engine),
            |v| v.map_or(0, |e| e as u64 + 1),
            |v, s| s.push_str(gpu_engine_word(&gw, *v)),
        );
        window_block(&mut b, self.windows.as_ref(), &self.general_words);
        let col3 = b.finish();

        self.columns = [col1, col2, col3];
        self.apply_filter();
    }

    /// Label column width per column: widest label + `sp5`, clamped to 96..=150 × ui_scale.
    /// Measured once per `style_version`.
    fn label_widths(&mut self, m: &UiMetrics, window: &Window, cx: &mut App) -> [Pixels; 3] {
        if let Some((version, widths)) = self.label_widths
            && version == m.style_version
        {
            return widths;
        }
        if !cx.has_global::<TextMetricsCache>() {
            cx.set_global(TextMetricsCache::default());
        }
        let size = m.text.base;
        let words = self.general_words.clone();
        let widths = cx.update_global::<TextMetricsCache, _>(|cache, cx| {
            cache.clear_if_stale(m.style_version);
            COLUMNS.map(|ids| {
                let widest = ids
                    .iter()
                    .map(|id| {
                        words.label(*id).chars().fold(px(0.), |sum, ch| {
                            sum + cache.char_advance(ch, size, window, cx)
                        })
                    })
                    .fold(px(0.), |a, b| a.max(b));
                (widest + m.sp[5])
                    .max(px(LABEL_MIN * m.ui_scale))
                    .min(px(LABEL_MAX * m.ui_scale))
            })
        });
        self.label_widths = Some((m.style_version, widths));
        widths
    }
}

/// The window block: the topmost window's six fields and "And N more", or one `Window:` row.
fn window_block(b: &mut Builder, view: Option<&WindowsView>, words: &GeneralWords) {
    let Some(view) = view else {
        b.collecting(FieldId::Window, KvKind::Text);
        return;
    };
    match view.validity {
        Validity::Valid | Validity::Stale => {}
        Validity::Unsupported => {
            let value = b.words.unsupported(UnsupportedWord::NotSupported).clone();
            b.shown(
                FieldId::Window,
                KvKind::Text,
                Validity::Unsupported,
                value,
                None,
                Some(words.window_no_bridge.clone()),
                None,
            );
            return;
        }
        v => {
            b.add(
                FieldId::Window,
                KvKind::Text,
                v,
                UnsupportedWord::NotSupported,
                0,
                |_| {},
            );
            return;
        }
    }
    let v = view.validity;
    let Some(w) = view.entries.first() else {
        b.shown(
            FieldId::Window,
            KvKind::Text,
            v,
            words.window_none.clone(),
            None,
            None,
            None,
        );
        return;
    };
    let text = |b: &mut Builder, id, value: &Arc<str>| {
        b.add(
            id,
            KvKind::Text,
            v,
            UnsupportedWord::NotSupported,
            ptr_bits(value),
            |s| escape_controls(s, value),
        );
    };
    text(b, FieldId::WindowTitle, &w.caption);
    text(b, FieldId::WindowId, &w.internal_id);
    b.add(
        FieldId::WindowState,
        KvKind::Text,
        v,
        UnsupportedWord::NotSupported,
        u64::from(w.minimized) | (u64::from(w.maximized) << 1),
        |s| s.push_str(window_state_word(words, w)),
    );
    text(b, FieldId::WindowClass, &w.resource_class);
    let g = w.frame_geometry;
    let g_bits = hash_of((g.x, g.y, g.width, g.height));
    b.add(
        FieldId::WindowRect,
        KvKind::Text,
        v,
        UnsupportedWord::NotSupported,
        g_bits,
        |s| fmt_window_rect(s, g),
    );
    b.add(
        FieldId::WindowSize,
        KvKind::Text,
        v,
        UnsupportedWord::NotSupported,
        g_bits,
        |s| fmt_window_size(s, g),
    );
    if let Some((more, tip)) = &view.more {
        // Stale draws the plain text_secondary row.
        b.shown(
            FieldId::WindowMore,
            KvKind::Text,
            Validity::Stale,
            more.clone(),
            None,
            Some(tip.clone()),
            None,
        );
    }
}

fn section(rows: Vec<TlKvRow>) -> Rc<[TlKvSection]> {
    Rc::from(vec![TlKvSection {
        title: None,
        blocks: vec![TlKvBlock {
            indent: None,
            heading: None,
            rows,
        }],
    }])
}

/// Grid row for `field`. Restricted values get the R-050 tooltip; a placeholder word that is not
/// the grid's own word (`Computing…`) and exited links draw as Stale, which keeps the text and
/// uses `text_secondary`. A deleted executable draws in `status.warning`. A Valid link field of
/// a running process draws as a link with its action's tooltip.
fn to_row(field: &KvField, exited: bool, words: &GeneralWords) -> TlKvRow {
    let mut value = TlValue::new(field.value.clone());
    value.path = field.id.is_path();
    if field.id == FieldId::Executable && is_deleted_exe(field, words) {
        value.tone = Some(ValueTone::Warning);
    }
    if field.tone.is_some() {
        value.tone = field.tone;
    }
    value.tooltip = field.tooltip.clone();
    value.validity = match field.validity {
        Validity::Valid if exited && matches!(field.kind, KvKind::Link(_)) => Validity::Stale,
        Validity::WarmingUp if field.value != words.collecting => Validity::Stale,
        Validity::Restricted => {
            value.tooltip = Some(words.restricted_tooltip.clone());
            Validity::Restricted
        }
        Validity::Unsupported => {
            value.word = Some(field.word.unwrap_or_default());
            Validity::Unsupported
        }
        v => v,
    };
    if !exited
        && value.validity == Validity::Valid
        && let Some((action, tip)) = link_of(field, words)
    {
        value.link = Some(action);
        value.tooltip = Some(tip);
    }
    TlKvRow {
        label: field.label.clone(),
        value,
    }
}

/// The action and tooltip of a link field. A deleted executable has no file location to open;
/// the Unit link comes with E23-016.
fn link_of(field: &KvField, words: &GeneralWords) -> Option<(Box<dyn Action>, SharedString)> {
    let KvKind::Link(target) = field.kind else {
        return None;
    };
    match target {
        LinkTarget::Parent => Some((Box::new(GeneralGoToParent), words.link_parent.clone())),
        LinkTarget::Executable if !is_deleted_exe(field, words) => {
            Some((Box::new(GeneralOpenFileLocation), words.link_exe.clone()))
        }
        LinkTarget::Affinity => Some((Box::new(GeneralSetAffinity), words.link_affinity.clone())),
        LinkTarget::Executable | LinkTarget::Unit => None,
    }
}

/// The Executable value ends with " (deleted)" in the current words.
fn is_deleted_exe(field: &KvField, words: &GeneralWords) -> bool {
    field
        .value
        .strip_suffix(words.exe_deleted.as_ref())
        .is_some_and(|rest| rest.ends_with(' '))
}

/// "Label: value" with the label's ":" not doubled.
fn line(label: &str, value: &str) -> String {
    let label = label.trim_end();
    let label = label.strip_suffix(':').unwrap_or(label);
    format!("{label}: {value}")
}

/// A toast in `window`'s own stack.
fn window_toast(window: &mut Window, cx: &mut App, kind: ToastKind, text: SharedString) {
    match ToastQueue::of_window(window, cx) {
        Some(queue) => queue.update(cx, |q, cx| q.push(kind, text, None, None, cx)),
        None => warn!(target: "tasklens::ui", "no toast stack for the General tab"),
    }
}

/// The Verify against package button in its own row, aligned with the value column.
fn verify_row(
    button: VerifyButton,
    focus: FocusHandle,
    label_width: Pixels,
    height: Pixels,
) -> impl IntoElement {
    let mut b = TlButton::new("general-verify", TlButtonKind::Secondary)
        .size(TlButtonSize::Sm)
        .focus_handle(focus)
        .label(button.label)
        .action(Box::new(PropsVerifyPackage))
        .loading(button.loading)
        .disabled(button.disabled);
    if let Some(tooltip) = button.tooltip {
        b = b.tooltip(tooltip);
    }
    h_flex()
        .debug_selector(|| "GeneralTab-verify-row".into())
        .h(height)
        .pl(label_width)
        .items_center()
        .child(tab_slot(TAB_VERIFY, b))
}

/// `child` at tab index `index` of the tab's tab group. The slot itself is not a tab stop.
fn tab_slot(index: isize, child: impl IntoElement) -> gpui_kit::Div {
    div()
        .tab_group()
        .tab_index(index)
        .tab_stop(false)
        .child(child)
}

impl Render for GeneralTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *metrics(cx);
        let theme = current(cx).clone();
        if self.toasts.is_none() {
            self.toasts = ToastQueue::of_window(window, cx).map(|q| q.downgrade());
        }
        let label_widths = self.label_widths(&m, window, cx);
        let measured = self.width.get();
        let body_width = if measured > px(0.) {
            measured
        } else {
            window.viewport_size().width - m.sp[5] * 2.
        };
        let wide = body_width >= px(THREE_COLUMN_MIN * m.ui_scale);
        let row_height = px(ROW_HEIGHT * m.ui_scale);
        let focused = self.focus.contains_focused(window, cx);
        let view = cx.entity().downgrade();
        let value_copied = self.general_words.toast_value_copied.clone();
        // Grid `part` (0 the first, 1 the tail) of column `col`.
        let grid = |id: gpui_kit::ElementId, col: usize, part: usize, sections: &[TlKvSection]| {
            let offset = if part == 0 { 0 } else { self.main_len[col] };
            let view = view.clone();
            let copied = value_copied.clone();
            TlKeyValueGrid::new(id, KvLayout::Inline)
                .row_height(row_height)
                .label_width(label_widths[col])
                .sections(sections)
                .copy_on_click(true)
                .link_tab_stop(false)
                .copy_texts(self.copies[col][part].clone())
                .cursor(self.cursor_in(col, part, focused))
                .on_copied(move |_, window, cx| {
                    window_toast(window, cx, ToastKind::Success, copied.clone());
                })
                .on_value_mouse_down({
                    let view = view.clone();
                    move |n, _, cx| {
                        let _ = view.update(cx, |this, cx| this.set_cursor(col, offset + n, cx));
                    }
                })
                .on_context_menu(move |n, position, window, cx| {
                    let _ = view.update(cx, |this, cx| {
                        this.open_field_menu(col, offset + n, Some(position), window, cx);
                    });
                })
        };

        let panels: Vec<AnyElement> = (0..3)
            .map(|i| {
                let grid_main = grid(("general-col", i).into(), i, 0, &self.sections[i]);
                let verify = (i == 1 && self.verify_row)
                    .then(|| self.verify_button())
                    .flatten()
                    .map(|button| {
                        let tail = grid("general-col-tail".into(), i, 1, &self.col2_tail);
                        let focus = self.verify_focus.clone();
                        (verify_row(button, focus, label_widths[i], row_height), tail)
                    });
                let windows = (i == 2 && !self.col3_tail.is_empty()).then(|| {
                    let tail = grid("general-col-windows".into(), i, 1, &self.col3_tail);
                    let divider = div()
                        .debug_selector(|| "GeneralTab-window-divider".into())
                        .mt(px(WINDOW_GAP * m.ui_scale))
                        .h(px(1.))
                        .w_full()
                        .bg(theme.divider);
                    (divider, tail)
                });
                let bounds = self.panel_bounds.clone();
                div()
                    .debug_selector(move || format!("GeneralTab-panel-{i}"))
                    .relative()
                    .min_w_0()
                    .when(wide, |d| d.flex_1().flex_basis(px(0.)))
                    .when(!wide, |d| d.w_full())
                    .bg(theme.list_bg)
                    .rounded(m.radius_small)
                    .px(m.sp[4])
                    .py(m.sp[3])
                    .child(
                        canvas(move |b, _, _| bounds[i].set(b), |_, _, _, _| {})
                            .absolute()
                            .inset_0(),
                    )
                    .child(grid_main)
                    .when_some(verify, |d, (row, tail)| d.child(row).child(tail))
                    .when_some(windows, |d, (divider, tail)| d.child(divider).child(tail))
                    .into_any_element()
            })
            .collect();
        let panels = if wide {
            h_flex()
                .debug_selector(|| "GeneralTab-panels".into())
                .w_full()
                .items_start()
                .gap(m.sp[4])
                .children(panels)
        } else {
            v_flex()
                .debug_selector(|| "GeneralTab-panels".into())
                .w_full()
                .gap(m.sp[4])
                .children(panels)
        };
        let panels = div()
            .id("general-fields")
            .key_context(KV_GRID_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &PropsFieldUp, _, cx| this.step_row(-1, cx)))
            .on_action(cx.listener(|this, _: &PropsFieldDown, _, cx| this.step_row(1, cx)))
            .on_action(cx.listener(|this, _: &PropsFieldLeft, _, cx| this.step_column(false, cx)))
            .on_action(cx.listener(|this, _: &PropsFieldRight, _, cx| this.step_column(true, cx)))
            .on_action(cx.listener(|this, _: &PropsFieldHome, _, cx| this.row_end(false, cx)))
            .on_action(cx.listener(|this, _: &PropsFieldEnd, _, cx| this.row_end(true, cx)))
            .on_action(cx.listener(|this, _: &PropsActivateField, window, cx| {
                this.activate(window, cx);
            }))
            .on_action(cx.listener(|this, _: &PropsCopyValue, window, cx| {
                this.copy_value(window, cx);
            }))
            // Ctrl+C without a text selection, from inside a grid.
            .on_action(cx.listener(|this, _: &Copy, window, cx| this.copy_value(window, cx)))
            .on_action(cx.listener(|this, _: &GeneralCopyRow, window, cx| {
                this.copy_row(window, cx);
            }))
            .on_action(cx.listener(|this, _: &GeneralCopySection, window, cx| {
                this.copy_section(window, cx);
            }))
            .on_action(cx.listener(|this, _: &PropsCopyAllFields, window, cx| {
                this.copy_all_fields(window, cx);
            }))
            .on_action(cx.listener(|this, _: &PropsFieldMenu, window, cx| {
                this.on_menu_key(window, cx);
            }))
            .on_action(cx.listener(|this, _: &GeneralGoToParent, window, cx| {
                this.go_to_parent(window, cx);
            }))
            .on_action(
                cx.listener(|this, _: &GeneralOpenFileLocation, window, cx| {
                    this.open_file_location(window, cx);
                }),
            )
            .on_action(cx.listener(|this, _: &GeneralSetAffinity, window, cx| {
                this.set_affinity(window, cx);
            }))
            .w_full()
            .child(panels);

        let width = self.width.clone();
        let pad = m.sp[5] * 2.;
        div()
            .debug_selector(|| "GeneralTab".into())
            .on_action(cx.listener(|this, _: &PropsVerifyPackage, _, cx| {
                this.verify_package(cx);
            }))
            .relative()
            .size_full()
            .child(
                canvas(
                    move |bounds, window, _| {
                        let inner = (bounds.size.width - pad).max(px(0.));
                        if width.get() != inner {
                            width.set(inner);
                            window.request_animation_frame();
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(
                div()
                    .id("general-body")
                    .size_full()
                    .overflow_y_scroll()
                    .child(
                        v_flex()
                            // Its own tab group, so the fields, Copy and Verify take the order
                            // of their tab indexes at this point of the window's order.
                            .tab_group()
                            .tab_index(0)
                            .tab_stop(false)
                            .w_full()
                            .px(m.sp[5])
                            .py(m.sp[4])
                            .child(panels)
                            .child(div().mt(m.sp[4]).child(self.cmdline.clone())),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_of(ranges: &[(u16, u16)]) -> CpuSet {
        let mut set = CpuSet::empty();
        for &(a, b) in ranges {
            for cpu in a..=b {
                set.insert(cpu);
            }
        }
        set
    }

    fn w() -> GeneralWords {
        GeneralWords::en()
    }

    #[test]
    fn field_all_matches_declaration_order() {
        for (i, id) in FieldId::ALL.iter().enumerate() {
            assert_eq!(*id as usize, i, "{id:?}");
        }
        let w = w();
        assert_eq!(w.label(FieldId::Process).as_ref(), "Process:");
        assert_eq!(w.label(FieldId::Window).as_ref(), "Window:");
        assert_eq!(w.label(FieldId::WindowMore).as_ref(), "");
    }

    fn run(f: impl FnOnce(&mut String)) -> String {
        let mut s = String::new();
        f(&mut s);
        s
    }

    #[test]
    fn fmt_priority_other_and_fifo() {
        assert_eq!(
            run(|s| fmt_priority(s, &w(), SchedPolicy::Other, 0, 0, 120)),
            "Nice 0, SCHED_OTHER, kernel prio 120"
        );
        assert_eq!(
            run(|s| fmt_priority(s, &w(), SchedPolicy::Fifo, 0, 50, 49)),
            "RT 50, SCHED_FIFO, kernel prio 49"
        );
        assert_eq!(
            run(|s| fmt_priority(s, &w(), SchedPolicy::Batch, -5, 0, 115)),
            "Nice -5, SCHED_BATCH, kernel prio 115"
        );
        assert_eq!(
            run(|s| fmt_priority(s, &w(), SchedPolicy::Deadline, 0, 0, -1)),
            "Nice 0, SCHED_DEADLINE, kernel prio -1"
        );
    }

    #[test]
    fn fmt_affinity_all_and_list_threads_differ() {
        let online = set_of(&[(0, 31)]);
        assert_eq!(
            run(|s| fmt_affinity(s, &w(), &online, &online, false)),
            "All (0-31)"
        );
        let some = set_of(&[(0, 7), (16, 23)]);
        assert_eq!(
            run(|s| fmt_affinity(s, &w(), &some, &online, true)),
            "0-7,16-23 (16 CPUs) (threads differ)"
        );
        assert_eq!(
            run(|s| fmt_affinity(s, &w(), &set_of(&[(3, 3)]), &online, false)),
            "3 (1 CPU)"
        );
        // Online not known yet: never "All".
        assert_eq!(
            run(|s| fmt_affinity(s, &w(), &online, &CpuSet::empty(), false)),
            "0-31 (32 CPUs)"
        );
    }

    #[test]
    fn fmt_user_with_euid() {
        let same = IdTriple {
            real: 1000,
            effective: 1000,
            saved: 1000,
            fs: 1000,
        };
        assert_eq!(
            run(|s| fmt_user(s, &w(), &same, ("oystein", "oystein", "oystein"))),
            "oystein (1000)"
        );
        let setuid = IdTriple {
            real: 1000,
            effective: 0,
            saved: 0,
            fs: 0,
        };
        assert_eq!(
            run(|s| fmt_user(s, &w(), &setuid, ("oystein", "root", "root"))),
            "oystein (1000), euid root (0), suid root (0)"
        );
    }

    #[test]
    fn fmt_aslr_personality() {
        assert_eq!(run(|s| fmt_aslr(s, &w(), 2, false)), "Full (2)");
        assert_eq!(run(|s| fmt_aslr(s, &w(), 1, false)), "Partial (1)");
        assert_eq!(
            run(|s| fmt_aslr(s, &w(), 0, true)),
            "Off (0), disabled for this process"
        );
        assert_eq!(
            run(|s| fmt_aslr(s, &w(), 2, true)),
            "Full (2), disabled for this process"
        );
    }

    #[test]
    fn fmt_seccomp_filters() {
        assert_eq!(run(|s| fmt_seccomp(s, &w(), Seccomp::Disabled)), "Disabled");
        assert_eq!(run(|s| fmt_seccomp(s, &w(), Seccomp::Strict)), "Strict");
        assert_eq!(
            run(|s| fmt_seccomp(s, &w(), Seccomp::Filter { filters: 3 })),
            "Filter (3 filters)"
        );
    }

    #[test]
    fn fmt_hardening_order_and_none() {
        assert_eq!(
            run(|s| fmt_hardening(s, &w(), Hardening::default())),
            "None"
        );
        let all = Hardening {
            pie: true,
            nx_stack: true,
            full_relro: true,
            shadow_stack: true,
            no_new_privs: true,
            seccomp: true,
        };
        assert_eq!(
            run(|s| fmt_hardening(s, &w(), all)),
            "PIE, NX stack, Full RELRO, Shadow stack, NoNewPrivs, Seccomp"
        );
        let some = Hardening {
            nx_stack: true,
            seccomp: true,
            ..Hardening::default()
        };
        assert_eq!(run(|s| fmt_hardening(s, &w(), some)), "NX stack, Seccomp");
    }

    #[test]
    fn fmt_net_io_bits() {
        assert_eq!(
            run(|s| fmt_net_io(s, &w(), 150_000., 37_500., NetUnit::Bits, false)),
            "Receive 1.2 Mbps, send 0.3 Mbps"
        );
    }

    #[test]
    fn window_rect_format() {
        let g = FrameGeometry {
            x: 0,
            y: 32,
            width: 1280,
            height: 800,
        };
        assert_eq!(run(|s| fmt_window_rect(s, g)), "L:0 T:32 R:1280 B:832");
        assert_eq!(run(|s| fmt_window_size(s, g)), "1280 x 800");
        let off = FrameGeometry {
            x: -8,
            y: -4,
            width: 100,
            height: 50,
        };
        assert_eq!(run(|s| fmt_window_rect(s, off)), "L:-8 T:-4 R:92 B:46");
    }

    #[test]
    fn escape_controls_in_cmdline() {
        assert_eq!(
            run(|s| escape_controls(s, "a\tb\u{202E}c")),
            "a\\u{0009}b\\u{202E}c"
        );
        assert_eq!(run(|s| escape_controls(s, "ls -l /tmp")), "ls -l /tmp");
    }
}
