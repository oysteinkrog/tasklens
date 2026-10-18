//! The Security tab of Process Properties: the summary of identity, capabilities, flags, LSM
//! label, sessions and namespaces in two key/value columns (UI-PWN-014, UI-PWN-018, PROC-082,
//! R-050), followed by the Capabilities and Groups tables (UI-PWN-015).
//!
//! `PropsWindow` feeds the tab its payload and the exited state. The about 21 value strings are
//! built only when the payload changes; the filter only re-picks the rows. Labels and value
//! words are English, as on the General tab.
//!
//! A click on a value copies it (the grid's own copy). The keyboard and the field menu work as
//! on the General tab: one focus stop with a field cursor, Up and Down in a column, Left and
//! Right across, Home and End, Enter and Ctrl+C copy the value, Shift+F10 or the Menu key opens
//! Copy value, Copy row, Copy section and Copy all fields. Keyboard copies go through
//! `util::clipboard_write`; its failure toast "Could not copy token summary value." cannot show
//! in v1 (R-199).
//!
//! The security label is untrusted text: control characters are escaped (E15).

use std::cmp::Ordering;
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::input::Copy;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    AnyView, App, AppContext as _, BorrowAppContext as _, ClipboardItem, ContentMask, Context,
    Entity, FocusHandle, FontWeight, Global, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, PathBuilder, Pixels, Point, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, canvas, div, point, px,
    relative,
};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use tl_model::details::{
    CapRow, Elevated, GroupMembershipRow, IdTriple, NsKind, NsState, Seccomp, SecurityDetail,
    TabPayload,
};
use tl_model::ids::ProcessKey;
use tl_model::table::Total;
use tl_model::units;
use tl_model::validity::Validity;
use tracing::warn;

use super::filter::{FilterChanged, PropsFilter, lowercase, matches};
use super::general::{
    GeneralCopyRow, GeneralCopySection, GeneralWords, escape_controls, general_words,
};
use crate::components::kv_grid::{
    KvLayout, KvWords, TlKeyValueGrid, TlKvBlock, TlKvRow, TlKvSection, TlValue, ValueTone,
    kv_words, shown_text_in, value_copy_form,
};
use crate::components::menu::{TlMenu, open_context_menu_at};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::i18n::{Strings, tr};
use crate::pages::performance::details::Template;
use crate::table::actions::CopyRow;
use crate::table::cells::CellOut;
use crate::table::columns::{
    Availability, ColumnAlign, ColumnKind, ColumnLayout, ColumnSpec, TableColumn, TableId,
};
use crate::table::tree_table::{
    HEADER_H_ONE, HeaderLines, IconSlot, NameCell, NameIcon, RowFlags, RowKind, TableSort,
    TlTreeTable, TreeTableDelegate, TreeTableEvent,
};
use crate::text::metrics::TextMetricsCache;
use crate::text::style::label_style;
use crate::theme::{UiMetrics, current, metrics};
use crate::util::clipboard_write;
use crate::windows::actions::{
    KV_GRID_CONTEXT, PropsActivateField, PropsCopyAllFields, PropsCopyValue, PropsFieldDown,
    PropsFieldEnd, PropsFieldHome, PropsFieldLeft, PropsFieldMenu, PropsFieldRight, PropsFieldUp,
};

/// Row height, × ui_scale.
const ROW_HEIGHT: f32 = 26.;
/// Label column clamp, × ui_scale (UI-PWN-006).
const LABEL_MIN: f32 = 96.;
/// Row counts at which the Capabilities and Groups tables take their full 60% and 40% share.
const CAPS_SHARE_ROWS: usize = 18;
const GROUPS_SHARE_ROWS: usize = 12;
const LABEL_MAX: f32 = 150.;
/// The summary takes at most this share of the body.
const SUMMARY_MAX: f32 = 0.45;
const SECTION_HEADER_HEIGHT: f32 = 24.;
/// The Lucide `check` viewport size in capability cells.
pub const CAP_CHECK_SIZE: f32 = 14.;
/// `None` in the audit session and login UID files.
pub const NO_ID: u32 = u32::MAX;

const KEY_COPY_FAILED: &str = "pwn-security-toast-copy-failed";

gpui_kit::actions!(tasklens_security, [SecurityCopyCell]);
/// The en-US tooltips, for tests. The tab shows `pwn-security-risky-tooltip` and
/// `pwn-security-ns-tip-*`.
pub const RISKY_TOOLTIP: &str = "Capabilities that allow taking over the system";
pub const NS_TIP_INIT: &str = "Compared with PID 1";
pub const NS_TIP_SELF: &str = "Compared with tasklens (PID 1 not readable)";

/// Capability names without `CAP_`, by number (`linux/capability.h`).
pub const CAP_NAMES: [&str; 41] = [
    "CHOWN",
    "DAC_OVERRIDE",
    "DAC_READ_SEARCH",
    "FOWNER",
    "FSETID",
    "KILL",
    "SETGID",
    "SETUID",
    "SETPCAP",
    "LINUX_IMMUTABLE",
    "NET_BIND_SERVICE",
    "NET_BROADCAST",
    "NET_ADMIN",
    "NET_RAW",
    "IPC_LOCK",
    "IPC_OWNER",
    "SYS_MODULE",
    "SYS_RAWIO",
    "SYS_CHROOT",
    "SYS_PTRACE",
    "SYS_PACCT",
    "SYS_ADMIN",
    "SYS_BOOT",
    "SYS_NICE",
    "SYS_RESOURCE",
    "SYS_TIME",
    "SYS_TTY_CONFIG",
    "MKNOD",
    "LEASE",
    "AUDIT_WRITE",
    "AUDIT_CONTROL",
    "SETFCAP",
    "MAC_OVERRIDE",
    "MAC_ADMIN",
    "SYSLOG",
    "WAKE_ALARM",
    "BLOCK_SUSPEND",
    "AUDIT_READ",
    "PERFMON",
    "BPF",
    "CHECKPOINT_RESTORE",
];

/// The PROC-082 risky capabilities in the order the summary lists them.
const RISKY_ORDER: [u8; 14] = [21, 19, 1, 2, 16, 17, 7, 6, 12, 39, 38, 22, 0, 3];

/// The name of capability `cap`, `CAP_<n>` past the table.
pub fn cap_name(cap: u8) -> std::borrow::Cow<'static, str> {
    match CAP_NAMES.get(usize::from(cap)) {
        Some(name) => (*name).into(),
        None => format!("{cap}").into(),
    }
}

/// One field of the summary, in column order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecField {
    User,
    Group,
    Groups,
    RiskyCaps,
    AdminGroups,
    Flags,
    Label,
    Session,
    AuditSession,
    LoginUid,
    Elevated,
    Ns(NsKind),
    StoreBypass,
    IndirectBranch,
}

const NS_KINDS: [NsKind; 8] = [
    NsKind::User,
    NsKind::Pid,
    NsKind::Net,
    NsKind::Mnt,
    NsKind::Ipc,
    NsKind::Uts,
    NsKind::Cgroup,
    NsKind::Time,
];

/// The fields of the two columns.
pub const LEFT: [SecField; 10] = [
    SecField::User,
    SecField::Group,
    SecField::Groups,
    SecField::RiskyCaps,
    SecField::AdminGroups,
    SecField::Flags,
    SecField::Label,
    SecField::Session,
    SecField::AuditSession,
    SecField::LoginUid,
];

pub const RIGHT: [SecField; 11] = [
    SecField::Elevated,
    SecField::Ns(NsKind::User),
    SecField::Ns(NsKind::Pid),
    SecField::Ns(NsKind::Net),
    SecField::Ns(NsKind::Mnt),
    SecField::Ns(NsKind::Ipc),
    SecField::Ns(NsKind::Uts),
    SecField::Ns(NsKind::Cgroup),
    SecField::Ns(NsKind::Time),
    SecField::StoreBypass,
    SecField::IndirectBranch,
];

impl SecField {
    /// The Fluent key of the label.
    pub const fn label_key(self) -> &'static str {
        match self {
            Self::User => "pwn-security-label-user",
            Self::Group => "pwn-security-label-group",
            Self::Groups => "pwn-security-label-groups",
            Self::RiskyCaps => "pwn-security-label-risky-caps",
            Self::AdminGroups => "pwn-security-label-admin-groups",
            Self::Flags => "pwn-security-label-flags",
            Self::Label => "pwn-security-label-label",
            Self::Session => "pwn-security-label-session",
            Self::AuditSession => "pwn-security-label-audit-session",
            Self::LoginUid => "pwn-security-label-login-uid",
            Self::Elevated => "pwn-security-label-elevated",
            Self::Ns(NsKind::User) => "pwn-security-label-ns-user",
            Self::Ns(NsKind::Pid) => "pwn-security-label-ns-pid",
            Self::Ns(NsKind::Net) => "pwn-security-label-ns-net",
            Self::Ns(NsKind::Mnt) => "pwn-security-label-ns-mnt",
            Self::Ns(NsKind::Ipc) => "pwn-security-label-ns-ipc",
            Self::Ns(NsKind::Uts) => "pwn-security-label-ns-uts",
            Self::Ns(NsKind::Cgroup) => "pwn-security-label-ns-cgroup",
            Self::Ns(NsKind::Time) => "pwn-security-label-ns-time",
            Self::StoreBypass => "pwn-security-label-store-bypass",
            Self::IndirectBranch => "pwn-security-label-indirect-branch",
        }
    }
}

// ---- Words (R-132) ----

/// The Security tab's labels and value words, translated once per strings version by
/// [`security_words`].
pub struct SecurityWords {
    /// Labels in [`LEFT`] then [`RIGHT`] order.
    labels: Vec<SharedString>,
    restricted_tooltip: SharedString,
    risky_tooltip: SharedString,
    ns_tip_init: SharedString,
    ns_tip_self: SharedString,
    /// real, effective, saved, fs.
    ids: [Template; 4],
    root_equivalent: SharedString,
    no_new_privs: SharedString,
    seccomp_strict: SharedString,
    seccomp_one: SharedString,
    seccomp_many: Template,
    dumpable: SharedString,
    ns_shared_with_init: SharedString,
    ns_isolated: Template,
    ns_same_as_tasklens: SharedString,
    /// Root, Capabilities, Setuid, No.
    elevated: [SharedString; 4],
    none_reported: SharedString,
    none: SharedString,
    cap_title: Template,
    group_title: Template,
    cap_headers: [SharedString; 7],
    group_headers: [SharedString; 3],
    cap_descriptions: [SharedString; 41],
    yes: SharedString,
    risky_capability: SharedString,
    primary: SharedString,
    supplementary: SharedString,
    admin: SharedString,
    copy: SharedString,
    copy_row: SharedString,
    collecting: SharedString,
    copied: SharedString,
}

impl SecurityWords {
    fn with(t: impl Fn(&str) -> SharedString, tmpl: impl Fn(&str, &[&str]) -> Template) -> Self {
        let id = |key| tmpl(key, &["name", "id"]);
        Self {
            labels: LEFT
                .iter()
                .chain(RIGHT.iter())
                .map(|f| t(f.label_key()))
                .collect(),
            restricted_tooltip: t("pwn-general-restricted-tooltip"),
            risky_tooltip: t("pwn-security-risky-tooltip"),
            ns_tip_init: t("pwn-security-ns-tip-init"),
            ns_tip_self: t("pwn-security-ns-tip-self"),
            ids: [
                id("pwn-security-ids-real"),
                id("pwn-security-ids-effective"),
                id("pwn-security-ids-saved"),
                id("pwn-security-ids-fs"),
            ],
            root_equivalent: t("pwn-security-root-equivalent"),
            no_new_privs: t("pwn-security-flag-no-new-privs"),
            seccomp_strict: t("pwn-security-flag-seccomp-strict"),
            seccomp_one: t("pwn-security-flag-seccomp-filter-one"),
            seccomp_many: tmpl("pwn-security-flag-seccomp-filter-many", &["n"]),
            dumpable: t("pwn-security-flag-dumpable"),
            ns_shared_with_init: t("pwn-security-ns-shared-with-init"),
            ns_isolated: tmpl("pwn-security-ns-isolated", &["inode"]),
            ns_same_as_tasklens: t("pwn-security-ns-same-as-tasklens"),
            elevated: [
                t("pwn-security-elevated-root"),
                t("pwn-security-elevated-capabilities"),
                t("pwn-security-elevated-setuid"),
                t("pwn-security-elevated-no"),
            ],
            none_reported: t("pwn-security-none-reported"),
            none: t("pwn-security-none"),
            cap_title: tmpl("pwn-security-caps-title", &["n"]),
            group_title: tmpl("pwn-security-groups-title", &["n"]),
            cap_headers: [
                t("pwn-security-caps-col-name"),
                t("pwn-security-caps-col-effective"),
                t("pwn-security-caps-col-permitted"),
                t("pwn-security-caps-col-inheritable"),
                t("pwn-security-caps-col-bounding"),
                t("pwn-security-caps-col-ambient"),
                t("pwn-security-caps-col-description"),
            ],
            group_headers: [
                t("pwn-security-groups-col-name"),
                t("pwn-security-groups-col-gid"),
                t("pwn-security-groups-col-kind"),
            ],
            cap_descriptions: [
                t("pwn-security-cap-desc-chown"),
                t("pwn-security-cap-desc-dac-override"),
                t("pwn-security-cap-desc-dac-read-search"),
                t("pwn-security-cap-desc-fowner"),
                t("pwn-security-cap-desc-fsetid"),
                t("pwn-security-cap-desc-kill"),
                t("pwn-security-cap-desc-setgid"),
                t("pwn-security-cap-desc-setuid"),
                t("pwn-security-cap-desc-setpcap"),
                t("pwn-security-cap-desc-linux-immutable"),
                t("pwn-security-cap-desc-net-bind-service"),
                t("pwn-security-cap-desc-net-broadcast"),
                t("pwn-security-cap-desc-net-admin"),
                t("pwn-security-cap-desc-net-raw"),
                t("pwn-security-cap-desc-ipc-lock"),
                t("pwn-security-cap-desc-ipc-owner"),
                t("pwn-security-cap-desc-sys-module"),
                t("pwn-security-cap-desc-sys-rawio"),
                t("pwn-security-cap-desc-sys-chroot"),
                t("pwn-security-cap-desc-sys-ptrace"),
                t("pwn-security-cap-desc-sys-pacct"),
                t("pwn-security-cap-desc-sys-admin"),
                t("pwn-security-cap-desc-sys-boot"),
                t("pwn-security-cap-desc-sys-nice"),
                t("pwn-security-cap-desc-sys-resource"),
                t("pwn-security-cap-desc-sys-time"),
                t("pwn-security-cap-desc-sys-tty-config"),
                t("pwn-security-cap-desc-mknod"),
                t("pwn-security-cap-desc-lease"),
                t("pwn-security-cap-desc-audit-write"),
                t("pwn-security-cap-desc-audit-control"),
                t("pwn-security-cap-desc-setfcap"),
                t("pwn-security-cap-desc-mac-override"),
                t("pwn-security-cap-desc-mac-admin"),
                t("pwn-security-cap-desc-syslog"),
                t("pwn-security-cap-desc-wake-alarm"),
                t("pwn-security-cap-desc-block-suspend"),
                t("pwn-security-cap-desc-audit-read"),
                t("pwn-security-cap-desc-perfmon"),
                t("pwn-security-cap-desc-bpf"),
                t("pwn-security-cap-desc-checkpoint-restore"),
            ],
            yes: t("pwn-security-yes"),
            risky_capability: t("pwn-security-risky-capability"),
            primary: t("pwn-security-kind-primary"),
            supplementary: t("pwn-security-kind-supplementary"),
            admin: t("pwn-security-kind-admin"),
            copy: t("pwn-security-menu-copy"),
            copy_row: t("pwn-security-menu-copy-row"),
            collecting: t("pwn-general-collecting"),
            copied: t("pwn-general-toast-copied"),
        }
    }

    /// The en-US words, for code that runs without the strings.
    pub fn en() -> Self {
        Self::with(|key| crate::i18n::tr_en(key, &[]), Template::en)
    }

    /// The label of `id`, such as "User:".
    pub fn label(&self, id: SecField) -> &SharedString {
        let i = LEFT
            .iter()
            .chain(RIGHT.iter())
            .position(|f| *f == id)
            .unwrap_or(0);
        &self.labels[i]
    }
}

struct SecurityWordsCache {
    version: u64,
    words: Rc<SecurityWords>,
}

impl Global for SecurityWordsCache {}

/// The texts for the current strings. Translated only when the strings version changed; without
/// strings they are in en-US.
pub fn security_words(cx: &mut App) -> Rc<SecurityWords> {
    let Some(version) = cx.try_global::<Strings>().map(|s| s.strings_version) else {
        return Rc::new(SecurityWords::en());
    };
    if let Some(cache) = cx.try_global::<SecurityWordsCache>()
        && cache.version == version
    {
        return cache.words.clone();
    }
    let words = Rc::new(SecurityWords::with(
        |key| tr(cx, key),
        |key, args| Template::new(cx, key, args),
    ));
    cx.set_global(SecurityWordsCache {
        version,
        words: words.clone(),
    });
    words
}

// ---- Formatters (pure) ----

/// `oystein (1000)` when all four ids are equal; else
/// `real oystein (1000), effective root (0), saved root (0), fs root (0)`.
pub fn fmt_ids(out: &mut String, w: &SecurityWords, ids: &IdTriple, names: &[Arc<str>; 4]) {
    let all = [ids.real, ids.effective, ids.saved, ids.fs];
    if all.iter().all(|&id| id == ids.real) {
        let _ = write!(out, "{} ({})", names[0], ids.real);
        return;
    }
    for (i, (template, id)) in w.ids.iter().zip(all).enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        let _ = template.write(out, &[&names[i], &id]);
    }
}

/// The risky capabilities in effective or ambient, `SYS_ADMIN, NET_ADMIN`. Empty when none.
pub fn fmt_risky(out: &mut String, d: &SecurityDetail) {
    let mut caps: Vec<u8> = d
        .caps
        .iter()
        .filter(|c| c.risky && (c.effective || c.ambient))
        .map(|c| c.cap)
        .collect();
    caps.sort_by_key(|c| {
        RISKY_ORDER
            .iter()
            .position(|r| r == c)
            .unwrap_or(RISKY_ORDER.len() + usize::from(*c))
    });
    caps.dedup();
    for (i, cap) in caps.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&cap_name(*cap));
    }
}

/// `wheel, docker (root-equivalent)`: admin groups first, then root-equivalent ones. Empty when none.
pub fn fmt_admin_groups(out: &mut String, w: &SecurityWords, d: &SecurityDetail) {
    let mut groups: Vec<_> = d
        .groups
        .iter()
        .filter(|g| g.admin || g.root_equivalent)
        .collect();
    // Plain admin groups first, then the root-equivalent ones; each keeps the payload order.
    groups.sort_by_key(|g| g.root_equivalent);
    for (i, g) in groups.into_iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&g.name);
        if g.root_equivalent {
            out.push(' ');
            out.push_str(&w.root_equivalent);
        }
    }
}

/// `NoNewPrivs, Seccomp filter (3 filters), Dumpable`. Empty when none is set.
pub fn fmt_flags(
    out: &mut String,
    w: &SecurityWords,
    no_new_privs: bool,
    seccomp: Seccomp,
    dumpable: bool,
) {
    let mut parts: Vec<String> = Vec::new();
    if no_new_privs {
        parts.push(w.no_new_privs.to_string());
    }
    match seccomp {
        Seccomp::Disabled => {}
        Seccomp::Strict => parts.push(w.seccomp_strict.to_string()),
        Seccomp::Filter { filters: 1 } => parts.push(w.seccomp_one.to_string()),
        Seccomp::Filter { filters } => {
            let mut part = String::new();
            let _ = w.seccomp_many.write(&mut part, &[&filters]);
            parts.push(part);
        }
    }
    if dumpable {
        parts.push(w.dumpable.to_string());
    }
    out.push_str(&parts.join(", "));
}

/// `Shared with init`, `Isolated (4026532211)`, `Same as tasklens` or `Isolated (4026532211)`,
/// and the tooltip that says what it was compared with.
pub fn fmt_ns<'a>(out: &mut String, w: &'a SecurityWords, state: NsState) -> &'a SharedString {
    match state {
        NsState::SharedWithInit => {
            out.push_str(&w.ns_shared_with_init);
            &w.ns_tip_init
        }
        NsState::Isolated { inode } => {
            let _ = w.ns_isolated.write(out, &[&inode]);
            &w.ns_tip_init
        }
        NsState::SameAsTasklens => {
            out.push_str(&w.ns_same_as_tasklens);
            &w.ns_tip_self
        }
        NsState::IsolatedFromTasklens { inode } => {
            let _ = w.ns_isolated.write(out, &[&inode]);
            &w.ns_tip_self
        }
    }
}

fn elevated_word(w: &SecurityWords, e: Elevated) -> &SharedString {
    match e {
        Elevated::Root => &w.elevated[0],
        Elevated::Capabilities => &w.elevated[1],
        Elevated::Setuid => &w.elevated[2],
        Elevated::No => &w.elevated[3],
    }
}

/// The worst of two validities: the first that is not Valid.
fn worst(a: Validity, b: Validity) -> Validity {
    if a == Validity::Valid { b } else { a }
}

/// One built field.
#[derive(Clone)]
pub struct SecRow {
    pub id: SecField,
    pub row: TlKvRow,
}

fn row(
    w: &SecurityWords,
    id: SecField,
    text: impl Into<SharedString>,
    validity: Validity,
) -> SecRow {
    let mut value = TlValue::new(text);
    value.validity = validity;
    if validity == Validity::Restricted {
        value.tooltip = Some(w.restricted_tooltip.clone());
    }
    SecRow {
        id,
        row: TlKvRow {
            label: w.label(id).clone(),
            value,
        },
    }
}

/// A `Total` formatted by `f` when Valid or Stale; else the grid's word for its validity.
fn total<T>(
    w: &SecurityWords,
    id: SecField,
    t: &Total<T>,
    f: impl FnOnce(&T, &mut String),
) -> SecRow {
    let mut s = String::new();
    if matches!(t.validity, Validity::Valid | Validity::Stale) {
        f(&t.value, &mut s);
    }
    row(w, id, s, t.validity)
}

fn or_none_reported(w: &SecurityWords, s: String) -> String {
    if s.is_empty() {
        w.none_reported.to_string()
    } else {
        s
    }
}

/// The two columns from `d`; without a payload every value is `Collecting…`.
pub fn build(w: &SecurityWords, d: Option<&SecurityDetail>) -> [Vec<SecRow>; 2] {
    let Some(d) = d else {
        let collecting = |ids: &[SecField]| -> Vec<SecRow> {
            ids.iter()
                .map(|&id| row(w, id, "", Validity::WarmingUp))
                .collect()
        };
        return [collecting(&LEFT), collecting(&RIGHT)];
    };
    let mut left = Vec::with_capacity(LEFT.len());
    let ids_row = |id, ids: &Total<IdTriple>, names: &Total<[Arc<str>; 4]>| {
        let validity = worst(ids.validity, names.validity);
        let mut s = String::new();
        if matches!(validity, Validity::Valid | Validity::Stale) {
            fmt_ids(&mut s, w, &ids.value, &names.value);
        }
        row(w, id, s, validity)
    };
    left.push(ids_row(SecField::User, &d.uids, &d.uid_names));
    left.push(ids_row(SecField::Group, &d.gids, &d.gid_names));
    let mut count = String::new();
    units::count(
        &mut count,
        d.groups.iter().filter(|g| !g.primary).count() as u64,
    );
    left.push(row(w, SecField::Groups, count, Validity::Valid));
    let mut risky = String::new();
    fmt_risky(&mut risky, d);
    let mut risky_row = row(
        w,
        SecField::RiskyCaps,
        or_none_reported(w, risky.clone()),
        Validity::Valid,
    );
    if !risky.is_empty() {
        risky_row.row.value.tone = Some(ValueTone::Warning);
        risky_row.row.value.tooltip = Some(w.risky_tooltip.clone());
    }
    left.push(risky_row);
    let mut admin = String::new();
    fmt_admin_groups(&mut admin, w, d);
    left.push(row(
        w,
        SecField::AdminGroups,
        or_none_reported(w, admin),
        Validity::Valid,
    ));
    // An unreported Dumpable (R-163) is left out of the list; it does not make the row
    // unsupported.
    let flags_validity = worst(d.no_new_privs.validity, d.seccomp.validity);
    let mut flags = String::new();
    if matches!(flags_validity, Validity::Valid | Validity::Stale) {
        let dumpable =
            matches!(d.dumpable.validity, Validity::Valid | Validity::Stale) && d.dumpable.value;
        fmt_flags(
            &mut flags,
            w,
            d.no_new_privs.value,
            d.seccomp.value,
            dumpable,
        );
        flags = or_none_reported(w, flags);
    }
    left.push(row(w, SecField::Flags, flags, flags_validity));
    left.push(total(w, SecField::Label, &d.security_label, |v, s| {
        escape_controls(s, v)
    }));
    let session = match &d.session {
        Some(id) => {
            let mut s = String::new();
            escape_controls(&mut s, id);
            s
        }
        None => w.none.to_string(),
    };
    left.push(row(w, SecField::Session, session, Validity::Valid));
    let audit = match d.audit_session.filter(|&id| id != NO_ID) {
        Some(id) => id.to_string(),
        None => w.none.to_string(),
    };
    left.push(row(w, SecField::AuditSession, audit, Validity::Valid));
    let login = match d.login_uid.filter(|&id| id != NO_ID) {
        Some(uid) => login_text(d, uid),
        None => w.none.to_string(),
    };
    left.push(row(w, SecField::LoginUid, login, Validity::Valid));

    let mut right = Vec::with_capacity(RIGHT.len());
    right.push(total(w, SecField::Elevated, &d.elevated, |e, s| {
        s.push_str(elevated_word(w, *e))
    }));
    for kind in NS_KINDS {
        let ns = d.namespaces.iter().find(|n| n.kind == kind);
        let Some(ns) = ns else {
            right.push(row(w, SecField::Ns(kind), "", Validity::Unsupported));
            continue;
        };
        let mut s = String::new();
        let tip = matches!(ns.validity, Validity::Valid | Validity::Stale)
            .then(|| fmt_ns(&mut s, w, ns.state).clone());
        let mut r = row(w, SecField::Ns(kind), s, ns.validity);
        if let Some(tip) = tip {
            r.row.value.tooltip = Some(tip);
        }
        right.push(r);
    }
    right.push(total(w, SecField::StoreBypass, &d.store_bypass, |v, s| {
        escape_controls(s, v)
    }));
    right.push(total(
        w,
        SecField::IndirectBranch,
        &d.indirect_branch,
        |v, s| escape_controls(s, v),
    ));
    [left, right]
}

/// `oystein (1000)` when the login UID is one of the process's UIDs, whose names the payload
/// has; else the number alone.
fn login_text(d: &SecurityDetail, uid: u32) -> String {
    let ids = &d.uids.value;
    let known = d.uids.validity == Validity::Valid && d.uid_names.validity == Validity::Valid;
    let at = [ids.real, ids.effective, ids.saved, ids.fs]
        .iter()
        .position(|&id| id == uid)
        .filter(|_| known);
    match at {
        Some(i) => format!("{} ({uid})", d.uid_names.value[i]),
        None => uid.to_string(),
    }
}

// ---- Capabilities table ----

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CapColumn {
    Name,
    Effective,
    Permitted,
    Inheritable,
    Bounding,
    Ambient,
    Description,
}

impl CapColumn {
    pub const ALL: [Self; 7] = [
        Self::Name,
        Self::Effective,
        Self::Permitted,
        Self::Inheritable,
        Self::Bounding,
        Self::Ambient,
        Self::Description,
    ];

    fn width(self) -> f32 {
        match self {
            Self::Name => 180.,
            Self::Effective
            | Self::Permitted
            | Self::Inheritable
            | Self::Bounding
            | Self::Ambient => 84.,
            Self::Description => 320.,
        }
    }
}

impl TableColumn for CapColumn {
    fn key(self) -> &'static str {
        match self {
            Self::Name => "cap-name",
            Self::Effective => "cap-effective",
            Self::Permitted => "cap-permitted",
            Self::Inheritable => "cap-inheritable",
            Self::Bounding => "cap-bounding",
            Self::Ambient => "cap-ambient",
            Self::Description => "cap-description",
        }
    }

    fn parse(_: TableId, _: &str) -> Option<Self> {
        None
    }

    fn pinned_first(self) -> bool {
        false
    }

    fn min_width(self) -> Option<f32> {
        Some(match self {
            Self::Name => 120.,
            Self::Effective
            | Self::Permitted
            | Self::Inheritable
            | Self::Bounding
            | Self::Ambient => 64.,
            Self::Description => 160.,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CapSort {
    pub column: CapColumn,
    pub ascending: bool,
}

impl Default for CapSort {
    fn default() -> Self {
        Self {
            column: CapColumn::Effective,
            ascending: false,
        }
    }
}

impl TableSort<CapColumn> for CapSort {
    fn column(&self) -> CapColumn {
        self.column
    }

    fn ascending(&self) -> bool {
        self.ascending
    }
}

fn full_cap_name(cap: u8) -> String {
    match CAP_NAMES.get(usize::from(cap)) {
        Some(name) => format!("CAP_{name}"),
        None => format!("CAP_{cap}"),
    }
}

pub struct CapDelegate {
    rows: Arc<[CapRow]>,
    view: Vec<u8>,
    index: FxHashMap<u8, u32>,
    needle: SharedString,
    loading: bool,
    version: u64,
    sort: CapSort,
    specs: Vec<ColumnSpec<CapColumn>>,
    layout: ColumnLayout<CapColumn>,
    words: Rc<SecurityWords>,
    warning: Hsla,
    menu_cell: Option<(u8, CapColumn)>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub check_icon_bounds: Rc<std::cell::RefCell<Vec<gpui_kit::Bounds<Pixels>>>>,
}

impl CapDelegate {
    fn new(words: Rc<SecurityWords>, warning: Hsla) -> Self {
        let specs = cap_specs(&words);
        let layout =
            ColumnLayout::defaults(TableId::Processes, &specs, &CapColumn::ALL, &CapColumn::ALL);
        Self {
            rows: Arc::from([]),
            view: Vec::new(),
            index: FxHashMap::default(),
            needle: SharedString::default(),
            loading: true,
            version: 0,
            sort: CapSort::default(),
            specs,
            layout,
            words,
            warning,
            menu_cell: None,
            #[cfg(any(test, feature = "test-hooks"))]
            check_icon_bounds: Rc::new(std::cell::RefCell::new(Vec::new())),
        }
    }

    fn row(&self, cap: u8) -> Option<&CapRow> {
        self.rows.iter().find(|r| r.cap == cap)
    }

    fn description(&self, cap: u8) -> &str {
        self.words
            .cap_descriptions
            .get(usize::from(cap))
            .map_or("", SharedString::as_str)
    }

    fn set_detail(&mut self, detail: Option<&SecurityDetail>) {
        match detail {
            Some(detail) => {
                self.rows = detail.caps.clone();
                self.loading = false;
            }
            None => {
                self.rows = Arc::from([]);
                self.loading = true;
            }
        }
        self.rebuild();
    }

    fn set_filter(&mut self, needle: SharedString) {
        if self.needle != needle {
            self.needle = needle;
            self.rebuild();
        }
    }

    fn compare(&self, a: u8, b: u8) -> Ordering {
        let (Some(a), Some(b)) = (self.row(a), self.row(b)) else {
            return a.cmp(&b);
        };
        let by = match self.sort.column {
            CapColumn::Name => full_cap_name(a.cap).cmp(&full_cap_name(b.cap)),
            CapColumn::Effective => a.effective.cmp(&b.effective),
            CapColumn::Permitted => a.permitted.cmp(&b.permitted),
            CapColumn::Inheritable => a.inheritable.cmp(&b.inheritable),
            CapColumn::Bounding => a.bounding.cmp(&b.bounding),
            CapColumn::Ambient => a.ambient.cmp(&b.ambient),
            CapColumn::Description => self.description(a.cap).cmp(self.description(b.cap)),
        };
        let by = if self.sort.ascending {
            by
        } else {
            by.reverse()
        };
        by.then_with(|| full_cap_name(a.cap).cmp(&full_cap_name(b.cap)))
    }

    fn rebuild(&mut self) {
        let mut view: Vec<u8> = self
            .rows
            .iter()
            .filter(|r| r.effective || r.permitted || r.inheritable || r.ambient)
            .filter(|r| {
                self.needle.is_empty() || {
                    let hay = lowercase(&format!(
                        "{} {}",
                        full_cap_name(r.cap),
                        self.description(r.cap)
                    ));
                    matches(&hay, &self.needle)
                }
            })
            .map(|r| r.cap)
            .collect();
        view.sort_by(|&a, &b| self.compare(a, b));
        self.view = view;
        self.index.clear();
        for (i, &cap) in self.view.iter().enumerate() {
            self.index.insert(cap, i as u32);
        }
        self.version += 1;
    }

    pub fn view_ids(&self) -> impl Iterator<Item = u8> + '_ {
        self.view.iter().copied()
    }

    pub fn row_tsv(&self, cap: u8) -> Option<String> {
        let row = self.row(cap)?;
        let mut out = full_cap_name(cap);
        for set in [
            row.effective,
            row.permitted,
            row.inheritable,
            row.bounding,
            row.ambient,
        ] {
            out.push('\t');
            if set {
                out.push_str(&self.words.yes);
            }
        }
        out.push('\t');
        out.push_str(self.description(cap));
        Some(out)
    }

    fn cell_text(&self, cap: u8, col: CapColumn, copy: bool) -> Option<String> {
        let row = self.row(cap)?;
        Some(match col {
            CapColumn::Name => full_cap_name(cap),
            CapColumn::Effective => mark_text(row.effective, copy, &self.words.yes),
            CapColumn::Permitted => mark_text(row.permitted, copy, &self.words.yes),
            CapColumn::Inheritable => mark_text(row.inheritable, copy, &self.words.yes),
            CapColumn::Bounding => mark_text(row.bounding, copy, &self.words.yes),
            CapColumn::Ambient => mark_text(row.ambient, copy, &self.words.yes),
            CapColumn::Description => self.description(cap).to_owned(),
        })
    }
}

fn mark_text(set: bool, copy: bool, yes: &str) -> String {
    if !set {
        String::new()
    } else if copy {
        yes.to_owned()
    } else {
        String::new()
    }
}

fn cap_specs(words: &SecurityWords) -> Vec<ColumnSpec<CapColumn>> {
    CapColumn::ALL
        .iter()
        .enumerate()
        .map(|(i, &id)| {
            let label = words.cap_headers[i].clone();
            ColumnSpec {
                id,
                label: label.clone(),
                full_name: label.clone(),
                group: label,
                // Set cells are empty text cells. `cap_checks` paints their icon at the exact
                // column centre from this layout; the remaining cells use normal left text.
                align: ColumnAlign::Left,
                kind: ColumnKind::Text,
                sortable: true,
                template: None,
                default_width: px(id.width()),
                tooltip: None,
                availability: Availability::Available,
                hideable: false,
            }
        })
        .collect()
}

impl TreeTableDelegate for CapDelegate {
    type RowId = u8;
    type Column = CapColumn;
    type Sort = CapSort;

    fn row_count(&self) -> usize {
        self.view.len()
    }
    fn row_id(&self, ix: usize) -> u8 {
        self.view[ix]
    }
    fn row_index(&self, id: u8) -> Option<usize> {
        self.index.get(&id).map(|&i| i as usize)
    }
    fn row_kind(&self, _: usize) -> RowKind {
        RowKind::Plain
    }
    fn row_depth(&self, _: usize) -> u8 {
        0
    }
    fn row_expandable(&self, _: usize) -> Option<bool> {
        None
    }
    fn row_flags(&self, _: usize) -> RowFlags {
        RowFlags::default()
    }
    fn table_version(&self) -> u64 {
        self.version
    }
    fn columns(&self) -> &ColumnLayout<CapColumn> {
        &self.layout
    }
    fn column_spec(&self, id: CapColumn) -> &ColumnSpec<CapColumn> {
        &self.specs[CapColumn::ALL.iter().position(|c| *c == id).unwrap_or(0)]
    }
    fn cell(&self, ix: usize, col: CapColumn, out: &mut CellOut) {
        let cap = self.view[ix];
        let Some(row) = self.row(cap) else {
            return;
        };
        out.bits = u64::from(cap)
            | (u64::from(row.effective) << 8)
            | (u64::from(row.permitted) << 9)
            | (u64::from(row.inheritable) << 10)
            | (u64::from(row.bounding) << 11)
            | (u64::from(row.ambient) << 12);
        out.validity = Validity::Valid;
        if col == CapColumn::Name && row.risky {
            out.text_color = Some(self.warning);
            out.tooltip = Some(self.words.risky_capability.clone());
        }
        let set = match col {
            CapColumn::Effective => Some(row.effective),
            CapColumn::Permitted => Some(row.permitted),
            CapColumn::Inheritable => Some(row.inheritable),
            CapColumn::Bounding => Some(row.bounding),
            CapColumn::Ambient => Some(row.ambient),
            _ => None,
        };
        if set == Some(true) {
            out.tooltip = Some(self.words.yes.clone());
        }
        if out.want_text
            && let Some(text) = self.cell_text(cap, col, false)
        {
            out.text.push_str(&text);
        }
    }
    fn name_cell(&self, ix: usize) -> NameCell {
        let cap = self.view[ix];
        NameCell {
            icon: NameIcon::None,
            icon_size: IconSlot::Small,
            primary: self.description(cap).to_owned().into(),
            count_run: None,
            secondary: None,
            glyphs: SmallVec::new(),
            highlight_fffd: true,
            trailing: None,
        }
    }
    fn flat_names(&self) -> bool {
        true
    }

    fn name_column(&self) -> Option<CapColumn> {
        // Name needs a per-cell warning colour and tooltip. Description is the flat table's
        // navigation anchor, leaving Name to the normal cell painter.
        Some(CapColumn::Description)
    }
    fn header_total(&self, _: CapColumn, _: &mut CellOut) -> bool {
        false
    }
    fn sort(&self) -> CapSort {
        self.sort
    }
    fn selectable(&self, _: usize) -> bool {
        true
    }
    fn context_menu(
        &mut self,
        targets: &[u8],
        col: Option<CapColumn>,
        menu: TlMenu,
        _: &mut Window,
        _: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu {
        self.menu_cell = targets
            .first()
            .copied()
            .map(|id| (id, col.unwrap_or(CapColumn::Name)));
        menu.item(self.words.copy.clone(), Box::new(SecurityCopyCell))
            .item(self.words.copy_row.clone(), Box::new(CopyRow))
    }
    fn on_toggle_expand(&mut self, _: u8, _: &mut Context<TlTreeTable<Self>>) {}
    fn on_sort(&mut self, col: CapColumn, cx: &mut Context<TlTreeTable<Self>>) {
        self.sort = CapSort {
            column: col,
            ascending: self.sort.column != col || !self.sort.ascending,
        };
        self.rebuild();
        cx.notify();
    }
    fn on_columns_changed(
        &mut self,
        layout: &ColumnLayout<CapColumn>,
        _: &mut Context<TlTreeTable<Self>>,
    ) {
        self.layout = layout.clone();
    }
    fn on_visible_range(&mut self, _: &[u8], _: &mut Context<TlTreeTable<Self>>) {}
    fn on_activate(&mut self, _: u8, _: &mut Context<TlTreeTable<Self>>) {}
    fn row_tooltip(&self, _: usize) -> Option<AnyView> {
        None
    }
    fn empty_text(&self) -> Option<SharedString> {
        Some(if self.loading {
            self.words.collecting.clone()
        } else {
            self.words.none_reported.clone()
        })
    }
    fn header_lines(&self) -> HeaderLines {
        HeaderLines::One
    }
    fn columns_fixed(&self) -> bool {
        true
    }
}

// ---- Groups table ----

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum GroupColumn {
    Name,
    Gid,
    Kind,
}

impl GroupColumn {
    pub const ALL: [Self; 3] = [Self::Name, Self::Gid, Self::Kind];
}

impl TableColumn for GroupColumn {
    fn key(self) -> &'static str {
        match self {
            Self::Name => "group-name",
            Self::Gid => "group-gid",
            Self::Kind => "group-kind",
        }
    }
    fn parse(_: TableId, _: &str) -> Option<Self> {
        None
    }
    fn pinned_first(self) -> bool {
        false
    }
    fn min_width(self) -> Option<f32> {
        Some(match self {
            Self::Name => 80.,
            Self::Gid => 60.,
            Self::Kind => 120.,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GroupSort {
    pub column: GroupColumn,
    pub ascending: bool,
}

impl Default for GroupSort {
    fn default() -> Self {
        Self {
            column: GroupColumn::Name,
            ascending: true,
        }
    }
}

impl TableSort<GroupColumn> for GroupSort {
    fn column(&self) -> GroupColumn {
        self.column
    }
    fn ascending(&self) -> bool {
        self.ascending
    }
}

pub struct GroupDelegate {
    rows: Arc<[GroupMembershipRow]>,
    view: Vec<u32>,
    index: FxHashMap<u32, u32>,
    needle: SharedString,
    loading: bool,
    version: u64,
    sort: GroupSort,
    specs: Vec<ColumnSpec<GroupColumn>>,
    layout: ColumnLayout<GroupColumn>,
    words: Rc<SecurityWords>,
    menu_cell: Option<(u32, GroupColumn)>,
}

impl GroupDelegate {
    fn new(words: Rc<SecurityWords>) -> Self {
        let specs = group_specs(&words);
        let layout = ColumnLayout::defaults(
            TableId::Processes,
            &specs,
            &GroupColumn::ALL,
            &GroupColumn::ALL,
        );
        Self {
            rows: Arc::from([]),
            view: Vec::new(),
            index: FxHashMap::default(),
            needle: SharedString::default(),
            loading: true,
            version: 0,
            sort: GroupSort::default(),
            specs,
            layout,
            words,
            menu_cell: None,
        }
    }

    fn row(&self, gid: u32) -> Option<&GroupMembershipRow> {
        self.rows.iter().find(|r| r.gid == gid)
    }

    pub fn kind(&self, gid: u32) -> Option<String> {
        let row = self.row(gid)?;
        let mut out = if row.primary {
            self.words.primary.to_string()
        } else {
            self.words.supplementary.to_string()
        };
        if row.root_equivalent {
            out.push(' ');
            out.push_str(&self.words.root_equivalent);
        } else if row.admin {
            out.push(' ');
            out.push_str(&self.words.admin);
        }
        Some(out)
    }

    fn set_detail(&mut self, detail: Option<&SecurityDetail>) {
        match detail {
            Some(detail) => {
                self.rows = detail.groups.clone();
                self.loading = false;
            }
            None => {
                self.rows = Arc::from([]);
                self.loading = true;
            }
        }
        self.rebuild();
    }

    fn set_filter(&mut self, needle: SharedString) {
        if self.needle != needle {
            self.needle = needle;
            self.rebuild();
        }
    }

    fn compare(&self, a: u32, b: u32) -> Ordering {
        let (Some(a), Some(b)) = (self.row(a), self.row(b)) else {
            return a.cmp(&b);
        };
        let by = match self.sort.column {
            GroupColumn::Name => a.name.cmp(&b.name),
            GroupColumn::Gid => a.gid.cmp(&b.gid),
            GroupColumn::Kind => self.kind(a.gid).cmp(&self.kind(b.gid)),
        };
        let by = if self.sort.ascending {
            by
        } else {
            by.reverse()
        };
        by.then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.gid.cmp(&b.gid))
    }

    fn rebuild(&mut self) {
        let mut view: Vec<u32> = self
            .rows
            .iter()
            .filter(|row| {
                self.needle.is_empty()
                    || matches(
                        &lowercase(&format!("{} {}", row.name, row.gid)),
                        &self.needle,
                    )
            })
            .map(|row| row.gid)
            .collect();
        view.sort_by(|&a, &b| self.compare(a, b));
        self.view = view;
        self.index.clear();
        for (i, &gid) in self.view.iter().enumerate() {
            self.index.insert(gid, i as u32);
        }
        self.version += 1;
    }

    pub fn view_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.view.iter().copied()
    }

    pub fn row_tsv(&self, gid: u32) -> Option<String> {
        let row = self.row(gid)?;
        Some(format!("{}\t{}\t{}", row.name, row.gid, self.kind(gid)?))
    }

    fn cell_text(&self, gid: u32, col: GroupColumn) -> Option<String> {
        let row = self.row(gid)?;
        Some(match col {
            GroupColumn::Name => row.name.to_string(),
            GroupColumn::Gid => row.gid.to_string(),
            GroupColumn::Kind => self.kind(gid)?,
        })
    }
}

fn group_specs(words: &SecurityWords) -> Vec<ColumnSpec<GroupColumn>> {
    GroupColumn::ALL
        .iter()
        .enumerate()
        .map(|(i, &id)| {
            let label = words.group_headers[i].clone();
            ColumnSpec {
                id,
                label: label.clone(),
                full_name: label.clone(),
                group: label,
                align: match id {
                    GroupColumn::Gid => ColumnAlign::Right,
                    _ => ColumnAlign::Left,
                },
                kind: match id {
                    GroupColumn::Gid => ColumnKind::Numeric,
                    _ => ColumnKind::Text,
                },
                sortable: true,
                template: (id == GroupColumn::Gid).then_some("000000"),
                default_width: px(match id {
                    GroupColumn::Name => 200.,
                    GroupColumn::Gid => 90.,
                    GroupColumn::Kind => 320.,
                }),
                tooltip: None,
                availability: Availability::Available,
                hideable: false,
            }
        })
        .collect()
}

impl TreeTableDelegate for GroupDelegate {
    type RowId = u32;
    type Column = GroupColumn;
    type Sort = GroupSort;

    fn row_count(&self) -> usize {
        self.view.len()
    }
    fn row_id(&self, ix: usize) -> u32 {
        self.view[ix]
    }
    fn row_index(&self, id: u32) -> Option<usize> {
        self.index.get(&id).map(|&i| i as usize)
    }
    fn row_kind(&self, _: usize) -> RowKind {
        RowKind::Plain
    }
    fn row_depth(&self, _: usize) -> u8 {
        0
    }
    fn row_expandable(&self, _: usize) -> Option<bool> {
        None
    }
    fn row_flags(&self, _: usize) -> RowFlags {
        RowFlags::default()
    }
    fn table_version(&self) -> u64 {
        self.version
    }
    fn columns(&self) -> &ColumnLayout<GroupColumn> {
        &self.layout
    }
    fn column_spec(&self, id: GroupColumn) -> &ColumnSpec<GroupColumn> {
        &self.specs[GroupColumn::ALL.iter().position(|c| *c == id).unwrap_or(0)]
    }
    fn cell(&self, ix: usize, col: GroupColumn, out: &mut CellOut) {
        let gid = self.view[ix];
        let Some(row) = self.row(gid) else {
            return;
        };
        out.bits = u64::from(gid)
            | (u64::from(row.primary) << 32)
            | (u64::from(row.root_equivalent) << 33)
            | (u64::from(row.admin) << 34);
        out.validity = Validity::Valid;
        if out.want_text
            && let Some(text) = self.cell_text(gid, col)
        {
            out.text.push_str(&text);
        }
    }
    fn name_cell(&self, ix: usize) -> NameCell {
        let gid = self.view[ix];
        NameCell {
            icon: NameIcon::None,
            icon_size: IconSlot::Small,
            primary: self.row(gid).map_or("", |r| &*r.name).to_owned().into(),
            count_run: None,
            secondary: None,
            glyphs: SmallVec::new(),
            highlight_fffd: true,
            trailing: None,
        }
    }
    fn flat_names(&self) -> bool {
        true
    }

    fn name_column(&self) -> Option<GroupColumn> {
        Some(GroupColumn::Name)
    }
    fn header_total(&self, _: GroupColumn, _: &mut CellOut) -> bool {
        false
    }
    fn sort(&self) -> GroupSort {
        self.sort
    }
    fn selectable(&self, _: usize) -> bool {
        true
    }
    fn context_menu(
        &mut self,
        targets: &[u32],
        col: Option<GroupColumn>,
        menu: TlMenu,
        _: &mut Window,
        _: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu {
        self.menu_cell = targets
            .first()
            .copied()
            .map(|id| (id, col.unwrap_or(GroupColumn::Name)));
        menu.item(self.words.copy.clone(), Box::new(SecurityCopyCell))
            .item(self.words.copy_row.clone(), Box::new(CopyRow))
    }
    fn on_toggle_expand(&mut self, _: u32, _: &mut Context<TlTreeTable<Self>>) {}
    fn on_sort(&mut self, col: GroupColumn, cx: &mut Context<TlTreeTable<Self>>) {
        self.sort = GroupSort {
            column: col,
            ascending: self.sort.column != col || !self.sort.ascending,
        };
        self.rebuild();
        cx.notify();
    }
    fn on_columns_changed(
        &mut self,
        layout: &ColumnLayout<GroupColumn>,
        _: &mut Context<TlTreeTable<Self>>,
    ) {
        self.layout = layout.clone();
    }
    fn on_visible_range(&mut self, _: &[u32], _: &mut Context<TlTreeTable<Self>>) {}
    fn on_activate(&mut self, _: u32, _: &mut Context<TlTreeTable<Self>>) {}
    fn row_tooltip(&self, _: usize) -> Option<AnyView> {
        None
    }
    fn empty_text(&self) -> Option<SharedString> {
        Some(if self.loading {
            self.words.collecting.clone()
        } else {
            self.words.none_reported.clone()
        })
    }
    fn header_lines(&self) -> HeaderLines {
        HeaderLines::One
    }
    fn columns_fixed(&self) -> bool {
        true
    }
}

// ---- The tab ----

/// The Security tab view. `PropsWindow` builds it with the process key and the tab's filter.
pub struct SecurityTab {
    key: ProcessKey,
    payload: Option<Arc<TabPayload>>,
    exited: bool,
    needle: SharedString,
    /// Built fields per column.
    columns: [Vec<SecRow>; 2],
    /// Per column, indexes into `columns` that pass the filter.
    visible: [Vec<usize>; 2],
    sections: [Rc<[TlKvSection]>; 2],
    /// (column, index into `visible`).
    cursor: Option<(usize, usize)>,
    label_widths: Option<(u64, [Pixels; 2])>,
    words: Rc<KvWords>,
    general_words: Rc<GeneralWords>,
    sec_words: Rc<SecurityWords>,
    copy_failed: SharedString,
    focus: FocusHandle,
    filter: Entity<PropsFilter>,
    caps_table: Option<Entity<TlTreeTable<CapDelegate>>>,
    groups_table: Option<Entity<TlTreeTable<GroupDelegate>>>,
    _subs: Vec<Subscription>,
}

impl SecurityTab {
    pub fn new(key: ProcessKey, filter: Entity<PropsFilter>, cx: &mut Context<Self>) -> Self {
        let sub = cx.subscribe(&filter, |this, filter, event: &FilterChanged, cx| {
            if filter.update(cx, |f, _| f.accept(event.generation)) {
                this.needle = event.needle.clone();
                this.apply_filter();
                this.apply_table_filter(cx);
                cx.notify();
            }
        });
        let sec_words = security_words(cx);
        let mut tab = Self {
            key,
            payload: None,
            exited: false,
            needle: SharedString::default(),
            columns: build(&sec_words, None),
            visible: Default::default(),
            sections: [Rc::from([]), Rc::from([])],
            cursor: None,
            label_widths: None,
            words: kv_words(cx),
            general_words: general_words(cx),
            sec_words,
            copy_failed: tr(cx, KEY_COPY_FAILED),
            focus: cx.focus_handle().tab_stop(true),
            filter,
            caps_table: None,
            groups_table: None,
            _subs: vec![sub],
        };
        tab.apply_filter();
        tab
    }

    pub fn key(&self) -> ProcessKey {
        self.key
    }

    pub fn filter(&self) -> &Entity<PropsFilter> {
        &self.filter
    }

    /// The fields' focus stop.
    pub fn grid_focus(&self) -> &FocusHandle {
        &self.focus
    }

    pub fn caps_table(&self) -> Option<&Entity<TlTreeTable<CapDelegate>>> {
        self.caps_table.as_ref()
    }

    pub fn groups_table(&self) -> Option<&Entity<TlTreeTable<GroupDelegate>>> {
        self.groups_table.as_ref()
    }

    fn detail(&self) -> Option<&SecurityDetail> {
        self.payload.as_deref().and_then(|p| match p {
            TabPayload::Security(detail) => Some(&**detail),
            _ => None,
        })
    }

    fn init_tables(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.caps_table.is_some() {
            return;
        }
        let mut caps = CapDelegate::new(self.sec_words.clone(), current(cx).status.warning);
        let mut groups = GroupDelegate::new(self.sec_words.clone());
        caps.set_detail(self.detail());
        groups.set_detail(self.detail());
        caps.set_filter(self.needle.clone());
        groups.set_filter(self.needle.clone());
        let caps_table = cx.new(|cx| TlTreeTable::new(caps, window, cx));
        let groups_table = cx.new(|cx| TlTreeTable::new(groups, window, cx));
        self._subs.push(cx.subscribe_in(
            &caps_table,
            window,
            |this, _, event: &TreeTableEvent, window, cx| {
                if matches!(event, TreeTableEvent::CopiedRows { .. }) {
                    this.toast_copied(window, cx);
                }
            },
        ));
        self._subs.push(cx.subscribe_in(
            &groups_table,
            window,
            |this, _, event: &TreeTableEvent, window, cx| {
                if matches!(event, TreeTableEvent::CopiedRows { .. }) {
                    this.toast_copied(window, cx);
                }
            },
        ));
        self.caps_table = Some(caps_table);
        self.groups_table = Some(groups_table);
    }

    /// Takes the Security payload. The same `Arc` again does nothing; while exited the values
    /// stay frozen.
    pub fn set_payload(&mut self, payload: Option<&Arc<TabPayload>>, cx: &mut Context<Self>) {
        if self.exited {
            return;
        }
        let same = match (&self.payload, payload) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        let detail = payload.and_then(|p| match &**p {
            TabPayload::Security(d) => Some(d.clone()),
            _ => None,
        });
        if payload.is_some() && detail.is_none() {
            return;
        }
        self.payload = payload.cloned();
        let words = security_words(cx);
        if !Rc::ptr_eq(&words, &self.sec_words) {
            self.sec_words = words;
            self.label_widths = None;
        }
        self.columns = build(&self.sec_words, detail.as_deref());
        self.apply_filter();
        if let Some(table) = &self.caps_table {
            table.update(cx, |table, cx| {
                table.delegate_mut().set_detail(detail.as_deref());
                table.on_new_version(cx);
            });
        }
        if let Some(table) = &self.groups_table {
            table.update(cx, |table, cx| {
                table.delegate_mut().set_detail(detail.as_deref());
                table.on_new_version(cx);
            });
        }
        cx.notify();
    }

    pub fn set_exited(&mut self, exited: bool, cx: &mut Context<Self>) {
        if self.exited != exited {
            self.exited = exited;
            cx.notify();
        }
    }

    pub fn exited(&self) -> bool {
        self.exited
    }

    /// The shown value text of `id`, or `None` when the filter hides it.
    pub fn value(&self, id: SecField) -> Option<SharedString> {
        let r = self.shown(id)?;
        shown_text_in(&r.row.value, &self.words)
    }

    /// The tooltip of `id`'s value.
    pub fn tooltip(&self, id: SecField) -> Option<SharedString> {
        self.shown(id)?.row.value.tooltip.clone()
    }

    /// The status colour of `id`'s value.
    pub fn tone(&self, id: SecField) -> Option<ValueTone> {
        self.shown(id)?.row.value.tone
    }

    /// The fields shown, per column, in order.
    pub fn visible_ids(&self) -> [Vec<SecField>; 2] {
        [0, 1].map(|c| {
            self.visible[c]
                .iter()
                .map(|&i| self.columns[c][i].id)
                .collect()
        })
    }

    pub fn cursor(&self) -> Option<(usize, usize)> {
        self.cursor
    }

    fn shown(&self, id: SecField) -> Option<&SecRow> {
        (0..2).find_map(|c| {
            self.visible[c]
                .iter()
                .map(|&i| &self.columns[c][i])
                .find(|r| r.id == id)
        })
    }

    fn passes(&self, r: &SecRow) -> bool {
        if self.needle.is_empty() {
            return true;
        }
        let mut haystack = lowercase(&r.row.label);
        haystack.push(' ');
        let shown = shown_text_in(&r.row.value, &self.words).unwrap_or_default();
        haystack.push_str(&lowercase(&shown));
        matches(&haystack, &self.needle)
    }

    fn apply_filter(&mut self) {
        for c in 0..2 {
            self.visible[c] = (0..self.columns[c].len())
                .filter(|&i| self.passes(&self.columns[c][i]))
                .collect();
            let rows = self.visible[c]
                .iter()
                .map(|&i| self.columns[c][i].row.clone())
                .collect();
            self.sections[c] = Rc::from(vec![TlKvSection {
                title: None,
                blocks: vec![TlKvBlock {
                    indent: None,
                    heading: None,
                    rows,
                }],
            }]);
        }
        self.cursor = self.cursor.and_then(|(col, row)| {
            let len = self.visible[col].len();
            (len > 0).then(|| (col, row.min(len - 1)))
        });
    }

    fn apply_table_filter(&mut self, cx: &mut Context<Self>) {
        if let Some(table) = &self.caps_table {
            table.update(cx, |table, cx| {
                table.delegate_mut().set_filter(self.needle.clone());
                table.on_new_version(cx);
            });
        }
        if let Some(table) = &self.groups_table {
            table.update(cx, |table, cx| {
                table.delegate_mut().set_filter(self.needle.clone());
                table.on_new_version(cx);
            });
        }
    }

    // ---- Cursor ----

    /// Moves the cursor to (`col`, `row`), clamped to the column's last field.
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

    fn first_cursor(&mut self, cx: &mut Context<Self>) -> bool {
        if self.cursor.is_some() {
            return false;
        }
        if let Some(col) = (0..2).find(|&c| !self.visible[c].is_empty()) {
            self.set_cursor(col, 0, cx);
        }
        true
    }

    fn step_row(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.first_cursor(cx) {
            return;
        }
        if let Some((col, row)) = self.cursor {
            self.set_cursor(col, row.saturating_add_signed(delta), cx);
        }
    }

    fn step_column(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.first_cursor(cx) {
            return;
        }
        let Some((col, row)) = self.cursor else {
            return;
        };
        let next = if forward {
            col + 1
        } else {
            col.wrapping_sub(1)
        };
        if next < 2 && !self.visible[next].is_empty() {
            self.set_cursor(next, row, cx);
        }
    }

    fn row_end(&mut self, last: bool, cx: &mut Context<Self>) {
        self.first_cursor(cx);
        if let Some((col, _)) = self.cursor {
            self.set_cursor(col, if last { usize::MAX } else { 0 }, cx);
        }
    }

    fn cursor_row(&self) -> Option<&SecRow> {
        let (col, row) = self.cursor?;
        self.visible[col].get(row).map(|&i| &self.columns[col][i])
    }

    // ---- Copy ----

    fn copy_text(&self, r: &SecRow) -> String {
        let shown = shown_text_in(&r.row.value, &self.words).unwrap_or_default();
        value_copy_form(&shown).into_owned()
    }

    fn row_line(&self, r: &SecRow) -> String {
        let label = r.row.label.trim_end();
        let label = label.strip_suffix(':').unwrap_or(label);
        format!("{label}: {}", self.copy_text(r))
    }

    fn column_lines(&self, col: usize) -> impl Iterator<Item = String> + '_ {
        self.visible[col]
            .iter()
            .map(move |&i| self.row_line(&self.columns[col][i]))
    }

    /// Every shown field as `Label: value` lines, left column first.
    pub fn all_fields_text(&self) -> String {
        (0..2)
            .flat_map(|c| self.column_lines(c))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The toast after a keyboard or menu copy: the success text for `kind_value` or a row,
    /// section or all fields, "Could not copy token summary value." on a failed write.
    pub fn copy_toast(
        &self,
        result: Result<(), ()>,
        kind_value: bool,
    ) -> (ToastKind, SharedString) {
        match result {
            Ok(()) if kind_value => (
                ToastKind::Success,
                self.general_words.toast_value_copied.clone(),
            ),
            Ok(()) => (ToastKind::Success, self.general_words.toast_copied.clone()),
            Err(()) => (ToastKind::Error, self.copy_failed.clone()),
        }
    }

    fn copy(&self, text: String, kind_value: bool, window: &mut Window, cx: &mut App) {
        let result = clipboard_write(cx, text);
        let (kind, text) = self.copy_toast(result, kind_value);
        window_toast(window, cx, kind, text);
    }

    fn copy_value(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.cursor_row().map(|r| self.copy_text(r)) {
            self.copy(text, true, window, cx);
        }
    }

    fn copy_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.cursor_row().map(|r| self.row_line(r)) {
            self.copy(text, false, window, cx);
        }
    }

    fn copy_section(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((col, _)) = self.cursor {
            let text = self.column_lines(col).collect::<Vec<_>>().join("\n");
            self.copy(text, false, window, cx);
        }
    }

    fn copy_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.all_fields_text();
        self.copy(text, false, window, cx);
    }

    /// Opens the field menu on (`col`, `row`): at `position`, else under the row.
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
        let position = position.unwrap_or_else(|| {
            let m = metrics(cx);
            let row_h = px(ROW_HEIGHT * m.ui_scale);
            point(m.sp[5] + m.sp[4], m.sp[4] + row_h * (row + 1) as f32)
        });
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

    fn on_menu_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.first_cursor(cx);
        if let Some((col, row)) = self.cursor {
            self.open_field_menu(col, row, None, window, cx);
        }
    }

    fn label_widths(&mut self, m: &UiMetrics, window: &Window, cx: &mut App) -> [Pixels; 2] {
        if let Some((version, widths)) = self.label_widths
            && version == m.style_version
        {
            return widths;
        }
        if !cx.has_global::<TextMetricsCache>() {
            cx.set_global(TextMetricsCache::default());
        }
        let size = m.text.base;
        let words = self.sec_words.clone();
        let widths = cx.update_global::<TextMetricsCache, _>(|cache, cx| {
            cache.clear_if_stale(m.style_version);
            [&LEFT[..], &RIGHT[..]].map(|ids| {
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

    fn focused_table(&self, window: &Window, cx: &App) -> Option<bool> {
        if self
            .caps_table
            .as_ref()
            .is_some_and(|t| t.read(cx).focus_handle().contains_focused(window, cx))
        {
            Some(true)
        } else if self
            .groups_table
            .as_ref()
            .is_some_and(|t| t.read(cx).focus_handle().contains_focused(window, cx))
        {
            Some(false)
        } else {
            None
        }
    }

    fn selected_rows_tsv(&self, caps: bool, cx: &App) -> String {
        if caps {
            let Some(table) = &self.caps_table else {
                return String::new();
            };
            let table = table.read(cx);
            let d = &table.delegate;
            let mut rows: Vec<(usize, u8)> = table
                .selection
                .ids()
                .filter_map(|id| d.row_index(id).map(|ix| (ix, id)))
                .collect();
            rows.sort_unstable();
            rows.into_iter()
                .filter_map(|(_, id)| d.row_tsv(id))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            let Some(table) = &self.groups_table else {
                return String::new();
            };
            let table = table.read(cx);
            let d = &table.delegate;
            let mut rows: Vec<(usize, u32)> = table
                .selection
                .ids()
                .filter_map(|id| d.row_index(id).map(|ix| (ix, id)))
                .collect();
            rows.sort_unstable();
            rows.into_iter()
                .filter_map(|(_, id)| d.row_tsv(id))
                .collect::<Vec<_>>()
                .join("\n")
        }
    }

    fn on_copy_table_rows(&mut self, _: &CopyRow, window: &mut Window, cx: &mut Context<Self>) {
        let Some(caps) = self.focused_table(window, cx) else {
            return;
        };
        let text = self.selected_rows_tsv(caps, cx);
        if text.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.toast_copied(window, cx);
    }

    fn on_copy_table_cell(
        &mut self,
        _: &SecurityCopyCell,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = match self.focused_table(window, cx) {
            Some(true) => self.caps_table.as_ref().and_then(|table| {
                let d = &table.read(cx).delegate;
                d.menu_cell.and_then(|(id, col)| d.cell_text(id, col, true))
            }),
            Some(false) => self.groups_table.as_ref().and_then(|table| {
                let d = &table.read(cx).delegate;
                d.menu_cell.and_then(|(id, col)| d.cell_text(id, col))
            }),
            None => None,
        };
        let Some(text) = text else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.toast_copied(window, cx);
    }

    fn toast_copied(&self, window: &mut Window, cx: &mut App) {
        window_toast(
            window,
            cx,
            ToastKind::Success,
            self.sec_words.copied.clone(),
        );
    }

    fn table_title(&self, caps: bool, cx: &App) -> String {
        let count = if caps {
            self.caps_table
                .as_ref()
                .map_or(0, |t| t.read(cx).delegate.row_count())
        } else {
            self.groups_table
                .as_ref()
                .map_or(0, |t| t.read(cx).delegate.row_count())
        };
        let mut n = String::new();
        units::count(&mut n, count as u64);
        let mut out = String::new();
        let template = if caps {
            &self.sec_words.cap_title
        } else {
            &self.sec_words.group_title
        };
        let _ = template.write(&mut out, &[&n]);
        out
    }
}

/// A toast in `window`'s own stack.
fn window_toast(window: &mut Window, cx: &mut App, kind: ToastKind, text: SharedString) {
    match ToastQueue::of_window(window, cx) {
        Some(queue) => queue.update(cx, |q, cx| q.push(kind, text, None, None, cx)),
        None => warn!(target: "tasklens::ui", "no toast stack for the Security tab"),
    }
}

/// The generic table paints metric cells as text. Capability marks are the Lucide `check`
/// path, so this transparent canvas paints them over the otherwise-empty set cells. It reads
/// the table's live layout and scroll handle during paint, which keeps the 14 px marks centred
/// after column resizing and while either axis scrolls.
fn cap_checks(table: Entity<TlTreeTable<CapDelegate>>, m: UiMetrics) -> impl IntoElement {
    let row_h = px(ROW_HEIGHT * m.ui_scale);
    let icon = px(CAP_CHECK_SIZE * m.ui_scale);
    let stroke = px(CAP_CHECK_SIZE / 12. * m.ui_scale);
    let header_h = px(HEADER_H_ONE * m.ui_scale);
    div()
        .debug_selector(|| "SecurityCapabilityChecks".into())
        .absolute()
        .top(header_h)
        .bottom_0()
        .left_0()
        .right_0()
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, (), window, cx| {
                    let table = table.read(cx);
                    let scroll = table.scroll_handle();
                    let scroll_offset = scroll.0.borrow().base_handle.offset();
                    let x_offset = table.x_offset();
                    let d = &table.delegate;
                    let layout = d.columns();
                    let color = current(cx).text_primary;
                    let columns = [
                        CapColumn::Effective,
                        CapColumn::Permitted,
                        CapColumn::Inheritable,
                        CapColumn::Bounding,
                        CapColumn::Ambient,
                    ];
                    #[cfg(any(test, feature = "test-hooks"))]
                    d.check_icon_bounds.borrow_mut().clear();
                    window.with_content_mask(Some(ContentMask { bounds }), |window| {
                        for ix in table.visible_range() {
                            let Some(row) = d.view.get(ix).and_then(|&cap| d.row(cap)) else {
                                continue;
                            };
                            let cy = bounds.top() + scroll_offset.y + row_h * (ix as f32 + 0.5);
                            for (col, set) in columns.iter().copied().zip([
                                row.effective,
                                row.permitted,
                                row.inheritable,
                                row.bounding,
                                row.ambient,
                            ]) {
                                if !set {
                                    continue;
                                }
                                let Some(pos) = layout
                                    .visible_columns()
                                    .iter()
                                    .position(|shown| *shown == col)
                                else {
                                    continue;
                                };
                                let cx =
                                    bounds.left() + layout.offsets()[pos] + layout.width(col) / 2.
                                        - x_offset;
                                let origin = point(cx - icon / 2., cy - icon / 2.);
                                #[cfg(any(test, feature = "test-hooks"))]
                                d.check_icon_bounds.borrow_mut().push(gpui_kit::Bounds::new(
                                    origin,
                                    gpui_kit::size(icon, icon),
                                ));
                                let at = |x: f32, y: f32| {
                                    origin + point(icon * (x / 24.), icon * (y / 24.))
                                };
                                // Lucide `check`: M20 6 9 17l-5-5, viewBox 24, stroke 2.
                                let mut path = PathBuilder::stroke(stroke);
                                path.move_to(at(20., 6.));
                                path.line_to(at(9., 17.));
                                path.line_to(at(4., 12.));
                                if let Ok(path) = path.build() {
                                    window.paint_path(path, color);
                                }
                            }
                        }
                    });
                },
            )
            .size_full(),
        )
}

impl Render for SecurityTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.init_tables(window, cx);
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let widths = self.label_widths(&m, window, cx);
        let row_height = px(ROW_HEIGHT * m.ui_scale);
        let focused = self.focus.contains_focused(window, cx);
        let view = cx.entity().downgrade();
        let copied = self.general_words.toast_value_copied.clone();
        let cursor = self.cursor;
        let grids = (0..2).map(|col| {
            let view = view.clone();
            let menu_view = view.clone();
            let copied = copied.clone();
            div().flex_1().flex_basis(px(0.)).min_w_0().child(
                TlKeyValueGrid::new(("security-col", col), KvLayout::Inline)
                    .row_height(row_height)
                    .label_width(widths[col])
                    .sections(&self.sections[col])
                    .copy_on_click(true)
                    .cursor(cursor.filter(|(c, _)| *c == col && focused).map(|(_, r)| r))
                    .on_copied(move |_, window, cx| {
                        window_toast(window, cx, ToastKind::Success, copied.clone());
                    })
                    .on_value_mouse_down(move |n, _, cx| {
                        let _ = view.update(cx, |this, cx| this.set_cursor(col, n, cx));
                    })
                    .on_context_menu(move |n, position, window, cx| {
                        let _ = menu_view.update(cx, |this, cx| {
                            this.open_field_menu(col, n, Some(position), window, cx);
                        });
                    }),
            )
        });
        let summary = div()
            .id("security-fields")
            .debug_selector(|| "SecuritySummary".into())
            .key_context(KV_GRID_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &PropsFieldUp, _, cx| this.step_row(-1, cx)))
            .on_action(cx.listener(|this, _: &PropsFieldDown, _, cx| this.step_row(1, cx)))
            .on_action(cx.listener(|this, _: &PropsFieldLeft, _, cx| this.step_column(false, cx)))
            .on_action(cx.listener(|this, _: &PropsFieldRight, _, cx| this.step_column(true, cx)))
            .on_action(cx.listener(|this, _: &PropsFieldHome, _, cx| this.row_end(false, cx)))
            .on_action(cx.listener(|this, _: &PropsFieldEnd, _, cx| this.row_end(true, cx)))
            .on_action(cx.listener(|this, _: &PropsActivateField, window, cx| {
                this.copy_value(window, cx);
            }))
            .on_action(cx.listener(|this, _: &PropsCopyValue, window, cx| {
                this.copy_value(window, cx);
            }))
            .on_action(cx.listener(|this, _: &Copy, window, cx| this.copy_value(window, cx)))
            .on_action(cx.listener(|this, _: &GeneralCopyRow, window, cx| {
                this.copy_row(window, cx);
            }))
            .on_action(cx.listener(|this, _: &GeneralCopySection, window, cx| {
                this.copy_section(window, cx);
            }))
            .on_action(cx.listener(|this, _: &PropsCopyAllFields, window, cx| {
                this.copy_all(window, cx);
            }))
            .on_action(cx.listener(|this, _: &PropsFieldMenu, window, cx| {
                this.on_menu_key(window, cx);
            }))
            .flex_none()
            .max_h(relative(SUMMARY_MAX))
            .overflow_y_scroll()
            .w_full()
            .px(m.sp[5])
            .pt(m.sp[4])
            .child(h_flex().w_full().items_start().gap(m.sp[4]).children(grids));
        let caps_title = self.table_title(true, cx);
        let groups_title = self.table_title(false, cx);
        let section_header = |title: String| {
            let mut label = div();
            *label.text_style() = label_style(&m, m.text.base, FontWeight::MEDIUM);
            label
                .h(px(SECTION_HEADER_HEIGHT * m.ui_scale))
                .flex_none()
                .flex()
                .items_center()
                .pl(m.sp[5])
                .bg(theme.row_hover)
                .text_color(theme.text_secondary)
                .child(title)
        };
        let caps_table = self.caps_table.as_ref().expect("initialized").clone();
        // The tables share the height by row count: 60% and 40% when both are long, less for a
        // short table (UI-PWN-015).
        let caps_weight = caps_table
            .read(cx)
            .delegate
            .row_count()
            .clamp(3, CAPS_SHARE_ROWS);
        let groups_weight = self
            .groups_table
            .as_ref()
            .map_or(0, |t| t.read(cx).delegate.row_count())
            .clamp(3, GROUPS_SHARE_ROWS);
        let caps = v_flex()
            .debug_selector(|| "SecurityCapabilities".into())
            .flex_grow(caps_weight as f32)
            .flex_basis(px(0.))
            .min_h_0()
            .child(section_header(caps_title))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(caps_table.clone())
                    .child(cap_checks(caps_table, m)),
            );
        let groups = v_flex()
            .debug_selector(|| "SecurityGroups".into())
            .flex_grow(groups_weight as f32)
            .flex_basis(px(0.))
            .min_h_0()
            .child(section_header(groups_title))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.groups_table.as_ref().expect("initialized").clone()),
            );
        v_flex()
            .debug_selector(|| "PropsSecurity".into())
            .size_full()
            .on_action(cx.listener(Self::on_copy_table_rows))
            .on_action(cx.listener(Self::on_copy_table_cell))
            .child(summary)
            .child(v_flex().flex_1().min_h_0().child(caps).child(groups))
    }
}

#[cfg(test)]
mod tests {
    use tl_model::details::{CapRow, GroupMembershipRow, NsRow};

    use super::*;

    fn valid<T>(value: T) -> Total<T> {
        Total {
            value,
            validity: Validity::Valid,
        }
    }

    fn names(n: [&str; 4]) -> [Arc<str>; 4] {
        n.map(Arc::from)
    }

    fn detail() -> SecurityDetail {
        let ids = IdTriple {
            real: 1000,
            effective: 1000,
            saved: 1000,
            fs: 1000,
        };
        let ns = |kind| NsRow {
            kind,
            state: NsState::SharedWithInit,
            validity: Validity::Valid,
        };
        SecurityDetail {
            uids: valid(ids.clone()),
            uid_names: valid(names(["oystein"; 4])),
            gids: valid(ids),
            gid_names: valid(names(["oystein"; 4])),
            elevated: valid(Elevated::No),
            no_new_privs: valid(false),
            seccomp: valid(Seccomp::Disabled),
            dumpable: valid(true),
            security_label: valid(Arc::from("unconfined")),
            session: Some(Arc::from("2")),
            audit_session: Some(NO_ID),
            login_uid: Some(1000),
            namespaces: NS_KINDS.map(ns),
            store_bypass: valid(Arc::from("thread vulnerable")),
            indirect_branch: valid(Arc::from("conditional enabled")),
            caps: Arc::from(vec![
                CapRow {
                    cap: 12,
                    effective: true,
                    permitted: true,
                    inheritable: false,
                    bounding: true,
                    ambient: false,
                    risky: true,
                },
                CapRow {
                    cap: 21,
                    effective: false,
                    permitted: true,
                    inheritable: false,
                    bounding: true,
                    ambient: true,
                    risky: true,
                },
            ]),
            groups: Arc::from(vec![GroupMembershipRow {
                name: Arc::from("oystein"),
                gid: 1000,
                primary: true,
                root_equivalent: false,
                admin: false,
            }]),
        }
    }

    fn text(rows: &[SecRow], id: SecField) -> String {
        rows.iter()
            .find(|r| r.id == id)
            .map(|r| r.row.value.text.to_string())
            .unwrap_or_default()
    }

    #[test]
    fn formatters() {
        let d = detail();
        let w = SecurityWords::en();
        let [left, right] = build(&w, Some(&d));
        assert_eq!(text(&left, SecField::RiskyCaps), "SYS_ADMIN, NET_ADMIN");
        assert_eq!(text(&left, SecField::Flags), "Dumpable");
        assert_eq!(text(&left, SecField::AuditSession), "None");
        assert_eq!(text(&left, SecField::LoginUid), "oystein (1000)");
        assert_eq!(text(&left, SecField::Groups), "0");
        assert_eq!(text(&left, SecField::AdminGroups), "None reported");
        assert_eq!(text(&right, SecField::Elevated), "No");
        assert_eq!(text(&right, SecField::Ns(NsKind::Net)), "Shared with init");
        let mut s = String::new();
        fmt_flags(&mut s, &w, true, Seccomp::Filter { filters: 3 }, false);
        assert_eq!(s, "NoNewPrivs, Seccomp filter (3 filters)");
        s.clear();
        fmt_ns(&mut s, &w, NsState::Isolated { inode: 4026532211 });
        assert_eq!(s, "Isolated (4026532211)");
        assert_eq!(build(&w, None)[1].len(), RIGHT.len());
    }

    /// tasklens-yms.41: an unreported Dumpable leaves the Flags row at "None reported"; the
    /// Login UID and speculation rows carry their values.
    #[test]
    fn flags_ignore_unreported_dumpable() {
        let mut d = detail();
        d.dumpable = Total {
            value: true,
            validity: Validity::Unsupported,
        };
        let w = SecurityWords::en();
        let [left, right] = build(&w, Some(&d));
        assert_eq!(text(&left, SecField::Flags), "None reported");
        assert_eq!(text(&left, SecField::LoginUid), "oystein (1000)");
        assert_eq!(text(&right, SecField::StoreBypass), "thread vulnerable");
        assert_eq!(
            text(&right, SecField::IndirectBranch),
            "conditional enabled"
        );
    }
}
