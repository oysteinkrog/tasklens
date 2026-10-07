//! "tasklens About": icon, name, version line, "Copy version info", "Third-party licenses" and
//! links to the repository and the log folder (UI-SHL-092, UI-SHL-090, APP-053, R-078, D-005,
//! UI-FND-021).
//!
//! A Tool window keyed `WindowKey::About`, 440 × 380 × ui_scale, not resizable; `OpenAbout`
//! focuses it when open. The build facts come from `AppEnv.build` (R-171). "Third-party
//! licenses" swaps the body for a scrollable list built once from the embedded `cargo-about`
//! JSON, with "Back" at the top. The links send `ActionKind::OpenUri` through the app's
//! `ActionTracker`. Esc and Ctrl+W close the window through the frame.
//!
//! The copied version block holds build facts and desktop facts only: no user names, paths or
//! process data.

use std::path::Path;
use std::rc::Rc;

use fluent_bundle::FluentValue;
use gpui_kit::base::Root;
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, FocusHandle, InteractiveElement as _,
    IntoElement, ParentElement, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled, Subscription, Window, WindowHandle, div, img, px,
    size,
};
use rustc_hash::FxHashSet;
use smallvec::SmallVec;
use tl_model::actions::{ActionId, ActionKind, ActionRequest};
use tl_model::validity::Validity;
use tracing::warn;

use super::registry::{GlobalRegistry, WindowKey};
use crate::action_tracker::{GlobalActionTracker, ToastTexts};
use crate::app::{AppEnv, BuildInfo};
use crate::components::button::{TlButton, TlButtonKind};
use crate::components::icons::AppIcon;
use crate::components::link::TlLink;
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::window_frame::{TlSecondaryWindow, TlWindowKind, TlWindowSpec};
use crate::hub::GlobalHub;
use crate::i18n::{Strings, tr, tr_args};
use crate::settings_global::TlSettings;
use crate::text::style::{numeric_style, t_sm, t_xl};
use crate::theme::{current, metrics};

gpui_kit::actions!(
    tasklens,
    [
        /// Help > About tasklens…: opens the About window, or focuses the open one.
        OpenAbout,
        /// Copies the version block and shows "Copied to clipboard".
        CopyVersionInfo,
        /// Swaps the body for the licence list.
        ShowLicenses,
        /// Returns from the licence list to the main view.
        AboutBack,
        /// Opens the repository URL.
        OpenRepository,
        /// Opens the log folder.
        OpenLogFolder
    ]
);

/// Window size, × ui_scale. Not resizable, so it is also the minimum size.
const SIZE: (f32, f32) = (440., 380.);
/// App icon edge, × ui_scale.
const ICON: f32 = 64.;
/// Licence row height, × ui_scale.
const ROW: f32 = 24.;

/// The repository URL from the workspace metadata.
pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

const TITLE: &str = "shell-about-title";
const VERSION: &str = "shell-about-version";
const COPY_VERSION: &str = "shell-about-copy-version";
const LICENSES: &str = "shell-about-licenses";
const REPOSITORY_LINK: &str = "shell-about-repository";
const OPEN_LOGS: &str = "shell-about-open-logs";
const BACK: &str = "shell-about-back";
const UNKNOWN: &str = "shell-about-unknown";
const COPIED: &str = "shell-about-copied";
const LINK_FAILED: &str = "shell-about-link-failed";
const LOGS_FAILED: &str = "shell-about-logs-failed";

/// The labels of the copied block, in order.
const FIELDS: [&str; 9] = [
    "shell-about-field-version",
    "shell-about-field-git-hash",
    "shell-about-field-build-date",
    "shell-about-field-gpui-rev",
    "shell-about-field-kernel",
    "shell-about-field-nvidia",
    "shell-about-field-compositor",
    "shell-about-field-session",
    "shell-about-field-scale",
];

/// Registers the `OpenAbout` handler. `TasklensUi::init` calls it.
pub fn init(cx: &mut App) {
    // Deferred: from a menu the main window is mid-update.
    cx.on_action(|_: &OpenAbout, cx| cx.defer(open_about));
}

/// Focuses the open window, or opens it. Without a window registry (tests) it always opens one.
pub fn open_about(cx: &mut App) {
    let s = metrics(cx).ui_scale;
    let window_size = size(px(SIZE.0 * s), px(SIZE.1 * s));
    let spec = TlWindowSpec::new(
        TlWindowKind::Tool { resizable: false },
        tr(cx, TITLE),
        window_size,
    )
    .min_size(window_size)
    .key(WindowKey::About);
    let open = move |cx: &mut App| {
        let handle = TlSecondaryWindow::open(cx, spec, |window, cx| {
            cx.new(|cx| AboutWindow::new(window, cx))
        })?;
        follow_title(handle, cx);
        Ok(handle.into())
    };
    match cx.try_global::<GlobalRegistry>().map(|r| r.0.clone()) {
        Some(registry) => {
            registry.update(cx, |r, cx| r.open_or_focus(WindowKey::About, cx, open));
        }
        None => {
            let _ = open(cx);
        }
    }
}

/// Sets the frame title again when the strings version changes, one `tr` call per version
/// (R-132).
fn follow_title(handle: WindowHandle<Root>, cx: &mut App) {
    let Some(frame) = TlSecondaryWindow::<AboutWindow>::find(handle, cx) else {
        return;
    };
    let _ = handle.update(cx, |_, window, cx| {
        frame.update(cx, |_, cx| {
            let mut version = strings_version(cx);
            cx.observe_global_in::<Strings>(window, move |frame, window, cx| {
                let now = strings_version(cx);
                if now != version {
                    version = now;
                    frame.set_title(tr(cx, TITLE), window, cx);
                }
            })
            .detach();
        });
    });
}

fn strings_version(cx: &App) -> u64 {
    cx.try_global::<Strings>().map_or(0, |s| s.strings_version)
}

fn build_info(cx: &App) -> BuildInfo {
    cx.try_global::<AppEnv>()
        .map(|e| e.build)
        .unwrap_or_default()
}

/// One row of the licence list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LicenseRow {
    pub name: SharedString,
    pub version: SharedString,
    pub license: SharedString,
}

/// One row per crate from `cargo-about` JSON output, sorted by name then version. A crate's own
/// `license` expression wins over the licence it is listed under. Text that does not parse
/// gives no rows.
pub fn parse_licenses(json: &str) -> Vec<LicenseRow> {
    let Ok(root) = serde_json::from_str::<serde_json::Value>(json) else {
        if !json.is_empty() {
            warn!(target: "tasklens::ui", "the embedded licence list does not parse");
        }
        return Vec::new();
    };
    let mut seen: FxHashSet<(String, String)> = FxHashSet::default();
    let mut rows = Vec::new();
    let licenses = root.get("licenses").and_then(|l| l.as_array());
    for license in licenses.into_iter().flatten() {
        let id = license.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let used_by = license.get("used_by").and_then(|u| u.as_array());
        for user in used_by.into_iter().flatten() {
            let Some(krate) = user.get("crate") else {
                continue;
            };
            let field = |k: &str| krate.get(k).and_then(|v| v.as_str());
            let (Some(name), Some(version)) = (field("name"), field("version")) else {
                continue;
            };
            if !seen.insert((name.to_owned(), version.to_owned())) {
                continue;
            }
            rows.push(LicenseRow {
                name: SharedString::from(name.to_owned()),
                version: SharedString::from(version.to_owned()),
                license: SharedString::from(field("license").unwrap_or(id).to_owned()),
            });
        }
    }
    rows.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.version.cmp(&b.version)));
    rows
}

/// `file://` and `path`, with every byte outside the URI unreserved set and `/` percent-encoded.
pub fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for &b in path.as_os_str().as_encoded_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

/// The window's own text, translated once per strings version (R-132).
struct AboutWords {
    version_line: SharedString,
    copy_version: SharedString,
    licenses: SharedString,
    repository: SharedString,
    open_logs: SharedString,
    back: SharedString,
    unknown: SharedString,
    copied: SharedString,
    link_failed: SharedString,
    logs_failed: SharedString,
    fields: [SharedString; 9],
}

impl AboutWords {
    fn new(cx: &App) -> Self {
        let build = build_info(cx);
        Self {
            version_line: tr_args(
                cx,
                VERSION,
                &[
                    ("version", FluentValue::from(build.version)),
                    ("hash", FluentValue::from(build.git_hash)),
                    ("date", FluentValue::from(build.build_date)),
                ],
            ),
            copy_version: tr(cx, COPY_VERSION),
            licenses: tr(cx, LICENSES),
            repository: tr(cx, REPOSITORY_LINK),
            open_logs: tr(cx, OPEN_LOGS),
            back: tr(cx, BACK),
            unknown: tr(cx, UNKNOWN),
            copied: tr(cx, COPIED),
            link_failed: tr(cx, LINK_FAILED),
            logs_failed: tr(cx, LOGS_FAILED),
            fields: FIELDS.map(|k| tr(cx, k)),
        }
    }
}

/// Which body the window shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AboutView {
    Main,
    Licenses,
}

/// The content of the About window.
pub struct AboutWindow {
    view: AboutView,
    /// Built on the first switch to the licence list.
    licenses: Option<Rc<[LicenseRow]>>,
    words: AboutWords,
    strings_version: u64,
    copy_focus: FocusHandle,
    licenses_focus: FocusHandle,
    back_focus: FocusHandle,
    scroll: ScrollHandle,
    _strings: Subscription,
}

impl AboutWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let copy_focus = cx.focus_handle();
        copy_focus.focus(window, cx);
        let _strings = cx.observe_global::<Strings>(|this: &mut Self, cx| {
            let version = strings_version(cx);
            if version != this.strings_version {
                this.strings_version = version;
                this.words = AboutWords::new(cx);
                cx.notify();
            }
        });
        Self {
            view: AboutView::Main,
            licenses: None,
            words: AboutWords::new(cx),
            strings_version: strings_version(cx),
            copy_focus,
            licenses_focus: cx.focus_handle(),
            back_focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            _strings,
        }
    }

    /// The body shown.
    pub fn view(&self) -> AboutView {
        self.view
    }

    /// "Version: 0.1.0 (a1b2c3d, 2026-10-03)".
    pub fn version_line(&self) -> &SharedString {
        &self.words.version_line
    }

    /// The licence rows, once the list has been shown.
    pub fn license_rows(&self) -> Option<&[LicenseRow]> {
        self.licenses.as_deref()
    }

    /// Every word the window draws in its current body, for the branding check.
    pub fn drawn_text(&self) -> Vec<SharedString> {
        let w = &self.words;
        match self.view {
            AboutView::Main => vec![
                "tasklens".into(),
                w.version_line.clone(),
                w.copy_version.clone(),
                w.licenses.clone(),
                w.repository.clone(),
                w.open_logs.clone(),
            ],
            AboutView::Licenses => {
                let mut out = vec![w.back.clone()];
                for row in self.licenses.iter().flat_map(|r| r.iter()) {
                    out.extend([row.name.clone(), row.version.clone(), row.license.clone()]);
                }
                out
            }
        }
    }

    /// The copied block: one `Label: value` line per field, "Unknown" for a missing value.
    pub fn version_info(&self, window: &Window, cx: &App) -> String {
        let build = build_info(cx);
        let unknown = self.words.unknown.as_ref();
        let or_unknown = |v: Option<String>| v.filter(|v| !v.is_empty());
        let scale_pct = cx
            .try_global::<TlSettings>()
            .map_or(100, |_| TlSettings::settings(cx).appearance.ui_scale_pct);
        let scale = (window.scale_factor() * f32::from(scale_pct)).round();
        let values = [
            or_unknown(Some(build.version.to_owned())),
            or_unknown(Some(build.git_hash.to_owned())),
            or_unknown(Some(build.build_date.to_owned())),
            or_unknown(Some(build.gpui_rev.to_owned())),
            or_unknown(kernel_release()),
            or_unknown(nvidia_driver(cx)),
            or_unknown(std::env::var("XDG_CURRENT_DESKTOP").ok()),
            or_unknown(std::env::var("XDG_SESSION_TYPE").ok()),
            Some(format!("{scale}%")),
        ];
        let mut out = String::new();
        for (label, value) in self.words.fields.iter().zip(values) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(label);
            out.push_str(": ");
            out.push_str(value.as_deref().unwrap_or(unknown));
        }
        out
    }

    fn on_copy(&mut self, _: &CopyVersionInfo, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.version_info(window, cx);
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        let copied = self.words.copied.clone();
        if let Some(queue) = ToastQueue::of_window(window, cx) {
            queue.update(cx, |q, cx| {
                q.push(ToastKind::Success, copied, None, None, cx)
            });
        }
    }

    fn on_licenses(&mut self, _: &ShowLicenses, window: &mut Window, cx: &mut Context<Self>) {
        if self.licenses.is_none() {
            self.licenses = Some(parse_licenses(build_info(cx).licenses_json).into());
        }
        self.view = AboutView::Licenses;
        self.back_focus.focus(window, cx);
        cx.notify();
    }

    fn on_back(&mut self, _: &AboutBack, window: &mut Window, cx: &mut Context<Self>) {
        self.view = AboutView::Main;
        self.licenses_focus.focus(window, cx);
        cx.notify();
    }

    fn on_repository(&mut self, _: &OpenRepository, window: &mut Window, cx: &mut Context<Self>) {
        let texts = ToastTexts::new("", self.words.link_failed.clone());
        send_open_uri(REPOSITORY.to_owned(), texts, window, cx);
    }

    fn on_log_folder(&mut self, _: &OpenLogFolder, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = cx.try_global::<AppEnv>().map(|e| e.logs_dir.clone()) else {
            warn!(target: "tasklens::ui", "no AppEnv; the log folder was not opened");
            return;
        };
        let texts = ToastTexts::new("", self.words.logs_failed.clone());
        send_open_uri(file_uri(&dir), texts, window, cx);
    }

    fn render_main(&self, cx: &App) -> gpui_kit::Div {
        let theme = current(cx);
        let m = *metrics(cx);
        let w = &self.words;
        let mut name = div().text_color(theme.text_primary).child("tasklens");
        *name.text_style() = t_xl(&m);
        let mut version = div()
            .text_color(theme.text_secondary)
            .child(w.version_line.clone());
        *version.text_style() = t_sm(&m);
        let buttons = div()
            .flex()
            .flex_row()
            .gap(m.sp[4])
            .child(
                TlButton::new("About-copy-version", TlButtonKind::Secondary)
                    .label(w.copy_version.clone())
                    .focus_handle(self.copy_focus.clone())
                    .action(Box::new(CopyVersionInfo)),
            )
            .child(
                TlButton::new("About-licenses", TlButtonKind::Secondary)
                    .label(w.licenses.clone())
                    .focus_handle(self.licenses_focus.clone())
                    .action(Box::new(ShowLicenses)),
            );
        let links = div()
            .flex()
            .flex_row()
            .gap(m.sp[6])
            .child(TlLink::new(
                "About-repository",
                w.repository.clone(),
                Box::new(OpenRepository),
            ))
            .child(TlLink::new(
                "About-open-logs",
                w.open_logs.clone(),
                Box::new(OpenLogFolder),
            ));
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .p(m.sp[7])
            .gap(m.sp[4])
            .child(img(AppIcon::Logo.path_str()).size(px(ICON * m.ui_scale)))
            .child(name)
            .child(version)
            .child(buttons)
            .child(links)
    }

    fn render_licenses(&self, cx: &App) -> gpui_kit::Div {
        let theme = current(cx);
        let m = *metrics(cx);
        let row_h = px(ROW * m.ui_scale);
        let mut list = div()
            .id("About-licenses-list")
            .track_scroll(&self.scroll)
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        for row in self.licenses.iter().flat_map(|r| r.iter()) {
            let mut version = div().flex_none().child(row.version.clone());
            *version.text_style() = numeric_style(&m, m.text.sm);
            let mut r = div()
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .gap(m.sp[4])
                .h(row_h)
                .px(m.sp[4])
                .text_color(theme.text_primary)
                .child(div().min_w_0().flex_1().truncate().child(row.name.clone()))
                .child(version)
                .child(
                    div()
                        .flex_none()
                        .text_color(theme.text_secondary)
                        .child(row.license.clone()),
                );
            *r.text_style() = t_sm(&m);
            list = list.child(r);
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(m.sp[4])
            .pt(m.sp[5])
            .child(
                div().px(m.sp[5]).child(
                    TlButton::new("About-back", TlButtonKind::Secondary)
                        .label(self.words.back.clone())
                        .focus_handle(self.back_focus.clone())
                        .action(Box::new(AboutBack)),
                ),
            )
            .child(list)
    }
}

/// The running kernel's release, as `uname -r` prints it.
fn kernel_release() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .map(|s| s.trim().to_owned())
}

/// The driver version of the first NVIDIA adapter in the snapshot's graphics rows.
fn nvidia_driver(cx: &App) -> Option<String> {
    let hub = cx.try_global::<GlobalHub>()?.0.read(cx);
    let inventory = &hub.current().inventory;
    inventory
        .graphics
        .iter()
        .filter(|g| {
            g.kernel_module.value.as_ref() == "nvidia"
                || g.name.value.to_ascii_lowercase().contains("nvidia")
        })
        .find(|g| g.driver_version.validity == Validity::Valid)
        .map(|g| g.driver_version.value.to_string())
}

/// Sends `OpenUri(uri)` through the app's `ActionTracker`. Only a failure shows a toast, in this
/// window.
fn send_open_uri(uri: String, texts: ToastTexts, window: &mut Window, cx: &mut App) {
    let Some(tracker) = cx.try_global::<GlobalActionTracker>().map(|t| t.0.clone()) else {
        warn!(target: "tasklens::ui", "no action tracker; the link was not opened");
        return;
    };
    let Some(queue) = ToastQueue::of_window(window, cx) else {
        warn!(target: "tasklens::ui", "no toast queue; the link was not opened");
        return;
    };
    let request = ActionRequest {
        id: ActionId(0),
        kind: ActionKind::OpenUri(uri.into()),
        targets: SmallVec::new(),
        privileged: false,
    };
    tracker.update(cx, |t, cx| {
        t.track_or_toast(request, queue.downgrade(), texts, cx);
    });
}

impl Render for AboutWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx).clone();
        let body = match self.view {
            AboutView::Main => self.render_main(cx),
            AboutView::Licenses => self.render_licenses(cx),
        };
        div()
            .id("About")
            .on_action(cx.listener(Self::on_copy))
            .on_action(cx.listener(Self::on_licenses))
            .on_action(cx.listener(Self::on_back))
            .on_action(cx.listener(Self::on_repository))
            .on_action(cx.listener(Self::on_log_folder))
            .size_full()
            .bg(theme.list_bg)
            .child(body)
    }
}
