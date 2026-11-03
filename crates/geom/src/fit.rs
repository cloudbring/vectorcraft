//! Least-squares cubic Bézier fitting (Schneider's algorithm).
//!
//! Fit one cubic with fixed end tangents, improve the parametrisation with Newton steps, and,
//! when asked, split at the worst point until every piece is within the tolerance. After
//! P. J. Schneider, "An algorithm for automatically fitting digitized curves" (Graphics Gems,
//! 1990), written from that description. Boolean tidy and Image Trace both call this. Each passes
//! a [`FitOptions`]: tolerance, Newton rounds, join policy, and whether to split. [`Kernel`] selects
//! the caller's arithmetic, so the two stay bit-identical to the fitters they replaced.

use kurbo::{CubicBez, ParamCurve, ParamCurveDeriv, Point, Vec2};

/// Unit vector, or `None` when `v` is shorter than 1e-12 or not finite.
pub fn unit(v: Vec2) -> Option<Vec2> {
    let l = v.hypot();
    if l > 1e-12 && l.is_finite() { Some(v / l) } else { None }
}

/// Direction the curve leaves `p0` (robust to retracted handles).
pub fn start_tangent(c: &CubicBez) -> Vec2 {
    unit(c.p1 - c.p0).or_else(|| unit(c.p2 - c.p0)).or_else(|| unit(c.p3 - c.p0)).unwrap_or(Vec2::new(1.0, 0.0))
}

/// Direction the curve arrives at `p3`, pointing forward along the direction of travel.
pub fn end_tangent(c: &CubicBez) -> Vec2 {
    unit(c.p3 - c.p2).or_else(|| unit(c.p3 - c.p1)).or_else(|| unit(c.p3 - c.p0)).unwrap_or(Vec2::new(1.0, 0.0))
}

/// End tangents of a run. Both point forward, along the direction of travel.
#[derive(Clone, Copy, Debug)]
pub struct EndTangents {
    /// Unit tangent leaving the first point.
    pub start: Vec2,
    /// Unit tangent arriving at the last point.
    pub end: Vec2,
}

/// Direction used when the samples on either side of a smooth join coincide.
#[derive(Clone, Copy, Debug)]
pub enum SmoothFallback {
    /// The run's start tangent (boolean tidy).
    Start,
    /// The reverse of the run's start tangent (Image Trace).
    ReverseStart,
}

/// How the two pieces meet at a split.
#[derive(Clone, Copy, Debug)]
pub enum JoinPolicy {
    /// One tangent through the neighbouring samples, shared by both pieces (G1).
    Smooth { fallback: SmoothFallback },
    /// The piece before the split arrives along the chord into it, and the piece after leaves
    /// along the chord out of it.
    Corner,
}

/// Which caller's arithmetic and stopping rule to use.
///
/// The two historical fitters solve the same normal equations with different product order,
/// different derivatives, and different handle limits. Mixing them moves the control points.
#[derive(Clone, Copy, Debug)]
pub enum Kernel {
    /// Boolean tidy and Simplify. Up to [`FitOptions::rounds`] Newton steps with kurbo's
    /// derivative; keep the lowest error and stop when a step does not strictly improve it.
    /// Accept `error <= tolerance` (tolerance raised to at least 1e-9). Drop successive points
    /// closer than 1e-12. Split down to 3 points, margin 1, until depth 41. Handles shorter than
    /// `1e-6 * chord` (strict) become a third of the chord, then each handle is capped at
    /// `4 * chord + 1e-9`.
    Tidy,
    /// Image Trace, after Douglas–Peucker. Up to [`FitOptions::rounds`] Newton steps with the
    /// Bernstein derivative, and only while `tolerance <= error <= 6 * tolerance`. Accept as soon
    /// as `error < tolerance`. Reject a non-positive tolerance or a non-finite point, and keep
    /// duplicate points. Do not split a run shorter than 6 points; the split stays 2 points in
    /// from either end; stop at depth 14. A handle of length `<= 1e-6 * chord` becomes a third
    /// of the chord. Handles are not capped.
    Trace,
}

/// Knobs of [`fit_cubics`]. [`FitOptions::pathops`] and [`FitOptions::trace`] are the two callers.
#[derive(Clone, Copy, Debug)]
pub struct FitOptions {
    /// Maximum distance from a sample to its cubic. See [`Kernel`] for how equality is treated.
    pub tolerance: f64,
    /// Maximum Newton reparameterisation rounds. Boolean tidy uses 6, Image Trace uses 4.
    pub rounds: usize,
    pub join: JoinPolicy,
    /// Recurse at the worst sample. `false` returns the single best cubic.
    pub split: bool,
    pub kernel: Kernel,
}

impl FitOptions {
    /// Boolean tidy and Simplify.
    pub fn pathops(tolerance: f64) -> Self {
        Self { tolerance, rounds: 6, join: JoinPolicy::Smooth { fallback: SmoothFallback::Start }, split: true, kernel: Kernel::Tidy }
    }

    /// Image Trace.
    pub fn trace(tolerance: f64) -> Self {
        Self { tolerance, rounds: 4, join: JoinPolicy::Smooth { fallback: SmoothFallback::ReverseStart }, split: true, kernel: Kernel::Trace }
    }
}

/// Cubics through `pts` (first and last points interpolated) within `opts.tolerance`.
pub fn fit_cubics(pts: &[Point], tangents: EndTangents, opts: &FitOptions) -> Vec<CubicBez> {
    if !usable(pts, opts) {
        return Vec::new();
    }
    let mut opts = *opts;
    if matches!(opts.kernel, Kernel::Tidy) {
        opts.tolerance = opts.tolerance.max(1e-9);
    }
    let owned;
    let pts = if matches!(opts.kernel, Kernel::Tidy) {
        owned = dedup(pts);
        owned.as_slice()
    } else {
        pts
    };
    let mut out = Vec::new();
    if pts.len() >= 2 {
        fit_rec(pts, tangents, &opts, &mut out, 0);
    }
    out
}

/// One tidy-kernel cubic for `pts` from chord-length parameters. No split.
///
/// `rounds` is the Newton budget ([`FitOptions::rounds`] for boolean tidy, 6). The returned error
/// is the sample distance of that best cubic; boolean tidy compares it with its own precision.
pub fn fit_cubic(pts: &[Point], tangents: EndTangents, rounds: usize) -> (CubicBez, f64, usize) {
    fit_cubic_from(pts, chord_params(pts), tangents, rounds)
}

/// [`fit_cubic`] starting from the parameters `u` (one per point, 0 to 1).
pub fn fit_cubic_from(pts: &[Point], u: Vec<f64>, tangents: EndTangents, rounds: usize) -> (CubicBez, f64, usize) {
    tidy_best(pts, u, tangents, rounds)
}

/// Is the cubic (within `tol`) a straight line from p0 to p3 with handles inside the chord?
pub fn is_straight(c: &CubicBez, tol: f64) -> bool {
    let chord = c.p3 - c.p0;
    let len = chord.hypot();
    if len < 1e-12 {
        return c.p1.distance(c.p0) <= tol && c.p2.distance(c.p0) <= tol;
    }
    let dir = chord / len;
    [c.p1, c.p2].iter().all(|&h| {
        let v = h - c.p0;
        let along = v.dot(dir);
        v.cross(dir).abs() <= tol && along >= -tol && along <= len + tol
    })
}

/// Sample a cubic at `n+1` evenly spaced parameters (including both ends).
pub fn sample(c: &CubicBez, n: usize, out: &mut Vec<Point>, include_start: bool) {
    let start = if include_start { 0 } else { 1 };
    for i in start..=n {
        out.push(c.eval(i as f64 / n as f64));
    }
}

fn usable(pts: &[Point], opts: &FitOptions) -> bool {
    match opts.kernel {
        Kernel::Tidy => true,
        Kernel::Trace => opts.tolerance > 0.0 && pts.iter().all(|p| p.x.is_finite() && p.y.is_finite()),
    }
}

fn dedup(pts: &[Point]) -> Vec<Point> {
    let mut v: Vec<Point> = Vec::with_capacity(pts.len());
    for &p in pts {
        if v.last().is_none_or(|q: &Point| q.distance(p) > 1e-12) {
            v.push(p);
        }
    }
    v
}

struct Stop {
    max_depth: usize,
    min_points: usize,
    margin: usize,
}

fn stop_at(kernel: Kernel) -> Stop {
    match kernel {
        Kernel::Tidy => Stop { max_depth: 41, min_points: 3, margin: 1 },
        Kernel::Trace => Stop { max_depth: 14, min_points: 6, margin: 2 },
    }
}

fn fit_rec(pts: &[Point], tangents: EndTangents, opts: &FitOptions, out: &mut Vec<CubicBez>, depth: usize) {
    let n = pts.len();
    let (Some(&p0), Some(&pn)) = (pts.first(), pts.last()) else { return };
    if n == 2 {
        let d = p0.distance(pn) / 3.0;
        out.push(CubicBez::new(p0, p0 + tangents.start * d, pn - tangents.end * d, pn));
        return;
    }
    let (curve, err, at) = match opts.kernel {
        Kernel::Tidy => fit_cubic(pts, tangents, opts.rounds),
        Kernel::Trace => match trace_refine(pts, tangents, opts) {
            Attempt::Done(c) => {
                out.push(c);
                return;
            }
            Attempt::Split { curve, err, at } => (curve, err, at),
        },
    };
    let stop = stop_at(opts.kernel);
    let within = match opts.kernel {
        Kernel::Tidy => err <= opts.tolerance,
        Kernel::Trace => err < opts.tolerance,
    };
    if !opts.split || within || depth >= stop.max_depth || n < stop.min_points {
        out.push(curve);
        return;
    }
    let at = clamp_split(at, n, stop.margin);
    let (left_end, right_start) = join_dirs(pts, at, tangents.start, opts.join);
    let (Some(left), Some(right)) = (pts.get(..=at), pts.get(at..)) else {
        out.push(curve);
        return;
    };
    if left.len() < 2 || right.len() < 2 || left.len() == n || right.len() == n {
        out.push(curve);
        return;
    }
    fit_rec(left, EndTangents { start: tangents.start, end: left_end }, opts, out, depth + 1);
    fit_rec(right, EndTangents { start: right_start, end: tangents.end }, opts, out, depth + 1);
}

enum Attempt {
    Done(CubicBez),
    Split { curve: CubicBez, err: f64, at: usize },
}

fn trace_refine(pts: &[Point], tangents: EndTangents, opts: &FitOptions) -> Attempt {
    let mut u = chord_params(pts);
    let mut c = trace_curve(pts, &u, tangents);
    for _ in 0..opts.rounds {
        let (e, _) = max_error(pts, &c, &u, bez_trace);
        if e < opts.tolerance {
            return Attempt::Done(c);
        }
        if e > opts.tolerance * 6.0 {
            break;
        }
        for (ui, p) in u.iter_mut().zip(pts) {
            *ui = newton_bernstein(&c, *p, *ui);
        }
        c = trace_curve(pts, &u, tangents);
    }
    let (err, at) = max_error(pts, &c, &u, bez_trace);
    Attempt::Split { curve: c, err, at }
}

fn tidy_best(pts: &[Point], mut u: Vec<f64>, tangents: EndTangents, rounds: usize) -> (CubicBez, f64, usize) {
    let mut best = tidy_curve(pts, &u, tangents);
    let (mut best_err, mut best_split) = max_error(pts, &best, &u, |c, t| c.eval(t));
    for _ in 0..rounds {
        for (ui, p) in u.iter_mut().zip(pts) {
            *ui = newton_kurbo(&best, *p, *ui);
        }
        let c = tidy_curve(pts, &u, tangents);
        let (e, s) = max_error(pts, &c, &u, |c, t| c.eval(t));
        if e < best_err {
            best = c;
            best_err = e;
            best_split = s;
        } else {
            break;
        }
    }
    (best, best_err, best_split)
}

fn chord_params(pts: &[Point]) -> Vec<f64> {
    let mut u = Vec::with_capacity(pts.len());
    let mut acc = 0.0;
    u.push(0.0);
    for w in pts.windows(2) {
        let (Some(a), Some(b)) = (w.first(), w.get(1)) else { continue };
        acc += a.distance(*b);
        u.push(acc);
    }
    if acc > 0.0 {
        for x in &mut u {
            *x /= acc;
        }
    }
    u
}

fn weights_tidy(t: f64) -> [f64; 4] {
    let s = 1.0 - t;
    [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t]
}

fn weights_trace(t: f64) -> [f64; 4] {
    let s = 1.0 - t;
    // `3·t·s·s`, not `3·s·s·t`: Image Trace's product order is part of its bit pattern.
    [s * s * s, 3.0 * t * s * s, 3.0 * t * t * s, t * t * t]
}

fn accumulate(pts: &[Point], u: &[f64], t0: Vec2, dir_in: Vec2, weights: impl Fn(f64) -> [f64; 4]) -> (f64, f64, f64, f64, f64) {
    let (Some(&p0), Some(&p3)) = (pts.first(), pts.last()) else { return (0.0, 0.0, 0.0, 0.0, 0.0) };
    let (mut c00, mut c01, mut c11, mut x0, mut x1) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (p, &t) in pts.iter().zip(u) {
        let b = weights(t);
        let a0 = t0 * b[1];
        let a1 = dir_in * b[2];
        c00 += a0.dot(a0);
        c01 += a0.dot(a1);
        c11 += a1.dot(a1);
        let tmp = *p - (p0.to_vec2() * (b[0] + b[1]) + p3.to_vec2() * (b[2] + b[3])).to_point();
        x0 += a0.dot(tmp);
        x1 += a1.dot(tmp);
    }
    (c00, c01, c11, x0, x1)
}

fn solved(c00: f64, c01: f64, c11: f64, x0: f64, x1: f64) -> Option<(f64, f64)> {
    let det = c00 * c11 - c01 * c01;
    if det.abs() > 1e-12 { Some(((x0 * c11 - x1 * c01) / det, (c00 * x1 - c01 * x0) / det)) } else { None }
}

fn tidy_curve(pts: &[Point], u: &[f64], tangents: EndTangents) -> CubicBez {
    let (Some(&p0), Some(&p3)) = (pts.first(), pts.last()) else { return CubicBez::new(Point::ZERO, Point::ZERO, Point::ZERO, Point::ZERO) };
    let dir_in = -tangents.end;
    let (c00, c01, c11, x0, x1) = accumulate(pts, u, tangents.start, dir_in, weights_tidy);
    let seg = p0.distance(p3);
    let eps = 1e-6 * seg;
    let (mut al, mut ar) = solved(c00, c01, c11, x0, x1).unwrap_or((seg / 3.0, seg / 3.0));
    if !al.is_finite() || !ar.is_finite() || al < eps || ar < eps {
        al = seg / 3.0;
        ar = seg / 3.0;
    }
    let cap = seg * 4.0 + 1e-9;
    al = al.min(cap);
    ar = ar.min(cap);
    CubicBez::new(p0, p0 + tangents.start * al, p3 + dir_in * ar, p3)
}

fn trace_curve(pts: &[Point], u: &[f64], tangents: EndTangents) -> CubicBez {
    let (Some(&p0), Some(&p3)) = (pts.first(), pts.last()) else { return CubicBez::new(Point::ZERO, Point::ZERO, Point::ZERO, Point::ZERO) };
    let dir_in = -tangents.end;
    let (c00, c01, c11, x0, x1) = accumulate(pts, u, tangents.start, dir_in, weights_trace);
    let seg = p0.distance(p3);
    let (mut al, mut ar) = solved(c00, c01, c11, x0, x1).unwrap_or((0.0, 0.0));
    if !al.is_finite() || !ar.is_finite() || al <= 1e-6 * seg || ar <= 1e-6 * seg {
        al = seg / 3.0;
        ar = seg / 3.0;
    }
    CubicBez::new(p0, p0 + tangents.start * al, p3 + dir_in * ar, p3)
}

fn max_error(pts: &[Point], c: &CubicBez, u: &[f64], at: impl Fn(&CubicBez, f64) -> Point) -> (f64, usize) {
    let mut e = 0.0;
    let mut idx = pts.len() / 2;
    for (i, (p, &t)) in pts.iter().zip(u).enumerate() {
        let d = at(c, t).distance(*p);
        if d > e {
            e = d;
            idx = i;
        }
    }
    (e, idx)
}

fn bez_trace(c: &CubicBez, t: f64) -> Point {
    let u = 1.0 - t;
    let (a, b, cc, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    Point::new(a * c.p0.x + b * c.p1.x + cc * c.p2.x + d * c.p3.x, a * c.p0.y + b * c.p1.y + cc * c.p2.y + d * c.p3.y)
}

fn newton_kurbo(c: &CubicBez, p: Point, t: f64) -> f64 {
    let d1 = c.deriv();
    let d2 = d1.deriv();
    let q = c.eval(t) - p;
    let q1 = d1.eval(t).to_vec2();
    let q2 = d2.eval(t).to_vec2();
    let num = q.dot(q1);
    let den = q1.dot(q1) + q.dot(q2);
    if den.abs() < 1e-12 {
        return t;
    }
    let nt = t - num / den;
    if nt.is_finite() { nt.clamp(0.0, 1.0) } else { t }
}

fn newton_bernstein(c: &CubicBez, p: Point, t: f64) -> f64 {
    let s = 1.0 - t;
    let d = bez_trace(c, t) - p;
    let (v0, v1, v2) = (c.p1 - c.p0, c.p2 - c.p1, c.p3 - c.p2);
    let d1 = (v0 * (s * s) + v1 * (2.0 * s * t) + v2 * (t * t)) * 3.0;
    let d2 = ((v1 - v0) * s + (v2 - v1) * t) * 6.0;
    let den = d1.dot(d1) + d.dot(d2);
    if den.abs() > 1e-12 { (t - d.dot(d1) / den).clamp(0.0, 1.0) } else { t }
}

fn clamp_split(at: usize, n: usize, margin: usize) -> usize {
    let hi = n.saturating_sub(margin.saturating_add(1));
    let lo = margin.min(hi);
    at.clamp(lo, hi)
}

fn join_dirs(pts: &[Point], at: usize, t0: Vec2, join: JoinPolicy) -> (Vec2, Vec2) {
    match join {
        JoinPolicy::Smooth { fallback } => {
            let fb = match fallback {
                SmoothFallback::Start => t0,
                SmoothFallback::ReverseStart => -t0,
            };
            let tc = neighbor_tangent(pts, at).unwrap_or(fb);
            (tc, tc)
        }
        JoinPolicy::Corner => {
            let into = chord_dir(pts, at.wrapping_sub(1), at).unwrap_or(t0);
            let out_of = chord_dir(pts, at, at.saturating_add(1)).unwrap_or(t0);
            (into, out_of)
        }
    }
}

fn neighbor_tangent(pts: &[Point], at: usize) -> Option<Vec2> {
    let prev = at.checked_sub(1).and_then(|i| pts.get(i)).copied()?;
    let next = pts.get(at.saturating_add(1)).copied()?;
    unit(next - prev)
}

fn chord_dir(pts: &[Point], from: usize, to: usize) -> Option<Vec2> {
    let (a, b) = (pts.get(from).copied()?, pts.get(to).copied()?);
    unit(b - a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::ParamCurveNearest;
    use proptest::prelude::*;

    fn ends(t0: Vec2, t1: Vec2) -> EndTangents {
        EndTangents { start: t0, end: t1 }
    }

    fn nearest_error(pts: &[Point], curves: &[CubicBez]) -> f64 {
        pts.iter().map(|p| curves.iter().map(|c| c.nearest(*p, 1e-9).distance_sq.sqrt()).fold(f64::MAX, f64::min)).fold(0.0, f64::max)
    }

    fn g1(curves: &[CubicBez]) -> bool {
        curves.windows(2).all(|w| {
            let (a, b) = (&w[0], &w[1]);
            a.p3.distance(b.p0) < 1e-9 && end_tangent(a).dot(start_tangent(b)) > 1.0 - 1e-9
        })
    }

    #[test]
    fn fits_a_cubic_exactly() {
        let c = CubicBez::new((0.0, 0.0), (30.0, 60.0), (70.0, 60.0), (100.0, 0.0));
        let mut pts = Vec::new();
        sample(&c, 40, &mut pts, true);
        let out = fit_cubics(&pts, ends(start_tangent(&c), end_tangent(&c)), &FitOptions::pathops(0.05));
        assert_eq!(out.len(), 1);
        assert!(out[0].p1.distance(c.p1) < 1.0, "{:?}", out[0]);
        assert!(nearest_error(&pts, &out) <= 0.05);
    }

    #[test]
    fn splits_when_needed_and_joins_are_g1() {
        let pts: Vec<Point> = (0..=40).map(|i| Point::new(i as f64, ((i as f64) * 0.5).sin() * 10.0)).collect();
        let out = fit_cubics(&pts, ends(Vec2::new(1.0, 0.0), Vec2::new(1.0, 0.0)), &FitOptions::pathops(0.1));
        assert!(out.len() > 1);
        assert!(g1(&out));
        assert!(nearest_error(&pts, &out) <= 0.1 + 1e-9);
    }

    #[test]
    fn straight_detection() {
        let c = CubicBez::new((0.0, 0.0), (3.0, 0.0), (6.0, 0.0), (9.0, 0.0));
        assert!(is_straight(&c, 1e-6));
        let c = CubicBez::new((0.0, 0.0), (3.0, 1.0), (6.0, 0.0), (9.0, 0.0));
        assert!(!is_straight(&c, 1e-6));
    }

    #[test]
    fn two_points_are_one_cubic_between_them() {
        let pts = [Point::new(0.0, 0.0), Point::new(3.0, 0.0)];
        let out = fit_cubics(&pts, ends(Vec2::new(1.0, 0.0), Vec2::new(1.0, 0.0)), &FitOptions::pathops(0.1));
        assert_eq!(out.len(), 1);
        assert!(out[0].p0.distance(pts[0]) < 1e-12);
        assert!(out[0].p3.distance(pts[1]) < 1e-12);
        assert!(is_straight(&out[0], 1e-9));
    }

    #[test]
    fn a_collinear_run_stays_straight() {
        let pts: Vec<Point> = (0..12).map(|i| Point::new(i as f64 * 1.5, i as f64 * -0.25)).collect();
        let t = unit(pts[1] - pts[0]).unwrap();
        let out = fit_cubics(&pts, ends(t, t), &FitOptions::pathops(1e-6));
        assert!(!out.is_empty());
        assert!(out.iter().all(|c| is_straight(c, 1e-4)), "{out:?}");
        assert!(nearest_error(&pts, &out) <= 1e-6 + 1e-9);
    }

    #[test]
    fn duplicate_points_do_not_panic() {
        let t = Vec2::new(1.0, 0.0);
        assert!(fit_cubics(&[], ends(t, t), &FitOptions::pathops(0.1)).is_empty());
        assert!(fit_cubics(&[Point::new(0.0, 0.0)], ends(t, t), &FitOptions::pathops(0.1)).is_empty());
        let dup = vec![Point::new(1.0, 1.0); 10];
        assert!(fit_cubics(&dup, ends(t, t), &FitOptions::pathops(0.1)).is_empty());
        let mixed = [Point::new(0.0, 0.0), Point::new(0.0, 0.0), Point::new(4.0, 0.0), Point::new(4.0, 0.0)];
        let out = fit_cubics(&mixed, ends(t, t), &FitOptions::pathops(0.1));
        assert_eq!(out.len(), 1);
        assert!(out[0].p0.distance(mixed[0]) < 1e-12 && out[0].p3.distance(mixed[2]) < 1e-12);
        let traced = fit_cubics(&dup, ends(t, -t), &FitOptions::trace(0.1));
        assert!(traced.iter().all(|c| [c.p0, c.p1, c.p2, c.p3].iter().all(|p| p.x.is_finite() && p.y.is_finite())));
        assert!(fit_cubics(&[Point::new(f64::NAN, 0.0), Point::new(1.0, 0.0)], ends(t, t), &FitOptions::trace(0.1)).is_empty());
        assert!(fit_cubics(&mixed, ends(t, t), &FitOptions::trace(0.0)).is_empty());
        assert_eq!(fit_cubics(&pts_line(), ends(t, t), &FitOptions::trace(0.1)).len(), 1);
    }

    fn pts_line() -> [Point; 2] {
        [Point::new(0.0, 0.0), Point::new(3.0, 0.0)]
    }

    #[test]
    fn a_corner_policy_keeps_the_turn_and_a_smooth_policy_does_not() {
        let mut pts = Vec::new();
        for i in 0..=8 {
            pts.push(Point::new(i as f64, 0.0));
        }
        for i in 1..=8 {
            pts.push(Point::new(8.0, i as f64));
        }
        let tangents = ends(Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0));
        let mut corner = FitOptions::pathops(0.05);
        corner.join = JoinPolicy::Corner;
        let bent = fit_cubics(&pts, tangents, &corner);
        assert!(bent.len() > 1, "{bent:?}");
        let turn = bent.windows(2).map(|w| end_tangent(&w[0]).dot(start_tangent(&w[1]))).fold(1.0_f64, f64::min);
        assert!(turn < 0.5, "corner join dot {turn}");
        let smooth = fit_cubics(&pts, tangents, &FitOptions::pathops(0.05));
        assert!(smooth.len() > 1);
        assert!(g1(&smooth), "{smooth:?}");
    }

    #[test]
    fn turning_split_off_returns_one_cubic() {
        let pts: Vec<Point> = (0..=40).map(|i| Point::new(i as f64, ((i as f64) * 0.5).sin() * 10.0)).collect();
        let mut opts = FitOptions::pathops(0.1);
        opts.split = false;
        let out = fit_cubics(&pts, ends(Vec2::new(1.0, 0.0), Vec2::new(1.0, 0.0)), &opts);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn trace_preset_fits_a_circle_arc_within_tolerance() {
        let pts: Vec<Point> = (0..200).map(|i| Point::new(20.0 * (i as f64 * 0.006).cos(), 20.0 * (i as f64 * 0.006).sin())).collect();
        let t0 = unit(pts[4] - pts[0]).unwrap();
        let t1 = unit(pts[199] - pts[195]).unwrap();
        let out = fit_cubics(&pts, ends(t0, t1), &FitOptions::trace(0.05));
        assert_eq!(out.len(), 1);
        assert!(nearest_error(&pts, &out) <= 0.05 + 1e-9);
    }

    #[test]
    fn an_infinite_trace_tolerance_accepts_the_first_cubic() {
        let pts: Vec<Point> = (0..8).map(|i| Point::new(i as f64, (i as f64).sin())).collect();
        let t = Vec2::new(1.0, 0.0);
        let out = fit_cubics(&pts, ends(t, t), &FitOptions::trace(f64::INFINITY));
        assert_eq!(out.len(), 1);
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 64, failure_persistence: None, ..ProptestConfig::default() })]

        /// Random polylines stay finite, meet end to end, and are G1 where both handles have a direction.
        #[test]
        fn random_polylines_join_and_stay_finite(steps in proptest::collection::vec((0.4f64..3.0, -2.0f64..2.0), 2..18)) {
            let mut p = Point::ORIGIN;
            let mut pts = vec![p];
            for (dx, dy) in steps {
                p += Vec2::new(dx, dy);
                pts.push(p);
            }
            let t0 = unit(pts[1] - pts[0]).unwrap();
            let t1 = unit(*pts.last().unwrap() - pts[pts.len() - 2]).unwrap();
            for opts in [FitOptions::pathops(0.5), FitOptions::trace(0.5)] {
                let out = fit_cubics(&pts, ends(t0, t1), &opts);
                prop_assert!(!out.is_empty());
                prop_assert!(out.iter().all(|c| [c.p0, c.p1, c.p2, c.p3].iter().all(|q| q.x.is_finite() && q.y.is_finite())));
                prop_assert!(out[0].p0.distance(pts[0]) < 1e-9);
                prop_assert!(out.last().unwrap().p3.distance(*pts.last().unwrap()) < 1e-9);
                for w in out.windows(2) {
                    prop_assert!(w[0].p3.distance(w[1].p0) < 1e-9);
                    if (w[0].p3 - w[0].p2).hypot() > 1e-6 && (w[1].p1 - w[1].p0).hypot() > 1e-6 {
                        prop_assert!(end_tangent(&w[0]).dot(start_tangent(&w[1])) > 0.999);
                    }
                }
                if matches!(opts.kernel, Kernel::Tidy) {
                    prop_assert!(nearest_error(&pts, &out) <= 0.5 + 1e-6);
                }
            }
        }
    }
}
