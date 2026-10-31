//! One Information section: the title band and its rows (UI-SYS-081, UI-SYS-082, UI-FND-025,
//! PERF-013, PERF-094, PERF-095, R-050).
//!
//! The band is `row_h` high with the `header` fill, a 3 px `accent` bar at its left edge, the
//! title, the distro logo (Operating System only) and a copy button at the right end that
//! dispatches [`CopyInformationSection`]. The button's tooltip is "Copy section" and its
//! accessible name "Copy <section title>"; its muted icon turns `text_primary` on hover. A row's
//! `label_tooltip` shows on its label, and a row with `label_lines` 2 wraps its label inside the
//! label column and is two rows high (Peak committed, UI-SYS-089). The rows draw in a
//! `TlKeyValueGrid` inline layout; until the page's first data an 8-line skeleton block stands in
//! for them (UI-SYS-085). A value with several lines is several rows high. The row menu's "Copy
//! section" dispatches the same action; "Copy value" and "Copy row" copy and show "Copied to
//! clipboard". A value click does not copy, so text selection works.
//!
//! The page builds the rows and passes them in as one `Arc`. A new `Arc` with the same rows does
//! not re-render the section. Render builds no strings: the grid rows are made in `set_rows`
//! and the title is translated once per strings version.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::accesskit::Role;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::gpui::private::schemars::JsonSchema;
use gpui_kit::{
    App, Context, ElementId, EntityId, InteractiveElement, IntoElement, ParentElement, Pixels,
    Render, SharedString, StatefulInteractiveElement as _, Styled, Window, canvas, div, gpui, img,
    px,
};
use serde::Deserialize;
use tl_model::client::EngineClient;
use tl_model::command::{Command, CommandError, DeviceCommand};

use super::actions::{CopyInformationSection, FetchInformationPublicIp};
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::icons::TlIcon;
use crate::components::kv_grid::{
    KvLayout, KvWords, TlKeyValueGrid, TlKvBlock, TlKvRow, TlKvSection, TlValue, shown_text_in,
    value_copy_form,
};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::host::HostContext;
use crate::i18n::{Strings, tr, tr_args, tr_en};
use crate::text::style::t_md;
use crate::theme::{current, metrics};
use crate::window::public_ip::request_public_ip_fetch;

/// Width of the accent bar at the band's left edge, × ui_scale.
const BAR_WIDTH: f32 = 3.;
/// Distro logo size, × ui_scale.
const LOGO_SIZE: f32 = 16.;
/// From the label start to the value start, × ui_scale (measured, UI-SYS-081).
const LABEL_WIDTH: f32 = 232.;
const KEY_COPY_SECTION: &str = "information-copy-section";
const KEY_COPY_NAME: &str = "information-copy-section-name";
const KEY_COPIED: &str = "information-toast-copied";
const KEY_FETCH: &str = "information-public-ip-fetch";
/// Fetch button width, × ui_scale (the Network page's).
const FETCH_BUTTON_W: f32 = 64.;

/// The ten sections.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Deserialize, JsonSchema)]
#[schemars(crate = "gpui::private::schemars")]
pub enum InfoSectionId {
    OperatingSystem,
    Processor,
    System,
    Memory,
    MemoryModules,
    DiskDrives,
    BiosFirmware,
    Motherboard,
    Graphics,
    NetworkAdapters,
}

impl InfoSectionId {
    /// Single-column order (PERF-094).
    pub const ALL: [InfoSectionId; 10] = [
        Self::OperatingSystem,
        Self::Processor,
        Self::System,
        Self::Memory,
        Self::MemoryModules,
        Self::DiskDrives,
        Self::BiosFirmware,
        Self::Motherboard,
        Self::Graphics,
        Self::NetworkAdapters,
    ];

    /// The message key of the title.
    pub fn key(self) -> &'static str {
        match self {
            Self::OperatingSystem => "information-section-operating-system",
            Self::Processor => "information-section-processor",
            Self::System => "information-section-system",
            Self::Memory => "information-section-memory",
            Self::MemoryModules => "information-section-memory-modules",
            Self::DiskDrives => "information-section-disk-drives",
            Self::BiosFirmware => "information-section-bios-firmware",
            Self::Motherboard => "information-section-motherboard",
            Self::Graphics => "information-section-graphics",
            Self::NetworkAdapters => "information-section-network-adapters",
        }
    }

    /// The en-US title, translated once per thread. The band shows [`InfoSectionId::key`]
    /// through `tr`.
    pub fn title(self) -> SharedString {
        thread_local! {
            static TITLES: [SharedString; 10] = InfoSectionId::ALL.map(|id| tr_en(id.key(), &[]));
        }
        TITLES.with(|titles| titles[self as usize].clone())
    }
}

/// One row of a section.
#[derive(Clone)]
pub struct InfoRow {
    /// Without the ":"; the section appends it.
    pub label: SharedString,
    /// Several lines (Resolution, IP address) are separated by `\n`.
    pub value: TlValue,
    /// Changes at 1 Hz.
    pub live: bool,
    /// 0 for unindented rows, n >= 1 for the rows of sub-block n.
    pub block: u8,
    /// The System section's Handle count label (UI-SYS-088).
    pub label_tooltip: Option<SharedString>,
    /// Lines the label wraps to; 1 cuts it with "…". Only Peak committed has 2 (UI-SYS-089).
    pub label_lines: u8,
    /// `Some(loading)` marks the Public IP row, which draws the Fetch button after its value.
    pub fetch: Option<bool>,
}

impl PartialEq for InfoRow {
    fn eq(&self, other: &Self) -> bool {
        let (a, b) = (&self.value, &other.value);
        let link = match (&a.link, &b.link) {
            (Some(x), Some(y)) => x.partial_eq(y.as_ref()),
            (None, None) => true,
            _ => false,
        };
        self.label == other.label
            && self.live == other.live
            && self.block == other.block
            && self.label_tooltip == other.label_tooltip
            && self.label_lines == other.label_lines
            && self.fetch == other.fetch
            && a.text == b.text
            && a.validity == b.validity
            && a.word == b.word
            && a.tooltip == b.tooltip
            && a.path == b.path
            && a.tone == b.tone
            && link
    }
}

/// Appends the copy form of the row's value: the text, or the PERF-013 word for a value that is
/// not Valid or Stale, with several lines joined by ", ". The words are en-US.
pub fn value_copy_text(row: &InfoRow, out: &mut String) {
    thread_local! {
        static WORDS: KvWords = KvWords::default();
    }
    let Some(text) = WORDS.with(|words| shown_text_in(&row.value, words)) else {
        return;
    };
    out.push_str(&value_copy_form(&text));
}

/// The band text, translated once per strings version. A new table is in en-US.
struct SectionWords {
    /// `None` before the first refresh; then the strings version, `None` without strings.
    version: Option<Option<u64>>,
    title: SharedString,
    copy_section: SharedString,
    /// The copy button's accessible name, "Copy <title>".
    copy_name: SharedString,
    /// The toast after a row menu copy.
    copied: SharedString,
    /// The Public IP row's button label.
    fetch: SharedString,
}

impl SectionWords {
    fn new(id: InfoSectionId) -> Self {
        let title = id.title();
        Self {
            version: None,
            copy_name: tr_en(KEY_COPY_NAME, &[("section", title.to_string().into())]),
            title,
            copy_section: tr_en(KEY_COPY_SECTION, &[]),
            copied: tr_en(KEY_COPIED, &[]),
            fetch: tr_en(KEY_FETCH, &[]),
        }
    }

    fn refresh(&mut self, id: InfoSectionId, cx: &App) {
        let version = Some(cx.try_global::<Strings>().map(|s| s.strings_version));
        if self.version == version {
            return;
        }
        self.version = version;
        if version == Some(None) {
            return;
        }
        self.title = tr(cx, id.key());
        self.copy_name = tr_args(
            cx,
            KEY_COPY_NAME,
            &[("section", self.title.to_string().into())],
        );
        self.copy_section = tr(cx, KEY_COPY_SECTION);
        self.copied = tr(cx, KEY_COPIED);
        self.fetch = tr(cx, KEY_FETCH);
    }
}

/// The grid input for `rows`: labels with ":", one block per run of rows with the same `block`.
/// Blocks after block 0 are indented.
fn grid_section(rows: &[InfoRow]) -> TlKvSection {
    let mut blocks: Vec<TlKvBlock> = Vec::new();
    let mut current = None;
    for row in rows {
        if current != Some(row.block) {
            current = Some(row.block);
            blocks.push(TlKvBlock {
                indent: Some(row.block > 0),
                heading: None,
                rows: Vec::new(),
            });
        }
        if let Some(block) = blocks.last_mut() {
            block.rows.push(TlKvRow {
                label: format!("{}:", row.label).into(),
                value: row.value.clone(),
            });
        }
    }
    TlKvSection {
        title: None,
        blocks,
    }
}

/// One section of the Information page.
pub struct InfoSection {
    id: InfoSectionId,
    rows: Arc<[InfoRow]>,
    logo: Option<SharedString>,
    copy_enabled: bool,
    words: SectionWords,
    /// The grid input built from `rows` in `set_rows`.
    grid: Rc<[TlKvSection]>,
    /// The label tooltips by row index, built in `set_rows`.
    label_tips: Rc<[(usize, SharedString)]>,
    /// The wrapped labels by row index, built in `set_rows`.
    label_lines: Rc<[(usize, u8)]>,
    /// The index of the row with the Fetch button and whether a fetch is running, set in
    /// `set_rows`.
    fetch_row: Option<(usize, bool)>,
    /// Draws an 8-line skeleton block instead of the rows, until the first data (UI-SYS-085).
    loading: bool,
    /// The drawn height, measured each paint. A cached view needs its size from its owner, so the
    /// page reads this and is notified when it changes.
    height: Rc<Cell<Pixels>>,
    owner: Option<EntityId>,
    #[cfg(any(test, feature = "test-hooks"))]
    render_count: usize,
}

impl InfoSection {
    pub fn new(id: InfoSectionId, _cx: &mut Context<Self>) -> Self {
        Self {
            id,
            rows: Arc::from(Vec::new()),
            logo: None,
            copy_enabled: false,
            words: SectionWords::new(id),
            grid: Rc::from(Vec::new()),
            label_tips: Rc::from(Vec::new()),
            label_lines: Rc::from(Vec::new()),
            fetch_row: None,
            loading: false,
            height: Rc::new(Cell::new(Pixels::ZERO)),
            owner: None,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: 0,
        }
    }

    pub fn id(&self) -> InfoSectionId {
        self.id
    }

    pub fn rows(&self) -> &Arc<[InfoRow]> {
        &self.rows
    }

    /// Replaces the rows. Notifies only when they differ from the current ones.
    pub fn set_rows(&mut self, rows: Arc<[InfoRow]>, cx: &mut Context<Self>) {
        if Arc::ptr_eq(&self.rows, &rows) || *self.rows == *rows {
            return;
        }
        self.grid = Rc::from([grid_section(&rows)]);
        self.label_tips = rows
            .iter()
            .enumerate()
            .filter_map(|(n, row)| row.label_tooltip.clone().map(|tip| (n, tip)))
            .collect();
        self.label_lines = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.label_lines > 1)
            .map(|(n, row)| (n, row.label_lines))
            .collect();
        self.fetch_row = rows
            .iter()
            .enumerate()
            .find_map(|(n, row)| row.fetch.map(|loading| (n, loading)));
        self.rows = rows;
        cx.notify();
    }

    /// The distro logo path. Drawn only in the Operating System section.
    pub fn set_logo(&mut self, path: Option<SharedString>, cx: &mut Context<Self>) {
        if self.logo != path {
            self.logo = path;
            cx.notify();
        }
    }

    /// The copy button is disabled while the section has no data.
    pub fn set_copy_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.copy_enabled != on {
            self.copy_enabled = on;
            cx.notify();
        }
    }

    /// The skeleton body while the page has no data yet. The band stays.
    pub fn set_loading(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.loading != on {
            self.loading = on;
            cx.notify();
        }
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// The height drawn at the last paint. `owner` is notified each time it changes.
    pub fn measured_height(&mut self, owner: EntityId) -> Rc<Cell<Pixels>> {
        self.owner = Some(owner);
        Rc::clone(&self.height)
    }

    /// The copy button's accessible name, "Copy <section title>".
    pub fn copy_button_name(&self) -> &SharedString {
        &self.words.copy_name
    }

    /// Test hook: how many times the section rendered.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn render_count(&self) -> usize {
        self.render_count
    }

    fn band(&self, cx: &App) -> impl IntoElement {
        let m = metrics(cx);
        let theme = current(cx);
        let id = self.id;
        let n = id as usize;

        let mut title = div()
            .id(("info-title", n))
            .debug_selector(move || format!("InfoSection-{n}-title"))
            .role(Role::Heading)
            .ml(m.sp[4])
            .min_w_0();
        *title.text_style() = t_md(m);
        let title = title
            .truncate()
            .text_color(theme.text_primary)
            .child(self.words.title.clone());

        let logo = self
            .logo
            .clone()
            .filter(|_| id == InfoSectionId::OperatingSystem)
            .map(|path| {
                let size = px(LOGO_SIZE * m.ui_scale);
                div()
                    .debug_selector(move || format!("InfoSection-{n}-logo"))
                    .flex_none()
                    .ml(m.sp[3])
                    .size(size)
                    .child(img(path).size(size))
            });

        let copy = TlButton::new(ElementId::from(("info-copy", n)), TlButtonKind::Icon)
            .icon(TlIcon::Copy)
            .icon_size(m.icon.sm)
            .icon_muted(true)
            .icon_hover(true)
            .size(TlButtonSize::Sm)
            .tooltip(self.words.copy_section.clone())
            .accessible_name(self.words.copy_name.clone())
            .disabled(!self.copy_enabled)
            .action(Box::new(CopyInformationSection { section: id }));

        h_flex()
            .debug_selector(move || format!("InfoSection-{n}-band"))
            .w_full()
            .h(m.row_height)
            .rounded(m.radius_small)
            .overflow_hidden()
            .bg(theme.header)
            .child(
                div()
                    .debug_selector(move || format!("InfoSection-{n}-bar"))
                    .flex_none()
                    .w(px(BAR_WIDTH * m.ui_scale))
                    .h_full()
                    .bg(theme.accent),
            )
            .child(title)
            .children(logo)
            .child(div().flex_1())
            .child(div().flex_none().mr(m.sp[2]).child(copy))
    }
}

impl Render for InfoSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.render_count += 1;
        }
        self.words.refresh(self.id, cx);
        let m = metrics(cx);
        let n = self.id as usize;
        let id = self.id;
        let copied = self.words.copied.clone();
        let fetch_row = self.fetch_row;
        let fetch_word = self.words.fetch.clone();
        let mut grid = TlKeyValueGrid::new(("info-rows", n), KvLayout::Inline)
            .sections(&self.grid)
            .row_height(m.row_height)
            .label_width(px(LABEL_WIDTH * m.ui_scale))
            .block_indent(m.sp[5])
            .block_gap(m.sp[5])
            .label_inset(m.sp[4])
            .label_tooltips(self.label_tips.clone())
            .label_lines(self.label_lines.clone())
            .loading(self.loading)
            .on_copy_section(move |_, window, cx| {
                window.dispatch_action(Box::new(CopyInformationSection { section: id }), cx);
            })
            .on_copied(move |_, window, cx| {
                // `write_to_clipboard` reports no failure, so this is the only toast (R-199).
                if let Some(queue) = ToastQueue::of_window(window, cx) {
                    let text = copied.clone();
                    queue.update(cx, |q, cx| q.push(ToastKind::Success, text, None, None, cx));
                }
            });
        if let Some((at, loading)) = fetch_row {
            grid = grid.value_suffix(move |row, _, cx| {
                if row != at {
                    return None;
                }
                let width = px(FETCH_BUTTON_W * metrics(cx).ui_scale);
                let button = TlButton::new("info-public-ip-fetch", TlButtonKind::Secondary)
                    .size(TlButtonSize::Sm)
                    .label(fetch_word.clone())
                    .action(Box::new(FetchInformationPublicIp))
                    .loading(loading)
                    .disabled(loading);
                Some((width, button.into_any_element()))
            });
        }
        let (measured, owner) = (Rc::clone(&self.height), self.owner);
        let measure = canvas(
            move |bounds, _, cx| {
                if measured.get() != bounds.size.height {
                    measured.set(bounds.size.height);
                    if let Some(owner) = owner {
                        cx.notify(owner);
                    }
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        v_flex()
            .debug_selector(move || format!("InfoSection-{n}"))
            .relative()
            .w_full()
            .child(measure)
            .child(self.band(cx))
            .child(div().mt(m.sp[4]).child(grid))
    }
}

/// The Fetch button's action, handled by the page (a click leaves focus in the page, so the
/// action reaches the page and not the section). The notice comes first (PERF-060); the
/// Inventory worker runs the fetch and the page reads `snapshot.inventory.public_ip`. This page
/// makes no request of its own. The fetch goes to `host`'s engine.
pub fn fetch_public_ip(host: &HostContext, window: &mut Window, cx: &mut App) {
    let engine = host.client().cloned();
    request_public_ip_fetch(window, cx, move |_, _| send_public_ip_fetch(engine));
}

fn send_public_ip_fetch(engine: Option<Arc<dyn EngineClient>>) {
    let Some(engine) = engine else {
        return;
    };
    match engine.send(Command::Device(DeviceCommand::FetchPublicIp)) {
        Ok(()) => {}
        Err(CommandError::Busy) => {
            tracing::warn!(target: "tasklens::ui", "public IP fetch not queued: busy");
        }
        Err(_) => tracing::warn!(target: "tasklens::ui", "public IP fetch not queued"),
    }
}
