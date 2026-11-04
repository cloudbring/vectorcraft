//! Bezier evaluation, de Casteljau subdivision and the control-polygon bounding box, checked against
//! the exact rational vectors that craft-libs' `lean/bezier` project generates and proves (ticket
//! cloudbring/artcraft-algorithms#216; `tests/vectors/lean-bezier.csv`, header names the craft-libs commit).
//!
//! Lean theorems the tests stand on (all in `CraftBezier`, craft-libs `lean/bezier`):
//! * `deCasteljau_eq_bez`: de Casteljau evaluation equals the Bernstein form;
//! * `subdiv_left`, `subdiv_right`: the two de Casteljau halves reparametrise the curve exactly;
//! * `bez_mem_convexHull`, `bez_mem_bbox`: for `t` in `[0,1]` the curve lies in the convex hull, hence
//!   in the axis-aligned box, of its control points. This is why `PathData::control_bounds` is a valid
//!   fast-path cull box;
//! * `xCurve_strictMonoOn`: a cubic easing with monotone control x-values has strictly monotone `x(t)`.
//!
//! The vectors are exact; Rust computes in `f64`, so agreement is within a tolerance. Each test prints
//! its maximum error (run with `--nocapture`).
// Integration tests: unwrapping and panicking on failure is fine here, unlike in shipped code (AGENTS.md › Robustness).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kurbo::{CubicBez, ParamCurve, Point};
use proptest::prelude::*;
use vectorcraft_geom::{Anchor, PathData, Rect, SubPath};

const CSV: &str = include_str!("vectors/lean-bezier.csv");

/// Absolute tolerance: coordinates are at most 4 in size, so a few hundred ulps.
const TOL: f64 = 1e-12;

struct Row {
    name: String,
    s: f64,
    p: [Point; 4],
    b: Point,
    left: [Point; 4],
    right: [Point; 4],
}

/// `n/d` as the nearest `f64` (both parts are small integers, so each is exact).
fn frac(f: &str) -> f64 {
    let (n, d) = f.split_once('/').unwrap_or_else(|| panic!("not a fraction: {f}"));
    n.parse::<i64>().unwrap() as f64 / d.parse::<i64>().unwrap() as f64
}

fn rows() -> Vec<Row> {
    let mut lines = CSV.lines().filter(|l| !l.starts_with('#'));
    let header = lines.next().unwrap();
    assert!(header.starts_with("name,s,p0x,p0y"), "{header}");
    lines
        .map(|line| {
            let f: Vec<&str> = line.split(',').collect();
            assert_eq!(f.len(), 28, "{line}");
            let pt = |i: usize| Point::new(frac(f[2 + 2 * i]), frac(f[3 + 2 * i]));
            Row {
                name: f[0].to_string(),
                s: frac(f[1]),
                p: [pt(0), pt(1), pt(2), pt(3)],
                b: pt(4),
                left: [pt(5), pt(6), pt(7), pt(8)],
                right: [pt(9), pt(10), pt(11), pt(12)],
            }
        })
        .collect()
}

fn err(a: Point, b: Point) -> f64 {
    (a.x - b.x).abs().max((a.y - b.y).abs())
}

fn cubic(p: [Point; 4]) -> CubicBez {
    CubicBez::new(p[0], p[1], p[2], p[3])
}

#[test]
fn vector_file_shape() {
    let r = rows();
    assert_eq!(r.len(), 90);
    assert!(r.iter().any(|x| x.name == "css-ease") && r.iter().any(|x| x.name == "loop"));
}

/// `CubicBez::eval` at the split parameter is the exact `B(s)` (Lean: `deCasteljau_eq_bez`).
#[test]
fn evaluation_matches_exact_vectors() {
    let mut max = 0.0f64;
    for r in rows() {
        let e = err(cubic(r.p).eval(r.s), r.b);
        max = max.max(e);
        assert!(e <= TOL, "{} s={}: eval error {e:e}", r.name, r.s);
        // The split point is the end of the left half and the start of the right half.
        assert!(err(r.left[3], r.b) == 0.0 && err(r.right[0], r.b) == 0.0);
    }
    eprintln!("max evaluation error vs Lean vectors: {max:e}");
}

/// kurbo's `subsegment` halves have the control points of the exact de Casteljau split (Lean:
/// `subdiv_left`, `subdiv_right`), and evaluating a half at `u` is the original at the reparametrised
/// parameter.
#[test]
fn subdivision_matches_exact_vectors() {
    let (mut max_pts, mut max_reparam) = (0.0f64, 0.0f64);
    for r in rows() {
        let c = cubic(r.p);
        let left = c.subsegment(0.0..r.s);
        let right = c.subsegment(r.s..1.0);
        for (got, want) in [(left, r.left), (right, r.right)] {
            for (g, w) in [got.p0, got.p1, got.p2, got.p3].into_iter().zip(want) {
                let e = err(g, w);
                max_pts = max_pts.max(e);
                assert!(e <= TOL, "{} s={}: control point error {e:e}", r.name, r.s);
            }
        }
        for u in [0.0, 0.2, 0.5, 0.75, 1.0] {
            let e_left = err(cubic(r.left).eval(u), c.eval(r.s * u));
            let e_right = err(cubic(r.right).eval(u), c.eval(r.s + (1.0 - r.s) * u));
            max_reparam = max_reparam.max(e_left).max(e_right);
            assert!(e_left <= TOL && e_right <= TOL, "{} s={} u={u}: {e_left:e} {e_right:e}", r.name, r.s);
        }
    }
    eprintln!("max subdivision control-point error vs Lean vectors: {max_pts:e}");
    eprintln!("max reparametrisation error (exact halves evaluated in f64): {max_reparam:e}");
}

/// The control-polygon box (as `PathData::control_bounds` takes it) contains the curve and both halves
/// (Lean: `bez_mem_bbox`), on the exact vector polygons.
#[test]
fn control_box_contains_vector_curves() {
    let mut max_over = 0.0f64;
    for r in rows() {
        for poly in [r.p, r.left, r.right] {
            let bx = poly.iter().fold(Rect::from_points(poly[0], poly[0]), |b, &p| b.union_pt(p));
            let c = cubic(poly);
            for i in 0..=64 {
                let q = c.eval(i as f64 / 64.0);
                let over = (bx.x0 - q.x).max(q.x - bx.x1).max(bx.y0 - q.y).max(q.y - bx.y1);
                max_over = max_over.max(over);
                assert!(over <= TOL, "{} s={}: {q:?} outside {bx:?}", r.name, r.s);
            }
        }
    }
    eprintln!("max overshoot of a sampled point beyond the control box: {max_over:e}");
}

/// CSS easing polygons have monotone control x-values, so `x(t)` is strictly monotone (Lean:
/// `xCurve_strictMonoOn`) and the easing solve has a unique root: sampled `x(t)` strictly increases.
#[test]
fn easing_x_is_strictly_monotone() {
    let mut n = 0;
    for r in rows().into_iter().filter(|r| r.name.starts_with("css-") && r.s == 0.0) {
        let c = cubic(r.p);
        assert!(r.p[0].x <= r.p[1].x && r.p[1].x <= r.p[2].x && r.p[2].x <= r.p[3].x);
        let mut prev = f64::NEG_INFINITY;
        for i in 0..=1000 {
            let x = c.eval(i as f64 / 1000.0).x;
            assert!(x > prev, "{}: x({}) = {x} not above {prev}", r.name, i as f64 / 1000.0);
            prev = x;
        }
        n += 1;
    }
    assert_eq!(n, 4);
}

fn arb_pt() -> impl Strategy<Value = Point> {
    (-500.0..500.0f64, -500.0..500.0f64).prop_map(|(x, y)| Point::new(x, y))
}

proptest! {
    /// Lean `bez_mem_bbox`: a cubic's sampled points lie in the box of its four control points, and
    /// the split halves stay inside the parent's control box (hull of the halves is inside the hull).
    #[test]
    fn cubic_stays_in_control_box(p in [arb_pt(), arb_pt(), arb_pt(), arb_pt()], t in 0.0..=1.0f64, s in 0.0..=1.0f64) {
        let bx = p.iter().fold(Rect::from_points(p[0], p[0]), |b, &q| b.union_pt(q));
        let inside = |q: Point| q.x >= bx.x0 - 1e-9 && q.x <= bx.x1 + 1e-9 && q.y >= bx.y0 - 1e-9 && q.y <= bx.y1 + 1e-9;
        let c = cubic(p);
        prop_assert!(inside(c.eval(t)));
        for q in [c.subsegment(0.0..s), c.subsegment(s..1.0)] {
            prop_assert!(inside(q.p1) && inside(q.p2), "half control points {q:?} outside {bx:?}");
        }
    }

    /// The fast path used for culling, `PathData::control_bounds`, contains every sampled point of
    /// every segment of a path (Lean `bez_mem_bbox`, applied per segment, joined by union).
    #[test]
    fn control_bounds_fast_path_contains_samples(
        anchors in prop::collection::vec((arb_pt(), arb_pt(), arb_pt()), 2..8),
        closed in any::<bool>(),
        ts in prop::collection::vec(0.0..=1.0f64, 16),
    ) {
        let anchors: Vec<Anchor> = anchors.into_iter().map(|(p, i, o)| Anchor::with_handles(p, i, o)).collect();
        let pd = PathData::new(vec![SubPath::new(anchors, closed)]);
        let bx = pd.control_bounds().unwrap();
        for seg in pd.to_bezpath().segments() {
            for &t in &ts {
                let q = seg.eval(t);
                prop_assert!(q.x >= bx.x0 - 1e-9 && q.x <= bx.x1 + 1e-9 && q.y >= bx.y0 - 1e-9 && q.y <= bx.y1 + 1e-9,
                    "{q:?} outside control_bounds {bx:?}");
            }
        }
    }
}
