//! Component gallery (UI-FND-030, ADR-013, R-138): one window with a page per shared component,
//! for review by eye and as the host of the golden-image render tests (`tests/render_gallery.rs`).
//!
//! Each page draws every variant and state twice, side by side: the left half in Default Dark and
//! the right half in Default Light. The gallery does not switch the global theme. It builds the
//! two `TlTheme` values from `ThemeStore.builtin`, maps each onto its own copy of the kit `Theme`,
//! and draws each half inside a [`ThemeScope`]. That element puts its theme in
//! `ThemeStore.preview` and its kit theme in the kit global only while its own subtree is laid
//! out, prepainted and painted, and puts the previous values back after each phase. Components
//! read `theme::current(cx)` as usual and get the half's theme. Popups and hover tooltips are
//! deferred draws, so they paint after the scope ends and use the window's own theme.
//!
//! A `TlSwitch` "Reduced motion" at the top sets `App::set_reduce_motion` for the whole app. The
//! render tests force reduced motion on, so animations show their end state.
//!
//! Budget (UI-FND-023): a page shows at most 2 spinners or indeterminate bars that animate when
//! motion is on. [`GalleryView::animated_count`] reports the count of the last render. Spinners
//! drawn with `.reduced_motion(true)` never animate and are not counted.
//!
//! Compiled only with the `bench` feature and opened by `tasklens --gallery` (E01).

use std::sync::Arc;

use gpui_kit::base::slider::SliderState;
use gpui_kit::component::Icon;
use gpui_kit::component::input::InputState;
use gpui_kit::component::theme::{Theme, ThemeMode as KitMode};
use gpui_kit::{
    Action, AnyElement, AnyView, App, AppContext as _, Bounds, Context, Div, Element, ElementId,
    Entity, FontWeight, Global, GlobalElementId, InspectorElementId, InteractiveElement,
    IntoElement, KeyBinding, LayoutId, ParentElement, Pixels, Render, SharedString, Styled, Window,
    WindowBounds, WindowHandle, WindowOptions, div, px, size,
};
use tl_model::settings::AppearanceSettings;
use tl_model::units;
use tl_model::validity::Validity;

use super::badge::{TlBadge, TlBadgeKind, count_text};
use super::banner::{TlBanner, TlBannerKind};
use super::button::{TlButton, TlButtonKind, TlButtonSize};
use super::checkbox::{TlCheckbox, bind_keys_components};
use super::empty::TlEmptyState;
use super::focus::TlFocusRing;
use super::icons::{AppIcon, TlIcon, icon};
use super::input::{TlFilterBox, TlInputSize, TlTextInput};
use super::kv_grid::{
    self, KvLayout, TlKeyValueGrid, TlKvBlock, TlKvRow, TlKvSection, TlValue, UnsupportedWord,
};
use super::link::TlLink;
use super::progress::{TlProgress, TlProgressSize, TlSpinner};
use super::radio::{RadioDirection, TlRadio, TlRadioGroup};
use super::select::{TlCombo, TlSelect, TlSelectItem, TlSelectState};
use super::skeleton::{TlSkeletonBlock, TlSkeletonRows};
use super::slider::TlSlider;
use super::stepper::TlStepper;
use super::switch::TlSwitch;
use super::tabs::{NextInWindowTab, PrevInWindowTab, TlTabSpec, TlTabs, tabs_window_context};
use super::tooltip::TlTooltip;
use super::validity::glyph;
use crate::text::ellipsis::middle_ellipsis;
use crate::text::fonts::{MONO_FAMILY, UI_FAMILY};
use crate::text::metrics::TextMetricsCache;
use crate::text::style::{label_style, numeric_style, t_base, t_sm, t_xs};
use crate::theme::map;
use crate::theme::{ThemeMode, ThemeStore, TlTheme, UiMetrics, current, metrics};

gpui_kit::actions!(
    tl_gallery,
    [GalleryNoop, GalleryFocusNext, GalleryFocusPrev]
);

/// One page per E15 component module, plus `text`. E16-011 appends `toast`, `dialog`,
/// `window_frame`, `menu` and `shield`.
pub const GALLERY_PAGES: &[&str] = &[
    "button", "checkbox", "radio", "switch", "slider", "stepper", "input", "select", "tabs",
    "tooltip", "progress", "skeleton", "kv_grid", "validity", "badge", "empty", "banner", "link",
    "icons", "text",
];

/// Window size in logical px.
pub const WINDOW_WIDTH: f32 = 1280.;
pub const WINDOW_HEIGHT: f32 = 800.;

const ROOT_CONTEXT: &str = "TlGallery";
/// Width of the row label column at 100% scale.
const LABEL_W: f32 = 112.;

/// Set once [`init`] has bound the gallery keys.
struct GalleryKeys;

impl Global for GalleryKeys {}

/// Binds the component keys, Tab and Shift+Tab between controls, and Ctrl+E to a no-op so the
/// tooltip page can show a shortcut line. Runs once per app; later calls do nothing.
pub fn init(cx: &mut App) {
    if cx.has_global::<GalleryKeys>() {
        return;
    }
    cx.set_global(GalleryKeys);
    bind_keys_components(cx);
    kv_grid::init(cx);
    cx.bind_keys([
        KeyBinding::new("tab", GalleryFocusNext, Some(ROOT_CONTEXT)),
        KeyBinding::new("shift-tab", GalleryFocusPrev, Some(ROOT_CONTEXT)),
        KeyBinding::new("ctrl-e", GalleryNoop, None),
    ]);
}

/// Opens the gallery window at its first page. The theme store and the kit must be set up, as
/// the app does before its first window.
pub fn open_gallery(cx: &mut App) -> WindowHandle<GalleryView> {
    init(cx);
    let bounds = Bounds::centered(None, size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        },
        |window, cx| cx.new(|cx| GalleryView::new(0, window, cx)),
    )
    .expect("open the gallery window")
}

/// One theme half: its theme, its kit theme, and the entities its samples need.
struct Half {
    id: &'static str,
    title: &'static str,
    theme: Arc<TlTheme>,
    kit: Theme,
    sliders: [Entity<SliderState>; 5],
    steppers: [Entity<InputState>; 5],
    inputs: [Entity<InputState>; 10],
    filters: [Entity<InputState>; 2],
    selects: [Entity<TlSelectState>; 4],
    tooltips: [AnyView; 5],
}

/// The gallery window.
pub struct GalleryView {
    page: usize,
    halves: [Half; 2],
    animated: usize,
}

impl GalleryView {
    /// A gallery showing `GALLERY_PAGES[page]`.
    pub fn new(page: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (dark, light) = scoped_themes(cx);
        let halves = [
            Half::new("dark", "Default Dark", dark, window, cx),
            Half::new("light", "Default Light", light, window, cx),
        ];
        Self {
            page: page.min(GALLERY_PAGES.len() - 1),
            halves,
            animated: 0,
        }
    }

    /// The page on show, an index into `GALLERY_PAGES`.
    pub fn page(&self) -> usize {
        self.page
    }

    /// Spinners and indeterminate bars that animate when motion is on, in the last render of the
    /// page, both halves together.
    pub fn animated_count(&self) -> usize {
        self.animated
    }

    fn set_page(&mut self, page: usize, cx: &mut Context<Self>) {
        self.page = page;
        cx.notify();
    }
}

/// Builds Default Dark and Default Light with their kit themes. The kit themes are made by the
/// same two `Theme::update` calls the theme store uses (R-274); the kit is put back on the
/// current theme afterwards.
fn scoped_themes(cx: &mut App) -> ((Arc<TlTheme>, Theme), (Arc<TlTheme>, Theme)) {
    let base = (**current(cx)).clone();
    let defs = cx.global::<ThemeStore>().builtin.clone();
    let appearance = AppearanceSettings::default();
    let build = |name: &str| {
        let def = defs
            .iter()
            .find(|d| &*d.name == name)
            .unwrap_or_else(|| panic!("built-in theme {name} is missing"));
        Arc::new(TlTheme::from_def(
            def,
            &base,
            &appearance,
            true,
            base.version,
        ))
    };
    let dark = build("Default Dark");
    let light = build("Default Light");
    let kit_dark = kit_theme(&dark, cx);
    let kit_light = kit_theme(&light, cx);
    let active = current(cx).clone();
    kit_theme(&active, cx);
    ((dark, kit_dark), (light, kit_light))
}

/// Maps `tl` onto the kit global and returns a copy of the result.
fn kit_theme(tl: &TlTheme, cx: &mut App) -> Theme {
    let m = *metrics(cx);
    let mode = match tl.mode {
        ThemeMode::Dark => KitMode::Dark,
        ThemeMode::Light => KitMode::Light,
    };
    if Theme::global(cx).mode != mode {
        Theme::update(cx, |kit| kit.mode = mode);
    }
    Theme::update(cx, |kit| map::apply(tl, &m, kit));
    Theme::global(cx).clone()
}

impl Half {
    fn new(
        id: &'static str,
        title: &'static str,
        (theme, kit): (Arc<TlTheme>, Theme),
        window: &mut Window,
        cx: &mut Context<GalleryView>,
    ) -> Self {
        let slider = |value: f32, cx: &mut Context<GalleryView>| {
            cx.new(|_| {
                SliderState::new()
                    .min(50.)
                    .max(200.)
                    .step(5.)
                    .default_value(value)
            })
        };
        let sliders = [
            slider(100., cx),
            slider(50., cx),
            slider(200., cx),
            slider(125., cx),
            slider(150., cx),
        ];
        let steppers = std::array::from_fn(|_| cx.new(|cx| InputState::new(window, cx)));
        let inputs = std::array::from_fn(|i| {
            let (placeholder, text) = INPUT_TEXTS[i];
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(text)
            })
        });
        let filters = std::array::from_fn(|i| {
            cx.new(|cx| InputState::new(window, cx).default_value(["", "fire"][i]))
        });
        let languages = || {
            vec![
                TlSelectItem::new("en", "English"),
                TlSelectItem::new("nb", "Norsk bokmål"),
                TlSelectItem::new("de", "Deutsch"),
            ]
        };
        let tabs: Vec<TlSelectItem> = [
            "Processes",
            "Performance",
            "App history",
            "Startup",
            "Users",
            "Details",
            "Services",
            "Network",
            "Disks",
            "Drivers",
            "Journal",
            "Inventory",
            "Containers",
            "Sensors",
        ]
        .iter()
        .map(|t| TlSelectItem::new(t.to_lowercase(), *t))
        .collect();
        let selects = [
            cx.new(|cx| TlSelectState::new(languages(), Some("en"), cx)),
            cx.new(|cx| TlSelectState::new(languages(), None, cx)),
            cx.new(|cx| TlSelectState::new(languages(), Some("nb"), cx)),
            cx.new(|cx| TlSelectState::new(tabs, Some("performance"), cx)),
        ];
        let many: Vec<SharedString> = (1..=14)
            .map(|n| format!("Line {n} of a process row tooltip").into())
            .collect();
        let tooltips = [
            TlTooltip::text("Copy the selected rows").view(cx),
            TlTooltip::text("End the selected process")
                .action(&GalleryNoop, None)
                .view(cx),
            TlTooltip::text(
                "A long tooltip text wraps at 360 px. It keeps going so that it needs a second \
                 and a third row to fit.",
            )
            .view(cx),
            TlTooltip::lines(vec![
                "firefox".into(),
                "PID 4127, user oystein".into(),
                "/usr/lib/firefox/firefox".into(),
            ])
            .view(cx),
            TlTooltip::lines(many).view(cx),
        ];
        Self {
            id,
            title,
            theme,
            kit,
            sliders,
            steppers,
            inputs,
            filters,
            selects,
            tooltips,
        }
    }
}

/// Placeholder and text of each `Half.inputs` entry, in the order the input page uses them.
const INPUT_TEXTS: [(&str, &str); 10] = [
    ("Filter processes", ""),
    ("Filter processes", ""),
    ("Name", "firefox"),
    ("Search", ""),
    ("Search", "steam"),
    ("Number", "abc"),
    ("Search", "kworker"),
    ("Name", "Disabled field"),
    ("", "512"),
    ("Name", "Square corners"),
];

impl Render for GalleryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = current(cx).clone();
        let m = *metrics(cx);
        let view = cx.entity().downgrade();
        let page = GALLERY_PAGES[self.page];

        let switch_view = view.clone();
        let switch = TlSwitch::new("gallery-reduced-motion")
            .label("Reduced motion")
            .checked(cx.reduce_motion())
            .on_change(move |on, _, cx| {
                cx.set_reduce_motion(on);
                switch_view.update(cx, |_, cx| cx.notify()).ok();
            });
        let tabs = TlTabs::new("gallery-pages")
            .tabs(GALLERY_PAGES.iter().map(|p| TlTabSpec::new(*p)).collect())
            .selected(self.page)
            .on_change(move |ix, _, cx| {
                view.update(cx, |this, cx| this.set_page(ix, cx)).ok();
            });

        let mut animated = 0;
        let mut halves = Vec::with_capacity(2);
        for half in &self.halves {
            let mut p = PageCx {
                t: half.theme.clone(),
                m,
                half,
                animated: 0,
            };
            let body = page_body(page, &mut p, window, cx);
            animated += p.animated;
            let content = div()
                .flex_1()
                .min_w_0()
                .h_full()
                .overflow_hidden()
                .flex()
                .flex_col()
                .gap(m.sp[5])
                .p(m.sp[6])
                .bg(half.theme.window_bg)
                .text_color(half.theme.text_primary)
                .font_family(UI_FAMILY)
                .text_size(m.text.base)
                .child(caption(&m, half.theme.text_secondary, half.title))
                .children(body);
            halves.push(ThemeScope {
                id: half.id.into(),
                tl: Some(half.theme.clone()),
                kit: half.kit.clone(),
                child: content.into_any_element(),
            });
        }
        self.animated = animated;

        div()
            .id("gallery")
            .key_context(ROOT_CONTEXT)
            .key_context(tabs_window_context())
            .on_action(|_: &GalleryFocusNext, window, cx| window.focus_next(cx))
            .on_action(|_: &GalleryFocusPrev, window, cx| window.focus_prev(cx))
            .on_action(cx.listener(|this, _: &NextInWindowTab, _, cx| {
                this.set_page((this.page + 1) % GALLERY_PAGES.len(), cx)
            }))
            .on_action(cx.listener(|this, _: &PrevInWindowTab, _, cx| {
                let n = GALLERY_PAGES.len();
                this.set_page((this.page + n - 1) % n, cx)
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(t.window_bg)
            .text_color(t.text_primary)
            .font_family(UI_FAMILY)
            .text_size(m.text.base)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(m.sp[5])
                    .px(m.sp[6])
                    .py(m.sp[3])
                    .bg(t.toolbar)
                    .child(switch)
                    .child(caption(
                        &m,
                        t.text_secondary,
                        "Left: Default Dark. Right: Default Light.",
                    )),
            )
            .child(tabs)
            .child(div().flex_1().min_h_0().flex().flex_row().children(halves))
    }
}

/// What a page builder needs for one half.
struct PageCx<'a> {
    t: Arc<TlTheme>,
    m: UiMetrics,
    half: &'a Half,
    /// Animated elements this half added.
    animated: usize,
}

impl PageCx<'_> {
    /// Counts one spinner or indeterminate bar that animates when motion is on.
    fn animated(&mut self) {
        self.animated += 1;
    }

    fn label_w(&self) -> Pixels {
        px(LABEL_W * self.m.ui_scale)
    }

    /// A labelled row of samples.
    fn row(&self, label: &str, items: impl IntoIterator<Item = AnyElement>) -> AnyElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(self.m.sp[4])
            .child(
                caption(&self.m, self.t.text_secondary, label)
                    .w(self.label_w())
                    .flex_none(),
            )
            .children(items)
            .into_any_element()
    }

    /// A small heading over a group of rows.
    fn heading(&self, text: &str) -> AnyElement {
        let mut el = div();
        *el.text_style() = label_style(&self.m, self.m.text.sm, FontWeight::MEDIUM);
        el.text_color(self.t.text_secondary)
            .child(SharedString::from(text.to_owned()))
            .into_any_element()
    }

    /// A box with a 1 px divider border, for samples that fill their parent.
    fn frame(&self, w: f32, h: f32, child: impl IntoElement) -> AnyElement {
        let s = self.m.ui_scale;
        div()
            .w(px(w * s))
            .h(px(h * s))
            .flex_none()
            .overflow_hidden()
            .border_1()
            .border_color(self.t.divider)
            .child(child)
            .into_any_element()
    }
}

/// `t_sm` text in `color`.
fn caption(m: &UiMetrics, color: gpui_kit::Hsla, text: &str) -> Div {
    let mut el = div();
    *el.text_style() = t_sm(m);
    el.text_color(color)
        .whitespace_nowrap()
        .child(SharedString::from(text.to_owned()))
}

fn page_body(page: &str, p: &mut PageCx, window: &mut Window, cx: &mut App) -> Vec<AnyElement> {
    match page {
        "button" => page_button(p),
        "checkbox" => page_checkbox(p),
        "radio" => page_radio(p),
        "switch" => page_switch(p),
        "slider" => page_slider(p),
        "stepper" => page_stepper(p),
        "input" => page_input(p),
        "select" => page_select(p),
        "tabs" => page_tabs(p),
        "tooltip" => page_tooltip(p),
        "progress" => page_progress(p),
        "skeleton" => page_skeleton(p),
        "kv_grid" => page_kv_grid(p),
        "validity" => page_validity(p),
        "badge" => page_badge(p),
        "empty" => page_empty(p),
        "banner" => page_banner(p),
        "link" => page_link(p),
        "icons" => page_icons(p),
        "text" => page_text(p, window, cx),
        _ => vec![p.heading(&format!("No page for {page}"))],
    }
}

fn page_button(p: &mut PageCx) -> Vec<AnyElement> {
    let kinds: [(&str, TlButtonKind); 9] = [
        ("Primary", TlButtonKind::Primary),
        ("Secondary", TlButtonKind::Secondary),
        ("Danger", TlButtonKind::Danger),
        ("Ghost", TlButtonKind::Ghost),
        ("Icon", TlButtonKind::Icon),
        ("Toggle off", TlButtonKind::Toggle { on: false }),
        ("Toggle on", TlButtonKind::Toggle { on: true }),
        ("Segment off", TlButtonKind::Segment { on: false }),
        ("Segment on", TlButtonKind::Segment { on: true }),
    ];
    let sample = |id: String, kind: TlButtonKind, size: TlButtonSize| {
        let b = TlButton::new(ElementId::Name(id.into()), kind).size(size);
        match kind {
            TlButtonKind::Primary => b.label("Apply"),
            TlButtonKind::Secondary => b.label("Cancel"),
            TlButtonKind::Danger => b.label("End task"),
            TlButtonKind::Ghost => b.label("Details"),
            TlButtonKind::Icon => b.icon(TlIcon::Copy).tooltip("Copy"),
            TlButtonKind::Toggle { .. } => b.icon(TlIcon::Pin).tooltip("Pin"),
            TlButtonKind::Segment { .. } => b.label("Overall"),
        }
    };
    let mut out = vec![p.heading(
        "Each row: sm default, sm pressed, sm disabled, md default, md pressed, md disabled",
    )];
    for (name, kind) in kinds {
        let mut items = Vec::new();
        for (sz, size) in [("sm", TlButtonSize::Sm), ("md", TlButtonSize::Md)] {
            items.push(sample(format!("{name}-{sz}"), kind, size).into_any_element());
            items.push(
                sample(format!("{name}-{sz}-pressed"), kind, size)
                    .pressed(true)
                    .into_any_element(),
            );
            items.push(
                sample(format!("{name}-{sz}-disabled"), kind, size)
                    .disabled(true)
                    .into_any_element(),
            );
        }
        out.push(p.row(name, items));
    }
    let focused = TlFocusRing::ring(
        div().relative().flex_none().child(
            TlButton::new("focused", TlButtonKind::Secondary)
                .size(TlButtonSize::Md)
                .label("Focused"),
        ),
        p.m.radius_small,
        &p.t,
    );
    p.animated();
    out.push(
        p.row(
            "Other states",
            [
                TlButton::new("with-icon", TlButtonKind::Secondary)
                    .icon(TlIcon::RotateCcwClock)
                    .label("Reset")
                    .into_any_element(),
                focused.into_any_element(),
                TlButton::new("loading", TlButtonKind::Primary)
                    .label("Apply")
                    .loading(true)
                    .into_any_element(),
            ],
        ),
    );
    out.push(p.heading("Hover shows on pointer hover; the focus ring is drawn here by hand."));
    out
}

fn page_checkbox(p: &mut PageCx) -> Vec<AnyElement> {
    let states: [(&str, Option<bool>); 3] = [
        ("Unchecked", Some(false)),
        ("Checked", Some(true)),
        ("Indeterminate", None),
    ];
    let mut out = Vec::new();
    for (name, checked) in states {
        out.push(p.row(
            name,
            [false, true].map(|disabled| {
                TlCheckbox::new(ElementId::Name(format!("{name}-{disabled}").into()))
                    .checked(checked)
                    .label(if disabled {
                        "Disabled"
                    } else {
                        "Include children"
                    })
                    .disabled(disabled)
                    .into_any_element()
            }),
        ));
    }
    out.push(p.row(
        "Inline (20 px)",
        [Some(false), Some(true)].map(|checked| {
            TlCheckbox::new(ElementId::Name(format!("inline-{checked:?}").into()))
                .checked(checked)
                .inline(true)
                .label("Running")
                .into_any_element()
        }),
    ));
    out
}

fn page_radio(p: &mut PageCx) -> Vec<AnyElement> {
    let items = || -> Vec<SharedString> { vec!["Low".into(), "Normal".into(), "High".into()] };
    vec![
        p.row(
            "Vertical",
            [TlRadioGroup::new("vertical")
                .items(items())
                .selected(1)
                .into_any_element()],
        ),
        p.row(
            "Horizontal",
            [TlRadioGroup::new("horizontal")
                .items(items())
                .selected(0)
                .direction(RadioDirection::Horizontal)
                .into_any_element()],
        ),
        p.row(
            "Disabled group",
            [TlRadioGroup::new("disabled")
                .items(items())
                .selected(2)
                .direction(RadioDirection::Horizontal)
                .disabled(true)
                .into_any_element()],
        ),
        p.row(
            "Single radios",
            [
                TlRadio::new("off", "Off").into_any_element(),
                TlRadio::new("on", "On").checked(true).into_any_element(),
                TlRadio::new("off-disabled", "Disabled")
                    .disabled(true)
                    .into_any_element(),
                TlRadio::new("on-disabled", "Disabled")
                    .checked(true)
                    .disabled(true)
                    .into_any_element(),
            ],
        ),
    ]
}

fn page_switch(p: &mut PageCx) -> Vec<AnyElement> {
    let mut out = Vec::new();
    for (name, disabled) in [("Enabled", false), ("Disabled", true)] {
        out.push(p.row(
            name,
            [false, true].map(|on| {
                TlSwitch::new(ElementId::Name(format!("{name}-{on}").into()))
                    .checked(on)
                    .disabled(disabled)
                    .into_any_element()
            }),
        ));
    }
    out.push(
        p.row(
            "With label",
            [TlSwitch::new("labelled")
                .label("Sync theme with system")
                .checked(true)
                .into_any_element()],
        ),
    );
    out
}

fn page_slider(p: &mut PageCx) -> Vec<AnyElement> {
    let s = &p.half.sliders;
    let rows: [(&str, TlSlider); 5] = [
        (
            "Settings",
            TlSlider::new(&s[0]).label("UI Scale").value_text("100%"),
        ),
        (
            "At minimum",
            TlSlider::new(&s[1]).label("UI Scale").value_text("50%"),
        ),
        (
            "At maximum",
            TlSlider::new(&s[2]).label("UI Scale").value_text("200%"),
        ),
        (
            "Disabled",
            TlSlider::new(&s[3])
                .label("Row Height")
                .value_text("125%")
                .disabled(true),
        ),
        (
            "In a menu",
            TlSlider::new(&s[4])
                .label("Drop Shadow")
                .value_text("150%")
                .in_menu(true),
        ),
    ];
    rows.into_iter()
        .map(|(name, slider)| p.row(name, [slider.into_any_element()]))
        .collect()
}

fn page_stepper(p: &mut PageCx) -> Vec<AnyElement> {
    let s = &p.half.steppers;
    let rows: [(&str, TlStepper); 5] = [
        (
            "With suffix",
            TlStepper::new("interval", &s[0])
                .range(250, 10_000, 250)
                .value(1_000)
                .suffix("ms"),
        ),
        (
            "At minimum",
            TlStepper::new("min", &s[1])
                .range(250, 10_000, 250)
                .value(250)
                .suffix("ms"),
        ),
        (
            "At maximum",
            TlStepper::new("max", &s[2])
                .range(1, 48, 1)
                .value(48)
                .suffix("h"),
        ),
        (
            "No suffix",
            TlStepper::new("nice", &s[3]).range(-20, 19, 1).value(0),
        ),
        (
            "Disabled",
            TlStepper::new("disabled", &s[4])
                .range(64, 4_096, 64)
                .value(512)
                .suffix("MB")
                .disabled(true),
        ),
    ];
    rows.into_iter()
        .map(|(name, stepper)| p.row(name, [stepper.into_any_element()]))
        .collect()
}

fn page_input(p: &mut PageCx) -> Vec<AnyElement> {
    let w = px(200. * p.m.ui_scale);
    let i = &p.half.inputs;
    let field = |n: usize| TlTextInput::new(ElementId::Name(format!("input-{n}").into()), &i[n]);
    p.animated();
    let rows: Vec<(&str, AnyElement)> = vec![
        (
            "sm, empty",
            field(0)
                .size(TlInputSize::Sm)
                .width(w)
                .placeholder(INPUT_TEXTS[0].0)
                .into_any_element(),
        ),
        (
            "md, empty",
            field(1)
                .width(w)
                .placeholder(INPUT_TEXTS[1].0)
                .into_any_element(),
        ),
        ("md, text", field(2).width(w).into_any_element()),
        (
            "Prefix icon",
            field(3)
                .width(w)
                .prefix_icon(TlIcon::Search)
                .placeholder(INPUT_TEXTS[3].0)
                .into_any_element(),
        ),
        (
            "Clear button",
            field(4)
                .width(w)
                .prefix_icon(TlIcon::Search)
                .clear_button(true)
                .into_any_element(),
        ),
        (
            "Invalid",
            field(5).width(w).invalid(true).into_any_element(),
        ),
        (
            "Loading",
            field(6).width(w).loading(true).into_any_element(),
        ),
        (
            "Disabled",
            field(7).width(w).disabled(true).into_any_element(),
        ),
        (
            "Right, suffix",
            field(8)
                .size(TlInputSize::Sm)
                .width(px(96. * p.m.ui_scale))
                .align_right(true)
                .suffix_text("MB")
                .into_any_element(),
        ),
        (
            "Square corners",
            field(9).width(w).square_corners(true).into_any_element(),
        ),
    ];
    let f = &p.half.filters;
    let mut out: Vec<AnyElement> = rows
        .into_iter()
        .map(|(name, el)| p.row(name, [el]))
        .collect();
    out.push(p.row(
        "Filter box",
        [
            TlFilterBox::new("filter-empty", &f[0]).into_any_element(),
            TlFilterBox::new("filter-text", &f[1]).into_any_element(),
        ],
    ));
    out
}

fn page_select(p: &mut PageCx) -> Vec<AnyElement> {
    let w = px(200. * p.m.ui_scale);
    let s = &p.half.selects;
    vec![
        p.row(
            "Selected",
            [TlSelect::new("selected", &s[0]).width(w).into_any_element()],
        ),
        p.row(
            "No selection",
            [TlSelect::new("none", &s[1]).width(w).into_any_element()],
        ),
        p.row(
            "Disabled",
            [TlSelect::new("disabled", &s[2])
                .width(w)
                .disabled(true)
                .into_any_element()],
        ),
        p.row(
            "Combo, 14 items",
            [TlCombo::new("combo", &s[3]).width(w).into_any_element()],
        ),
        p.heading("The popup opens on click; it is a deferred draw in the window's own theme."),
    ]
}

fn page_tabs(p: &mut PageCx) -> Vec<AnyElement> {
    let specs = || {
        vec![
            TlTabSpec::new("General"),
            TlTabSpec::new("Threads").count("17"),
            TlTabSpec::new("Modules").count("212"),
            TlTabSpec::new("Environment"),
            TlTabSpec::new("Security").disabled("Needs administrator access."),
        ]
    };
    let strip = |id: &'static str, selected: usize, w: f32| {
        div()
            .w(px(w * p.m.ui_scale))
            .child(TlTabs::new(id).tabs(specs()).selected(selected))
            .into_any_element()
    };
    vec![
        p.heading("First selected, then the third, then an overflowing strip 260 px wide"),
        strip("tabs-first", 0, 560.),
        strip("tabs-third", 2, 560.),
        strip("tabs-narrow", 1, 260.),
    ]
}

fn page_tooltip(p: &mut PageCx) -> Vec<AnyElement> {
    let names = [
        "Text",
        "With shortcut",
        "Wrapped text",
        "Lines",
        "14 lines, cut to 12",
    ];
    names
        .into_iter()
        .zip(p.half.tooltips.iter())
        .map(|(name, view)| {
            p.row(
                name,
                [div().flex_none().child(view.clone()).into_any_element()],
            )
        })
        .collect()
}

fn page_progress(p: &mut PageCx) -> Vec<AnyElement> {
    let bar = |w: f32, el: TlProgress| div().w(px(w * p.m.ui_scale)).child(el).into_any_element();
    let mut out = vec![
        p.row(
            "md 0, 37, 100%",
            [0., 37., 100.].map(|v| bar(140., TlProgress::new().value(v))),
        ),
        p.row(
            "sm 0, 37, 100%",
            [0., 37., 100.].map(|v| bar(140., TlProgress::new().value(v).size(TlProgressSize::Sm))),
        ),
        p.row(
            "With label",
            [bar(220., TlProgress::new().value(64.).show_label(true))],
        ),
    ];
    p.animated();
    out.push(p.row(
        "Indeterminate",
        [
            bar(220., TlProgress::new()),
            caption(&p.m, p.t.text_secondary, "Working…").into_any_element(),
        ],
    ));
    out.push(
        p.row(
            "Spinner sm, md",
            [p.m.icon.sm, p.m.icon.md]
                .map(|size| TlSpinner::new(size).reduced_motion(true).into_any_element()),
        ),
    );
    out.push(p.heading(
        "The spinners here are the reduced-motion form; the button and input pages show the \
         animated one (at most 2 per window).",
    ));
    out
}

fn page_skeleton(p: &mut PageCx) -> Vec<AnyElement> {
    let s = p.m.ui_scale;
    vec![
        p.heading("Table rows, 4 columns"),
        p.frame(
            560.,
            150.,
            TlSkeletonRows::new(5, [220., 80., 80., 120.].map(|w| px(w * s))),
        ),
        p.heading("Key/value block, 4 lines"),
        p.frame(320., 130., TlSkeletonBlock::new().lines(4)),
    ]
}

fn page_kv_grid(p: &mut PageCx) -> Vec<AnyElement> {
    let value = |text: &str, validity: Validity| TlValue {
        validity,
        ..TlValue::new(text.to_owned())
    };
    let row = |label: &str, value: TlValue| TlKvRow {
        label: label.to_owned().into(),
        value,
    };
    let stacked = [TlKvSection {
        title: Some("Memory".into()),
        blocks: vec![TlKvBlock {
            heading: None,
            rows: vec![
                row("In use", TlValue::new("3.2 GiB")),
                row("Available", TlValue::new("12.1 GiB")),
                row("Committed", value("", Validity::WarmingUp)),
                row(
                    "Cached",
                    TlValue {
                        tooltip: Some("Last updated 4 s ago".into()),
                        ..value("2.0 GiB", Validity::Stale)
                    },
                ),
                row("Paged pool", value("", Validity::Restricted)),
                row(
                    "Non-paged pool",
                    TlValue {
                        word: Some(UnsupportedWord::NotReported),
                        ..value("", Validity::Unsupported)
                    },
                ),
                row(
                    "Speed",
                    TlValue {
                        word: Some(UnsupportedWord::NeedsSetup),
                        link: Some(GalleryNoop.boxed_clone()),
                        ..value("", Validity::Unsupported)
                    },
                ),
                row("Hardware reserved", value("", Validity::Missing)),
            ],
        }],
    }];
    let inline = [TlKvSection {
        title: Some("General".into()),
        blocks: vec![
            TlKvBlock {
                heading: None,
                rows: vec![
                    row("Name:", TlValue::new("firefox")),
                    row(
                        "Executable:",
                        TlValue {
                            path: true,
                            ..TlValue::new(
                                "/usr/lib/firefox/browser/features/webcompat@mozilla.org.xpi",
                            )
                        },
                    ),
                ],
            },
            TlKvBlock {
                heading: Some("Module 1".into()),
                rows: vec![row("Size:", TlValue::new("1.4 MiB"))],
            },
        ],
    }];
    let w = 600.;
    vec![
        p.frame(
            w,
            230.,
            TlKeyValueGrid::new("stacked", KvLayout::Stacked).sections(&stacked),
        ),
        p.frame(
            w,
            150.,
            TlKeyValueGrid::new("inline", KvLayout::Inline)
                .label_width(px(110. * p.m.ui_scale))
                .sections(&inline),
        ),
        p.row(
            "Loading",
            [p.frame(
                220.,
                70.,
                TlKeyValueGrid::new("loading", KvLayout::Inline).loading(true),
            )],
        ),
        p.row(
            "Filtered empty",
            [p.frame(
                360.,
                50.,
                TlKeyValueGrid::new("empty", KvLayout::Inline).filtered_empty(true),
            )],
        ),
    ]
}

fn page_validity(p: &mut PageCx) -> Vec<AnyElement> {
    let all = [
        ("Valid", Validity::Valid),
        ("WarmingUp", Validity::WarmingUp),
        ("Stale", Validity::Stale),
        ("Restricted", Validity::Restricted),
        ("Unsupported", Validity::Unsupported),
        ("Missing", Validity::Missing),
        ("Failed", Validity::Failed),
    ];
    let value: SharedString = "42.0%".into();
    let cell_w = px(96. * p.m.ui_scale);
    let mut out = vec![p.heading("Not selected, selected")];
    for (name, validity) in all {
        let cells = [false, true].map(|selected| {
            div()
                .w(cell_w)
                .h(p.m.row_height)
                .flex()
                .items_center()
                .px(p.m.sp[3])
                .when_selected(selected, p.t.row_selected)
                .child(glyph(validity, &value, selected, &p.m, &p.t))
                .into_any_element()
        });
        out.push(p.row(name, cells));
    }
    out
}

/// Fills `bg` when `selected`.
trait WhenSelected {
    fn when_selected(self, selected: bool, bg: gpui_kit::Hsla) -> Self;
}

impl WhenSelected for Div {
    fn when_selected(self, selected: bool, bg: gpui_kit::Hsla) -> Self {
        if selected { self.bg(bg) } else { self }
    }
}

fn page_badge(p: &mut PageCx) -> Vec<AnyElement> {
    let kinds = [
        ("Neutral", TlBadgeKind::Neutral, "Unknown"),
        ("Success", TlBadgeKind::Success, "Running"),
        ("Warning", TlBadgeKind::Warning, "Degraded"),
        ("Danger", TlBadgeKind::Danger, "Failed"),
        ("Info", TlBadgeKind::Info, "Activating"),
    ];
    let mut out: Vec<AnyElement> = kinds
        .into_iter()
        .map(|(name, kind, text)| {
            p.row(
                name,
                [false, true]
                    .map(|dot| TlBadge::new(kind, text.into()).dot(dot).into_any_element()),
            )
        })
        .collect();
    out.push(p.row(
        "Count",
        [7, 42, 150].map(|n| {
            TlBadge::new(TlBadgeKind::Count, count_text(n))
                .count(n)
                .into_any_element()
        }),
    ));
    out.push(
        p.row(
            "Long text",
            [TlBadge::new(
                TlBadgeKind::Info,
                "Waiting for the network to come online".into(),
            )
            .into_any_element()],
        ),
    );
    out
}

fn page_empty(p: &mut PageCx) -> Vec<AnyElement> {
    vec![
        p.heading("Plain"),
        p.frame(560., 110., TlEmptyState::new("No threads found.")),
        p.heading("Icon, second line and Clear filter"),
        p.frame(
            560.,
            200.,
            TlEmptyState::new("No properties match the current filter.")
                .icon(TlIcon::Search)
                .second_line("The filter applies to names and values.")
                .clear_filter(GalleryNoop.boxed_clone()),
        ),
    ]
}

fn page_banner(p: &mut PageCx) -> Vec<AnyElement> {
    vec![
        TlBanner::new(
            TlBannerKind::Info,
            "Some journal entries are hidden. Join the systemd-journal group to see them.".into(),
        )
        .button("Additional setup…".into(), GalleryNoop.boxed_clone())
        .into_any_element(),
        TlBanner::new(
            TlBannerKind::Warning,
            "Recording is off. History shows only the last 60 seconds.".into(),
        )
        .button("Turn on".into(), GalleryNoop.boxed_clone())
        .button("Hide".into(), GalleryNoop.boxed_clone())
        .into_any_element(),
        TlBanner::new(
            TlBannerKind::Info,
            "A banner with no buttons and a long text that does not fit on one line ends with an \
             ellipsis, and the full text shows in a hover tooltip."
                .into(),
        )
        .into_any_element(),
        p.heading("Banners fill the width of their pane."),
    ]
}

fn page_link(p: &mut PageCx) -> Vec<AnyElement> {
    let mut sentence = div();
    *sentence.text_style() = t_base(&p.m);
    vec![
        p.row(
            "t_base",
            [
                TlLink::new("base", "Network settings", GalleryNoop.boxed_clone())
                    .into_any_element(),
            ],
        ),
        p.row(
            "t_sm",
            [
                TlLink::new("small", "Needs setup", GalleryNoop.boxed_clone())
                    .text_size(p.m.text.sm)
                    .into_any_element(),
            ],
        ),
        p.row(
            "Disabled",
            [
                TlLink::new("disabled", "Additional setup…", GalleryNoop.boxed_clone())
                    .disabled("The helper is not installed.")
                    .into_any_element(),
            ],
        ),
        p.row(
            "In a sentence",
            [sentence
                .flex()
                .flex_row()
                .gap(p.m.sp[2])
                .child("Parent:")
                .child(TlLink::new("ppid", "1873", GalleryNoop.boxed_clone()))
                .child("(plasmashell)")
                .into_any_element()],
        ),
    ]
}

fn page_icons(p: &mut PageCx) -> Vec<AnyElement> {
    let m = p.m;
    let cell = |el: AnyElement, name: String| {
        let mut text = div();
        *text.text_style() = t_xs(&m);
        div()
            .w(px(96. * m.ui_scale))
            .flex()
            .flex_col()
            .items_center()
            .gap(m.sp[2])
            .py(m.sp[3])
            .child(el)
            .child(text.text_color(p.t.text_secondary).child(name))
            .into_any_element()
    };
    let grid = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .children(TlIcon::ALL.iter().map(|&name| {
            cell(
                icon(name, m.icon.md, p.t.text_primary).into_any_element(),
                format!("{name:?}"),
            )
        }));
    let sizes = [
        ("xs 12", m.icon.xs),
        ("sm 14", m.icon.sm),
        ("md 16", m.icon.md),
        ("lg 20", m.icon.lg),
    ]
    .map(|(label, size)| {
        cell(
            icon(TlIcon::Info, size, p.t.text_primary).into_any_element(),
            label.into(),
        )
    });
    let status = [
        p.t.status.success,
        p.t.status.warning,
        p.t.status.danger,
        p.t.status.info,
    ];
    let tinted = [
        TlIcon::CircleCheck,
        TlIcon::TriangleAlert,
        TlIcon::CircleX,
        TlIcon::Info,
    ]
    .into_iter()
    .zip(status)
    .map(|(name, color)| icon(name, m.icon.md, color).into_any_element());
    let app = AppIcon::ALL.map(|a| {
        cell(
            Icon::new(a)
                .size(m.icon.lg)
                .text_color(p.t.text_primary)
                .into_any_element(),
            format!("{a:?}"),
        )
    });
    vec![
        p.heading("Lucide icons at i_md in text_primary"),
        grid.into_any_element(),
        p.row("Sizes", sizes),
        p.row("Status colours", tinted),
        p.row("App SVGs", app),
    ]
}

fn page_text(p: &mut PageCx, window: &mut Window, cx: &mut App) -> Vec<AnyElement> {
    let m = p.m;
    let t = p.t.clone();
    let styled = |style: gpui_kit::TextStyleRefinement, text: String| {
        let mut el = div();
        *el.text_style() = style;
        el.whitespace_nowrap().child(text).into_any_element()
    };
    let sizes = [
        ("t_xs 11", m.text.xs),
        ("t_sm 12", m.text.sm),
        ("t_base 13", m.text.base),
        ("t_md 14", m.text.md),
        ("t_lg 16", m.text.lg),
        ("t_xl 20", m.text.xl),
    ];
    let mut out: Vec<AnyElement> =
        vec![
            p.row(
                "Sizes",
                sizes.map(|(name, size)| {
                    styled(label_style(&m, size, FontWeight::NORMAL), name.into())
                }),
            ),
        ];
    out.push(
        p.row(
            "Weights",
            [
                ("Regular 400", FontWeight::NORMAL),
                ("Medium 500", FontWeight::MEDIUM),
                ("SemiBold 600", FontWeight::SEMIBOLD),
            ]
            .map(|(name, weight)| styled(label_style(&m, m.text.base, weight), name.into())),
        ),
    );
    out.push(
        p.row(
            "Mono",
            [div()
                .font_family(MONO_FAMILY)
                .text_size(m.text.base)
                .child("Noto Sans Mono 0123456789 /proc/self")
                .into_any_element()],
        ),
    );

    // Each digit repeated, grouped with thin spaces. With `tnum` every row has the same width,
    // so the right-aligned column lines up; the proportional column beside it does not.
    let column = |numeric: bool| {
        div()
            .w(px(96. * m.ui_scale))
            .flex()
            .flex_col()
            .items_end()
            .children((0..=9u64).map(|d| {
                let mut s = String::new();
                units::count(&mut s, d * 111_111);
                let style = if numeric {
                    numeric_style(&m, m.text.base)
                } else {
                    label_style(&m, m.text.base, FontWeight::NORMAL)
                };
                styled(style, s)
            }))
            .into_any_element()
    };
    out.push(p.row(
        "Digits 0 to 9",
        [
            column(true),
            column(false),
            caption(&m, t.text_secondary, "left: tnum, right: proportional").into_any_element(),
        ],
    ));
    out.push(p.row(
        "Grouping",
        [0, 999, 1_000, 12_345, 1_234_567, u64::MAX].map(|n| {
            let mut s = String::new();
            units::count(&mut s, n);
            styled(numeric_style(&m, m.text.base), s)
        }),
    ));

    if !cx.has_global::<TextMetricsCache>() {
        cx.set_global(TextMetricsCache::default());
    }
    let paths = [
        ("/usr/lib/firefox/firefox-bin", 160.),
        (
            "/home/user/.local/share/Steam/steamapps/common/Proton 9.0/files/bin/wine64-preloader",
            320.,
        ),
        (
            "/usr/lib/x86_64-linux-gnu/libQt6WebEngineCore.so.6.8.2",
            220.,
        ),
    ];
    let size = m.text.base;
    let cut: Vec<(SharedString, f32)> = cx.update_global::<TextMetricsCache, _>(|cache, cx| {
        cache.clear_if_stale(m.style_version);
        paths
            .iter()
            .map(|&(path, w)| {
                let mut width_of = |s: &str| {
                    s.chars().fold(px(0.), |sum, ch| {
                        sum + cache.char_advance(ch, size, window, cx)
                    })
                };
                (middle_ellipsis(path, px(w * m.ui_scale), &mut width_of), w)
            })
            .collect()
    });
    for (text, w) in cut {
        out.push(
            p.row(
                &format!("Middle cut {w:.0} px"),
                [div()
                    .w(px(w * m.ui_scale))
                    .border_1()
                    .border_color(t.divider)
                    .child(styled(t_base(&m), text.to_string()))
                    .into_any_element()],
            ),
        );
    }
    out
}

/// Draws `child` with `tl` as the tasklens theme and `kit` as the kit theme. Both are swapped
/// into the globals before each phase of the child and swapped back after it.
struct ThemeScope {
    id: ElementId,
    tl: Option<Arc<TlTheme>>,
    kit: Theme,
    child: AnyElement,
}

impl ThemeScope {
    /// Exchanges the scope's themes with the globals. Calling it twice restores both.
    fn swap(&mut self, cx: &mut App) {
        std::mem::swap(&mut cx.global_mut::<ThemeStore>().preview, &mut self.tl);
        std::mem::swap(Theme::global_mut(cx), &mut self.kit);
    }
}

impl IntoElement for ThemeScope {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for ThemeScope {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        self.swap(cx);
        let layout_id = self.child.request_layout(window, cx);
        self.swap(cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.swap(cx);
        self.child.prepaint(window, cx);
        self.swap(cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.swap(cx);
        self.child.paint(window, cx);
        self.swap(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Modules in `components/` that are helpers, not components, and have no page.
    const NOT_COMPONENTS: [&str; 3] = ["focus", "motion", "gallery"];

    /// Module names from the `pub mod` lines of `components/mod.rs`.
    fn component_modules() -> Vec<&'static str> {
        include_str!("mod.rs")
            .lines()
            .filter_map(|line| {
                let rest = &line[line.find("pub mod ")? + "pub mod ".len()..];
                rest.strip_suffix(';')
            })
            .collect()
    }

    #[test]
    fn every_component_has_a_page() {
        let modules = component_modules();
        assert!(modules.contains(&"button") && modules.contains(&"gallery"));
        for module in &modules {
            assert!(
                NOT_COMPONENTS.contains(module) || GALLERY_PAGES.contains(module),
                "components::{module} has no gallery page"
            );
        }
        // Every page is a component module, except `text`, which shows `crate::text`.
        for page in GALLERY_PAGES {
            assert!(
                *page == "text" || modules.contains(page),
                "gallery page {page} is not a module in components/"
            );
        }
    }
}
