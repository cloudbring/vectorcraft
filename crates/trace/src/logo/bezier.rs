//! Least-squares cubic Bézier fitting (Schneider's algorithm). The fitter lives in
//! [`vectorcraft_geom::fit`]. Image Trace passes the end tangent at the start pointing forward and
//! the end tangent at the finish pointing back along the run; the shared fitter takes both
//! pointing forward.

use super::geom::Cubic;
use kurbo::Vec2;
use vectorcraft_geom::Point;
use vectorcraft_geom::fit::{EndTangents, FitOptions};

/// Cubics through `p` (first and last points are interpolated) with end tangents `t1` (pointing
/// along the run at its start) and `t2` (pointing back along the run at its end), each within
/// `tol` of the points.
pub(crate) fn fit_cubics(p: &[Point], t1: Vec2, t2: Vec2, tol: f64) -> Vec<Cubic> {
    // `-t2` is the forward arrival. The solver negates it again, which restores `t2`.
    vectorcraft_geom::fit::fit_cubics(p, EndTangents { start: t1, end: -t2 }, &FitOptions::trace(tol))
        .into_iter()
        .map(|c| [c.p0, c.p1, c.p2, c.p3])
        .collect()
}

/// Unit direction from `p[i]` towards `p[i + k]` (`sign > 0`) or `p[i - k]` (`sign < 0`).
pub(crate) fn tangent(p: &[Point], i: usize, k: usize, sign: i32) -> Vec2 {
    let j = if sign > 0 { (i + k).min(p.len().saturating_sub(1)) } else { i.saturating_sub(k) };
    match (p.get(i), p.get(j)) {
        (Some(&a), Some(&b)) if a.distance(b) > 1e-12 => (b - a) / a.distance(b),
        _ => Vec2::new(1.0, 0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logo::geom::sample_cubics;

    fn fnv_curves(curves: &[Cubic]) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for c in curves {
            for p in c {
                for bits in [p.x.to_bits(), p.y.to_bits()] {
                    h ^= bits;
                    h = h.wrapping_mul(0x100_0000_01b3);
                }
            }
        }
        h
    }

    /// Control points the Image Trace fitter produced on these fixtures before the move.
    #[test]
    fn output_matches_the_fitter_it_replaced() {
        let arc: Vec<Point> = (0..200).map(|i| Point::new(20.0 * (i as f64 * 0.006).cos(), 20.0 * (i as f64 * 0.006).sin())).collect();
        let mut hashes = vec![fnv_curves(&fit_cubics(&arc, tangent(&arc, 0, 4, 1), tangent(&arc, 199, 4, -1), 0.05))];
        let n = 1200;
        let mut circle: Vec<Point> = (0..=n)
            .map(|i| {
                Point::new(
                    50.0 + 20.0 * (i as f64 / n as f64 * std::f64::consts::TAU).cos(),
                    50.0 + 20.0 * (i as f64 / n as f64 * std::f64::consts::TAU).sin(),
                )
            })
            .collect();
        circle[n] = circle[0];
        let t = (circle[1] - circle[n - 1]) / circle[1].distance(circle[n - 1]);
        hashes.push(fnv_curves(&fit_cubics(&circle, t, -t, 0.05)));
        let wave: Vec<Point> = (0..600).map(|i| Point::new(i as f64 * 0.05, 3.0 * (i as f64 * 0.02).sin())).collect();
        hashes.push(fnv_curves(&fit_cubics(&wave, tangent(&wave, 0, 4, 1), tangent(&wave, 599, 4, -1), 0.04)));
        let join: Vec<Point> = (0..400).map(|i| Point::new(i as f64 * 0.1, (i as f64 * 0.05).sin() * 4.0)).collect();
        hashes.push(fnv_curves(&fit_cubics(&join, tangent(&join, 0, 4, 1), tangent(&join, 399, 4, -1), 0.02)));
        assert_eq!(hashes, [6_737_439_215_139_984_406, 15_137_840_694_227_842_059, 16_432_155_519_750_773_581, 6_603_620_059_492_543_387]);
    }

    fn dev(cs: &[Cubic], pts: &[Point]) -> f64 {
        let s = sample_cubics(cs, 0.01);
        pts.iter().map(|p| s.iter().map(|q| q.distance(*p)).fold(f64::MAX, f64::min)).fold(0.0, f64::max)
    }

    #[test]
    fn a_circle_arc_is_one_cubic() {
        let pts: Vec<Point> = (0..200).map(|i| Point::new(20.0 * (i as f64 * 0.006).cos(), 20.0 * (i as f64 * 0.006).sin())).collect();
        let cs = fit_cubics(&pts, tangent(&pts, 0, 4, 1), tangent(&pts, 199, 4, -1), 0.05);
        assert_eq!(cs.len(), 1);
        assert!(dev(&cs, &pts) < 0.05);
    }

    #[test]
    fn a_full_circle_stays_within_tolerance_with_few_pieces() {
        let n = 1200;
        let mut pts: Vec<Point> = (0..=n)
            .map(|i| {
                Point::new(
                    50.0 + 20.0 * (i as f64 / n as f64 * std::f64::consts::TAU).cos(),
                    50.0 + 20.0 * (i as f64 / n as f64 * std::f64::consts::TAU).sin(),
                )
            })
            .collect();
        pts[n] = pts[0];
        let t = (pts[1] - pts[n - 1]) / pts[1].distance(pts[n - 1]);
        let cs = fit_cubics(&pts, t, -t, 0.05);
        assert!(cs.len() <= 12, "{} pieces", cs.len());
        assert!(dev(&cs, &pts) < 0.06, "deviation {}", dev(&cs, &pts));
    }

    #[test]
    fn a_wavy_curve_is_fitted_within_tolerance() {
        let pts: Vec<Point> = (0..600).map(|i| Point::new(i as f64 * 0.05, 3.0 * (i as f64 * 0.02).sin())).collect();
        let cs = fit_cubics(&pts, tangent(&pts, 0, 4, 1), tangent(&pts, 599, 4, -1), 0.04);
        assert!(dev(&cs, &pts) < 0.05);
        assert!(cs.len() < 20);
    }

    #[test]
    fn pieces_join_end_to_end() {
        let pts: Vec<Point> = (0..400).map(|i| Point::new(i as f64 * 0.1, (i as f64 * 0.05).sin() * 4.0)).collect();
        let cs = fit_cubics(&pts, tangent(&pts, 0, 4, 1), tangent(&pts, 399, 4, -1), 0.02);
        for w in cs.windows(2) {
            assert!(w[0][3].distance(w[1][0]) < 1e-9);
        }
    }

    #[test]
    fn degenerate_input_gives_nothing_or_a_line_not_a_panic() {
        let t = Vec2::new(1.0, 0.0);
        assert!(fit_cubics(&[], t, t, 0.1).is_empty());
        assert!(fit_cubics(&[Point::new(0.0, 0.0)], t, t, 0.1).is_empty());
        assert_eq!(fit_cubics(&[Point::new(0.0, 0.0), Point::new(3.0, 0.0)], t, -t, 0.1).len(), 1);
        assert!(fit_cubics(&[Point::new(f64::NAN, 0.0), Point::new(1.0, 1.0), Point::new(2.0, 0.0)], t, t, 0.1).is_empty());
        assert!(fit_cubics(&[Point::new(0.0, 0.0), Point::new(1.0, 0.0), Point::new(2.0, 0.0)], t, t, 0.0).is_empty());
        // Duplicate points do not divide by zero.
        let _ = fit_cubics(&[Point::new(1.0, 1.0); 10], t, t, 0.1);
    }
}
