//! Performance tab Overview page (E24, UI-PRF-017, D-022): the drawing parts.

use gpui_kit::px;
use tl_ui::pages::performance::overview::parts::{CoreBarsModel, core_bars_layout, meter_fill_w};

#[test]
fn meter_fill_width_rounds_to_physical_px() {
    let sf = 1.1;
    assert_eq!(
        meter_fill_w(0.5, px(101.), sf),
        px((50.5f32 * sf).round() / sf)
    );
    assert_eq!(meter_fill_w(0.0001, px(101.), sf), px(1. / sf));
    assert_eq!(meter_fill_w(1.4, px(101.), sf), px(101.));
    assert_eq!(meter_fill_w(0., px(101.), sf), px(0.));
    assert_eq!(meter_fill_w(-0.2, px(101.), sf), px(0.));
    assert_eq!(meter_fill_w(f32::NAN, px(101.), sf), px(0.));
}

#[test]
fn core_bars_layout_32_cores() {
    let l = core_bars_layout(32, px(776.), px(400.), 1.);
    assert_eq!((l.cols, l.rows), (5, 7));
    assert_eq!(l.row_h, px(18.));
    assert!(l.labels);
    // Too low for 18 px rows: squeezed, labels kept at 14 px and hidden below.
    let l = core_bars_layout(32, px(776.), px(7. * 15.), 1.);
    assert_eq!(l.row_h, px(15.));
    assert!(l.labels);
    let l = core_bars_layout(32, px(776.), px(50.), 1.);
    assert_eq!(l.row_h, px(12.));
    assert!(!l.labels);
    // Narrower than one cell: one column.
    let l = core_bars_layout(4, px(100.), px(400.), 1.);
    assert_eq!((l.cols, l.rows), (1, 4));
}

#[test]
fn core_bars_bucket_change_only() {
    let mut model = CoreBarsModel::default();
    model.set_cpus((0..2).map(|i| format!("C{i}").into()));
    assert!(model.set_values([Some(12.2), Some(50.)].into_iter()));
    assert!(!model.set_values([Some(12.8), Some(50.)].into_iter()));
    assert_eq!(model.values[0], Some(12.8));
    assert!(model.set_values([Some(13.1), Some(50.)].into_iter()));
    // Valid to no sample is a change too.
    assert!(model.set_values([Some(13.1), None].into_iter()));
    assert!(!model.set_values([Some(13.1), None].into_iter()));
}

#[cfg(feature = "test-hooks")]
mod drawn {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui_kit::{
        App, Context, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
        VisualTestContext, Window, div, hsla, px, size,
    };
    use tl_model::settings::{AppearanceSettings, ThemeSettings};
    use tl_ui::pages::performance::overview::parts::{
        CoreBars, CoreBarsModel, METER_H, Meter, take_meter_quads,
    };

    enum Part {
        Meter(Option<f32>),
        Cores(Rc<RefCell<CoreBarsModel>>),
    }

    struct Host(Part);

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let body = match &self.0 {
                Part::Meter(fraction) => Meter {
                    fraction: *fraction,
                    color: hsla(0.5, 0.5, 0.5, 1.),
                }
                .into_any_element(),
                Part::Cores(model) => CoreBars {
                    model: Rc::clone(model),
                }
                .into_any_element(),
            };
            div().size_full().child(
                div()
                    .absolute()
                    .top(px(10.))
                    .left(px(20.))
                    .w(px(101.))
                    .h(px(200.))
                    .child(body),
            )
        }
    }

    fn setup(cx: &mut App) {
        gpui_kit::init(cx);
        tl_ui::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
    }

    fn open(
        cx: &mut TestAppContext,
        part: Part,
    ) -> (gpui_kit::Entity<Host>, &mut VisualTestContext) {
        cx.update(setup);
        let (host, vcx) = cx.add_window_view(move |_, _| Host(part));
        vcx.simulate_scale_factor_change(1.0);
        vcx.simulate_resize(size(px(400.), px(300.)));
        (host, vcx)
    }

    #[gpui_kit::test]
    fn meter_none_draws_track_only(cx: &mut TestAppContext) {
        let (host, vcx) = open(cx, Part::Meter(None));
        take_meter_quads();
        draw(vcx);
        let quads = take_meter_quads();
        assert_eq!(quads.len(), 1, "track only: {quads:?}");
        assert_eq!(quads[0].origin, gpui_kit::point(px(20.), px(10.)));
        assert_eq!(quads[0].size, size(px(101.), px(METER_H)));

        // With a value the fill follows the track.
        vcx.update(|_, cx| {
            host.update(cx, |h, cx| {
                h.0 = Part::Meter(Some(0.5));
                cx.notify();
            })
        });
        draw(vcx);
        let quads = take_meter_quads();
        assert_eq!(quads.len(), 2, "track and fill: {quads:?}");
        assert_eq!(quads[1].size.width, px(51.));
    }

    #[gpui_kit::test]
    fn core_bars_reshape_only_changed_labels(cx: &mut TestAppContext) {
        let model = Rc::new(RefCell::new(CoreBarsModel::default()));
        {
            let mut m = model.borrow_mut();
            m.set_cpus((0..4).map(|i| format!("C{i}").into()));
            m.set_values([Some(10.), Some(20.), Some(30.), Some(40.)].into_iter());
        }
        let (host, vcx) = open(cx, Part::Cores(Rc::clone(&model)));
        draw(vcx);
        let (paints, shaped) = {
            let m = model.borrow();
            (m.paint_count.get(), m.shape_count.get())
        };
        assert!(paints >= 1, "painted");
        assert_eq!(shaped, 8, "4 names and 4 percents shaped once");

        assert!(
            model
                .borrow_mut()
                .set_values([Some(10.), Some(25.), Some(30.), Some(40.)].into_iter())
        );
        vcx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        draw(vcx);
        let m = model.borrow();
        assert!(m.paint_count.get() > paints, "painted again");
        assert_eq!(
            m.shape_count.get(),
            shaped + 1,
            "only C1's percent re-shaped"
        );
    }
}
