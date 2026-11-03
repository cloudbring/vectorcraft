use super::drag::{drag_id, drop_press};
use super::{CornerRadius, Drag, Point, Pos2, Rect, SCROLL_BAR, Sense, Stroke, Tokens, Ui, VectorcraftApp, View, Xf, pos2, vec2};
use craft_view::{Anchor, FitMode, ViewXform, ZoomPolicy, doc_tuple};

/// Fit the view (View → Fit Artboard / Fit All / Actual Size, Arrange Tools › Zoom to Selection).
pub fn fit(app: &mut VectorcraftApp, how: &str) {
    let Some(rect) = app.canvas_rect else {
        if let Some(v) = app.view_mut() {
            v.fitted = false;
        }
        return;
    };
    let current = app.view().map_or(0, |v| v.artboard);
    let Some(st) = app.session.active() else { return };
    let target = match how {
        "view.fitAll" => {
            st.doc.art_bounds().map(|a| st.doc.artboards.iter().fold(a, |r, ab| r.union(ab.rect))).or(st.doc.artboards.first().map(|a| a.rect))
        }
        crate::addons::arrange::ZOOM => st.doc.bounds_of(&st.selection.objects, true),
        // The navigator's artboard (the first if it has gone).
        _ => st.doc.artboards.get(current).or(st.doc.artboards.first()).map(|a| a.rect),
    };
    let Some(target) = target else { return };
    let actual = (how == "view.actualSize").then(|| actual_size_zoom(app.session.prefs.display_print_size));
    let Some(v) = app.view_mut() else { return };
    with_xform(v, rect, |xf| {
        xf.fit_with_mode([[target.x0, target.y0], [target.x1, target.y1]], 30.0, FitMode::Contain, &ZOOM_POLICY);
        if let Some(zoom) = actual {
            xf.set_zoom_keep_centre(zoom, &ZOOM_POLICY);
        }
    });
    v.fitted = true;
}

/// The zoom at which `target`, turned `rotation` degrees with the view, fits in `rect` with a
/// 30-point margin on each side.
pub(super) fn fit_zoom(rect: egui::Rect, target: Rect, rotation: f64) -> f64 {
    let mut v = View { rotation, ..View::default() };
    with_xform(&mut v, rect, |xf| xf.fit_with_mode([[target.x0, target.y0], [target.x1, target.y1]], 30.0, FitMode::Contain, &ZOOM_POLICY));
    v.zoom
}

/// How much one point of wheel motion zooms (as `exp(points × WHEEL_ZOOM)`).
const WHEEL_ZOOM: f64 = 0.01;

/// How fast a wheel notch's zoom glides in with Zoom with Mouse Wheel: the time constant (s) of
/// the part still to come.
const WHEEL_GLIDE: f64 = 0.05;

/// What the wheel did across frames ([`wheel`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct WheelTurn {
    /// The last wheel turn was an Alt-wheel one.
    alt: bool,
    /// The zoom (natural log) wheel notches still have to glide in.
    pub(super) glide: f64,
}

/// Animated Zoom: how much one point of sideways drag zooms (100 points to the right double it).
pub(super) const SCRUB_ZOOM: f64 = std::f64::consts::LN_2 / 100.0;

/// Animated Zoom: how far the pointer moves before a press is a drag, in points.
pub(super) const SCRUB_SLOP: f32 = 3.0;

/// Animated Zoom: how long a press is held still before it zooms on, in seconds…
pub(super) const HOLD_DELAY: f32 = 0.3;

/// …and how fast it zooms then (as `exp(seconds × HOLD_ZOOM)`: twice as close each second).
pub(super) const HOLD_ZOOM: f64 = std::f64::consts::LN_2;

/// Performance › Animated Zoom, which needs GPU Performance as in Illustrator: the Zoom tool zooms
/// as it is dragged sideways or held, instead of zooming to the area dragged across, and a wheel
/// notch's zoom glides in ([`wheel`]).
pub(crate) fn animated_zoom(p: &vectorcraft_engine::Prefs) -> bool {
    p.animated_zoom && p.gpu_performance
}

/// Zoom for View → Actual Size (`view.actualSize`). With Preferences › General › Display Print
/// Size at 100% Zoom off, one document point is one screen point. On, one document inch (72 pt)
/// fills an inch of the screen as the system counts it: 96 screen points, the reference density
/// that display scaling is set against (a CSS inch), whatever the scale factor.
pub(crate) fn actual_size_zoom(display_print_size: bool) -> f64 {
    if display_print_size { 96.0 / 72.0 } else { 1.0 }
}

const ZOOM_POLICY: ZoomPolicy = ZoomPolicy { min: 0.0313, max: 640.0, steps: &[], anchor: Anchor::Pointer };

fn with_xform(vm: &mut View, rect: egui::Rect, f: impl FnOnce(&mut ViewXform)) -> f64 {
    let mut xf = Xf::new(rect, vm).xform();
    f(&mut xf);
    (vm.center, vm.zoom) = (Point::from(doc_tuple(xf.centre)), xf.zoom);
    vectorcraft_geom::normalize_deg(xf.rotation_degrees())
}

/// Zoom view `vm` of the canvas `rect` to `zoom` (clamped to the zoom range), keeping the document
/// point under screen point `p` where it is.
pub(super) fn zoom_about(vm: &mut View, rect: egui::Rect, p: Pos2, zoom: f64) {
    with_xform(vm, rect, |xf| xf.zoom_about(p, zoom, &ZOOM_POLICY));
}

/// Turn view `vm` of the canvas `rect` by `degrees` clockwise, keeping the document point under
/// screen point `p` where it is.
pub(super) fn rotate_about(vm: &mut View, rect: egui::Rect, p: Pos2, degrees: f64) {
    vm.rotation = with_xform(vm, rect, |xf| xf.rotate_about(p, degrees.to_radians()));
}

/// Move view `vm`'s content by `(dx, dy)` device pixels along the screen's axes, at `ppp` device
/// pixels a point: the canvas's frame moves by exactly those pixels.
pub(super) fn pan_pixels(vm: &mut View, ppp: f32, (dx, dy): (f64, f64)) {
    with_xform(vm, egui::Rect::NOTHING, |xf| xf.pan_pixels(ppp, (dx, dy)));
}

/// Screen points `d` in the nearest whole device pixels, at `ppp` device pixels a point.
pub(super) fn whole_pixels(d: egui::Vec2, ppp: f32) -> (f64, f64) {
    ((f64::from(d.x) * f64::from(ppp)).round(), (f64::from(d.y) * f64::from(ppp)).round())
}

/// What the scrolls of the view have left to move ([`scroll_view`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct ScrollRest {
    /// Device pixels along the screen's axes, at most half a pixel each.
    pub(super) x: f64,
    pub(super) y: f64,
    /// The document, the view's center, zoom and rotation, and the device pixels a point that the
    /// last scroll left.
    after: Option<(u64, Point, f64, f64, f32)>,
}

/// Scroll the active view so that its content moves `by` screen points (the wheel, a trackpad, a
/// touch screen, a scroll bar), in whole device pixels: the part of a pixel left over is added to
/// the next scroll. It is dropped when the view changed in between (a zoom, a turn, Fit, a pan
/// of another kind, another document) or the device pixels a point did.
pub(super) fn scroll_view(app: &mut VectorcraftApp, ui: &Ui, by: egui::Vec2) {
    let ppp = ui.ctx().pixels_per_point();
    let Some(doc) = app.session.active().map(|st| st.uid) else { return };
    let Some(vm) = app.view_mut() else { return };
    let id = egui::Id::unique("canvas-scroll-rest");
    let rest = scroll_by(vm, doc, ppp, by, ui.data(|d| d.get_temp(id)).unwrap_or_default());
    ui.data_mut(|d| d.insert_temp(id, rest));
}

/// [`scroll_view`] for view `vm` of document `doc` at `ppp` device pixels a point, after the
/// scrolls that left `rest`: what this scroll leaves.
pub(super) fn scroll_by(vm: &mut View, doc: u64, ppp: f32, by: egui::Vec2, rest: ScrollRest) -> ScrollRest {
    let (x, y) = if rest.after == Some((doc, vm.center, vm.zoom, vm.rotation, ppp)) { (rest.x, rest.y) } else { (0.0, 0.0) };
    let (x, y) = (x + f64::from(by.x) * f64::from(ppp), y + f64::from(by.y) * f64::from(ppp));
    let (dx, dy) = (x.round(), y.round());
    pan_pixels(vm, ppp, (dx, dy));
    ScrollRest { x: x - dx, y: y - dy, after: Some((doc, vm.center, vm.zoom, vm.rotation, ppp)) }
}

/// How far a trackpad rotation turns before it turns the view, in degrees.
const ROTATE_DEAD_ZONE: f64 = 5.0;

/// How long after its last rotation event a trackpad rotation ends, in seconds.
const ROTATE_GESTURE_END: f64 = 0.25;

/// A trackpad rotation under way ([`trackpad_turn`]).
#[derive(Clone, Copy, Debug)]
struct TrackpadTurn {
    /// Degrees it has turned so far, clockwise.
    total: f64,
    /// When its last rotation event came (egui's input time, in seconds).
    last: f64,
    /// It has reached [`ROTATE_DEAD_ZONE`] and turns the view.
    turning: bool,
}

/// General › Trackpad Gesture to Rotate View (`on`): the degrees, clockwise, that a two-finger
/// rotation on a Mac trackpad turns the view this frame. egui sends the rotation as
/// [`egui::Event::Rotate`], from winit on macOS and from Safari's gesture events in the web build.
/// A rotation does not turn the view until it reaches [`ROTATE_DEAD_ZONE`]; it then turns the view
/// by all of it, and by each later event as it comes. It ends [`ROTATE_GESTURE_END`] after its last
/// event. Rotation events that come while fingers are on a touch screen do not turn the view.
pub(super) fn trackpad_turn(ui: &Ui, on: bool) -> Option<f64> {
    if !on {
        return None;
    }
    let (deg, now) = ui.input(|i| {
        let rad: f32 =
            if i.multi_touch().is_some() { 0.0 } else { i.events.iter().map(|e| if let egui::Event::Rotate(r) = e { *r } else { 0.0 }).sum() };
        (f64::from(rad).to_degrees(), i.time)
    });
    if deg == 0.0 || !deg.is_finite() {
        return None;
    }
    let id = egui::Id::unique("canvas-trackpad-turn");
    let kept: Option<TrackpadTurn> = ui.data(|d| d.get_temp(id));
    let mut t = kept.filter(|t| now - t.last <= ROTATE_GESTURE_END).unwrap_or(TrackpadTurn { total: 0.0, last: now, turning: false });
    t.total += deg;
    t.last = now;
    let turn = if t.turning {
        Some(deg)
    } else if t.total.abs() >= ROTATE_DEAD_ZONE {
        t.turning = true;
        Some(t.total)
    } else {
        None
    };
    ui.data_mut(|d| d.insert_temp(id, t));
    turn
}

/// One scroll bar along its axis, in screen points: the span it scrolls over (`lo..hi`) and the
/// part of it the canvas shows (`v0..v1`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Span {
    pub(super) lo: f32,
    pub(super) hi: f32,
    pub(super) v0: f32,
    pub(super) v1: f32,
}

impl Span {
    /// The thumb on a track `len` long: where it starts and how long it is.
    pub(super) fn thumb(self, len: f32) -> (f32, f32) {
        let total = (self.hi - self.lo).max(1.0);
        let size = (len * (self.v1 - self.v0) / total).clamp(MIN_THUMB.min(len), len);
        let room = total - (self.v1 - self.v0);
        let at = if room > 0.0 { (self.v0 - self.lo) / room * (len - size) } else { 0.0 };
        (at.clamp(0.0, (len - size).max(0.0)), size)
    }
    /// Screen points the view moves for each point the thumb moves on a track `len` long.
    pub(super) fn ratio(self, len: f32) -> f32 {
        let room = (self.hi - self.lo) - (self.v1 - self.v0);
        let free = len - self.thumb(len).1;
        if room > 0.0 && free > 0.0 { room / free } else { 0.0 }
    }
}

const MIN_THUMB: f32 = 20.0;

/// The scroll bars' spans, horizontal then vertical: the art and the artboards (`content`) as the
/// view turns them on screen, half a window past each side, and the view itself, so the thumb is
/// always on its track. The canvas has no edge of its own (no drawing area), so the content sets it.
pub(super) fn scroll_spans(xf: &Xf, content: Option<Rect>) -> [Span; 2] {
    let r = xf.rect;
    let around = content.map(|c| egui::Rect::from_points(&xf.quad(c)).expand2(r.size() / 2.0)).filter(|b| b.is_finite());
    let b = around.map_or(r, |b| b.union(r));
    [Span { lo: b.left(), hi: b.right(), v0: r.left(), v1: r.right() }, Span { lo: b.top(), hi: b.bottom(), v0: r.top(), v1: r.bottom() }]
}

/// The tracks of the horizontal and the vertical scroll bar, along the bottom and the right edge
/// of the window (`outer`), as long as the canvas (`rect`) beside them.
pub(super) fn scroll_tracks(rect: egui::Rect, outer: egui::Rect) -> [egui::Rect; 2] {
    let corner = outer.max - vec2(SCROLL_BAR, SCROLL_BAR);
    [
        egui::Rect::from_min_max(pos2(rect.left(), corner.y), pos2(corner.x, outer.bottom())),
        egui::Rect::from_min_max(pos2(corner.x, rect.top()), pos2(outer.right(), corner.y)),
    ]
}

/// The window's scroll bars ([`scroll_spans`]): a thumb dragged scrolls the view along the
/// screen's axis (a turned view too), a click on the track pages towards it, and the wheel over a
/// bar scrolls along it.
pub(super) fn scroll_bars(app: &mut VectorcraftApp, ui: &Ui, rect: egui::Rect, outer: egui::Rect, t: &Tokens) {
    let content = app.content_bounds();
    let Some(v) = app.view().copied() else { return };
    let xf = Xf::new(rect, &v);
    let painter = ui.painter();
    painter.rect_filled(egui::Rect::from_min_max(outer.max - vec2(SCROLL_BAR, SCROLL_BAR), outer.max), 0.0, t.ruler);
    let hover = ui.input(|i| i.pointer.hover_pos());
    // Screen points the view moves this frame, or the center a dragged thumb's drag began at and
    // the screen points it has moved the view since.
    let mut moved = egui::Vec2::ZERO;
    let mut center = None;
    for (k, (track, span)) in scroll_tracks(rect, outer).into_iter().zip(scroll_spans(&xf, content)).enumerate() {
        let horizontal = k == 0;
        let along = |p: Pos2| if horizontal { p.x - track.left() } else { p.y - track.top() };
        let axis = if horizontal { vec2(1.0, 0.0) } else { vec2(0.0, 1.0) };
        let len = if horizontal { track.width() } else { track.height() };
        let (at, size) = span.thumb(len);
        let on_thumb = |p: Pos2| (at..=at + size).contains(&along(p));
        let id = egui::Id::unique(("canvas-scroll-bar", k));
        let resp = ui.interact(track, id, Sense::click_and_drag());
        if resp.drag_started()
            && let Some(p) = ui.input(|i| i.pointer.press_origin())
            && on_thumb(p)
        {
            // The span moves with the view: the drag keeps the one it started with.
            let d = ThumbDrag { from: along(p), center: v.center, ratio: span.ratio(len), min: span.lo - span.v0, max: span.hi - span.v1 };
            ui.data_mut(|dd| dd.insert_temp(id, d));
        }
        let drag: Option<ThumbDrag> = ui.data(|dd| dd.get_temp(id));
        if let Some(d) = drag {
            if resp.dragged()
                && let Some(p) = resp.interact_pointer_pos()
            {
                let by = ((along(p) - d.from) * d.ratio).clamp(d.min, d.max);
                center = Some((d.center, axis * by));
            }
            if !resp.dragged() || resp.drag_stopped() {
                ui.data_mut(|dd| dd.remove::<ThumbDrag>(id));
            }
        } else if resp.clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            // A page is most of the window, and never past the end of the span.
            let page = 0.9 * (span.v1 - span.v0);
            if along(p) < at {
                moved -= axis * page.min(span.v0 - span.lo);
            } else if along(p) > at + size {
                moved += axis * page.min(span.hi - span.v1);
            }
        }
        if resp.hovered() {
            let d = ui.input(|i| i.smooth_scroll_delta);
            moved -= axis * if horizontal { d.x + d.y } else { d.y };
        }
        painter.rect_filled(track, 0.0, t.ruler);
        let edge = if horizontal { [track.left_top(), track.right_top()] } else { [track.left_top(), track.left_bottom()] };
        painter.line_segment(edge, Stroke::new(1.0, t.border));
        let thumb = if horizontal {
            egui::Rect::from_min_size(pos2(track.left() + at, track.top()), vec2(size, track.height())).shrink2(vec2(1.0, 3.0))
        } else {
            egui::Rect::from_min_size(pos2(track.left(), track.top() + at), vec2(track.width(), size)).shrink2(vec2(3.0, 1.0))
        };
        let hot = drag.is_some() || hover.is_some_and(|p| thumb.contains(p));
        painter.rect_filled(thumb, CornerRadius::same(3), if hot { t.text_dim } else { t.text_disabled });
    }
    let ppp = ui.ctx().pixels_per_point();
    if let Some((c, by)) = center
        && let Some(vm) = app.view_mut()
    {
        vm.center = c;
        pan_pixels(vm, ppp, whole_pixels(-by, ppp));
    }
    if moved != egui::Vec2::ZERO {
        scroll_view(app, ui, -moved);
    }
}

/// A scroll bar thumb being dragged: where along the track it was pressed, and the view's centre,
/// the span's travel ratio ([`Span::ratio`]) and how far the view could move either way then.
#[derive(Clone, Copy, Debug)]
struct ThumbDrag {
    from: f32,
    center: Point,
    ratio: f32,
    min: f32,
    max: f32,
}

/// What the wheel, a pinch and a two-finger drag on a touch screen did over the canvas this frame:
/// a zoom factor (about the pointer) and a scroll (screen points the content moves). The wheel scrolls, Cmd- and Alt-wheel (Option
/// on the Mac) zoom. With General › Zoom with Mouse Wheel (`wheel_zooms`) the wheel and Alt-wheel
/// zoom, Shift-wheel scrolls up and down and Cmd/Ctrl-wheel sideways. `line` and `page`: points
/// per wheel line and page. `turn`: kept by the caller across frames. With Performance › Animated
/// Zoom (`animate`) a mouse wheel's notches zoom in a glide over a few frames, as scrolling does
/// (#888); off, each notch zooms on the frame it arrives (#4283). A trackpad's fine steps zoom at
/// once either way.
pub(super) fn wheel(i: &egui::InputState, wheel_zooms: bool, animate: bool, line: f32, page: f32, turn: &mut WheelTurn) -> (f64, egui::Vec2) {
    // Fingers on a touch screen pan the content with them (a trackpad sends scrolls instead).
    let pan = i.multi_touch().map_or(egui::Vec2::ZERO, |t| t.translation_delta);
    // A wheel event's motion in points.
    let points = |unit: &egui::MouseWheelUnit, delta: egui::Vec2| match unit {
        egui::MouseWheelUnit::Point => delta,
        egui::MouseWheelUnit::Line => delta * line,
        egui::MouseWheelUnit::Page => delta * page,
    };
    if !wheel_zooms {
        // egui makes Cmd-wheel (and a pinch) its zoom and the rest a scroll it spreads over a few
        // frames: the rest of an Alt-wheel turn zooms too, however soon Alt is let go.
        if let Some(m) = i.events.iter().rev().find_map(|e| if let egui::Event::MouseWheel { modifiers, .. } = e { Some(modifiers) } else { None }) {
            turn.alt = m.alt && !m.command;
        }
        let (zoom, scroll) = (f64::from(i.zoom_delta()), i.smooth_scroll_delta);
        if turn.alt && !animate {
            // Without Animated Zoom, Alt-wheel zooms by the wheel events themselves, on the frame
            // they arrive; the rest of egui's spread scroll is dropped.
            let d: f32 = i
                .events
                .iter()
                .filter_map(|e| if let egui::Event::MouseWheel { unit, delta, .. } = e { Some(points(unit, *delta)) } else { None })
                .map(|d| d.x + d.y)
                .sum();
            return (zoom * (f64::from(d) * WHEEL_ZOOM).exp(), pan);
        }
        return if turn.alt && scroll != egui::Vec2::ZERO {
            (zoom * (f64::from(scroll.x + scroll.y) * WHEEL_ZOOM).exp(), pan)
        } else {
            (zoom, scroll + pan)
        };
    }
    // The wheel events themselves: egui's own handling turns Cmd-wheel into a zoom.
    let mut zoom = i.multi_touch().map_or(1.0, |t| f64::from(t.zoom_delta));
    let mut scroll = pan;
    for e in &i.events {
        match e {
            egui::Event::MouseWheel { unit, delta, modifiers, .. } => {
                let d = points(unit, *delta);
                if modifiers.command {
                    scroll.x += d.x + d.y;
                } else if modifiers.shift {
                    scroll.y += d.x + d.y;
                } else if *unit == egui::MouseWheelUnit::Point || !animate {
                    zoom *= (f64::from(d.y) * WHEEL_ZOOM).exp();
                    scroll.x += d.x;
                } else {
                    turn.glide += f64::from(d.y) * WHEEL_ZOOM;
                    scroll.x += d.x;
                }
            }
            egui::Event::Zoom(f) => zoom *= f64::from(*f),
            _ => {}
        }
    }
    // This frame's part of the glide: what is left shrinks by e every WHEEL_GLIDE seconds.
    let part = if turn.glide.abs() < 1e-3 { 1.0 } else { 1.0 - (-f64::from(i.stable_dt.min(0.1)) / WHEEL_GLIDE).exp() };
    let step = turn.glide * part;
    turn.glide -= step;
    (zoom * step.exp(), scroll)
}

/// Enable Touch Gestures: this frame's finger contacts on the canvas ([`crate::touch`]). A second
/// finger drops the press the first one began (its tool interaction rolled back; the rest of the
/// gesture presses nothing), and a tap of two fingers undoes, one of three redoes.
pub(super) fn touch_gestures(app: &mut VectorcraftApp, ui: &Ui, rect: egui::Rect, view: vectorcraft_engine::ViewInfo) {
    if !app.session.prefs.touch_gestures {
        return;
    }
    let (touches, now): (Vec<_>, f64) = ui.input(|i| {
        let t = i.events.iter().filter_map(|e| match e {
            egui::Event::Touch { id, phase, pos, .. } => Some((*id, *phase, *pos)),
            _ => None,
        });
        (t.collect(), i.time)
    });
    if touches.is_empty() {
        return;
    }
    let key = egui::Id::unique("canvas-touch");
    let mut taps: crate::touch::Taps = ui.data(|d| d.get_temp(key)).unwrap_or_default();
    let mut tap = None;
    for (id, phase, pos) in touches {
        // A gesture begins on the canvas.
        if phase == egui::TouchPhase::Start && !taps.is_down() && !rect.contains(pos) {
            continue;
        }
        // A cancelled contact (a palm the shell drops when a pen lands) keeps nothing it began.
        if phase == egui::TouchPhase::Cancel {
            drop_press(app, ui, view);
        }
        match taps.feed(id, phase, pos, now) {
            Some(crate::touch::Touch::Fingers) => {
                drop_press(app, ui, view);
                ui.data_mut(|d| d.insert_temp(drag_id(), Drag::Gesture));
            }
            Some(crate::touch::Touch::Tap(n)) => tap = crate::touch::tap_command(n),
            None => {}
        }
    }
    ui.data_mut(|d| d.insert_temp(key, taps));
    // In Presentation Mode a tap of two or three fingers undoes or redoes nothing.
    if let Some(cmd) = tap
        && app.ui.screen_mode < 3
    {
        crate::menus::invoke(app, cmd, serde_json::json!({}));
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    struct OldXf(egui::Rect, f64, Point, f64);
    impl OldXf {
        fn new(rect: egui::Rect, v: &View) -> Self {
            Self(rect, v.zoom, v.center, v.rotation.to_radians())
        }
        fn to_screen(&self, p: Point) -> Pos2 {
            let (c, (sn, cs)) = (self.0.center(), self.3.sin_cos());
            let (dx, dy) = ((p.x - self.2.x) * self.1, (p.y - self.2.y) * self.1);
            pos2(c.x + (dx * cs - dy * sn) as f32, c.y + (dx * sn + dy * cs) as f32)
        }
        fn to_doc(&self, p: Pos2) -> Point {
            let (c, (sn, cs)) = (self.0.center(), self.3.sin_cos());
            let (dx, dy) = ((p.x - c.x) as f64, (p.y - c.y) as f64);
            Point::new(self.2.x + (dx * cs + dy * sn) / self.1, self.2.y + (-dx * sn + dy * cs) / self.1)
        }
    }
    fn old_zoom_about(vm: &mut View, rect: egui::Rect, p: Pos2, zoom: f64) {
        let before = OldXf::new(rect, vm).to_doc(p);
        vm.zoom = zoom.clamp(0.0313, 640.0);
        vm.center += before - OldXf::new(rect, vm).to_doc(p);
    }
    fn old_rotate_about(vm: &mut View, rect: egui::Rect, p: Pos2, degrees: f64) {
        let before = OldXf::new(rect, vm).to_doc(p);
        vm.rotation = vectorcraft_geom::normalize_deg(vm.rotation + degrees);
        vm.center += before - OldXf::new(rect, vm).to_doc(p);
    }
    fn old_pan_pixels(vm: &mut View, ppp: f32, (dx, dy): (f64, f64)) {
        let ((sn, cs), k) = (vm.rotation.to_radians().sin_cos(), vm.zoom * f64::from(ppp));
        vm.center -= vectorcraft_geom::Vec2::new((dx * cs + dy * sn) / k, (-dx * sn + dy * cs) / k);
    }
    fn old_fit_zoom(rect: egui::Rect, target: Rect, rotation: f64) -> f64 {
        let ((sn, cs), (w, h)) = (rotation.to_radians().sin_cos(), (target.width(), target.height()));
        let zx = (rect.width() as f64 - 60.0) / (w * cs.abs() + h * sn.abs()).max(1.0);
        let zy = (rect.height() as f64 - 60.0) / (w * sn.abs() + h * cs.abs()).max(1.0);
        zx.min(zy).clamp(0.0313, 640.0)
    }
    fn sample(seed: &mut u64, min: f64, max: f64) -> f64 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        min + ((*seed >> 11) as f64 / (1_u64 << 53) as f64) * (max - min)
    }
    fn same(a: &View, b: &View) {
        assert!((a.center.x - b.center.x).abs() <= 1e-9 && (a.center.y - b.center.y).abs() <= 1e-9);
        assert!(a.zoom == b.zoom && (a.rotation - b.rotation).abs() <= 1e-12);
    }
    #[test]
    fn shared_view_matches_previous_algebra() {
        let mut s = 72;
        for i in 0..2000 {
            let mut r32 = |lo, hi| (sample(&mut s, lo, hi) as f32).round();
            let rect = egui::Rect::from_min_size(pos2(r32(-500.0, 500.0), r32(-500.0, 500.0)), vec2(r32(61.0, 2000.0), r32(61.0, 2000.0)));
            let pt = pos2(r32(-500.0, 2500.0), r32(-500.0, 2500.0));
            let vm = View {
                center: Point::new(sample(&mut s, -1000.0, 1000.0), sample(&mut s, -1000.0, 1000.0)),
                zoom: sample(&mut s, 0.0313, 640.0),
                rotation: sample(&mut s, -179.0, 179.0),
                ..View::default()
            };
            let doc_pt = vm.center + vectorcraft_geom::Vec2::new(sample(&mut s, -500.0, 500.0) / vm.zoom, sample(&mut s, -500.0, 500.0) / vm.zoom);
            let (ds, dd) = (
                Xf::new(rect, &vm).to_screen(doc_pt) - OldXf::new(rect, &vm).to_screen(doc_pt),
                Xf::new(rect, &vm).to_doc(pt) - OldXf::new(rect, &vm).to_doc(pt),
            );
            assert!(ds.x.abs() <= 1e-3 && ds.y.abs() <= 1e-3 && dd.x.abs() <= 1e-9 && dd.y.abs() <= 1e-9);
            let (ppp, z, deg, d) = (
                [1.0, 1.5, 2.0][i % 3],
                sample(&mut s, 0.5, 640.0),
                sample(&mut s, -180.0, 180.0),
                (sample(&mut s, -1000.0, 1000.0), sample(&mut s, -1000.0, 1000.0)),
            );
            let (mut a, mut b) = (vm, vm);
            zoom_about(&mut a, rect, pt, z);
            old_zoom_about(&mut b, rect, pt, z);
            same(&a, &b);
            (a, b) = (vm, vm);
            rotate_about(&mut a, rect, pt, deg);
            old_rotate_about(&mut b, rect, pt, deg);
            same(&a, &b);
            (a, b) = (vm, vm);
            pan_pixels(&mut a, ppp, d);
            old_pan_pixels(&mut b, ppp, d);
            same(&a, &b);
            let target = Rect::new(0.0, 0.0, sample(&mut s, 1.0, 2000.0), sample(&mut s, 1.0, 2000.0));
            assert!((fit_zoom(rect, target, vm.rotation) - old_fit_zoom(rect, target, vm.rotation)).abs() <= 1e-12);
        }
    }
}
