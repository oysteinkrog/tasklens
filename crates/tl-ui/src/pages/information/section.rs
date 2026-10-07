//! One Information section: the title band and its rows (UI-SYS-081, UI-SYS-082, UI-FND-025,
//! PERF-013, PERF-094, PERF-095, R-050).
//!
//! The band is `row_h` high with the `header` fill, a 3 px `accent` bar at its left edge, the
//! title, the distro logo (Operating System only) and a copy button at the right end that
//! dispatches [`CopyInformationSection`]. The rows draw in a `TlKeyValueGrid` inline layout.
//!
//! The page builds the rows and passes them in as one `Arc`. A new `Arc` with the same rows does
//! not re-render the section. Render builds no strings: the grid rows are made in `set_rows`
//! and the title is translated once per strings version.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::accesskit::Role;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::gpui::private::schemars::JsonSchema;
use gpui_kit::{
    App, Context, ElementId, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement as _, Styled, Window, div, gpui, img, px,
};
use serde::Deserialize;

use super::actions::CopyInformationSection;
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::icons::TlIcon;
use crate::components::kv_grid::{
    KvLayout, KvWords, TlKeyValueGrid, TlKvBlock, TlKvRow, TlKvSection, TlValue, shown_text_in,
};
use crate::i18n::{Strings, tr, tr_en};
use crate::text::style::t_md;
use crate::theme::{current, metrics};

/// Width of the accent bar at the band's left edge, × ui_scale.
const BAR_WIDTH: f32 = 3.;
/// Distro logo size, × ui_scale.
const LOGO_SIZE: f32 = 16.;
/// From the label start to the value start, × ui_scale (measured, UI-SYS-081).
const LABEL_WIDTH: f32 = 232.;
const KEY_COPY_SECTION: &str = "information-copy-section";

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
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(line);
    }
}

/// The band text, translated once per strings version. A new table is in en-US.
struct SectionWords {
    /// `None` before the first refresh; then the strings version, `None` without strings.
    version: Option<Option<u64>>,
    title: SharedString,
    copy_section: SharedString,
}

impl SectionWords {
    fn new(id: InfoSectionId) -> Self {
        Self {
            version: None,
            title: id.title(),
            copy_section: tr_en(KEY_COPY_SECTION, &[]),
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
        self.copy_section = tr(cx, KEY_COPY_SECTION);
    }
}

/// The grid input for `rows`: labels with ":", one block per run of rows with the same `block`.
fn grid_section(rows: &[InfoRow]) -> TlKvSection {
    let mut blocks: Vec<TlKvBlock> = Vec::new();
    let mut current = None;
    for row in rows {
        if current != Some(row.block) {
            current = Some(row.block);
            blocks.push(TlKvBlock {
                indent: None,
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
            .size(TlButtonSize::Sm)
            .tooltip(self.words.copy_section.clone())
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
        let grid = TlKeyValueGrid::new(("info-rows", n), KvLayout::Inline)
            .sections(&self.grid)
            .row_height(m.row_height)
            .label_width(px(LABEL_WIDTH * m.ui_scale))
            .block_indent(m.sp[5])
            .block_gap(m.sp[5]);
        v_flex()
            .debug_selector(move || format!("InfoSection-{n}"))
            .w_full()
            .child(self.band(cx))
            .child(div().mt(m.sp[4]).pl(m.sp[4]).child(grid))
    }
}
