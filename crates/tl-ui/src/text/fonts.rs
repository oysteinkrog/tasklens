//! Bundled Noto faces and the start-up check that each one resolves (ADR-009, UI-FND-050).
//!
//! The four static faces are embedded in the binary and added to the text system before the first
//! window. GPUI drops faces without an `m` glyph (zed#61660) and renders variable fonts at regular
//! weight (zed#64783), so every face is checked after loading. A failed UI face makes the UI fall
//! back to `.SystemUIFont`; a failed mono face falls back to the kit's mono chain (UI-FND-002).

use std::borrow::Cow;

use gpui_kit::{App, Font, FontId, FontWeight, Global, TextSystem, font, px};
use smallvec::SmallVec;

/// Family of the bundled UI faces.
pub const UI_FAMILY: &str = "Noto Sans";
/// Family of the bundled monospace face.
pub const MONO_FAMILY: &str = "Noto Sans Mono";
/// GPUI's name for the platform UI font, used when a bundled UI face fails.
pub const SYSTEM_UI_FAMILY: &str = ".SystemUIFont";

const BUNDLED_FACES: [&[u8]; 4] = [
    include_bytes!("../../../../assets/fonts/NotoSans-Regular.ttf"),
    include_bytes!("../../../../assets/fonts/NotoSans-Medium.ttf"),
    include_bytes!("../../../../assets/fonts/NotoSans-SemiBold.ttf"),
    include_bytes!("../../../../assets/fonts/NotoSansMono-Regular.ttf"),
];

/// One bundled face as the start-up check expects to find it.
struct ExpectedFace {
    /// Name logged and listed in [`FontLoadReport::failed`] when the check fails.
    name: &'static str,
    family: &'static str,
    weight: FontWeight,
    /// Advance width of `m` in font units (1000 per em), read from the bundled file's `hmtx`
    /// table. The weights differ here, so it tells the bundled face apart from the nearest
    /// weight the text system picks when a face is missing.
    m_advance: f32,
}

/// The UI weights tl-ui uses (400, 500, 600).
const UI_FACES: [ExpectedFace; 3] = [
    ExpectedFace {
        name: "Noto Sans Regular",
        family: UI_FAMILY,
        weight: FontWeight::NORMAL,
        m_advance: 935.,
    },
    ExpectedFace {
        name: "Noto Sans Medium",
        family: UI_FAMILY,
        weight: FontWeight::MEDIUM,
        m_advance: 947.,
    },
    ExpectedFace {
        name: "Noto Sans SemiBold",
        family: UI_FAMILY,
        weight: FontWeight::SEMIBOLD,
        m_advance: 960.,
    },
];
const MONO_FACE: ExpectedFace = ExpectedFace {
    name: "Noto Sans Mono Regular",
    family: MONO_FAMILY,
    weight: FontWeight::NORMAL,
    m_advance: 600.,
};

/// Result of [`load_bundled_fonts`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FontLoadReport {
    /// True only when all three Noto Sans weights resolve to their bundled face.
    pub ui_ok: bool,
    /// True when Noto Sans Mono 400 resolves.
    pub mono_ok: bool,
    /// Names of the faces that failed the check.
    pub failed: SmallVec<[&'static str; 4]>,
}

/// The last [`FontLoadReport`], for the About window note "Bundled font failed to load" (E21).
#[derive(Clone, Debug)]
pub struct BundledFontStatus(pub FontLoadReport);

impl Global for BundledFontStatus {}

/// Adds the bundled faces to the text system, checks each one and stores the report as
/// [`BundledFontStatus`]. Call once before the first window opens.
pub fn load_bundled_fonts(cx: &mut App) -> FontLoadReport {
    let text_system = cx.text_system().clone();
    let faces = BUNDLED_FACES
        .iter()
        .map(|bytes| Cow::Borrowed(*bytes))
        .collect();
    if let Err(err) = text_system.add_fonts(faces) {
        tracing::error!(target: "tasklens::ui", error = %err, "bundled fonts could not be added");
    }

    let mut failed = SmallVec::new();
    for face in &UI_FACES {
        if resolve_face(&text_system, face).is_none() {
            tracing::error!(target: "tasklens::ui", face = face.name, "bundled font failed to load");
            failed.push(face.name);
        }
    }
    let ui_ok = failed.is_empty();

    let mono_ok = resolve_face(&text_system, &MONO_FACE).is_some();
    if !mono_ok {
        tracing::warn!(target: "tasklens::ui", face = MONO_FACE.name, "bundled font failed to load");
        failed.push(MONO_FACE.name);
    }

    let report = FontLoadReport {
        ui_ok,
        mono_ok,
        failed,
    };
    cx.set_global(BundledFontStatus(report.clone()));
    report
}

/// The family the UI uses: Noto Sans when all its weights loaded, else the platform UI font.
pub fn effective_ui_family(report: &FontLoadReport) -> &'static str {
    if report.ui_ok {
        UI_FAMILY
    } else {
        SYSTEM_UI_FAMILY
    }
}

/// Resolves `face` and returns its font ID only when the text system picked that face.
///
/// `TextSystem::font_id` is private in gpui-pre 0.3.7 and `resolve_font` falls back without
/// saying so, in two ways. A missing family falls back to the default stack; the text system
/// caches the failed lookup as an error, so the returned ID then maps back to a font of another
/// family. A missing weight in a present family resolves to the nearest weight; the `m` advance
/// catches that.
fn resolve_face(text_system: &TextSystem, face: &ExpectedFace) -> Option<FontId> {
    let requested = Font {
        weight: face.weight,
        ..font(face.family)
    };
    let id = text_system.resolve_font(&requested);
    let resolved = text_system.get_font_for_id(id)?;
    if resolved.family.as_ref() != face.family {
        return None;
    }
    // At 1000 px the advance in pixels is the advance in font units.
    let m_advance = text_system.advance(id, px(1000.), 'm').ok()?.width;
    ((f32::from(m_advance) - face.m_advance).abs() < 0.5).then_some(id)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use gpui_kit::{
        FontFeatures, TestAppContext, TestDispatcher, TextRun, WindowTextSystem, black,
    };

    use super::*;

    /// A test context backed by the real Linux text system (cosmic-text). The default
    /// `TestAppContext` uses GPUI's no-op text system, which resolves every font.
    fn cosmic_cx() -> TestAppContext {
        let text_system = gpui_kit::platform::current_platform(true).text_system();
        TestAppContext::build_with_text_system(TestDispatcher::new(0), None, text_system)
    }

    fn tnum(family: &str) -> Font {
        Font {
            features: FontFeatures(Arc::new(vec![("tnum".into(), 1)])),
            ..font(family.to_owned())
        }
    }

    #[test]
    fn three_weights_load() {
        let cx = cosmic_cx();
        cx.update(|cx| {
            let report = load_bundled_fonts(cx);
            assert!(report.ui_ok, "failed faces: {:?}", report.failed);
            let text_system = cx.text_system().clone();
            let ids: Vec<_> = UI_FACES
                .iter()
                .map(|face| {
                    resolve_face(&text_system, face)
                        .unwrap_or_else(|| panic!("{} did not resolve", face.name))
                })
                .collect();
            assert_ne!(ids[0], ids[1]);
            assert_ne!(ids[1], ids[2]);
            assert_ne!(ids[0], ids[2]);
            assert!(cx.global::<BundledFontStatus>().0.ui_ok);
        });
    }

    #[test]
    fn mono_loads() {
        let cx = cosmic_cx();
        cx.update(|cx| {
            let report = load_bundled_fonts(cx);
            assert!(report.mono_ok, "failed faces: {:?}", report.failed);
            let text_system = cx.text_system().clone();
            assert!(resolve_face(&text_system, &MONO_FACE).is_some());
        });
    }

    #[test]
    fn tnum_digits_equal_width() {
        let cx = cosmic_cx();
        cx.update(|cx| {
            load_bundled_fonts(cx);
            let window_text = WindowTextSystem::new(cx.text_system().clone());
            let digits = "0123456789";
            for family in [UI_FAMILY, MONO_FAMILY] {
                let run = TextRun {
                    len: digits.len(),
                    font: tnum(family),
                    color: black(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let layout = window_text.layout_line(digits, px(13.), &[run], None);
                let expected_id = cx.text_system().resolve_font(&tnum(family));
                assert!(
                    layout.runs.iter().all(|r| r.font_id == expected_id),
                    "{family}: digits were shaped with a fallback face"
                );
                let mut edges: Vec<f32> = layout
                    .runs
                    .iter()
                    .flat_map(|r| r.glyphs.iter())
                    .map(|g| f32::from(g.position.x))
                    .collect();
                assert_eq!(edges.len(), digits.len(), "{family}: one glyph per digit");
                edges.push(f32::from(layout.width));
                let advances: Vec<f32> = edges.windows(2).map(|w| w[1] - w[0]).collect();
                let first = advances[0];
                assert!(first > 0.0, "{family}: zero digit advance");
                for (digit, advance) in digits.chars().zip(&advances) {
                    assert!(
                        (advance - first).abs() < 0.01,
                        "{family}: digit {digit} advance {advance} differs from {first}"
                    );
                }
            }
        });
    }

    #[test]
    fn fallback_family_on_failure() {
        let failed = FontLoadReport {
            ui_ok: false,
            mono_ok: true,
            failed: SmallVec::from_slice(&["Noto Sans SemiBold"]),
        };
        assert_eq!(effective_ui_family(&failed), ".SystemUIFont");
        let ok = FontLoadReport {
            ui_ok: true,
            mono_ok: true,
            failed: SmallVec::new(),
        };
        assert_eq!(effective_ui_family(&ok), "Noto Sans");
    }

    #[test]
    fn no_bold_in_tl_ui() {
        // Built from two parts so this file does not match its own scan.
        let needle = concat!("FontWeight", "::BOLD");
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut hits = Vec::new();
        let mut dirs = vec![src];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).expect("read tl-ui source dir") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let text = std::fs::read_to_string(&path).expect("read source file");
                    if text.contains(needle) {
                        hits.push(path);
                    }
                }
            }
        }
        assert!(hits.is_empty(), "weight 700 used in: {hits:?}");
    }
}
