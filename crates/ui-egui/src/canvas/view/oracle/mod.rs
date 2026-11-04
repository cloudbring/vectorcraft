//! #188: OLD (hand-written `Xf` maths) vs NEW (`craft-view`) canvas view transforms.
//!
//! OLD is vendored in `old.rs` (vectorcraft @ 11862f61198c8f62c0b22b6c9a331cc5950ac98d, the
//! merge-base of `algo/72-use-craft-view-v2` with `origin/main`); NEW is this crate's `Xf`,
//! `zoom_about`, `rotate_about`, `pan_pixels`, `scroll_by`, `fit_zoom` and `fit`. Screen↔doc,
//! zoom about a point, pan, quarter-turn rotation and fit are compared over random viewports.
//! Tolerance: `|a - b| <= 1e-9 * max(1, |a|, |b|)`.
//!
//! One difference is by construction and not a bug: OLD's `to_doc` subtracts the rectangle centre from
//! the pointer in f32 (`(p.x - c.x) as f64`), NEW subtracts in f64. For screen points whose
//! f32 differences are exact (quarter-pixel positions, the `*_exact` tests) the two agree to 1e-9;
//! for arbitrary f32 points they differ by up to f32 rounding of the screen distance, divided by the
//! zoom (the `*_f32_bound` tests assert that bound). NEW is the more accurate one.

mod old;

use super::*;
use crate::VectorcraftApp;
use old::*;
use proptest::prelude::*;
use serde_json::json;
use vectorcraft_engine::Session;

const TOL: f64 = 1e-9;

fn close(a: f64, b: f64) -> bool {
    (a.is_finite() && b.is_finite()) && (a - b).abs() <= TOL * 1.0f64.max(a.abs()).max(b.abs())
}

fn view_close(a: &View, b: &View) -> Result<(), String> {
    if close(a.zoom, b.zoom) && close(a.center.x, b.center.x) && close(a.center.y, b.center.y) && close(a.rotation, b.rotation) {
        Ok(())
    } else {
        Err(format!(
            "old zoom={} center=({}, {}) rot={}  new zoom={} center=({}, {}) rot={}",
            a.zoom, a.center.x, a.center.y, a.rotation, b.zoom, b.center.x, b.center.y, b.rotation
        ))
    }
}

fn rect() -> impl Strategy<Value = egui::Rect> {
    (-2000.0f32..2000.0, -2000.0f32..2000.0, 60.0f32..4000.0, 60.0f32..3000.0)
        .prop_map(|(x, y, w, h)| egui::Rect::from_min_size(pos2(x, y), egui::vec2(w, h)))
}

/// Round to a quarter point, so that every f32 subtraction of two such values below 2^15 is exact.
fn snap(x: f32) -> f32 {
    (x * 4.0).round() / 4.0
}

fn rect_exact() -> impl Strategy<Value = egui::Rect> {
    rect().prop_map(|r| {
        egui::Rect::from_min_size(
            pos2(snap(r.min.x), snap(r.min.y)),
            egui::vec2((snap(r.width()) / 2.0).round() * 2.0, (snap(r.height()) / 2.0).round() * 2.0),
        )
    })
}

fn point_in(r: egui::Rect, seed: u64, exact: bool) -> Pos2 {
    let (u, v) = ((seed % 1000) as f32 / 1000.0, (seed / 1000 % 1000) as f32 / 1000.0);
    let (x, y) = (r.min.x + u * r.width(), r.min.y + v * r.height());
    if exact { pos2(snap(x), snap(y)) } else { pos2(x, y) }
}

/// The most NEW and OLD can differ by in a document coordinate because OLD subtracts f32 screen
/// positions: 8 f32 epsilons of the largest screen coordinate involved, over the smaller zoom.
fn f32_bound(r: egui::Rect, p: Pos2, zoom_a: f64, zoom_b: f64) -> f64 {
    let s = [r.min.x, r.min.y, r.max.x, r.max.y, p.x, p.y].iter().fold(1.0f32, |m, v| m.max(v.abs()));
    8.0 * f64::from(f32::EPSILON) * f64::from(s) / zoom_a.min(zoom_b)
}

fn near(a: f64, b: f64, abs: f64) -> bool {
    (a - b).abs() <= abs + TOL * 1.0f64.max(a.abs()).max(b.abs())
}

fn zoom() -> impl Strategy<Value = f64> {
    (0.0313f64.ln()..640.0f64.ln()).prop_map(f64::exp)
}

fn degrees() -> impl Strategy<Value = f64> {
    prop_oneof![-720.0f64..720.0, (-8i32..=8).prop_map(|k| f64::from(k) * 90.0), (-3600i32..3600).prop_map(|k| f64::from(k) * 0.1)]
}

fn view() -> impl Strategy<Value = View> {
    (zoom(), -1e5f64..1e5, -1e5f64..1e5, degrees()).prop_map(|(zoom, x, y, rot)| View {
        zoom,
        center: Point::new(x, y),
        rotation: vectorcraft_geom::normalize_deg(rot),
        ..View::default()
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, ..ProptestConfig::default() })]

    /// screen<->doc and delta mapping.
    #[test]
    fn xf_maps_agree(r in rect(), v in view(), dx in -1e5f64..1e5, dy in -1e5f64..1e5, d in (-3000.0f32..3000.0, -3000.0f32..3000.0), seed in any::<u64>()) {
        let (o, n) = (OldXf::new(r, &v), Xf::new(r, &v));
        let (a, b) = (o.to_screen(Point::new(dx, dy)), n.to_screen(Point::new(dx, dy)));
        // f32 output: within 4 f32 ulps of its magnitude (OLD rounds the rotated offset to f32 before adding the
        // centre, NEW rounds once).
        // The offset from the view centre can be far larger than the result, so the bound is on it.
        let mag = ((dx - v.center.x).abs().max((dy - v.center.y).abs()) * v.zoom) as f32;
        let ok = |p: f32, q: f32| (p - q).abs() <= 8.0 * f32::EPSILON * 1.0f32.max(p.abs()).max(q.abs()).max(mag);
        prop_assert!(ok(a.x, b.x) && ok(a.y, b.y), "to_screen old {a:?} new {b:?} offset magnitude {mag}");
        let dv = egui::vec2(d.0, d.1);
        let (a, b) = (o.delta_to_doc(dv), n.delta_to_doc(dv));
        prop_assert!(close(a.x, b.x) && close(a.y, b.y), "delta_to_doc old {a:?} new {b:?}");
        // Arbitrary f32 pointer: within the f32 bound (see the module docs).
        let p = point_in(r, seed, false);
        let (a, b) = (o.to_doc(p), n.to_doc(p));
        let tol = f32_bound(r, p, v.zoom, v.zoom);
        prop_assert!(near(a.x, b.x, tol) && near(a.y, b.y, tol), "to_doc old {a:?} new {b:?} bound {tol}");
    }

    /// to_doc with exact f32 inputs: equal to 1e-9.
    #[test]
    fn to_doc_agrees_exact(r in rect_exact(), v in view(), seed in any::<u64>()) {
        let p = point_in(r, seed, true);
        let (a, b) = (OldXf::new(r, &v).to_doc(p), Xf::new(r, &v).to_doc(p));
        prop_assert!(close(a.x, b.x) && close(a.y, b.y), "to_doc old {a:?} new {b:?}");
    }

    #[test]
    fn zoom_about_agrees_exact(r in rect_exact(), v in view(), z in prop_oneof![zoom(), Just(0.001), Just(5000.0)], seed in any::<u64>()) {
        let p = point_in(r, seed, true);
        let (mut a, mut b) = (v, v);
        old_zoom_about(&mut a, r, p, z);
        zoom_about(&mut b, r, p, z);
        view_close(&a, &b).map_err(TestCaseError::fail)?;
    }

    #[test]
    fn zoom_about_agrees_f32_bound(r in rect(), v in view(), z in zoom(), seed in any::<u64>()) {
        let p = point_in(r, seed, false);
        let (mut a, mut b) = (v, v);
        old_zoom_about(&mut a, r, p, z);
        zoom_about(&mut b, r, p, z);
        let tol = f32_bound(r, p, v.zoom, a.zoom);
        prop_assert!(close(a.zoom, b.zoom) && close(a.rotation, b.rotation) && near(a.center.x, b.center.x, tol) && near(a.center.y, b.center.y, tol), "old {a:?} new {b:?} bound {tol}");
    }

    /// Rotation about a point: the view's rotation, centre and zoom all agree.
    #[test]
    fn rotate_about_agrees_exact(r in rect_exact(), v in view(), by in degrees(), seed in any::<u64>()) {
        let p = point_in(r, seed, true);
        let (mut a, mut b) = (v, v);
        old_rotate_about(&mut a, r, p, by);
        rotate_about(&mut b, r, p, by);
        view_close(&a, &b).map_err(TestCaseError::fail)?;
    }

    #[test]
    fn rotate_about_agrees_f32_bound(r in rect(), v in view(), by in degrees(), seed in any::<u64>()) {
        let p = point_in(r, seed, false);
        let (mut a, mut b) = (v, v);
        old_rotate_about(&mut a, r, p, by);
        rotate_about(&mut b, r, p, by);
        let tol = f32_bound(r, p, v.zoom, v.zoom);
        prop_assert!(close(a.zoom, b.zoom) && close(a.rotation, b.rotation) && near(a.center.x, b.center.x, tol) && near(a.center.y, b.center.y, tol), "old {a:?} new {b:?} bound {tol}");
    }

    #[test]
    fn pan_pixels_agrees(v in view(), ppp in prop_oneof![Just(1.0f32), Just(1.25), Just(2.0), Just(3.0)], dx in -4000i32..4000, dy in -4000i32..4000) {
        let (mut a, mut b) = (v, v);
        old_pan_pixels(&mut a, ppp, (f64::from(dx), f64::from(dy)));
        pan_pixels(&mut b, ppp, (f64::from(dx), f64::from(dy)));
        view_close(&a, &b).map_err(TestCaseError::fail)?;
    }

    /// A run of scrolls, carrying the sub-pixel remainder through both.
    #[test]
    fn scroll_runs_agree(v in view(), ppp in prop_oneof![Just(1.0f32), Just(1.5), Just(2.0)], by in prop::collection::vec((-40.0f32..40.0, -40.0f32..40.0), 1..8)) {
        let (mut a, mut b) = (v, v);
        let (mut ra, mut rb) = (ScrollRest::default(), ScrollRest::default());
        for (x, y) in by {
            ra = old_scroll_by(&mut a, 7, ppp, egui::vec2(x, y), ra);
            rb = scroll_by(&mut b, 7, ppp, egui::vec2(x, y), rb);
            view_close(&a, &b).map_err(TestCaseError::fail)?;
            prop_assert!(close(ra.x, rb.x) && close(ra.y, rb.y), "rest old ({}, {}) new ({}, {})", ra.x, ra.y, rb.x, rb.y);
        }
    }

    #[test]
    fn fit_zoom_agrees(r in rect(), x in -1e4f64..1e4, y in -1e4f64..1e4, w in 0.0f64..20000.0, h in 0.0f64..20000.0, rot in degrees()) {
        let t = vectorcraft_geom::Rect::new(x, y, x + w, y + h);
        let (a, b) = (old_fit_zoom(r, t, rot), fit_zoom(r, t, rot));
        prop_assert!(close(a, b), "old {a} new {b}");
    }
}

/// Four quarter turns, then eight, from a rotated and off-centre view: every step agrees. The
/// representation of ±180 (OLD keeps 180, `normalize_deg` maps (-180, 180]) is part of the check.
#[test]
fn quarter_turns_agree_step_by_step() {
    let r = egui::Rect::from_min_size(pos2(10.0, 20.0), egui::vec2(1200.0, 800.0));
    for start in [0.0, 45.0, 90.0, -90.0, 179.0, -179.0, 180.0] {
        for dir in [90.0, -90.0] {
            let v0 = View { rotation: start, center: Point::new(-300.5, 4000.25), zoom: 1.75, ..View::default() };
            let (mut a, mut b) = (v0, v0);
            for i in 0..8 {
                old_rotate_about(&mut a, r, pos2(400.0, 300.0), dir);
                rotate_about(&mut b, r, pos2(400.0, 300.0), dir);
                view_close(&a, &b).unwrap_or_else(|e| panic!("start {start} dir {dir} turn {i}: {e}"));
            }
        }
    }
}

fn app_with_artboard(w: u32, h: u32) -> VectorcraftApp {
    let mut app = VectorcraftApp::new(Session::new(), Default::default());
    app.run("file.new", json!({"width": w, "height": h})).unwrap();
    app
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 300, ..ProptestConfig::default() })]

    /// The real `fit` (Fit Artboard, Fit All, Actual Size, both Display Print Size settings)
    /// against the OLD arithmetic, on a document of random size and a random starting view.
    #[test]
    fn fit_agrees(r in rect(), v in view(), w in 10u32..14000, h in 10u32..14000, print_size in any::<bool>()) {
        for how in ["view.fitArtboard", "view.fitAll", "view.actualSize"] {
            let mut app = app_with_artboard(w, h);
            app.canvas_rect = Some(r);
            *app.view_mut().unwrap() = View { fitted: false, artboard: 0, ..v };
            app.session.prefs.display_print_size = print_size;
            let st = app.session.active().unwrap();
            let target = match how {
                "view.fitAll" => st.doc.art_bounds().map(|a| st.doc.artboards.iter().fold(a, |r, ab| r.union(ab.rect))).or(st.doc.artboards.first().map(|a| a.rect)),
                _ => st.doc.artboards.first().map(|a| a.rect),
            }
            .unwrap();
            let actual = (how == "view.actualSize").then(|| actual_size_zoom(print_size));
            let (center, zoom) = old_fit(r, target, v.rotation, actual);
            fit(&mut app, how);
            let got = app.view().unwrap();
            prop_assert!(got.fitted);
            prop_assert!(close(got.zoom, zoom) && close(got.center.x, center.x) && close(got.center.y, center.y), "{how}: old zoom={zoom} center={center:?} new zoom={} center={:?}", got.zoom, got.center);
            prop_assert!(close(got.rotation, v.rotation), "{how}: rotation changed {} -> {}", v.rotation, got.rotation);
        }
    }
}

/// The comparison is sensitive: a view zoomed to another level is not "close".
#[test]
fn comparison_detects_a_difference() {
    let r = egui::Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
    let (mut a, mut b) = (View::default(), View::default());
    old_zoom_about(&mut a, r, pos2(100.0, 100.0), 2.0);
    zoom_about(&mut b, r, pos2(100.0, 100.0), 2.0);
    assert!(view_close(&a, &b).is_ok());
    zoom_about(&mut b, r, pos2(100.0, 100.0), 2.000001);
    assert!(view_close(&a, &b).is_err());
    assert_ne!(a.center, View::default().center);
}
