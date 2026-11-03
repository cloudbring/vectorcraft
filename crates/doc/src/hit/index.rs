//! Packed Hilbert R-tree of object reach boxes, in document space.
//!
//! A click or a marquee asks this for the children of one parent whose boxes meet the query, then
//! the existing walk ([`super::hit_test`], [`super::marquee`]) applies locks, clips, guides and the
//! exact test. The tree is a candidate filter: a miss here is a miss, and a hit is only a maybe.
//!
//! There is no geometry revision on [`crate::Document`]. [`HitIndex`] counts edits that go through
//! [`crate::Document::node_mut`] and the other tree-editing methods, and the stamp also mixes the
//! top-level layer arcs (and each one's child count), symbol, pattern and graph-design art, and
//! the registered live-input and image-paint hooks. An in-place `Arc::make_mut` that bypasses
//! those methods is not seen.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use vectorcraft_geom::{Point, Rect};

use super::edits_contents;
use crate::{Document, Node, NodeId};

const HILBERT_N: u32 = 65_536;
const LEAF: u32 = 16;

thread_local! {
    static ACTIVE: RefCell<Option<Arc<HitTree>>> = const { RefCell::new(None) };
}

/// Cache of one document's hit tree. Equal for any two documents: it is not content.
#[derive(Default)]
pub(crate) struct HitIndex {
    revision: AtomicU64,
    slot: RwLock<Option<Cached>>,
}

#[derive(Clone)]
struct Cached {
    stamp: u64,
    tree: Arc<HitTree>,
}

impl HitIndex {
    /// The tree no longer matches this document. The next query builds it again.
    pub(crate) fn bump(&self) {
        self.revision.fetch_add(1, Ordering::AcqRel);
    }

    fn fresh(&self, stamp: u64) -> Option<Arc<HitTree>> {
        let slot = self.slot.read().ok()?;
        let cached = slot.as_ref()?;
        (cached.stamp == stamp).then(|| Arc::clone(&cached.tree))
    }

    fn store(&self, stamp: u64, tree: Arc<HitTree>) {
        let Ok(mut slot) = self.slot.write() else { return };
        if slot.as_ref().is_some_and(|cached| cached.stamp == stamp) {
            return;
        }
        *slot = Some(Cached { stamp, tree });
    }
}

impl Clone for HitIndex {
    fn clone(&self) -> Self {
        let revision = self.revision.load(Ordering::Acquire);
        let slot = self.slot.read().ok().and_then(|guard| guard.clone());
        Self { revision: AtomicU64::new(revision), slot: RwLock::new(slot) }
    }
}

impl PartialEq for HitIndex {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for HitIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HitIndex")
    }
}

/// Installs this document's tree for the current thread. `use_index` false installs nothing, so
/// the walk stays linear (the test oracle). Drop puts the previous tree back.
pub(super) struct Guard {
    prev: Option<Arc<HitTree>>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        let prev = self.prev.take();
        ACTIVE.with(|slot| {
            if let Ok(mut current) = slot.try_borrow_mut() {
                *current = prev;
            }
        });
    }
}

pub(super) fn prepare(doc: &Document, use_index: bool) -> Guard {
    let prev = ACTIVE.with(|slot| slot.borrow().clone());
    let next = use_index.then(|| cached_tree(doc));
    ACTIVE.with(|slot| *slot.borrow_mut() = next);
    Guard { prev }
}

/// Children of `parent` whose stored reach meets `rect`, ascending and without duplicates.
/// `None` when no tree is installed, the tree could not be built, `rect` is not finite, or
/// `parent` was not in the document when the tree was built (a repeat's expanded copies are new
/// nodes): the caller walks every child. An empty `Some` is a real miss.
pub(super) fn overlapping_children(parent: &Node, rect: Rect) -> Option<Vec<usize>> {
    if !finite_rect(rect) {
        return None;
    }
    let tree = ACTIVE.with(|slot| slot.borrow().clone())?;
    if tree.fallback || !tree.parents.contains(&node_addr(parent)) {
        return None;
    }
    Some(tree.query(parent.id, rect.abs()))
}

/// Closed box around `p` grown by `tol`. `None` when the point or the tolerance is not a
/// finite, non-negative distance: the caller walks every child.
pub(super) fn point_query_rect(p: Point, tol: f64) -> Option<Rect> {
    if !p.x.is_finite() || !p.y.is_finite() || !tol.is_finite() || tol < 0.0 {
        return None;
    }
    Some(Rect::new(p.x - tol, p.y - tol, p.x + tol, p.y + tol))
}

fn cached_tree(doc: &Document) -> Arc<HitTree> {
    let stamp = stamp_of(doc);
    if let Some(tree) = doc.hit_index.fresh(stamp) {
        return tree;
    }
    let built = Arc::new(build(doc));
    if stamp_of(doc) == stamp {
        doc.hit_index.store(stamp, Arc::clone(&built));
    }
    built
}

fn stamp_of(doc: &Document) -> u64 {
    let mut hash = doc.hit_index.revision.load(Ordering::Acquire);
    hash = mix(hash, doc.layers.len() as u64);
    for layer in &doc.layers {
        hash = mix(hash, arc_addr(layer));
        hash = mix(hash, layer.children().map_or(0, Vec::len) as u64);
    }
    for symbol in &doc.symbols {
        hash = mix(hash, arc_addr(&symbol.art));
    }
    for pattern in &doc.patterns {
        hash = mix(hash, pattern.art.len() as u64);
        for art in &pattern.art {
            hash = mix(hash, arc_addr(art));
        }
    }
    for design in &doc.graph_designs {
        hash = mix(hash, arc_addr(&design.art));
    }
    mix(mix(hash, super::live_inputs()), image_paint_stamp())
}

fn mix(hash: u64, value: u64) -> u64 {
    hash.wrapping_mul(0x100_0000_01b3) ^ value
}

fn arc_addr(node: &Arc<Node>) -> u64 {
    Arc::as_ptr(node).addr() as u64
}

fn image_paint_stamp() -> u64 {
    u64::from(super::IMAGE_PAINT.get().is_some())
}

struct Item {
    parent: NodeId,
    index: u32,
    bounds: Rect,
}

struct BvhNode {
    bounds: Rect,
    /// `u32::MAX` on a leaf. An interior node has both children.
    left: u32,
    right: u32,
    start: u32,
    count: u32,
}

struct HitTree {
    /// The document did not fit in the flat tree. Queries fall back to the linear walk.
    fallback: bool,
    root: Option<u32>,
    nodes: Vec<BvhNode>,
    items: Vec<Item>,
    always: HashMap<NodeId, Vec<u32>>,
    /// Address of each container indexed. A node that is not here (expanded repeat copies are
    /// new allocations, often all [`NodeId`] zero) is walked in full.
    parents: HashSet<usize>,
}

struct Built {
    items: Vec<Item>,
    always: Vec<(NodeId, u32)>,
    parents: HashSet<usize>,
    fallback: bool,
}

impl HitTree {
    fn unusable() -> Self {
        Self { fallback: true, root: None, nodes: Vec::new(), items: Vec::new(), always: HashMap::new(), parents: HashSet::new() }
    }

    fn query(&self, parent: NodeId, rect: Rect) -> Vec<usize> {
        let mut out = Vec::new();
        if let Some(root) = self.root {
            self.collect(root, rect, parent, &mut out);
        }
        if let Some(extra) = self.always.get(&parent) {
            out.extend(extra.iter().map(|index| *index as usize));
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    fn collect(&self, at: u32, rect: Rect, parent: NodeId, out: &mut Vec<usize>) {
        let Some(node) = self.nodes.get(at as usize) else { return };
        if !node.bounds.overlaps(rect) {
            return;
        }
        if node.left == u32::MAX {
            self.collect_leaf(node, rect, parent, out);
            return;
        }
        self.collect(node.left, rect, parent, out);
        self.collect(node.right, rect, parent, out);
    }

    fn collect_leaf(&self, node: &BvhNode, rect: Rect, parent: NodeId, out: &mut Vec<usize>) {
        let start = node.start as usize;
        let end = start.saturating_add(node.count as usize).min(self.items.len());
        let Some(items) = self.items.get(start..end) else { return };
        for item in items {
            if item.parent == parent && item.bounds.overlaps(rect) {
                out.push(item.index as usize);
            }
        }
    }
}

fn build(doc: &Document) -> HitTree {
    let symbols = doc.symbol_boxes();
    let mut built = Built { items: Vec::new(), always: Vec::new(), parents: HashSet::new(), fallback: false };
    for layer in &doc.layers {
        visit(layer, &symbols, &mut built);
    }
    if built.fallback {
        return HitTree::unusable();
    }
    pack(built.items, built.always, built.parents)
}

fn visit(parent: &Node, symbols: &crate::SymbolBoxes<'_>, built: &mut Built) {
    if built.fallback {
        return;
    }
    let Some(children) = parent.children() else { return };
    built.parents.insert(node_addr(parent));
    for (index, child) in children.iter().enumerate() {
        consider(parent.id, index, child, symbols, built);
        if built.fallback {
            return;
        }
        visit(child, symbols, built);
    }
}

/// One child under `parent`. Boxes that are missing, non-finite, being edited as envelope
/// contents, or a Shaper group stay off the tree: a non-finite box would make every query visit
/// its node, and those objects can hit outside a finite reach. [`super::may_hit`] still decides.
fn consider(parent: NodeId, index: usize, node: &Node, symbols: &crate::SymbolBoxes<'_>, built: &mut Built) {
    let Ok(index) = u32::try_from(index) else {
        built.fallback = true;
        return;
    };
    let bounds = node.reach_bounds_with(symbols);
    let aside = edits_contents(node) || node.extras().shaper.is_some() || bounds.is_none_or(|bounds| !finite_rect(bounds));
    if aside {
        built.always.push((parent, index));
    } else if let Some(bounds) = bounds {
        built.items.push(Item { parent, index, bounds: bounds.abs() });
    }
}

fn pack(mut items: Vec<Item>, always: Vec<(NodeId, u32)>, parents: HashSet<usize>) -> HitTree {
    // More nodes than a `u32` index can name: give up rather than drop objects.
    if items.len() > (u32::MAX / 4) as usize {
        return HitTree::unusable();
    }
    items.sort_by_key(|item| hilbert_key(item.bounds));
    let mut nodes = Vec::new();
    let root = (!items.is_empty()).then(|| build_range(&mut nodes, &items, 0, items.len() as u32));
    HitTree { fallback: false, root, nodes, items, always: group_always(always), parents }
}

fn node_addr(node: &Node) -> usize {
    std::ptr::from_ref(node).addr()
}

fn group_always(rows: Vec<(NodeId, u32)>) -> HashMap<NodeId, Vec<u32>> {
    let mut map: HashMap<NodeId, Vec<u32>> = HashMap::new();
    for (parent, index) in rows {
        map.entry(parent).or_default().push(index);
    }
    for kids in map.values_mut() {
        kids.sort_unstable();
        kids.dedup();
    }
    map
}

fn build_range(nodes: &mut Vec<BvhNode>, items: &[Item], start: u32, count: u32) -> u32 {
    let bounds = union_bounds(items, start, count);
    let at = nodes.len() as u32;
    nodes.push(BvhNode { bounds, left: u32::MAX, right: u32::MAX, start, count });
    if count > LEAF {
        let left_count = count / 2;
        let left = build_range(nodes, items, start, left_count);
        let right = build_range(nodes, items, start + left_count, count - left_count);
        if let Some(node) = nodes.get_mut(at as usize) {
            node.left = left;
            node.right = right;
        }
    }
    at
}

fn union_bounds(items: &[Item], start: u32, count: u32) -> Rect {
    let start = start as usize;
    let Some(first) = items.get(start).map(|item| item.bounds) else { return Rect::new(0.0, 0.0, 0.0, 0.0) };
    let end = start.saturating_add(count as usize).min(items.len());
    let mut acc = first;
    for item in items.iter().take(end).skip(start + 1) {
        acc = acc.union(item.bounds);
    }
    acc
}

fn finite_rect(rect: Rect) -> bool {
    rect.x0.is_finite() && rect.y0.is_finite() && rect.x1.is_finite() && rect.y1.is_finite()
}

/// Hilbert index of the box centre on a 16-bit grid. Sort quality only: queries use the boxes.
fn hilbert_key(bounds: Rect) -> u64 {
    let center = bounds.center();
    xy2d(HILBERT_N, quant(center.x), quant(center.y))
}

fn quant(value: f64) -> u32 {
    const SPAN: f64 = 1_048_576.0;
    if !value.is_finite() {
        return 0;
    }
    let t = ((value + SPAN) / (SPAN * 2.0)).clamp(0.0, 1.0);
    ((t * f64::from(HILBERT_N)) as u32).min(HILBERT_N - 1)
}

/// Wikipedia's `xy2d` for a power-of-two order. `x` and `y` are in `0..n`.
fn xy2d(n: u32, mut x: u32, mut y: u32) -> u64 {
    let mut d = 0u64;
    let mut s = n >> 1;
    while s > 0 {
        let rx = u32::from((x & s) > 0);
        let ry = u32::from((y & s) > 0);
        d += u64::from(s) * u64::from(s) * u64::from((3 * rx) ^ ry);
        rot(n, &mut x, &mut y, rx, ry);
        s >>= 1;
    }
    d
}

fn rot(n: u32, x: &mut u32, y: &mut u32, rx: u32, ry: u32) {
    if ry == 0 {
        if rx == 1 {
            *x = n - 1 - *x;
            *y = n - 1 - *y;
        }
        std::mem::swap(x, y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hilbert_order_of_a_2_square() {
        assert_eq!(xy2d(2, 0, 0), 0);
        assert_eq!(xy2d(2, 0, 1), 1);
        assert_eq!(xy2d(2, 1, 1), 2);
        assert_eq!(xy2d(2, 1, 0), 3);
    }

    #[test]
    fn hilbert_keys_of_an_8_grid_are_unique() {
        let mut keys: Vec<u64> = (0..8).flat_map(|x| (0..8).map(move |y| xy2d(8, x, y))).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), 64);
    }

    #[test]
    fn an_inflated_contains_is_an_overlap_of_the_query_rect() {
        let bounds = Rect::new(10.0, 20.0, 30.0, 40.0);
        let tol = 3.0;
        let point = Point::new(bounds.x1 + tol - 0.01, bounds.y1 + tol - 0.01);
        assert!(bounds.inflate(tol, tol).contains(point));
        let query = Rect::new(point.x - tol, point.y - tol, point.x + tol, point.y + tol);
        assert!(bounds.abs().overlaps(query.abs()));
        // The right edge is inside the closed query and outside the half-open contains.
        let edge = Point::new(bounds.x1 + tol, bounds.center().y);
        assert!(!bounds.inflate(tol, tol).contains(edge));
        let edge_query = Rect::new(edge.x - tol, edge.y - tol, edge.x + tol, edge.y + tol);
        assert!(bounds.abs().overlaps(edge_query.abs()));
    }
}
