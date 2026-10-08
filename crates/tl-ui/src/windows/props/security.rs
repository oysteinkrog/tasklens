//! The Security tab of Process Properties: the summary of identity, capabilities, flags, LSM
//! label, sessions and namespaces in two key/value columns (UI-PWN-014, UI-PWN-018, PROC-082,
//! R-050). The Capabilities and Groups tables under it are E23-022.
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

use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::input::Copy;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    App, BorrowAppContext as _, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Point, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, point, px, relative,
};
use tl_model::details::{Elevated, IdTriple, NsKind, NsState, Seccomp, SecurityDetail, TabPayload};
use tl_model::ids::ProcessKey;
use tl_model::table::Total;
use tl_model::units;
use tl_model::validity::Validity;
use tracing::warn;

use super::filter::{FilterChanged, PropsFilter, lowercase, matches};
use super::general::{
    GeneralCopyRow, GeneralCopySection, GeneralWords, RESTRICTED_TOOLTIP, escape_controls,
    general_words,
};
use crate::components::kv_grid::{
    KvLayout, KvWords, TlKeyValueGrid, TlKvBlock, TlKvRow, TlKvSection, TlValue, ValueTone,
    kv_words, shown_text_in, value_copy_form,
};
use crate::components::menu::{TlMenu, open_context_menu_at};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::i18n::tr;
use crate::text::metrics::TextMetricsCache;
use crate::theme::{UiMetrics, metrics};
use crate::util::clipboard_write;
use crate::windows::actions::{
    KV_GRID_CONTEXT, PropsActivateField, PropsCopyAllFields, PropsCopyValue, PropsFieldDown,
    PropsFieldEnd, PropsFieldHome, PropsFieldLeft, PropsFieldMenu, PropsFieldRight, PropsFieldUp,
};

/// Row height, × ui_scale.
const ROW_HEIGHT: f32 = 26.;
/// Label column clamp, × ui_scale (UI-PWN-006).
const LABEL_MIN: f32 = 96.;
const LABEL_MAX: f32 = 150.;
/// The summary takes at most this share of the body.
const SUMMARY_MAX: f32 = 0.45;
/// `None` in the audit session and login UID files.
pub const NO_ID: u32 = u32::MAX;

const KEY_COPY_FAILED: &str = "pwn-security-toast-copy-failed";
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
    pub const fn label(self) -> &'static str {
        match self {
            Self::User => "User:",
            Self::Group => "Group:",
            Self::Groups => "Groups:",
            Self::RiskyCaps => "Risky capabilities:",
            Self::AdminGroups => "Admin groups:",
            Self::Flags => "Flags:",
            Self::Label => "Security label:",
            Self::Session => "Session:",
            Self::AuditSession => "Audit session:",
            Self::LoginUid => "Login UID:",
            Self::Elevated => "Elevated:",
            Self::Ns(NsKind::User) => "User namespace:",
            Self::Ns(NsKind::Pid) => "PID namespace:",
            Self::Ns(NsKind::Net) => "Network namespace:",
            Self::Ns(NsKind::Mnt) => "Mount namespace:",
            Self::Ns(NsKind::Ipc) => "IPC namespace:",
            Self::Ns(NsKind::Uts) => "UTS namespace:",
            Self::Ns(NsKind::Cgroup) => "Cgroup namespace:",
            Self::Ns(NsKind::Time) => "Time namespace:",
            Self::StoreBypass => "Store bypass:",
            Self::IndirectBranch => "Indirect branch:",
        }
    }
}

// ---- Formatters (pure) ----

/// `oystein (1000)` when all four ids are equal; else
/// `real oystein (1000), effective root (0), saved root (0), fs root (0)`.
pub fn fmt_ids(out: &mut String, ids: &IdTriple, names: &[Arc<str>; 4]) {
    let all = [ids.real, ids.effective, ids.saved, ids.fs];
    if all.iter().all(|&id| id == ids.real) {
        let _ = write!(out, "{} ({})", names[0], ids.real);
        return;
    }
    for (i, (word, id)) in ["real", "effective", "saved", "fs"]
        .iter()
        .zip(all)
        .enumerate()
    {
        if i > 0 {
            out.push_str(", ");
        }
        let _ = write!(out, "{word} {} ({id})", names[i]);
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

/// `wheel, docker (root-equivalent)` in payload order. Empty when none.
pub fn fmt_admin_groups(out: &mut String, d: &SecurityDetail) {
    let groups = d.groups.iter().filter(|g| g.admin || g.root_equivalent);
    for (i, g) in groups.enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&g.name);
        if g.root_equivalent {
            out.push_str(" (root-equivalent)");
        }
    }
}

/// `NoNewPrivs, Seccomp filter (3 filters), Dumpable`. Empty when none is set.
pub fn fmt_flags(out: &mut String, no_new_privs: bool, seccomp: Seccomp, dumpable: bool) {
    let mut parts: Vec<String> = Vec::new();
    if no_new_privs {
        parts.push("NoNewPrivs".into());
    }
    match seccomp {
        Seccomp::Disabled => {}
        Seccomp::Strict => parts.push("Seccomp strict".into()),
        Seccomp::Filter { filters: 1 } => parts.push("Seccomp filter (1 filter)".into()),
        Seccomp::Filter { filters } => parts.push(format!("Seccomp filter ({filters} filters)")),
    }
    if dumpable {
        parts.push("Dumpable".into());
    }
    out.push_str(&parts.join(", "));
}

/// `Shared with init`, `Isolated (4026532211)`, `Same as tasklens` or `Isolated (4026532211)`,
/// and the tooltip that says what it was compared with.
pub fn fmt_ns(out: &mut String, state: NsState) -> &'static str {
    match state {
        NsState::SharedWithInit => {
            out.push_str("Shared with init");
            NS_TIP_INIT
        }
        NsState::Isolated { inode } => {
            let _ = write!(out, "Isolated ({inode})");
            NS_TIP_INIT
        }
        NsState::SameAsTasklens => {
            out.push_str("Same as tasklens");
            NS_TIP_SELF
        }
        NsState::IsolatedFromTasklens { inode } => {
            let _ = write!(out, "Isolated ({inode})");
            NS_TIP_SELF
        }
    }
}

fn elevated_word(e: Elevated) -> &'static str {
    match e {
        Elevated::Root => "Yes (root)",
        Elevated::Capabilities => "Yes (capabilities)",
        Elevated::Setuid => "Setuid",
        Elevated::No => "No",
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

fn row(id: SecField, text: impl Into<SharedString>, validity: Validity) -> SecRow {
    let mut value = TlValue::new(text);
    value.validity = validity;
    if validity == Validity::Restricted {
        value.tooltip = Some(RESTRICTED_TOOLTIP.into());
    }
    SecRow {
        id,
        row: TlKvRow {
            label: id.label().into(),
            value,
        },
    }
}

/// A `Total` formatted by `f` when Valid or Stale; else the grid's word for its validity.
fn total<T>(id: SecField, t: &Total<T>, f: impl FnOnce(&T, &mut String)) -> SecRow {
    let mut s = String::new();
    if matches!(t.validity, Validity::Valid | Validity::Stale) {
        f(&t.value, &mut s);
    }
    row(id, s, t.validity)
}

fn or_none_reported(s: String) -> String {
    if s.is_empty() {
        "None reported".into()
    } else {
        s
    }
}

/// The two columns from `d`; without a payload every value is `Collecting…`.
pub fn build(d: Option<&SecurityDetail>) -> [Vec<SecRow>; 2] {
    let Some(d) = d else {
        let collecting = |ids: &[SecField]| -> Vec<SecRow> {
            ids.iter()
                .map(|&id| row(id, "", Validity::WarmingUp))
                .collect()
        };
        return [collecting(&LEFT), collecting(&RIGHT)];
    };
    let mut left = Vec::with_capacity(LEFT.len());
    let ids_row = |id, ids: &Total<IdTriple>, names: &Total<[Arc<str>; 4]>| {
        let validity = worst(ids.validity, names.validity);
        let mut s = String::new();
        if matches!(validity, Validity::Valid | Validity::Stale) {
            fmt_ids(&mut s, &ids.value, &names.value);
        }
        row(id, s, validity)
    };
    left.push(ids_row(SecField::User, &d.uids, &d.uid_names));
    left.push(ids_row(SecField::Group, &d.gids, &d.gid_names));
    let mut count = String::new();
    units::count(
        &mut count,
        d.groups.iter().filter(|g| !g.primary).count() as u64,
    );
    left.push(row(SecField::Groups, count, Validity::Valid));
    let mut risky = String::new();
    fmt_risky(&mut risky, d);
    let mut risky_row = row(
        SecField::RiskyCaps,
        or_none_reported(risky.clone()),
        Validity::Valid,
    );
    if !risky.is_empty() {
        risky_row.row.value.tone = Some(ValueTone::Warning);
        risky_row.row.value.tooltip = Some(RISKY_TOOLTIP.into());
    }
    left.push(risky_row);
    let mut admin = String::new();
    fmt_admin_groups(&mut admin, d);
    left.push(row(
        SecField::AdminGroups,
        or_none_reported(admin),
        Validity::Valid,
    ));
    let flags_validity = worst(
        worst(d.no_new_privs.validity, d.seccomp.validity),
        d.dumpable.validity,
    );
    let mut flags = String::new();
    if matches!(flags_validity, Validity::Valid | Validity::Stale) {
        fmt_flags(
            &mut flags,
            d.no_new_privs.value,
            d.seccomp.value,
            d.dumpable.value,
        );
        flags = or_none_reported(flags);
    }
    left.push(row(SecField::Flags, flags, flags_validity));
    left.push(total(SecField::Label, &d.security_label, |v, s| {
        escape_controls(s, v)
    }));
    let session = match &d.session {
        Some(id) => {
            let mut s = String::new();
            escape_controls(&mut s, id);
            s
        }
        None => "None".into(),
    };
    left.push(row(SecField::Session, session, Validity::Valid));
    let audit = match d.audit_session.filter(|&id| id != NO_ID) {
        Some(id) => id.to_string(),
        None => "None".into(),
    };
    left.push(row(SecField::AuditSession, audit, Validity::Valid));
    let login = match d.login_uid.filter(|&id| id != NO_ID) {
        Some(uid) => login_text(d, uid),
        None => "None".into(),
    };
    left.push(row(SecField::LoginUid, login, Validity::Valid));

    let mut right = Vec::with_capacity(RIGHT.len());
    right.push(total(SecField::Elevated, &d.elevated, |e, s| {
        s.push_str(elevated_word(*e))
    }));
    for kind in NS_KINDS {
        let ns = d.namespaces.iter().find(|n| n.kind == kind);
        let Some(ns) = ns else {
            right.push(row(SecField::Ns(kind), "", Validity::Unsupported));
            continue;
        };
        let mut s = String::new();
        let tip = matches!(ns.validity, Validity::Valid | Validity::Stale)
            .then(|| fmt_ns(&mut s, ns.state));
        let mut r = row(SecField::Ns(kind), s, ns.validity);
        if let Some(tip) = tip {
            r.row.value.tooltip = Some(tip.into());
        }
        right.push(r);
    }
    right.push(total(SecField::StoreBypass, &d.store_bypass, |v, s| {
        escape_controls(s, v)
    }));
    right.push(total(
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
    copy_failed: SharedString,
    focus: FocusHandle,
    filter: Entity<PropsFilter>,
    _subs: Vec<Subscription>,
}

impl SecurityTab {
    pub fn new(key: ProcessKey, filter: Entity<PropsFilter>, cx: &mut Context<Self>) -> Self {
        let sub = cx.subscribe(&filter, |this, filter, event: &FilterChanged, cx| {
            if filter.update(cx, |f, _| f.accept(event.generation)) {
                this.needle = event.needle.clone();
                this.apply_filter();
                cx.notify();
            }
        });
        let mut tab = Self {
            key,
            payload: None,
            exited: false,
            needle: SharedString::default(),
            columns: build(None),
            visible: Default::default(),
            sections: [Rc::from([]), Rc::from([])],
            cursor: None,
            label_widths: None,
            words: kv_words(cx),
            general_words: general_words(cx),
            copy_failed: tr(cx, KEY_COPY_FAILED),
            focus: cx.focus_handle().tab_stop(true),
            filter,
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
        self.columns = build(detail.as_deref());
        self.apply_filter();
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
        let widths = cx.update_global::<TextMetricsCache, _>(|cache, cx| {
            cache.clear_if_stale(m.style_version);
            [&LEFT[..], &RIGHT[..]].map(|ids| {
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

/// A toast in `window`'s own stack.
fn window_toast(window: &mut Window, cx: &mut App, kind: ToastKind, text: SharedString) {
    match ToastQueue::of_window(window, cx) {
        Some(queue) => queue.update(cx, |q, cx| q.push(kind, text, None, None, cx)),
        None => warn!(target: "tasklens::ui", "no toast stack for the Security tab"),
    }
}

impl Render for SecurityTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *metrics(cx);
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
        // E23-022 adds the Capabilities (60%) and Groups (40%) sections under the summary.
        v_flex()
            .debug_selector(|| "PropsSecurity".into())
            .size_full()
            .child(summary)
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
        let [left, right] = build(Some(&d));
        assert_eq!(text(&left, SecField::RiskyCaps), "SYS_ADMIN, NET_ADMIN");
        assert_eq!(text(&left, SecField::Flags), "Dumpable");
        assert_eq!(text(&left, SecField::AuditSession), "None");
        assert_eq!(text(&left, SecField::LoginUid), "oystein (1000)");
        assert_eq!(text(&left, SecField::Groups), "0");
        assert_eq!(text(&left, SecField::AdminGroups), "None reported");
        assert_eq!(text(&right, SecField::Elevated), "No");
        assert_eq!(text(&right, SecField::Ns(NsKind::Net)), "Shared with init");
        let mut s = String::new();
        fmt_flags(&mut s, true, Seccomp::Filter { filters: 3 }, false);
        assert_eq!(s, "NoNewPrivs, Seccomp filter (3 filters)");
        s.clear();
        fmt_ns(&mut s, NsState::Isolated { inode: 4026532211 });
        assert_eq!(s, "Isolated (4026532211)");
        assert_eq!(build(None)[1].len(), RIGHT.len());
    }
}
