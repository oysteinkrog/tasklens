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
//! Copy, links (E23-011), hashes and Verify (E23-012) and the window block (E23-013) are later
//! beads on this file. Until then link fields draw as plain text and the hash and package status
//! fields show placeholders.
//!
//! The command line is untrusted text (ADR-015). It is shown with control characters escaped and
//! is never logged.

use std::cell::Cell;
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, AppContext as _, BorrowAppContext as _, Context, Entity, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, canvas, div,
    prelude::FluentBuilder as _, px,
};
use rustc_hash::{FxHashMap, FxHasher};
use tl_model::actions::SchedPolicy;
use tl_model::details::{
    ElfArch, ExeLocation, GeneralDetail, GpuEngine, IdTriple, Relro, Seccomp, TabPayload,
};
use tl_model::ids::ProcessKey;
use tl_model::process::ProcessRecord;
use tl_model::settings::{NetworkUnit, UnitsSettings};
use tl_model::system::CpuSet;
use tl_model::table::Total;
use tl_model::units::{self, LocalDateTime, NetUnit};
use tl_model::validity::{ProcessField, Validity};

use super::filter::{FilterChanged, PropsFilter, lowercase, matches};
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::icons::TlIcon;
use crate::components::kv_grid::{
    KvLayout, KvWords, TlKeyValueGrid, TlKvBlock, TlKvRow, TlKvSection, TlValue, UnsupportedWord,
    ValueTone, kv_words,
};
use crate::settings_global::TlSettings;
use crate::text::metrics::TextMetricsCache;
use crate::text::style::{label_style, t_sm};
use crate::theme::{UiMetrics, current, metrics};

/// Tooltip of a Restricted value (R-050, P2 text; E23-025 adds the shield sentence in P3).
pub const RESTRICTED_TOOLTIP: &str = "Needs administrator access.";
/// Title of the command line block.
pub const CMDLINE_TITLE: &str = "Command line";
/// Label of the command line copy button.
pub const CMDLINE_COPY: &str = "Copy";
/// Hash placeholder until E23-012.
pub const COMPUTING: &str = "Computing…";
/// Package status placeholder until E23-012.
pub const NOT_VERIFIED: &str = "Not verified";

const COLLECTING: &str = "Collecting…";

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

/// What a link field opens. The click handlers come with E23-011.
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
}

impl FieldId {
    pub const fn label(self) -> &'static str {
        use FieldId::*;
        match self {
            Process => "Process:",
            Pid => "PID:",
            Started => "Started:",
            Ppid => "PPID:",
            User => "User:",
            Priority => "Priority:",
            Autogroup => "Autogroup:",
            Affinity => "Affinity:",
            Hardening => "Hardening:",
            Pie => "PIE:",
            NxStack => "NX stack:",
            Relro => "RELRO:",
            ShadowStack => "Shadow stack:",
            Aslr => "ASLR:",
            NoNewPrivs => "NoNewPrivs:",
            Seccomp => "Seccomp:",
            Architecture => "Architecture:",
            Executable => "Executable:",
            Location => "Location:",
            Package => "Package:",
            Description => "Description:",
            WorkingDir => "Working dir:",
            Root => "Root:",
            Cgroup => "Cgroup:",
            Unit => "Unit:",
            AppId => "App ID:",
            Sha256 => "SHA-256:",
            Md5 => "MD5:",
            PackageStatus => "Package status:",
            Packager => "Packager:",
            Threads => "Threads:",
            Handles => "Handles:",
            Memory => "Memory:",
            WorkingSetTotal => "Working set total:",
            WorkingSetShared => "Working set shared:",
            Pss => "PSS:",
            Uss => "USS:",
            Swap => "Swap:",
            Commit => "Commit:",
            MemoryPct => "Memory %:",
            CpuPct => "CPU %:",
            CpuTime => "CPU time:",
            IoReadBytes => "I/O read bytes:",
            IoWriteBytes => "I/O write bytes:",
            IoReadOps => "I/O read ops:",
            IoWriteOps => "I/O write ops:",
            DiskReadBytes => "Disk read bytes:",
            DiskWriteBytes => "Disk write bytes:",
            CancelledWriteBytes => "Cancelled write bytes:",
            DiskReadRate => "Disk read rate:",
            DiskWriteRate => "Disk write rate:",
            DiskTotalRate => "Disk total rate:",
            NetIo => "Net I/O:",
            GpuPct => "GPU %:",
            Vram => "VRAM:",
            GpuMemShared => "GPU memory shared:",
            GpuEngine => "GPU engine:",
        }
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
        // The window block (E23-013) follows here.
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
    /// The untruncated text where the value shows a shortened form (hashes, E23-012).
    pub full: Option<SharedString>,
    /// Why an Unsupported value has none.
    pub word: Option<UnsupportedWord>,
}

// ---- Formatters (pure) ----

/// `Nice 0, SCHED_OTHER, kernel prio 120`; realtime `RT 50, SCHED_FIFO, kernel prio 49`.
pub fn fmt_priority(out: &mut String, policy: SchedPolicy, nice: i8, rt_prio: u8, kernel: i32) {
    let name = match policy {
        SchedPolicy::Other => "SCHED_OTHER",
        SchedPolicy::Batch => "SCHED_BATCH",
        SchedPolicy::Idle => "SCHED_IDLE",
        SchedPolicy::Fifo => "SCHED_FIFO",
        SchedPolicy::Rr => "SCHED_RR",
    };
    let _ = match policy {
        SchedPolicy::Fifo | SchedPolicy::Rr => {
            write!(out, "RT {rt_prio}, {name}, kernel prio {kernel}")
        }
        _ => write!(out, "Nice {nice}, {name}, kernel prio {kernel}"),
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
pub fn fmt_affinity(out: &mut String, set: &CpuSet, online: &CpuSet, threads_differ: bool) {
    if !online.is_empty() && set.intersection(online) == *online {
        out.push_str("All (");
        write_ranges(out, online);
        out.push(')');
    } else {
        write_ranges(out, set);
        let n = set.count();
        let _ = write!(out, " ({n} {})", if n == 1 { "CPU" } else { "CPUs" });
    }
    if threads_differ {
        out.push_str(" (threads differ)");
    }
}

/// `oystein (1000)`; when the effective or saved uid differs from the real one:
/// `oystein (1000), euid root (0), suid root (0)`. `names` are the real, effective and saved
/// user names.
pub fn fmt_user(out: &mut String, ids: &IdTriple, names: (&str, &str, &str)) {
    let (real, effective, saved) = names;
    let _ = write!(out, "{real} ({})", ids.real);
    if ids.effective != ids.real || ids.saved != ids.real {
        let _ = write!(
            out,
            ", euid {effective} ({}), suid {saved} ({})",
            ids.effective, ids.saved
        );
    }
}

/// `Full (2)`, `Partial (1)` or `Off (0)` from randomize_va_space, plus
/// `, disabled for this process` with ADDR_NO_RANDOMIZE.
pub fn fmt_aslr(out: &mut String, system: u8, disabled_for_process: bool) {
    let _ = match system {
        0 => write!(out, "Off (0)"),
        1 => write!(out, "Partial (1)"),
        2 => write!(out, "Full (2)"),
        n => write!(out, "Unknown ({n})"),
    };
    if disabled_for_process {
        out.push_str(", disabled for this process");
    }
}

/// `Disabled`, `Strict` or `Filter (3 filters)`.
pub fn fmt_seccomp(out: &mut String, seccomp: Seccomp) {
    let _ = match seccomp {
        Seccomp::Disabled => write!(out, "Disabled"),
        Seccomp::Strict => write!(out, "Strict"),
        Seccomp::Filter { filters: 1 } => write!(out, "Filter (1 filter)"),
        Seccomp::Filter { filters } => write!(out, "Filter ({filters} filters)"),
    };
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
pub fn fmt_hardening(out: &mut String, h: Hardening) {
    let items = [
        (h.pie, "PIE"),
        (h.nx_stack, "NX stack"),
        (h.full_relro, "Full RELRO"),
        (h.shadow_stack, "Shadow stack"),
        (h.no_new_privs, "NoNewPrivs"),
        (h.seccomp, "Seccomp"),
    ];
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
        out.push_str("None");
    }
}

/// `Receive 1.2 Mbps, send 0.3 Mbps` in the `units.network` unit. Rates in bytes per second.
pub fn fmt_net_io(out: &mut String, rx_bps: f64, tx_bps: f64, unit: NetUnit, auto_scale: bool) {
    out.push_str("Receive ");
    units::net_rate(out, rx_bps, unit, auto_scale);
    out.push_str(", send ");
    units::net_rate(out, tx_bps, unit, auto_scale);
}

fn arch_word(arch: ElfArch) -> &'static str {
    match arch {
        ElfArch::X86_64 => "x86-64",
        ElfArch::X86 => "x86 (32-bit)",
        ElfArch::AArch64 => "AArch64",
        ElfArch::Unknown => "Unknown",
    }
}

fn location_word(location: ExeLocation) -> &'static str {
    match location {
        ExeLocation::TempDirectory => "Temp directory",
        ExeLocation::UserDownloads => "User Downloads",
        ExeLocation::UserData => "User data",
        ExeLocation::UserProfile => "User profile",
        ExeLocation::System => "System",
        ExeLocation::OptionalSoftware => "Optional software",
        ExeLocation::Flatpak => "Flatpak",
        ExeLocation::AppImage => "AppImage",
        ExeLocation::WinePrefix => "Wine prefix",
        ExeLocation::Deleted => "Deleted",
        ExeLocation::Other => "Other",
    }
}

fn gpu_engine_word(engine: Option<GpuEngine>) -> &'static str {
    match engine {
        Some(GpuEngine::ThreeD) => "3D",
        Some(GpuEngine::Compute) => "Compute",
        Some(GpuEngine::VideoDecode) => "Video decode",
        Some(GpuEngine::VideoEncode) => "Video encode",
        None => "None",
    }
}

fn yes_no(yes: bool) -> &'static str {
    if yes { "Yes" } else { "No" }
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
fn write_local(out: &mut String, unix_s: i64) {
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
        Err(_) => out.push_str("Unknown"),
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
            Validity::WarmingUp => COLLECTING.into(),
            Validity::Restricted => self.words.restricted.clone(),
            Validity::Unsupported => self.words.unsupported(unsupported).clone(),
            Validity::Failed => self.words.unavailable.clone(),
            Validity::Missing => return,
        };
        self.out.push(KvField {
            id,
            label: id.label().into(),
            value,
            kind,
            validity,
            full: None,
            word: (validity == Validity::Unsupported).then_some(unsupported),
        });
    }

    /// A field with a fixed text.
    fn word(&mut self, id: FieldId, kind: KvKind, validity: Validity, text: &'static str) {
        self.out.push(KvField {
            id,
            label: id.label().into(),
            value: text.into(),
            kind,
            validity,
            full: None,
            word: None,
        });
    }

    /// An Unsupported field with no payload value, showing `word`.
    fn unsupported(&mut self, id: FieldId, kind: KvKind, word: UnsupportedWord) {
        self.out.push(KvField {
            id,
            label: id.label().into(),
            value: self.words.unsupported(word).clone(),
            kind,
            validity: Validity::Unsupported,
            full: None,
            word: Some(word),
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
            None => self.word(id, kind, Validity::WarmingUp, COLLECTING),
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
    source: Option<Arc<str>>,
    validity: Validity,
    words: Rc<KvWords>,
    text: SharedString,
    shape_count: u64,
}

impl CmdLineBlock {
    fn new(words: Rc<KvWords>) -> Self {
        Self {
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
        let theme = current(cx);
        let mut title = div().flex_1().min_w_0();
        *title.text_style() = label_style(&m, m.text.base, FontWeight::MEDIUM);
        let title = title
            .truncate()
            .text_color(theme.text_secondary)
            .child(CMDLINE_TITLE);
        let copy = TlButton::new("general-cmdline-copy", TlButtonKind::Ghost)
            .size(TlButtonSize::Sm)
            .icon(TlIcon::Copy)
            .icon_size(px(COPY_ICON * m.ui_scale))
            .label(CMDLINE_COPY);
        let valid = self.validity.is_valid() || self.validity == Validity::Stale;
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
                    .child(copy),
            )
            .child(body)
    }
}

// ---- The tab ----

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
    columns: [Arc<[KvField]>; 3],
    /// Grid sections per column, after the filter.
    sections: [Rc<[TlKvSection]>; 3],
    /// Label column widths per column, with the `style_version` they were measured for.
    label_widths: Option<(u64, [Pixels; 3])>,
    /// Body width from the last paint (inside the padding), zero before the first.
    width: Rc<Cell<Pixels>>,
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
            columns: [Arc::from([]), Arc::from([]), Arc::from([])],
            sections: [Rc::from([]), Rc::from([]), Rc::from([])],
            label_widths: None,
            width: Rc::default(),
            cmdline: cx.new(|cx| CmdLineBlock::new(kv_words(cx))),
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
        self.sections[i]
            .iter()
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

    /// Converts the built fields into grid rows, dropping the ones the filter hides.
    fn apply_filter(&mut self) {
        for i in 0..3 {
            let rows: Vec<TlKvRow> = self.columns[i]
                .iter()
                .filter(|f| self.passes(f))
                .map(|f| to_row(f, self.exited))
                .collect();
            self.sections[i] = Rc::from(vec![TlKvSection {
                title: None,
                blocks: vec![TlKvBlock {
                    heading: None,
                    rows,
                }],
            }]);
        }
    }

    fn rebuild(&mut self, cx: &mut App) {
        self.words = kv_words(cx);
        let d = self.payload.clone();
        let d = d.as_deref();
        let live = self.live.clone();
        let live = live.as_ref();
        let none = || Validity::WarmingUp;

        // Column 1.
        let mut b = Builder {
            cache: &mut self.cache,
            words: &self.words,
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
            None => b.word(FieldId::Process, KvKind::Text, none(), COLLECTING),
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
            |v, s| write_local(s, *v),
        );
        match live {
            Some(l) if l.ppid == 0 => {
                b.word(FieldId::Ppid, KvKind::Text, Validity::Valid, "0 (none)")
            }
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
            None => b.word(FieldId::Ppid, KvKind::Text, none(), COLLECTING),
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
                    |out| fmt_user(out, ids, (r, e, s)),
                );
            }
            None => b.word(FieldId::User, KvKind::Text, none(), COLLECTING),
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
                    |s| fmt_priority(s, policy, nice, rt, kernel),
                );
            }
            None => b.word(FieldId::Priority, KvKind::Text, none(), COLLECTING),
        }
        b.total(
            FieldId::Autogroup,
            KvKind::Text,
            d.map(|d| &d.autogroup),
            |(name, nice)| hash_of((ptr_bits(name), *nice)),
            |(name, nice), s| {
                let _ = write!(s, "{name} nice {nice}");
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
                |set, s| fmt_affinity(s, set, &online, differ),
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
                    |s| fmt_hardening(s, h),
                );
            }
            None => b.word(FieldId::Hardening, KvKind::Text, none(), COLLECTING),
        }
        b.total(
            FieldId::Pie,
            KvKind::Text,
            d.map(|d| &d.pie),
            |v| u64::from(*v),
            |v, s| s.push_str(yes_no(*v)),
        );
        b.total(
            FieldId::NxStack,
            KvKind::Text,
            d.map(|d| &d.nx_stack),
            |v| u64::from(*v),
            |v, s| s.push_str(if *v { "Yes" } else { "No (executable stack)" }),
        );
        b.total(
            FieldId::Relro,
            KvKind::Text,
            d.map(|d| &d.relro),
            |v| *v as u64,
            |v, s| {
                s.push_str(match v {
                    Relro::Full => "Full",
                    Relro::Partial => "Partial",
                    Relro::None => "None",
                })
            },
        );
        b.total(
            FieldId::ShadowStack,
            KvKind::Text,
            d.map(|d| &d.shadow_stack),
            |v| u64::from(*v),
            |v, s| s.push_str(if *v { "Enabled" } else { "Disabled" }),
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
                    |s| fmt_aslr(s, sys, off),
                );
            }
            None => b.word(FieldId::Aslr, KvKind::Text, none(), COLLECTING),
        }
        b.total(
            FieldId::NoNewPrivs,
            KvKind::Text,
            d.map(|d| &d.no_new_privs),
            |v| u64::from(*v),
            |v, s| s.push_str(yes_no(*v)),
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
            |v, s| fmt_seccomp(s, *v),
        );
        b.total(
            FieldId::Architecture,
            KvKind::Text,
            d.map(|d| &d.architecture),
            |v| *v as u64,
            |v, s| s.push_str(arch_word(*v)),
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
                        s.push_str(v);
                        if !v.ends_with(" (deleted)") {
                            s.push_str(" (deleted)");
                        }
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
            |v, s| s.push_str(location_word(*v)),
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
                None => s.push_str("Not packaged"),
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
            None => b.word(FieldId::Cgroup, KvKind::Text, none(), COLLECTING),
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
            out: Vec::with_capacity(COLUMNS[1].len()),
        };
        let (hash_validity, hash_text) = match d {
            // Placeholders in text_secondary until E23-012.
            Some(_) => (Validity::WarmingUp, COMPUTING),
            None => (Validity::WarmingUp, COLLECTING),
        };
        b.word(FieldId::Sha256, KvKind::Hash, hash_validity, hash_text);
        b.word(FieldId::Md5, KvKind::Hash, hash_validity, hash_text);
        match d {
            Some(_) => b.word(
                FieldId::PackageStatus,
                KvKind::Status,
                Validity::Valid,
                NOT_VERIFIED,
            ),
            None => b.word(FieldId::PackageStatus, KvKind::Status, none(), COLLECTING),
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
            None => b.word(FieldId::Packager, KvKind::Text, none(), COLLECTING),
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
            _ => b.word(id, KvKind::Text, Validity::WarmingUp, COLLECTING),
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
            _ => b.word(id, KvKind::Text, Validity::WarmingUp, COLLECTING),
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
            None => b.word(FieldId::Handles, KvKind::Text, none(), COLLECTING),
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
            _ => b.word(FieldId::MemoryPct, KvKind::Text, none(), COLLECTING),
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
            None => b.word(FieldId::CpuPct, KvKind::Text, none(), COLLECTING),
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
            None => b.word(id, KvKind::Text, Validity::WarmingUp, COLLECTING),
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
                    |s| fmt_net_io(s, rx, tx, unit, auto),
                );
            }
            None => b.word(FieldId::NetIo, KvKind::Text, none(), COLLECTING),
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
            None => b.word(FieldId::GpuPct, KvKind::Text, none(), COLLECTING),
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
            |v, s| s.push_str(gpu_engine_word(*v)),
        );
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
        let widths = cx.update_global::<TextMetricsCache, _>(|cache, cx| {
            cache.clear_if_stale(m.style_version);
            COLUMNS.map(|ids| {
                let widest = ids
                    .iter()
                    .map(|id| {
                        id.label().chars().fold(px(0.), |sum, ch| {
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

/// Grid row for `field`. Restricted values get the R-050 tooltip; a placeholder word that is not
/// the grid's own word (`Computing…`) and exited links draw as Stale, which keeps the text and
/// uses `text_secondary`. A deleted executable draws in `status.warning`.
fn to_row(field: &KvField, exited: bool) -> TlKvRow {
    let mut value = TlValue::new(field.value.clone());
    value.path = field.id.is_path();
    if field.id == FieldId::Executable && field.value.ends_with(" (deleted)") {
        value.tone = Some(ValueTone::Warning);
    }
    value.validity = match field.validity {
        Validity::Valid if exited && matches!(field.kind, KvKind::Link(_)) => Validity::Stale,
        Validity::WarmingUp if field.value.as_ref() != COLLECTING => Validity::Stale,
        Validity::Restricted => {
            value.tooltip = Some(RESTRICTED_TOOLTIP.into());
            Validity::Restricted
        }
        Validity::Unsupported => {
            value.word = Some(field.word.unwrap_or_default());
            Validity::Unsupported
        }
        v => v,
    };
    TlKvRow {
        label: field.label.clone(),
        value,
    }
}

impl Render for GeneralTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let label_widths = self.label_widths(&m, window, cx);
        let measured = self.width.get();
        let body_width = if measured > px(0.) {
            measured
        } else {
            window.viewport_size().width - m.sp[5] * 2.
        };
        let wide = body_width >= px(THREE_COLUMN_MIN * m.ui_scale);
        let row_height = px(ROW_HEIGHT * m.ui_scale);

        let panels: Vec<AnyElement> = (0..3)
            .map(|i| {
                let grid = TlKeyValueGrid::new(("general-col", i), KvLayout::Inline)
                    .row_height(row_height)
                    .label_width(label_widths[i])
                    .sections(&self.sections[i]);
                div()
                    .debug_selector(move || format!("GeneralTab-panel-{i}"))
                    .min_w_0()
                    .when(wide, |d| d.flex_1().flex_basis(px(0.)))
                    .when(!wide, |d| d.w_full())
                    .bg(theme.list_bg)
                    .rounded(m.radius_small)
                    .px(m.sp[4])
                    .py(m.sp[3])
                    .child(grid)
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

        let width = self.width.clone();
        let pad = m.sp[5] * 2.;
        div()
            .debug_selector(|| "GeneralTab".into())
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

    fn run(f: impl FnOnce(&mut String)) -> String {
        let mut s = String::new();
        f(&mut s);
        s
    }

    #[test]
    fn fmt_priority_other_and_fifo() {
        assert_eq!(
            run(|s| fmt_priority(s, SchedPolicy::Other, 0, 0, 120)),
            "Nice 0, SCHED_OTHER, kernel prio 120"
        );
        assert_eq!(
            run(|s| fmt_priority(s, SchedPolicy::Fifo, 0, 50, 49)),
            "RT 50, SCHED_FIFO, kernel prio 49"
        );
        assert_eq!(
            run(|s| fmt_priority(s, SchedPolicy::Batch, -5, 0, 115)),
            "Nice -5, SCHED_BATCH, kernel prio 115"
        );
    }

    #[test]
    fn fmt_affinity_all_and_list_threads_differ() {
        let online = set_of(&[(0, 31)]);
        assert_eq!(
            run(|s| fmt_affinity(s, &online, &online, false)),
            "All (0-31)"
        );
        let some = set_of(&[(0, 7), (16, 23)]);
        assert_eq!(
            run(|s| fmt_affinity(s, &some, &online, true)),
            "0-7,16-23 (16 CPUs) (threads differ)"
        );
        assert_eq!(
            run(|s| fmt_affinity(s, &set_of(&[(3, 3)]), &online, false)),
            "3 (1 CPU)"
        );
        // Online not known yet: never "All".
        assert_eq!(
            run(|s| fmt_affinity(s, &online, &CpuSet::empty(), false)),
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
            run(|s| fmt_user(s, &same, ("oystein", "oystein", "oystein"))),
            "oystein (1000)"
        );
        let setuid = IdTriple {
            real: 1000,
            effective: 0,
            saved: 0,
            fs: 0,
        };
        assert_eq!(
            run(|s| fmt_user(s, &setuid, ("oystein", "root", "root"))),
            "oystein (1000), euid root (0), suid root (0)"
        );
    }

    #[test]
    fn fmt_aslr_personality() {
        assert_eq!(run(|s| fmt_aslr(s, 2, false)), "Full (2)");
        assert_eq!(run(|s| fmt_aslr(s, 1, false)), "Partial (1)");
        assert_eq!(
            run(|s| fmt_aslr(s, 0, true)),
            "Off (0), disabled for this process"
        );
        assert_eq!(
            run(|s| fmt_aslr(s, 2, true)),
            "Full (2), disabled for this process"
        );
    }

    #[test]
    fn fmt_seccomp_filters() {
        assert_eq!(run(|s| fmt_seccomp(s, Seccomp::Disabled)), "Disabled");
        assert_eq!(run(|s| fmt_seccomp(s, Seccomp::Strict)), "Strict");
        assert_eq!(
            run(|s| fmt_seccomp(s, Seccomp::Filter { filters: 3 })),
            "Filter (3 filters)"
        );
    }

    #[test]
    fn fmt_hardening_order_and_none() {
        assert_eq!(run(|s| fmt_hardening(s, Hardening::default())), "None");
        let all = Hardening {
            pie: true,
            nx_stack: true,
            full_relro: true,
            shadow_stack: true,
            no_new_privs: true,
            seccomp: true,
        };
        assert_eq!(
            run(|s| fmt_hardening(s, all)),
            "PIE, NX stack, Full RELRO, Shadow stack, NoNewPrivs, Seccomp"
        );
        let some = Hardening {
            nx_stack: true,
            seccomp: true,
            ..Hardening::default()
        };
        assert_eq!(run(|s| fmt_hardening(s, some)), "NX stack, Seccomp");
    }

    #[test]
    fn fmt_net_io_bits() {
        assert_eq!(
            run(|s| fmt_net_io(s, 150_000., 37_500., NetUnit::Bits, false)),
            "Receive 1.2 Mbps, send 0.3 Mbps"
        );
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
