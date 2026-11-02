//! Headless screenshot helpers and the golden-image compare for `tests/ui/render_*.rs` (ADR-013,
//! R-291).
//!
//! `RenderHarness` draws a view with GPUI's `HeadlessAppContext` and the wgpu headless renderer.
//! Headless windows always have device scale 2.0, so a capture is twice the logical window size
//! in each direction. `set_scale` sets tasklens' own `ui_scale` (`UiMetrics`), and `render`
//! grows the window by the same factor, so a 100 × 40 view captures as 200 × 80 at 1.0 and
//! 220 × 88 at 1.1.
//!
//! The harness knows nothing of the crate under test (ADR-020: the testkit depends only on
//! tl-model and gpui-kit). That crate passes a `RenderSetup` with its assets and its font and
//! theme setup, and gives `assert_golden` its own golden directory.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use gpui_kit::{App, AssetSource, Entity, HeadlessAppContext, Pixels, Render, Size, Window, size};
pub use image::{Rgba, RgbaImage};

/// Where a failed compare writes `<name>.actual.png` and `<name>.diff.png`.
pub const DIFF_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/golden-diff");
/// At most 0.1% of pixels may differ.
pub const MAX_DIFF_FRACTION: f64 = 0.001;
/// A pixel differs when any RGBA channel differs by more than 2/255.
pub const CHANNEL_TOLERANCE: u8 = 2;

/// Lavapipe ICD file names: Ubuntu's, then Arch's (R-266).
const LAVAPIPE_ICDS: [&str; 2] = [
    "/usr/share/vulkan/icd.d/lvp_icd.x86_64.json",
    "/usr/share/vulkan/icd.d/lvp_icd.json",
];

/// Runs `body` with the Vulkan loader pointed at Mesa lavapipe.
///
/// When `VK_ICD_FILENAMES` is set, `body` runs in this process. Otherwise the test binary runs
/// the test `test_path` again in a child process with `VK_ICD_FILENAMES` set to the lavapipe ICD,
/// because setting the variable in a running test process is not thread safe. When no lavapipe
/// ICD exists, the test is skipped with a printed reason. `test_path` is the full test name, for
/// example `render_support::solid_fill_matches_window_bg`.
pub fn run_on_lavapipe(test_path: &str, body: impl FnOnce()) {
    if std::env::var_os("VK_ICD_FILENAMES").is_some() {
        body();
        return;
    }
    let Some(icd) = LAVAPIPE_ICDS.iter().find(|p| Path::new(p).is_file()) else {
        eprintln!("skipped {test_path}: no lavapipe ICD in /usr/share/vulkan/icd.d");
        return;
    };
    let exe = std::env::current_exe().expect("test binary path");
    let output = Command::new(exe)
        .args([test_path, "--exact", "--nocapture", "--test-threads=1"])
        .env("VK_ICD_FILENAMES", icd)
        .output()
        .expect("run the test again with lavapipe");
    eprint!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    assert!(
        output.status.success(),
        "{test_path} failed with VK_ICD_FILENAMES={icd}"
    );
}

/// What `RenderHarness` needs from the crate under test.
pub struct RenderSetup {
    /// The asset source of the app.
    pub assets: Arc<dyn AssetSource>,
    /// Loads the fonts, inits gpui-kit and the theme store with default appearance settings.
    /// Returns their `ui_scale_pct`.
    pub init: fn(&mut App) -> u16,
    /// Switches the theme at once, without a crossfade. Panics on an unknown name.
    pub set_theme: fn(&str, &mut App),
    /// Applies the metrics of default appearance settings with this `ui_scale_pct`.
    pub apply_metrics: fn(u16, &mut App),
}

/// A headless app set up by a `RenderSetup` with default appearance settings, and reduced motion
/// forced on so animated widths and fades render their final state.
pub struct RenderHarness {
    cx: HeadlessAppContext,
    setup: RenderSetup,
    /// The appearance `ui_scale_pct`; the other appearance settings stay at their defaults.
    ui_scale_pct: u16,
}

impl RenderHarness {
    /// Builds the app. Panics when no headless renderer is available.
    pub fn new(setup: RenderSetup) -> Self {
        match gpui_kit::platform::current_headless_renderer() {
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {
                panic!("headless renderer unavailable: set VK_ICD_FILENAMES to lavapipe")
            }
        }
        let text_system = gpui_kit::platform::current_platform(true).text_system();
        let mut cx = HeadlessAppContext::with_platform(text_system, setup.assets.clone(), || {
            gpui_kit::platform::current_headless_renderer()
        });
        let ui_scale_pct = cx.update(|cx| {
            let ui_scale_pct = (setup.init)(cx);
            // The UI ANDs `motion.animations` with this, so it stands in for
            // `motion.animations = false`.
            cx.set_reduce_motion(true);
            ui_scale_pct
        });
        cx.run_until_parked();
        Self {
            cx,
            setup,
            ui_scale_pct,
        }
    }

    /// Switches the theme at once, without a crossfade. Panics on an unknown name.
    pub fn set_theme(&mut self, name: &str) {
        let set_theme = self.setup.set_theme;
        self.cx.update(|cx| set_theme(name, cx));
        self.cx.run_until_parked();
    }

    /// Sets `ui_scale` (ADR-013 renders at 1.0 and 1.1). This is tasklens' metrics scale, not
    /// the device scale, which stays 2.0 (R-291).
    pub fn set_scale(&mut self, scale: f32) {
        self.ui_scale_pct = (scale * 100.0).round() as u16;
        let ui_scale_pct = self.ui_scale_pct;
        let apply_metrics = self.setup.apply_metrics;
        self.cx.update(|cx| apply_metrics(ui_scale_pct, cx));
        self.cx.run_until_parked();
    }

    /// The `ui_scale` set with `set_scale`.
    pub fn scale(&self) -> f32 {
        f32::from(self.ui_scale_pct) / 100.0
    }

    /// Opens a window of `size × ui_scale` logical px, draws the view and captures it. The image
    /// is twice that size in device pixels. The window is closed afterwards.
    pub fn render<V: Render>(
        &mut self,
        size_at_1x: Size<Pixels>,
        build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
    ) -> RgbaImage {
        let s = self.scale();
        let window_size = size(size_at_1x.width * s, size_at_1x.height * s);
        let window = self
            .cx
            .open_window(window_size, build)
            .expect("open headless window");
        self.cx.run_until_parked();
        let image = self
            .cx
            .capture_screenshot(window.into())
            .expect("capture headless window");
        self.cx
            .update_window(window.into(), |_, window, _| window.remove_window())
            .expect("close headless window");
        self.cx.run_until_parked();
        image
    }
}

/// Compares `img` with `<golden_dir>/<name>.png`, or writes the golden when
/// `TL_UPDATE_GOLDEN=1`. On a failed compare it writes `target/golden-diff/<name>.actual.png`
/// and `<name>.diff.png` (differing pixels red on black) and panics with the count.
pub fn assert_golden(golden_dir: &Path, name: &str, img: &RgbaImage) {
    let update = std::env::var("TL_UPDATE_GOLDEN").is_ok_and(|v| v == "1");
    if let Err(msg) = check_golden(golden_dir, Path::new(DIFF_DIR), name, img, update) {
        panic!("{msg}");
    }
}

/// `assert_golden` with explicit directories and update flag, returning the failure message.
fn check_golden(
    golden_dir: &Path,
    diff_dir: &Path,
    name: &str,
    img: &RgbaImage,
    update: bool,
) -> Result<(), String> {
    let golden_path = golden_dir.join(format!("{name}.png"));
    if update {
        save(img, &golden_path);
        return Ok(());
    }
    if !golden_path.is_file() {
        return Err(format!(
            "golden {name}.png missing; run with TL_UPDATE_GOLDEN=1"
        ));
    }
    let golden = image::open(&golden_path)
        .unwrap_or_else(|err| panic!("read {}: {err}", golden_path.display()))
        .to_rgba8();
    let actual_path = diff_dir.join(format!("{name}.actual.png"));
    if golden.dimensions() != img.dimensions() {
        save(img, &actual_path);
        return Err(format!(
            "golden {name}: size {:?} differs from the golden's {:?}; actual written to {}",
            img.dimensions(),
            golden.dimensions(),
            actual_path.display()
        ));
    }
    let (count, diff) = diff_pixels(&golden, img);
    let limit = MAX_DIFF_FRACTION * f64::from(img.width()) * f64::from(img.height());
    if count as f64 > limit {
        let diff_path = diff_dir.join(format!("{name}.diff.png"));
        save(img, &actual_path);
        save(&diff, &diff_path);
        return Err(format!(
            "golden {name}: {count} pixels differ by more than {CHANNEL_TOLERANCE}/255, limit \
             {limit}; see {} and {}",
            actual_path.display(),
            diff_path.display()
        ));
    }
    Ok(())
}

/// Counts pixels where any channel differs by more than `CHANNEL_TOLERANCE`, and returns an
/// image with those pixels red on black. Both images have the same size.
fn diff_pixels(golden: &RgbaImage, actual: &RgbaImage) -> (usize, RgbaImage) {
    let mut diff = RgbaImage::from_pixel(actual.width(), actual.height(), Rgba([0, 0, 0, 255]));
    let mut count = 0;
    for ((g, a), d) in golden.pixels().zip(actual.pixels()).zip(diff.pixels_mut()) {
        if g.0
            .iter()
            .zip(a.0)
            .any(|(g, a)| g.abs_diff(a) > CHANNEL_TOLERANCE)
        {
            count += 1;
            *d = Rgba([255, 0, 0, 255]);
        }
    }
    (count, diff)
}

fn save(img: &RgbaImage, path: &Path) {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .unwrap_or_else(|err| panic!("create {}: {err}", dir.display()));
    }
    img.save(path)
        .unwrap_or_else(|err| panic!("write {}: {err}", path.display()));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 100 × 100 grey image with `n` pixels changed by 10/255 in the red channel.
    fn with_changed(base: &RgbaImage, n: u32) -> RgbaImage {
        let mut img = base.clone();
        for i in 0..n {
            let p = img.get_pixel_mut(i % 100, i / 100);
            p.0[0] += 10;
        }
        img
    }

    #[test]
    fn golden_compare_tolerance() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let golden_dir = tmp.path().join("golden");
        let diff_dir = tmp.path().join("diff");
        let base = RgbaImage::from_pixel(100, 100, Rgba([100, 100, 100, 255]));
        check_golden(&golden_dir, &diff_dir, "base", &base, true).expect("write golden");

        // Equal, and within the channel tolerance everywhere.
        check_golden(&golden_dir, &diff_dir, "base", &base, false).expect("equal image");
        let near = RgbaImage::from_pixel(100, 100, Rgba([102, 98, 100, 255]));
        check_golden(&golden_dir, &diff_dir, "base", &near, false).expect("within 2/255");

        // 0.05% and 0.1% of 10,000 pixels pass.
        for n in [5, 10] {
            check_golden(
                &golden_dir,
                &diff_dir,
                "base",
                &with_changed(&base, n),
                false,
            )
            .unwrap_or_else(|msg| panic!("{n} changed pixels: {msg}"));
        }
        assert!(!diff_dir.join("base.diff.png").exists());

        // 0.2% fails and writes both images.
        let changed = with_changed(&base, 20);
        let msg = check_golden(&golden_dir, &diff_dir, "base", &changed, false)
            .expect_err("20 changed pixels must fail");
        assert!(msg.contains("20 pixels differ"), "{msg}");
        let actual = image::open(diff_dir.join("base.actual.png"))
            .expect("actual written")
            .to_rgba8();
        assert_eq!(actual, changed);
        let diff = image::open(diff_dir.join("base.diff.png"))
            .expect("diff written")
            .to_rgba8();
        let red = diff.pixels().filter(|p| p.0 == [255, 0, 0, 255]).count();
        let black = diff.pixels().filter(|p| p.0 == [0, 0, 0, 255]).count();
        assert_eq!((red, black), (20, 9_980));

        // A size change fails, and a missing golden names the fix.
        let small = RgbaImage::from_pixel(10, 10, Rgba([100, 100, 100, 255]));
        assert!(check_golden(&golden_dir, &diff_dir, "base", &small, false).is_err());
        let msg = check_golden(&golden_dir, &diff_dir, "absent", &base, false)
            .expect_err("missing golden");
        assert_eq!(
            msg,
            "golden absent.png missing; run with TL_UPDATE_GOLDEN=1"
        );
    }

    #[test]
    fn update_mode_writes_file() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let img = RgbaImage::from_pixel(4, 3, Rgba([1, 2, 3, 255]));
        check_golden(tmp.path(), tmp.path(), "nested/new", &img, true).expect("update passes");
        let written = image::open(tmp.path().join("nested/new.png"))
            .expect("golden written")
            .to_rgba8();
        assert_eq!(written, img);
    }
}
