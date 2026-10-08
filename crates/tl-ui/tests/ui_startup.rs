//! Startup Apps delegate tests (tasklens-9o0.6, UI-SYS-041, UI-SYS-042).

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::{App, TestAppContext, px};
use tl_model::settings::{AppearanceSettings, ThemeSettings};
use tl_model::snapshot::{
    ComponentHeader, Impact, ImpactDetail, Snapshot, StartupEntry, StartupKind, StartupSnapshot,
    StartupStatus,
};
use tl_model::table::{StartupColumn, StartupRef, StartupSource, SysColumnId, SysNodeId, SysRow};
use tl_model::time::BootId;
use tl_model::validity::Validity;
use tl_ui::pages::startup::delegate::{
    STARTUP_COLUMNS, StartupDelegate, StartupWords, default_width, id,
    impact_tooltip_lines as tooltip_lines, sys_status,
};
use tl_ui::pages::sys_status::{DotStyle, LabelTone, cell_colors, status_look};
use tl_ui::pages::sys_table::SysDelegate as _;
use tl_ui::table::cells::CellOut;
use tl_ui::table::columns::TableColumn as _;
use tl_ui::table::tree_table::{NameIcon, TreeTableDelegate as _};
use tl_ui::theme::{TlTheme, current};

fn setup(cx: &mut App) {
    gpui_kit::init(cx);
    tl_ui::theme::store::init(
        cx,
        &ThemeSettings::default(),
        &AppearanceSettings::default(),
        Vec::new(),
    );
}

fn entry(name: &str, kind: StartupKind) -> StartupEntry {
    StartupEntry {
        id: StartupRef {
            source: StartupSource::AutostartUser,
            id: Arc::from(format!("{name}.desktop")),
        },
        name: Arc::from(name),
        icon_path: None,
        publisher: Some(Arc::from("app 1.0")),
        status: StartupStatus::Enabled,
        impact: Impact::Low,
        impact_detail: None,
        state_code: None,
        kind,
        location: Arc::from("/home/u/.config/autostart"),
        command: Arc::from("/usr/bin/app --tray"),
        source_path: Arc::from("/home/u/.config/autostart/app.desktop"),
        can_toggle: true,
        can_delete: true,
        can_run: true,
        overridden_by_user: false,
    }
}

/// A delegate with `entries` installed as rows 0.., `HOME=/home/u`, and the theme.
fn delegate(
    cx: &mut TestAppContext,
    entries: Vec<StartupEntry>,
) -> (StartupDelegate, Arc<TlTheme>) {
    cx.update(|cx| {
        setup(cx);
        let mut d = StartupDelegate::new(Arc::from("/home/u"), cx);
        let rows: Vec<SysRow> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| SysRow {
                node: SysNodeId::Startup(e.id.clone()),
                depth: 0,
                expanded: false,
                has_children: false,
                record: Some(i as u32),
            })
            .collect();
        let mut snap = Snapshot::empty(BootId(1), 1);
        snap.startup = Arc::new(StartupSnapshot {
            header: ComponentHeader {
                version: 1,
                validity: Validity::Valid,
                ..ComponentHeader::default()
            },
            rows: rows.into(),
            query_generation: 1,
            entries: entries.into(),
            ..StartupSnapshot::default()
        });
        d.install(&Arc::new(snap));
        (d, current(cx).clone())
    })
}

fn cell(d: &StartupDelegate, ix: usize, c: StartupColumn) -> CellOut {
    let mut out = CellOut {
        want_tooltip: true,
        ..CellOut::default()
    };
    d.cell(ix, id(c), &mut out);
    out
}

fn text(out: &CellOut) -> &str {
    out.arc.as_deref().unwrap_or("")
}

#[gpui_kit::test]
fn startup_columns_widths(cx: &mut TestAppContext) {
    let (d, _) = delegate(cx, Vec::new());
    let ids: Vec<SysColumnId> = STARTUP_COLUMNS.map(id).to_vec();
    assert_eq!(d.columns().order, ids);
    let expect = [
        (StartupColumn::Name, 220., 120.),
        (StartupColumn::Publisher, 180., 60.),
        (StartupColumn::Status, 90., 80.),
        (StartupColumn::Impact, 100., 80.),
        (StartupColumn::Type, 190., 60.),
        (StartupColumn::Location, 260., 60.),
        (StartupColumn::Command, 170., 60.),
    ];
    for (c, width, min) in expect {
        assert_eq!(default_width(c), width, "{c:?}");
        assert_eq!(d.column_spec(id(c)).default_width, px(width), "{c:?}");
        assert_eq!(id(c).min_width(), Some(min), "{c:?}");
        assert_eq!(d.column_spec(id(c)).hideable, c != StartupColumn::Name);
    }
    let labels: Vec<&str> = STARTUP_COLUMNS
        .iter()
        .map(|&c| d.column_spec(id(c)).label.as_ref())
        .collect();
    assert_eq!(
        labels,
        [
            "Name",
            "Publisher",
            "Status",
            "Startup impact",
            "Type",
            "Location",
            "Command"
        ]
    );
    assert_eq!(d.sort().column, id(StartupColumn::Status));
    assert!(d.sort().ascending);
}

#[gpui_kit::test]
fn startup_row_fallback_icons(cx: &mut TestAppContext) {
    let mut with_icon = entry("d", StartupKind::AutostartUser);
    with_icon.icon_path = Some(Arc::from("/usr/share/icons/d.png"));
    let (d, _) = delegate(
        cx,
        vec![
            entry("a", StartupKind::AutostartSystem),
            entry("b", StartupKind::UserUnit),
            entry("c", StartupKind::PlasmaPreStartup),
            with_icon,
        ],
    );
    let icon = |ix| d.name_cell(ix).icon;
    assert!(matches!(icon(0), NameIcon::Lucide(IconName::AppWindow)));
    assert!(matches!(icon(1), NameIcon::Lucide(IconName::Cog)));
    assert!(matches!(icon(2), NameIcon::Lucide(IconName::FileCode)));
    assert!(matches!(icon(3), NameIcon::File(p) if &*p == "/usr/share/icons/d.png"));
    assert_eq!(d.name_cell(0).primary.as_ref(), "a");
}

#[gpui_kit::test]
fn startup_overridden_row_secondary(cx: &mut TestAppContext) {
    let mut e = entry("a", StartupKind::AutostartSystem);
    e.overridden_by_user = true;
    e.impact = Impact::High;
    let (d, theme) = delegate(cx, vec![e, entry("b", StartupKind::AutostartUser)]);
    for c in STARTUP_COLUMNS {
        assert_eq!(
            cell(&d, 0, c).text_color,
            Some(theme.text_secondary),
            "{c:?}"
        );
    }
    assert_eq!(cell(&d, 1, StartupColumn::Command).text_color, None);
    assert_eq!(cell(&d, 1, StartupColumn::Name).text_color, None);
}

#[gpui_kit::test]
fn startup_status_cell_looks(cx: &mut TestAppContext) {
    use LabelTone::{Primary, Secondary, Success, Warning};
    let cases = [
        (
            StartupStatus::Enabled,
            DotStyle::Filled(Success),
            Primary,
            "Enabled",
        ),
        (
            StartupStatus::Disabled,
            DotStyle::Ring(Secondary),
            Secondary,
            "Disabled",
        ),
        (
            StartupStatus::Masked,
            DotStyle::Filled(Warning),
            Primary,
            "Masked",
        ),
        (
            StartupStatus::NotInThisDesktop,
            DotStyle::Ring(Secondary),
            Secondary,
            "Not in this desktop",
        ),
        (
            StartupStatus::MissingProgram,
            DotStyle::Filled(Warning),
            Primary,
            "Missing program",
        ),
    ];
    let entries = cases
        .iter()
        .map(|(s, ..)| {
            let mut e = entry(&format!("{s:?}"), StartupKind::AutostartUser);
            e.status = *s;
            e
        })
        .collect();
    let (d, theme) = delegate(cx, entries);
    for (ix, (s, dot, label, word)) in cases.into_iter().enumerate() {
        let look = status_look(sys_status(s));
        assert_eq!(look.dot, dot, "{s:?}");
        assert_eq!(look.label, label, "{s:?}");
        let out = cell(&d, ix, StartupColumn::Status);
        assert_eq!(text(&out), word);
        let secondary = (label == Secondary).then_some(theme.text_secondary);
        assert_eq!(out.text_color, secondary, "{s:?}");
    }
    let enabled = cell_colors(
        status_look(sys_status(StartupStatus::Enabled)),
        false,
        false,
        &theme,
    );
    assert_eq!(enabled.dot, Some(theme.status.success));
}

#[gpui_kit::test]
fn impact_colors(cx: &mut TestAppContext) {
    let impacts = [
        Impact::High,
        Impact::Medium,
        Impact::Low,
        Impact::None,
        Impact::NotMeasured,
    ];
    let entries = impacts
        .iter()
        .map(|i| {
            let mut e = entry(&format!("{i:?}"), StartupKind::UserUnit);
            e.impact = *i;
            e
        })
        .collect();
    let (d, theme) = delegate(cx, entries);
    let expect = [
        (Some(theme.status.warning), "High"),
        (None, "Medium"),
        (None, "Low"),
        (Some(theme.text_secondary), "None"),
        (Some(theme.text_secondary), "Not measured"),
    ];
    for (ix, (color, word)) in expect.into_iter().enumerate() {
        let out = cell(&d, ix, StartupColumn::Impact);
        assert_eq!(out.text_color, color, "{word}");
        assert_eq!(text(&out), word);
    }
}

#[test]
fn impact_tooltip_lines() {
    let w = StartupWords::en();
    let detail = ImpactDetail {
        activation_ms: 412,
        started_after_login_ms: 3200,
        cpu_nsec: Some(1_000_000_000),
    };
    let three = [
        "Activation time: 412 ms",
        "Started 3.2 s after login",
        "CPU time: 0:00:01",
    ];
    let lines = tooltip_lines(Impact::High, Some(&detail), false, &w);
    assert_eq!(lines.as_slice(), three);
    let lines = tooltip_lines(Impact::High, Some(&detail), true, &w);
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[..3], three);
    assert_eq!(
        lines[3],
        "For this unit type only the time to start the program is measured."
    );
    assert_eq!(
        tooltip_lines(Impact::None, None, false, &w).as_slice(),
        ["Disabled entries do not run at login."]
    );
    assert_eq!(
        tooltip_lines(Impact::NotMeasured, None, false, &w).as_slice(),
        ["This entry did not start through systemd in this session."]
    );
    assert!(tooltip_lines(Impact::Low, None, false, &w).is_empty());
}

#[gpui_kit::test]
fn impact_tooltip_on_hovered_cell(cx: &mut TestAppContext) {
    let mut e = entry("a", StartupKind::UserUnit);
    e.impact = Impact::Medium;
    e.impact_detail = Some(ImpactDetail {
        activation_ms: 412,
        started_after_login_ms: 3200,
        cpu_nsec: Some(1_000_000_000),
    });
    let (d, _) = delegate(cx, vec![e]);
    let out = cell(&d, 0, StartupColumn::Impact);
    assert_eq!(
        out.tooltip.as_deref(),
        Some("Activation time: 412 ms\nStarted 3.2 s after login\nCPU time: 0:00:01")
    );
    let mut quiet = CellOut::default();
    d.cell(0, id(StartupColumn::Impact), &mut quiet);
    assert_eq!(quiet.tooltip, None, "no tooltip off the hovered row");
}

#[gpui_kit::test]
fn startup_location_home_tilde(cx: &mut TestAppContext) {
    let mut system = entry("b", StartupKind::AutostartSystem);
    system.location = Arc::from("/etc/xdg/autostart");
    let (d, theme) = delegate(cx, vec![entry("a", StartupKind::AutostartUser), system]);
    let out = cell(&d, 0, StartupColumn::Location);
    assert_eq!(text(&out), "~/.config/autostart");
    assert_eq!(out.text_color, Some(theme.text_secondary));
    assert_eq!(
        text(&cell(&d, 1, StartupColumn::Location)),
        "/etc/xdg/autostart"
    );
}

#[gpui_kit::test]
fn startup_name_tooltip_two_lines(cx: &mut TestAppContext) {
    let (d, _) = delegate(cx, vec![entry("a", StartupKind::AutostartUser)]);
    let lines = d.name_tooltip_lines(0).expect("row 0");
    assert_eq!(lines[0].as_ref(), "/usr/bin/app --tray");
    assert_eq!(lines[1].as_ref(), "/home/u/.config/autostart/app.desktop");
    assert_eq!(
        d.accessible_name(0).as_deref(),
        Some("a, Enabled, Autostart (user)")
    );
    assert!(d.name_tooltip_lines(1).is_none());
}

#[gpui_kit::test]
fn startup_publisher_user_secondary(cx: &mut TestAppContext) {
    let mut user = entry("a", StartupKind::UserUnit);
    user.publisher = Some(Arc::from("User"));
    let mut blank = entry("b", StartupKind::UserUnit);
    blank.publisher = None;
    let (d, theme) = delegate(
        cx,
        vec![user, blank, entry("c", StartupKind::AutostartUser)],
    );
    let out = cell(&d, 0, StartupColumn::Publisher);
    assert_eq!(text(&out), "User");
    assert_eq!(out.text_color, Some(theme.text_secondary));
    assert_eq!(text(&cell(&d, 1, StartupColumn::Publisher)), "");
    let out = cell(&d, 2, StartupColumn::Publisher);
    assert_eq!(text(&out), "app 1.0");
    assert_eq!(out.text_color, None);
}
