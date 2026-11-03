//! Bézier fitting lives in [`vectorcraft_geom::fit`]. These wrappers keep the boolean-tidy
//! calling convention: both end tangents point forward along the direction of travel.

use kurbo::{CubicBez, Point, Vec2};
use vectorcraft_geom::fit::{EndTangents, FitOptions};

pub(crate) use vectorcraft_geom::fit::{end_tangent, is_straight, sample, start_tangent, unit};

/// Fit a chain of cubics through `pts` with maximum error `tol`.
///
/// `t0` is the unit tangent leaving `pts[0]`; `t1` is the unit tangent arriving at the last point
/// (forward direction). Consecutive output cubics are G1 continuous.
pub(crate) fn fit_cubics(pts: &[Point], t0: Vec2, t1: Vec2, tol: f64) -> Vec<CubicBez> {
    vectorcraft_geom::fit::fit_cubics(pts, EndTangents { start: t0, end: t1 }, &FitOptions::pathops(tol))
}

/// Best single cubic for `pts` with the given end tangents. Returns (curve, max error, index of
/// the worst point).
pub(crate) fn fit_single(pts: &[Point], t0: Vec2, t1: Vec2) -> (CubicBez, f64, usize) {
    let rounds = FitOptions::pathops(0.0).rounds;
    vectorcraft_geom::fit::fit_cubic(pts, EndTangents { start: t0, end: t1 }, rounds)
}

/// [`fit_single`] starting from the parameters `u` (one per point, 0 to 1) instead of the
/// chord-length ones.
pub(crate) fn fit_single_from(pts: &[Point], u: Vec<f64>, t0: Vec2, t1: Vec2) -> (CubicBez, f64, usize) {
    let rounds = FitOptions::pathops(0.0).rounds;
    vectorcraft_geom::fit::fit_cubic_from(pts, u, EndTangents { start: t0, end: t1 }, rounds)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fnv_curves(curves: &[CubicBez]) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for c in curves {
            for p in [c.p0, c.p1, c.p2, c.p3] {
                for bits in [p.x.to_bits(), p.y.to_bits()] {
                    h ^= bits;
                    h = h.wrapping_mul(0x100_0000_01b3);
                }
            }
        }
        h
    }

    /// Control points the boolean-tidy fitter produced on these fixtures before the move.
    #[test]
    fn output_matches_the_fitter_it_replaced() {
        let c = CubicBez::new((0.0, 0.0), (30.0, 60.0), (70.0, 60.0), (100.0, 0.0));
        let mut pts = Vec::new();
        sample(&c, 40, &mut pts, true);
        let mut hashes = vec![fnv_curves(&fit_cubics(&pts, start_tangent(&c), end_tangent(&c), 0.05))];
        let sine: Vec<Point> = (0..=40).map(|i| Point::new(i as f64, ((i as f64) * 0.5).sin() * 10.0)).collect();
        hashes.push(fnv_curves(&fit_cubics(&sine, Vec2::new(1.0, 0.0), Vec2::new(1.0, 0.0), 0.1)));
        let line: Vec<Point> = (0..10).map(|i| Point::new(i as f64, 0.0)).collect();
        hashes.push(fnv_curves(&fit_cubics(&line, Vec2::new(1.0, 0.0), Vec2::new(1.0, 0.0), 0.01)));
        let elbow: Vec<Point> = (0..8).map(|i| if i < 4 { Point::new(i as f64, 0.0) } else { Point::new(3.0, (i - 3) as f64) }).collect();
        hashes.push(fnv_curves(&fit_cubics(&elbow, Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0), 0.05)));
        let (one, err, split) = fit_single(&pts, start_tangent(&c), end_tangent(&c));
        hashes.push(fnv_curves(&[one]));
        hashes.push(err.to_bits());
        hashes.push(split as u64);
        let dup = vec![Point::new(0.0, 0.0), Point::new(0.0, 0.0), Point::new(3.0, 1.0), Point::new(3.0, 1.0)];
        hashes.push(fnv_curves(&fit_cubics(&dup, Vec2::new(1.0, 0.0), Vec2::new(1.0, 0.0), 0.1)));
        assert_eq!(
            hashes,
            [
                8_932_132_464_832_370_493,
                568_372_485_519_000_034,
                13_857_378_320_941_088_052,
                2_373_646_869_911_480_358,
                8_932_132_464_832_370_493,
                4_576_671_427_206_752_086,
                12,
                12_994_929_304_267_098_505,
            ]
        );
    }

    #[test]
    fn fits_a_cubic_exactly() {
        let c = CubicBez::new((0.0, 0.0), (30.0, 60.0), (70.0, 60.0), (100.0, 0.0));
        let mut pts = Vec::new();
        sample(&c, 40, &mut pts, true);
        let out = fit_cubics(&pts, start_tangent(&c), end_tangent(&c), 0.05);
        assert_eq!(out.len(), 1);
        assert!(out[0].p1.distance(c.p1) < 1.0, "{:?}", out[0]);
    }

    #[test]
    fn splits_when_needed() {
        let pts: Vec<Point> = (0..=40).map(|i| Point::new(i as f64, ((i as f64) * 0.5).sin() * 10.0)).collect();
        let out = fit_cubics(&pts, Vec2::new(1.0, 0.0), Vec2::new(1.0, 0.0), 0.1);
        assert!(out.len() > 1);
        for w in out.windows(2) {
            assert!(w[0].p3.distance(w[1].p0) < 1e-9);
        }
    }

    #[test]
    fn straight_detection() {
        let c = CubicBez::new((0.0, 0.0), (3.0, 0.0), (6.0, 0.0), (9.0, 0.0));
        assert!(is_straight(&c, 1e-6));
        let c = CubicBez::new((0.0, 0.0), (3.0, 1.0), (6.0, 0.0), (9.0, 0.0));
        assert!(!is_straight(&c, 1e-6));
    }
}
