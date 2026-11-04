//! OLD canvas view maths, vendored for the #188 old-vs-new equivalence test.
//!
//! Source: https://github.com/storytold/vectorcraft @ 11862f61198c8f62c0b22b6c9a331cc5950ac98d
//! (merge-base of cloudbring/vectorcraft `algo/72-use-craft-view-v2` with `origin/main`):
//!   * `crates/ui-egui/src/canvas.rs`: `impl Xf` (`new`, `to_screen`, `to_doc`, `delta_to_doc`)
//!   * `crates/ui-egui/src/canvas/view.rs`: `fit` (its arithmetic, minus the app plumbing), `fit_zoom`, `zoom_about`, `rotate_about`, `pan_pixels`, `scroll_by`.
//!
//! Bodies are verbatim; the only edits are the `Old` prefixes and `OldXf`, and `fit` taking the target
//! and returning (center, zoom) instead of writing into the app. These functions are private to the crate (a git dependency could not reach them), which is why
//! they are vendored instead of linked.

#![allow(clippy::wrong_self_convention)]

use super::super::{Point, Pos2, Rect, ScrollRest, View, pos2};

#[derive(Clone, Copy, Debug)]
pub struct OldXf {
    pub rect: egui::Rect,
    pub zoom: f64,
    pub center: Point,
    pub rot: f64,
}

impl OldXf {
    pub fn new(rect: egui::Rect, v: &View) -> Self {
        Self { rect, zoom: v.zoom, center: v.center, rot: v.rotation.to_radians() }
    }
    pub fn to_screen(&self, p: Point) -> Pos2 {
        let c = self.rect.center();
        let (dx, dy) = ((p.x - self.center.x) * self.zoom, (p.y - self.center.y) * self.zoom);
        let (sn, cs) = self.rot.sin_cos();
        pos2(c.x + (dx * cs - dy * sn) as f32, c.y + (dx * sn + dy * cs) as f32)
    }
    pub fn to_doc(&self, p: Pos2) -> Point {
        let c = self.rect.center();
        let (dx, dy) = ((p.x - c.x) as f64, (p.y - c.y) as f64);
        let (sn, cs) = self.rot.sin_cos();
        let (ux, uy) = (dx * cs + dy * sn, -dx * sn + dy * cs);
        Point::new(self.center.x + ux / self.zoom, self.center.y + uy / self.zoom)
    }
    pub fn delta_to_doc(&self, d: egui::Vec2) -> vectorcraft_geom::Vec2 {
        let (sn, cs) = self.rot.sin_cos();
        let (dx, dy) = (d.x as f64, d.y as f64);
        vectorcraft_geom::Vec2::new((dx * cs + dy * sn) / self.zoom, (-dx * sn + dy * cs) / self.zoom)
    }
}

/// `fit` with the target already chosen; `actual` is `Some(actual_size_zoom(..))` for Actual Size.
pub fn old_fit(rect: egui::Rect, target: Rect, rotation: f64, actual: Option<f64>) -> (Point, f64) {
    let zoom = match actual {
        Some(z) => z,
        None => old_fit_zoom(rect, target, rotation),
    };
    (target.center(), zoom)
}

pub fn old_fit_zoom(rect: egui::Rect, target: Rect, rotation: f64) -> f64 {
    // The width and height of the box round the target turned with the view, at zoom 1.
    let (sn, cs) = rotation.to_radians().sin_cos();
    let (w, h) = (target.width(), target.height());
    let zx = (rect.width() as f64 - 60.0) / (w * cs.abs() + h * sn.abs()).max(1.0);
    let zy = (rect.height() as f64 - 60.0) / (w * sn.abs() + h * cs.abs()).max(1.0);
    zx.min(zy).clamp(0.0313, 640.0)
}

pub fn old_zoom_about(vm: &mut View, rect: egui::Rect, p: Pos2, zoom: f64) {
    let before = OldXf::new(rect, vm).to_doc(p);
    vm.zoom = zoom.clamp(0.0313, 640.0);
    vm.center += before - OldXf::new(rect, vm).to_doc(p);
}

pub fn old_rotate_about(vm: &mut View, rect: egui::Rect, p: Pos2, degrees: f64) {
    let before = OldXf::new(rect, vm).to_doc(p);
    vm.rotation = vectorcraft_geom::normalize_deg(vm.rotation + degrees);
    vm.center += before - OldXf::new(rect, vm).to_doc(p);
}

pub fn old_pan_pixels(vm: &mut View, ppp: f32, (dx, dy): (f64, f64)) {
    let (sn, cs) = vm.rotation.to_radians().sin_cos();
    let k = vm.zoom * f64::from(ppp);
    vm.center -= vectorcraft_geom::Vec2::new((dx * cs + dy * sn) / k, (-dx * sn + dy * cs) / k);
}

pub fn old_scroll_by(vm: &mut View, doc: u64, ppp: f32, by: egui::Vec2, rest: ScrollRest) -> ScrollRest {
    let (x, y) = if rest.after == Some((doc, vm.center, vm.zoom, vm.rotation, ppp)) { (rest.x, rest.y) } else { (0.0, 0.0) };
    let (x, y) = (x + f64::from(by.x) * f64::from(ppp), y + f64::from(by.y) * f64::from(ppp));
    let (dx, dy) = (x.round(), y.round());
    old_pan_pixels(vm, ppp, (dx, dy));
    ScrollRest { x: x - dx, y: y - dy, after: Some((doc, vm.center, vm.zoom, vm.rotation, ppp)) }
}
