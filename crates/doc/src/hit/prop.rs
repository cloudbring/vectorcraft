//! The linear walk is the oracle: the spatial index must pick the same object and the same marquee.

use std::sync::Arc;

use proptest::prelude::*;
use vectorcraft_geom::{Affine, Point, Rect, shapes};

use super::{HitOptions, hit_test, hit_test_linear, hit_test_visible, hit_test_visible_linear, marquee, marquee_linear};
use crate::{Appearance, Document, Node, NodeId, NodeKind};

#[derive(Clone, Debug)]
enum Obj {
    Rect { x: f64, y: f64, w: f64, h: f64, hidden: bool, locked: bool, guide: bool },
    Group { children: Vec<Obj>, clip: bool, hidden: bool },
}

#[derive(Clone, Debug)]
struct LayerSpec {
    hidden: bool,
    locked: bool,
    template: bool,
    preview: bool,
    objs: Vec<Obj>,
}

fn arb_obj() -> impl Strategy<Value = Obj> {
    let leaf = (0.0..480.0, 0.0..480.0, 4.0..40.0, 4.0..40.0, any::<bool>(), any::<bool>(), any::<bool>())
        .prop_map(|(x, y, w, h, hidden, locked, guide)| Obj::Rect { x, y, w, h, hidden, locked, guide });
    leaf.prop_recursive(2, 16, 4, |inner| {
        (prop::collection::vec(inner, 0..5), any::<bool>(), any::<bool>()).prop_map(|(children, clip, hidden)| Obj::Group { children, clip, hidden })
    })
}

fn arb_layers() -> impl Strategy<Value = Vec<LayerSpec>> {
    prop::collection::vec(
        (any::<bool>(), any::<bool>(), any::<bool>(), any::<bool>(), prop::collection::vec(arb_obj(), 0..8))
            .prop_map(|(hidden, locked, template, preview, objs)| LayerSpec { hidden, locked, template, preview, objs }),
        1..4,
    )
}

fn make_node(doc: &mut Document, obj: &Obj) -> Node {
    match obj {
        Obj::Rect { x, y, w, h, hidden, locked, guide } => {
            let mut node = Node::path(doc.alloc_id(), shapes::rectangle(Rect::new(*x, *y, x + w, y + h)), Appearance::default_art());
            node.visible = !hidden;
            node.locked = *locked;
            if let NodeKind::Path { guide: slot, .. } = &mut node.kind {
                *slot = *guide;
            }
            node
        }
        Obj::Group { children, clip, hidden } => {
            let kids: Vec<Arc<Node>> = children.iter().map(|child| Arc::new(make_node(doc, child))).collect();
            let mut node = Node::group(doc.alloc_id(), kids);
            let clips = *clip && node.children().and_then(|kids| kids.first()).is_some_and(|child| child.path_data().is_some());
            node.set_clips(clips);
            node.visible = !hidden;
            node
        }
    }
}

fn build(specs: Vec<LayerSpec>) -> Document {
    let mut doc = Document::new(800.0, 800.0);
    for (i, spec) in specs.into_iter().enumerate() {
        let id = if i == 0 { doc.layers[0].id } else { doc.add_layer(None) };
        if let Some(layer) = doc.node_mut(id) {
            layer.visible = !spec.hidden;
            layer.locked = spec.locked;
            if let NodeKind::Layer { template, preview, .. } = &mut layer.kind {
                *template = spec.template;
                *preview = spec.preview;
            }
        }
        for obj in spec.objs {
            let node = make_node(&mut doc, &obj);
            doc.insert(Some(id), usize::MAX, node).unwrap();
        }
    }
    doc
}

fn first_of(doc: &Document, want: impl Fn(&Node) -> bool) -> Option<NodeId> {
    let mut found = None;
    doc.walk(|node| {
        if found.is_none() && want(node) {
            found = Some(node.id);
        }
    });
    found
}

fn options(doc: &Document) -> Vec<HitOptions> {
    let mut opts = vec![
        HitOptions::default(),
        HitOptions { outline: true, tol: 0.0, ..HitOptions::default() },
        HitOptions { path_only: true, skip_guides: true, clip_outlines: true, tol: 4.0, ..HitOptions::default() },
        HitOptions { tol: f64::NAN, ..HitOptions::default() },
    ];
    if let Some(scope) = first_of(doc, |node| matches!(node.kind, NodeKind::Group { .. })) {
        opts.push(HitOptions { scope: Some(scope), clip_outlines: true, ..HitOptions::default() });
    }
    opts
}

fn assert_same(doc: &Document, points: &[Point], rects: &[Rect]) {
    let opts = options(doc);
    for &point in points {
        for opt in &opts {
            assert_eq!(hit_test(doc, point, *opt), hit_test_linear(doc, point, *opt));
            assert_eq!(hit_test_visible(doc, point, *opt), hit_test_visible_linear(doc, point, *opt));
        }
    }
    let scopes = [None, first_of(doc, |node| matches!(node.kind, NodeKind::Group { .. }))];
    for &rect in rects {
        for scope in scopes {
            for leaves in [false, true] {
                assert_eq!(marquee(doc, rect, scope, leaves), marquee_linear(doc, rect, scope, leaves));
            }
        }
    }
}

fn check(doc: &Document, points: &[(f64, f64)], rects: &[Rect]) -> Result<(), TestCaseError> {
    let opts = options(doc);
    let mut pts: Vec<Point> = points.iter().copied().map(|(x, y)| Point::new(x, y)).collect();
    pts.push(Point::new(f64::NAN, 0.0));
    pts.push(Point::new(f64::INFINITY, 1.0));
    for point in pts {
        for opt in &opts {
            prop_assert_eq!(hit_test(doc, point, *opt), hit_test_linear(doc, point, *opt));
            prop_assert_eq!(hit_test_visible(doc, point, *opt), hit_test_visible_linear(doc, point, *opt));
        }
    }
    let scopes = [None, first_of(doc, |node| matches!(node.kind, NodeKind::Group { .. }))];
    let mut rects = rects.to_vec();
    rects.push(Rect::new(10.0, 10.0, 10.0, 10.0));
    rects.push(Rect::new(40.0, 40.0, 5.0, 80.0));
    for rect in rects {
        for scope in scopes {
            for leaves in [false, true] {
                prop_assert_eq!(marquee(doc, rect, scope, leaves), marquee_linear(doc, rect, scope, leaves));
            }
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn indexed_hit_and_marquee_match_the_linear_walk(
        layers in arb_layers(),
        points in prop::collection::vec((-30.0..520.0, -30.0..520.0), 4..8),
        rects in prop::collection::vec((-20.0..500.0, -20.0..500.0, 0.0..120.0, -10.0..80.0), 3..6),
    ) {
        let rects: Vec<Rect> = rects.into_iter().map(|(x, y, w, h)| Rect::new(x, y, x + w, y + h)).collect();
        let doc = build(layers);
        check(&doc, &points, &rects)?;
        let mut doc = doc;
        if let Some(id) = first_of(&doc, |node| node.path_data().is_some()) {
            if let Some(node) = doc.node_mut(id) {
                node.transform(Affine::translate((40.0, -25.0)), false);
            }
            check(&doc, &points, &rects)?;
        }
    }
}

#[test]
fn indexed_grid_matches_linear_and_sees_a_move() {
    let mut doc = Document::new(2000.0, 2000.0);
    let layer = doc.layers[0].id;
    let nodes: Vec<Arc<Node>> = (0..3000)
        .map(|i| {
            let x = (i % 60) as f64 * 30.0;
            let y = (i / 60) as f64 * 30.0;
            Arc::new(Node::path(doc.alloc_id(), shapes::rectangle(Rect::new(x, y, x + 20.0, y + 20.0)), Appearance::default_art()))
        })
        .collect();
    doc.children_mut(Some(layer)).unwrap().extend(nodes);
    let points = [Point::new(10.0, 10.0), Point::new(25.0, 10.0), Point::new(500.0, 400.0), Point::new(-5.0, -5.0)];
    let rects = [Rect::new(0.0, 0.0, 100.0, 50.0), Rect::new(900.0, 600.0, 980.0, 640.0), Rect::new(80.0, 40.0, 10.0, 20.0)];
    assert_same(&doc, &points, &rects);
    let id = doc.layers[0].children().and_then(|kids| kids.first()).map(|node| node.id).unwrap();
    doc.node_mut(id).unwrap().transform(Affine::translate((15.0, 0.0)), false);
    assert_same(&doc, &points, &rects);
}
