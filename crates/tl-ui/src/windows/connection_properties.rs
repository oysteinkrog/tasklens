//! Connection Properties window (UI-SYS-068).
//!
//! A read-only list of the socket's values, found by socket cookie in every Connections or
//! Processes snapshot. When the cookie leaves the snapshot the window keeps the last values,
//! greys them and appends " (closed)" to the title. The window holds `Demand.connections` while
//! it is open, so the values keep refreshing after the Connections tab is left.

use std::rc::Rc;
use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::base::Root;
use gpui_kit::component::h_flex;
use gpui_kit::{
    App, AppContext as _, Context, ElementId, FocusHandle, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, px, size,
};
use tl_model::actions::{Family, Proto};
use tl_model::demand::Demand;
use tl_model::health::HelperState;
use tl_model::snapshot::{ComponentMask, ConnRecord, ConnState, ProcessSnapshot, Snapshot};
use tl_model::units;
use tl_model::validity::Validity;
use tracing::warn;

use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::icons::TlIcon;
use crate::components::kv_grid::{
    KvLayout, TlKeyValueGrid, TlKvBlock, TlKvRow, TlKvSection, TlValue, UnsupportedWord, kv_words,
    shown_text_in,
};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::window_frame::{
    CloseSecondaryWindow, TlSecondaryWindow, TlWindowKind, TlWindowSpec,
};
use crate::host::{HostContext, with_host_name};
use crate::hub::SnapshotChanged;
use crate::i18n::{tr, tr_args};
use crate::pages::connections::actions::ConnectionJumpToProcess;
use crate::pages::connections::jump_to_owner;
use crate::theme::{current, metrics};
use crate::util::clipboard_write;
use crate::window::main::push_main_toast;
use crate::window::registry::{GlobalRegistry, OpenOutcome, WindowKey};

/// Default window size at 100% scale.
const WINDOW_SIZE: (f32, f32) = (560., 520.);
/// Smallest window size at 100% scale.
const MIN_SIZE: (f32, f32) = (440., 360.);
/// Footer height at 100% scale.
const FOOTER_H: f32 = 48.;
/// Label column width at 100% scale.
const LABEL_W: f32 = 200.;
/// Row height at 100% scale.
const ROW_H: f32 = 28.;
/// Copy icon size and hit area at 100% scale.
const COPY_ICON: f32 = 14.;
const COPY_CELL: f32 = 20.;
/// The most lines the window shows: the TCP list.
const MAX_LINES: usize = 14;

/// One label and value.
#[derive(Clone)]
pub struct PropLine {
    pub label: SharedString,
    pub value: TlValue,
}

/// The title, such as "Connection properties: TCP ESTABLISHED".
fn title_for(record: &ConnRecord, closed: bool, cx: &App) -> SharedString {
    let key = if closed {
        "connections-props-title-closed"
    } else {
        "connections-props-title"
    };
    tr_args(
        cx,
        key,
        &[
            ("proto", FluentValue::from(proto_text(record.proto))),
            ("state", FluentValue::from(record.state.label())),
        ],
    )
}

fn proto_text(proto: Proto) -> &'static str {
    match proto {
        Proto::Tcp => "TCP",
        Proto::Udp => "UDP",
    }
}

/// Splits "addr:port", "[addr]:port" or "*:*" into address and port.
fn address_parts(value: &str) -> (&str, &str) {
    if value == "*:*" {
        return ("*", "*");
    }
    if let Some(rest) = value.strip_prefix('[')
        && let Some((address, port)) = rest.rsplit_once("]:")
    {
        return (address, port);
    }
    value.rsplit_once(':').unwrap_or((value, "*"))
}

fn address_line(cx: &App, value: &str, foreign: bool) -> TlValue {
    let (address, port) = address_parts(value);
    if foreign && (value == "*:*" || port == "*") {
        return TlValue::new("*");
    }
    TlValue::new(tr_args(
        cx,
        "connections-props-address",
        &[
            ("address", FluentValue::from(address)),
            ("port", FluentValue::from(port)),
        ],
    ))
}

fn counted(n: u64) -> String {
    let mut out = String::new();
    units::count(&mut out, n);
    out
}

fn not_reported() -> TlValue {
    TlValue {
        validity: Validity::Unsupported,
        word: Some(UnsupportedWord::NotReported),
        ..TlValue::new("")
    }
}

/// The process lookup for the lowest-PID owner.
fn owner_of<'a>(
    record: &ConnRecord,
    processes: &'a ProcessSnapshot,
) -> Option<&'a tl_model::process::ProcessRecord> {
    record
        .owners
        .iter()
        .filter_map(|key| processes.get(*key))
        .min_by_key(|process| process.key.pid)
}

/// The locale key for the "Closable here" value. First match wins.
fn closable_key(
    record: &ConnRecord,
    helper: HelperState,
    destroy_supported: Option<bool>,
) -> &'static str {
    if record.proto == Proto::Udp {
        "connections-props-closable-udp"
    } else if record.state == ConnState::TimeWait {
        "connections-props-closable-time-wait"
    } else if helper == HelperState::Missing {
        "connections-props-closable-needs-helper"
    } else if destroy_supported == Some(false) {
        "connections-props-closable-no-kernel"
    } else {
        "connections-props-closable-yes"
    }
}

/// Builds the lines in UI-SYS-068 order. TCP lines only for TCP sockets. `closed` marks every
/// value Stale, which the grid draws in `text_secondary`.
pub fn build_lines(
    record: &ConnRecord,
    processes: &ProcessSnapshot,
    helper: HelperState,
    destroy_supported: Option<bool>,
    closed: bool,
    cx: &App,
) -> Vec<PropLine> {
    let line = |key: &str, value: TlValue| PropLine {
        label: tr(cx, key),
        value,
    };
    let mut lines = Vec::with_capacity(MAX_LINES);
    let family = match record.family {
        Family::Inet => "IPv4",
        Family::Inet6 => "IPv6",
    };
    lines.push(line(
        "connections-props-label-protocol",
        TlValue::new(format!(
            "{} / {} / {family}",
            proto_text(record.proto),
            record.state.label()
        )),
    ));
    lines.push(line(
        "connections-props-label-local",
        address_line(cx, &record.local, false),
    ));
    lines.push(line(
        "connections-props-label-foreign",
        address_line(cx, &record.foreign, true),
    ));

    let kernel = record.inode == 0 && record.owners.is_empty();
    let process = if record.owner_validity == Validity::Restricted {
        TlValue {
            validity: Validity::Restricted,
            ..TlValue::new("")
        }
    } else if kernel {
        TlValue::new(tr(cx, "connections-props-no-owner"))
    } else if record.owners.is_empty() {
        TlValue::new("?")
    } else {
        let owner = owner_of(record, processes);
        let pid = record
            .owners
            .iter()
            .map(|key| key.pid)
            .min()
            .unwrap_or_default();
        let name = owner.map_or("?", |p| p.meta.name.as_ref());
        let text = tr_args(
            cx,
            "connections-props-process",
            &[
                ("name", FluentValue::from(name)),
                ("pid", FluentValue::from(pid.to_string())),
            ],
        );
        TlValue {
            link: Some(Box::new(ConnectionJumpToProcess)),
            ..TlValue::new(text)
        }
    };
    lines.push(line("connections-props-label-process", process));

    let user = owner_of(record, processes)
        .map(|p| p.meta.user.to_string())
        .or_else(|| {
            processes
                .records
                .iter()
                .find(|p| p.uid == record.uid)
                .map(|p| p.meta.user.to_string())
        })
        .unwrap_or_else(|| record.uid.to_string());
    lines.push(line("connections-props-label-user", TlValue::new(user)));
    lines.push(line(
        "connections-props-label-inode",
        TlValue::new(record.inode.to_string()),
    ));
    let interface = record
        .interface
        .as_ref()
        .map_or_else(|| tr(cx, "connections-props-none"), SharedString::from);
    lines.push(line(
        "connections-props-label-interface",
        TlValue::new(interface),
    ));
    lines.push(line(
        "connections-props-label-queues",
        TlValue::new(format!(
            "{} / {}",
            counted(u64::from(record.recv_q)),
            counted(u64::from(record.send_q))
        )),
    ));

    if record.proto == Proto::Tcp {
        // A listener has no connection to measure: the kernel reports zeros.
        let tcp = record.tcp.filter(|_| record.state != ConnState::Listening);
        let value = |f: &dyn Fn(&tl_model::snapshot::TcpInfoView) -> String| {
            tcp.as_ref()
                .map_or_else(not_reported, |info| TlValue::new(f(info)))
        };
        lines.push(line(
            "connections-props-label-rtt",
            value(&|t| {
                let mut rtt = String::new();
                units::response_ms(&mut rtt, f64::from(t.rtt_us) / 1000.);
                let mut var = String::new();
                units::response_ms(&mut var, f64::from(t.rttvar_us) / 1000.);
                tr_args(
                    cx,
                    "connections-props-rtt",
                    &[
                        ("rtt", FluentValue::from(rtt)),
                        ("var", FluentValue::from(var)),
                    ],
                )
                .to_string()
            }),
        ));
        lines.push(line(
            "connections-props-label-cwnd",
            value(&|t| {
                tr_args(
                    cx,
                    "connections-props-segments",
                    &[("n", FluentValue::from(counted(u64::from(t.snd_cwnd))))],
                )
                .to_string()
            }),
        ));
        lines.push(line(
            "connections-props-label-bytes",
            value(&|t| {
                let mut out = String::new();
                units::bytes_of(&mut out, t.bytes_acked, t.bytes_received);
                out
            }),
        ));
        lines.push(line(
            "connections-props-label-retransmits",
            value(&|t| counted(u64::from(t.retransmits))),
        ));
        lines.push(line(
            "connections-props-label-last-data",
            value(&|t| {
                tr_args(
                    cx,
                    "connections-props-ms-ago",
                    &[(
                        "ms",
                        FluentValue::from(counted(u64::from(t.last_data_recv_ms))),
                    )],
                )
                .to_string()
            }),
        ));
    }
    lines.push(line(
        "connections-props-label-closable",
        TlValue::new(tr(cx, closable_key(record, helper, destroy_supported))),
    ));
    if closed {
        for line in &mut lines {
            if line.value.validity == Validity::Valid {
                line.value.validity = Validity::Stale;
            }
        }
    }
    lines
}

/// What changes the look of the window: label, text and validity of each line.
fn signature(lines: &[PropLine]) -> Vec<(SharedString, SharedString, Validity)> {
    lines
        .iter()
        .map(|l| (l.label.clone(), l.value.text.clone(), l.value.validity))
        .collect()
}

fn copy_value(value: &TlValue, window: &mut Window, cx: &mut App) {
    let words = kv_words(cx);
    let Some(text) = shown_text_in(value, &words) else {
        return;
    };
    if clipboard_write(cx, text.to_string()).is_ok() {
        let toast = tr(cx, "connections-props-copied");
        if let Some(queue) = ToastQueue::of_window(window, cx) {
            queue.update(cx, |q, cx| {
                q.push(ToastKind::Success, toast, None, None, cx)
            });
        }
    }
}

pub struct ConnectionPropertiesView {
    cookie: u64,
    /// The last record seen. Kept after the socket closes.
    record: ConnRecord,
    closed: bool,
    /// The remote host the socket is on, for the title; `None` on the local host.
    remote: Option<Arc<str>>,
    lines: Vec<PropLine>,
    sig: Vec<(SharedString, SharedString, Validity)>,
    copy_focus: Vec<FocusHandle>,
    link_focus: FocusHandle,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

/// Opens or focuses the window for `cookie`. A full cap shows a Warning toast in the main
/// window. The window shows `host` and stays on it.
pub fn open_connection_properties(host: &HostContext, cookie: u64, cx: &mut App) {
    let Some(registry) = cx.try_global::<GlobalRegistry>().map(|r| r.0.clone()) else {
        warn!(target: "tasklens::ui", "no window registry, connection properties not opened");
        return;
    };
    let Some(record) = host.hub().and_then(|hub| {
        hub.read(cx)
            .current()
            .connections
            .records
            .iter()
            .find(|r| r.cookie == cookie)
            .cloned()
    }) else {
        return;
    };
    let s = metrics(cx).ui_scale;
    let key = WindowKey::ConnectionProperties(cookie).on_host(host.remote_name());
    let spec = TlWindowSpec::new(
        TlWindowKind::Properties,
        host.titled(&title_for(&record, false, cx)),
        size(px(WINDOW_SIZE.0 * s), px(WINDOW_SIZE.1 * s)),
    )
    .min_size(size(px(MIN_SIZE.0 * s), px(MIN_SIZE.1 * s)))
    .key(key.clone());
    let host = host.clone();
    let outcome = registry.update(cx, |registry, cx| {
        registry.open_or_focus(key, cx, move |cx| {
            let handle = TlSecondaryWindow::open(cx, spec, move |window, cx| {
                cx.new(|cx| ConnectionPropertiesView::new(&host, record, window, cx))
            })?;
            Ok(handle.into())
        })
    });
    match outcome {
        OpenOutcome::Opened | OpenOutcome::Focused => {}
        OpenOutcome::CapReached(_) => {
            let text = tr(cx, "connections-props-too-many-windows");
            push_main_toast(cx, ToastKind::Warning, text, None, None);
        }
        OpenOutcome::Failed => {
            warn!(target: "tasklens::ui", "connection properties window did not open");
        }
    }
}

impl ConnectionPropertiesView {
    fn new(
        host: &HostContext,
        record: ConnRecord,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subs = Vec::new();
        let mut processes = Arc::new(ProcessSnapshot::default());
        let mut helper = HelperState::Missing;
        let mut destroy_supported = None;
        if let Some(hub) = host.hub().cloned() {
            let current = hub.read(cx).current();
            processes = Arc::clone(&current.processes);
            helper = current.health.helper;
            destroy_supported = current.connections.destroy_supported;
            subs.push(cx.subscribe_in(
                &hub,
                window,
                |this, hub, event: &SnapshotChanged, window, cx| {
                    if event.mask.intersects(
                        ComponentMask::CONNECTIONS
                            | ComponentMask::PROCESSES
                            | ComponentMask::HEALTH,
                    ) {
                        let snapshot = Arc::clone(hub.read(cx).current());
                        this.take_snapshot(&snapshot, window, cx);
                    }
                },
            ));
        }
        if let Some(demand) = host.demand().cloned() {
            let id = cx.entity_id();
            let part = Demand {
                any_surface_visible: true,
                connections: true,
                ..Demand::default()
            };
            demand.update(cx, |d, cx| {
                d.set(id, Some(window.window_handle()), part, cx)
            });
            cx.on_release(move |_, cx| demand.update(cx, |d, cx| d.remove(id, cx)))
                .detach();
        }
        let lines = build_lines(&record, &processes, helper, destroy_supported, false, cx);
        let sig = signature(&lines);
        let copy_focus = (0..MAX_LINES).map(|_| cx.focus_handle()).collect();
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self {
            remote: host.remote_name().cloned(),
            cookie: record.cookie,
            record,
            closed: false,
            lines,
            sig,
            copy_focus,
            link_focus: cx.focus_handle(),
            focus,
            _subs: subs,
        }
    }

    fn take_snapshot(&mut self, snapshot: &Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        if self.closed {
            return;
        }
        let processes = &snapshot.processes;
        let helper = snapshot.health.helper;
        let destroy_supported = snapshot.connections.destroy_supported;
        match snapshot
            .connections
            .records
            .iter()
            .find(|r| r.cookie == self.cookie)
        {
            Some(record) => {
                let lines = build_lines(record, processes, helper, destroy_supported, false, cx);
                let sig = signature(&lines);
                self.record = record.clone();
                if sig != self.sig {
                    self.lines = lines;
                    self.sig = sig;
                    self.set_title(window, cx);
                    cx.notify();
                }
            }
            None => {
                self.closed = true;
                self.lines =
                    build_lines(&self.record, processes, helper, destroy_supported, true, cx);
                self.sig = signature(&self.lines);
                self.set_title(window, cx);
                cx.notify();
            }
        }
    }

    fn set_title(&self, window: &mut Window, cx: &mut Context<Self>) {
        let title: SharedString = with_host_name(
            &title_for(&self.record, self.closed, cx),
            self.remote.as_deref(),
        )
        .into();
        let frame = window.root::<Root>().flatten().and_then(|root| {
            root.read(cx)
                .view()
                .clone()
                .downcast::<TlSecondaryWindow<Self>>()
                .ok()
        });
        match frame {
            Some(frame) => frame.update(cx, |f, cx| f.set_title(title, window, cx)),
            None => window.set_window_title(&title),
        }
    }

    /// The flat row of the Process link, when the Process row is a link.
    fn link_row(&self) -> Option<usize> {
        self.lines
            .iter()
            .position(|l| l.value.link.is_some() && l.value.validity == Validity::Valid)
    }

    /// Copies line `row` and shows the toast in this window.
    fn copy_line(&self, row: usize, window: &mut Window, cx: &mut App) {
        if let Some(line) = self.lines.get(row).cloned() {
            copy_value(&line.value, window, cx);
        }
    }

    /// Ctrl+C copies the line whose copy button or link has focus.
    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = event.keystroke.modifiers;
        if event.keystroke.key != "c" || !m.control || m.shift || m.alt || m.platform {
            return;
        }
        let row = if self.link_focus.is_focused(window) {
            self.link_row()
        } else {
            (0..self.lines.len()).find(|&i| self.copy_focus[i].is_focused(window))
        };
        if let Some(row) = row {
            self.copy_line(row, window, cx);
            cx.stop_propagation();
        }
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn cookie(&self) -> u64 {
        self.cookie
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// The lines as "label: value", the way the golden file stores them.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn line_texts(&self, words: &crate::components::kv_grid::KvWords) -> Vec<String> {
        self.lines
            .iter()
            .filter_map(|l| {
                shown_text_in(&l.value, words).map(|text| format!("{}: {text}", l.label))
            })
            .collect()
    }

    /// How each value draws, for the closed-colour check.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn value_looks(
        &self,
        words: &crate::components::kv_grid::KvWords,
        theme: &crate::theme::TlTheme,
    ) -> Vec<crate::components::kv_grid::ValueLook> {
        self.lines
            .iter()
            .filter_map(|l| crate::components::kv_grid::value_look_in(&l.value, words, theme))
            .collect()
    }

    /// The focus handle of the Process link, for keyboard tests.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn link_focus_handle(&self) -> &FocusHandle {
        &self.link_focus
    }
}

impl Render for ConnectionPropertiesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx).clone();
        let m = metrics(cx);
        let s = m.ui_scale;
        let row_h = px(ROW_H * s);
        let section = TlKvSection {
            title: None,
            blocks: vec![TlKvBlock {
                heading: None,
                rows: self
                    .lines
                    .iter()
                    .map(|l| TlKvRow {
                        label: l.label.clone(),
                        value: l.value.clone(),
                    })
                    .collect(),
                indent: Some(false),
            }],
        };
        let this = cx.weak_entity();
        let mut grid = TlKeyValueGrid::new("ConnPropsGrid", KvLayout::Inline)
            .sections(&[section])
            .row_height(row_h)
            .label_width(px(LABEL_W * s))
            .on_link_click(move |_, _, _, cx| {
                let Some(record) = this.upgrade().map(|v| v.read(cx).record.clone()) else {
                    return;
                };
                cx.defer(move |cx| jump_to_owner(&record, cx));
            });
        if let Some(row) = self.link_row() {
            grid = grid.link_focus(Rc::from([(row, self.link_focus.clone())]));
        }

        let mut buttons = div().flex_none().flex().flex_col();
        for (i, line) in self.lines.iter().enumerate() {
            let handle = self.copy_focus[i].clone();
            let label = tr_args(
                cx,
                "connections-props-copy",
                &[("label", FluentValue::from(line.label.as_ref()))],
            );
            let this = cx.weak_entity();
            let lit = handle.contains_focused(window, cx);
            let button = TlButton::new(ElementId::from(("ConnPropsCopy", i)), TlButtonKind::Icon)
                .icon(TlIcon::Copy)
                .icon_size(px(COPY_ICON * s))
                .icon_muted(true)
                .size(TlButtonSize::Sm)
                .tooltip(label.clone())
                .accessible_name(label)
                .focus_handle(handle)
                .on_click(move |_, window, cx| {
                    let line = this
                        .upgrade()
                        .and_then(|view| view.read(cx).lines.get(i).cloned());
                    if let Some(line) = line {
                        copy_value(&line.value, window, cx);
                    }
                });
            buttons = buttons.child(
                div()
                    .id(ElementId::from(("ConnPropsCopyCell", i)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(row_h)
                    .w(px(COPY_CELL * s))
                    .opacity(if lit { 1.0 } else { 0.6 })
                    .hover(|style| style.opacity(1.0))
                    .child(button),
            );
        }

        let footer = div()
            .debug_selector(|| "ConnPropsFooter".into())
            .flex_none()
            .flex()
            .items_center()
            .justify_end()
            .px(m.sp[5])
            .h(px(FOOTER_H * s))
            .bg(theme.toolbar)
            .border_t_1()
            .border_color(theme.divider)
            .child(
                TlButton::new("ConnPropsClose", TlButtonKind::Secondary)
                    .label(tr(cx, "connections-props-close"))
                    .action(Box::new(CloseSecondaryWindow)),
            );

        div()
            .id("ConnectionPropertiesWindow")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.window_bg)
            .child(
                div()
                    .id("ConnPropsScroll")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_y_scroll()
                    .p(m.sp[6])
                    .child(
                        h_flex()
                            .items_start()
                            .gap(m.sp[3])
                            .child(div().flex_1().min_w_0().child(grid))
                            .child(buttons),
                    ),
            )
            .child(footer)
    }
}
