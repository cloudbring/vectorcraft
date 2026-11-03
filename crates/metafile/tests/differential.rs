//! Differential test comparing VectorCraft's own EMF/WMF importer against the shared
//! `craft-metafile` reader across VectorCraft's exported fixtures and record-level fixtures.
// Integration tests: unwrapping and panicking on failure is fine here, unlike in shipped code (AGENTS.md › Robustness).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use craft_metafile::{Item, Limits, Picture, Seg};
use kurbo::{BezPath, Shape};
use vectorcraft_color::{Color, Paint};
use vectorcraft_doc::{Appearance, CharStyle, Document, ImageBlob, ImageObject, LineCap, LineJoin, Node, NodeId, NodeKind, TextObject};
use vectorcraft_geom::{Affine, PathData, Point, Rect, shapes};
use vectorcraft_metafile::{Kind, export, import};

/// Known reader coverage gaps listed in `craft-metafile`'s `COVERAGE.md`.
/// Mismatches caused by these records are reported, whereas any geometry, path-count,
/// subpath-count, text, or bitmap mismatch fails the test.
const ALLOWLISTED_EMF_GAPS: &[(u32, &str)] = &[
    // COVERAGE.md: EMF record 58 (EMR_SETMITERLIMIT) — VectorCraft retains pen miter limit on
    // StrokeLayer, while craft-metafile's (color, width) stroke tuple does not store miter limits.
    (58, "EMR_SETMITERLIMIT"),
    // COVERAGE.md: EMF record 67 (EMR_SELECTCLIPPATH) — VectorCraft retains arbitrary path clips
    // as clipping groups, while craft-metafile only models rectangular GDI clip regions.
    (67, "EMR_SELECTCLIPPATH"),
];

#[derive(Debug, Clone)]
struct Fixture {
    name: &'static str,
    kind: Kind,
    bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GapOrMismatch {
    AllowlistedGap { record: u32, record_name: &'static str, detail: String },
    Mismatch { record: String, detail: String },
}

#[derive(Debug)]
struct VcSummary {
    page_span: f64,
    drawn_paths: usize,
    subpaths: usize,
    clip_paths: usize,
    bounds: Option<[f64; 4]>,
    texts: Vec<String>,
    bitmaps: Vec<(u32, u32)>,
    non_default_miter: Option<f64>,
}

#[derive(Debug)]
struct CmSummary {
    page_span: f64,
    drawn_paths: usize,
    subpaths: usize,
    bounds: Option<[f64; 4]>,
    texts: Vec<String>,
    bitmaps: Vec<(u32, u32)>,
}

fn add(doc: &mut Document, mut node: Node) {
    node.id = doc.alloc_id();
    let layer = doc.layers[0].id;
    let at = doc.layers[0].children().map_or(0, Vec::len);
    doc.insert(Some(layer), at, node).unwrap();
}

fn doc_with(w: f64, h: f64, nodes: Vec<Node>) -> Document {
    let mut doc = Document::new(w, h);
    for node in nodes {
        add(&mut doc, node);
    }
    doc
}

fn solid(r: u8, g: u8, b: u8) -> Paint {
    Paint::solid(Color::rgb8(r, g, b))
}

fn stroked_round(path: PathData, paint: Paint, width: f64) -> Node {
    let mut node = Node::path(NodeId(0), path, Appearance::basic(Paint::None, paint, width));
    if let Some(stroke) = node.appearance.stroke_mut() {
        stroke.join = LineJoin::Round;
        stroke.cap = LineCap::Round;
    }
    node
}

fn png_bytes(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba(rgba));
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

fn synthesized_documents() -> Vec<(&'static str, Document)> {
    // 1. Lines (coordinates at multiples of 18 pt so 0.01 mm device units round-trip exactly).
    let lines_doc = doc_with(
        144.0,
        108.0,
        vec![
            stroked_round(shapes::line(Point::new(18.0, 18.0), Point::new(126.0, 54.0)), solid(220, 30, 30), 2.0),
            stroked_round(shapes::line(Point::new(18.0, 90.0), Point::new(126.0, 36.0)), solid(30, 90, 220), 2.0),
        ],
    );

    // 2. Polylines.
    let mut poly_bp = BezPath::new();
    poly_bp.move_to((18.0, 18.0));
    poly_bp.line_to((54.0, 90.0));
    poly_bp.line_to((90.0, 36.0));
    poly_bp.line_to((126.0, 72.0));
    let polylines_doc = doc_with(144.0, 108.0, vec![stroked_round(PathData::from_bezpath(&poly_bp), solid(20, 150, 60), 3.0)]);

    // 3. Cubic Béziers.
    let mut bez_bp = BezPath::new();
    bez_bp.move_to((18.0, 54.0));
    bez_bp.curve_to((36.0, 18.0), (72.0, 90.0), (108.0, 36.0));
    bez_bp.curve_to((126.0, 18.0), (126.0, 72.0), (90.0, 90.0));
    let beziers_doc = doc_with(144.0, 108.0, vec![stroked_round(PathData::from_bezpath(&bez_bp), solid(140, 40, 180), 2.0)]);

    // 4. Rectangles and ellipses (filled).
    let rect_ellipse_doc = doc_with(
        144.0,
        108.0,
        vec![
            Node::path(NodeId(0), shapes::rectangle(Rect::new(18.0, 18.0, 72.0, 54.0)), Appearance::basic(solid(255, 0, 0), Paint::None, 0.0)),
            Node::path(NodeId(0), shapes::ellipse(Rect::new(72.0, 36.0, 126.0, 90.0)), Appearance::basic(solid(0, 0, 255), Paint::None, 0.0)),
        ],
    );

    // 5. Text (VectorCraft exports text as glyph outline paths; placed near origin for tight f32 precision).
    let text_obj = TextObject::point(Point::new(2.0, 12.0), "Hi", CharStyle { size: 10.0, fill: solid(20, 20, 20), ..CharStyle::default() });
    let text_doc = doc_with(36.0, 18.0, vec![Node::new(NodeId(0), NodeKind::Text(Box::new(text_obj)))]);

    // 6. Filled + stroked path with miter join (exercises EMR_SETMITERLIMIT 58 in EMF).
    let mut filled_stroked =
        Node::path(NodeId(0), shapes::rectangle(Rect::new(18.0, 18.0, 108.0, 72.0)), Appearance::basic(solid(240, 180, 40), solid(30, 30, 30), 4.0));
    if let Some(stroke) = filled_stroked.appearance.stroke_mut() {
        stroke.join = LineJoin::Miter;
        stroke.miter_limit = 4.0;
    }
    let filled_stroked_doc = doc_with(144.0, 108.0, vec![filled_stroked]);

    // 7. Clipped group (exercises EMR_SELECTCLIPPATH 67 in EMF).
    let mut clip_mask = Node::path(NodeId(10), shapes::rectangle(Rect::new(18.0, 18.0, 90.0, 72.0)), Appearance::default());
    if let NodeKind::Path { clipping, .. } = &mut clip_mask.kind {
        *clipping = true;
    }
    let clipped_fill =
        Node::path(NodeId(11), shapes::rectangle(Rect::new(36.0, 36.0, 108.0, 90.0)), Appearance::basic(solid(0, 180, 140), Paint::None, 0.0));
    let clip_group = Node::new(NodeId(12), NodeKind::Group { children: vec![Arc::new(clip_mask), Arc::new(clipped_fill)], clip: true });
    let clipped_group_doc = doc_with(144.0, 108.0, vec![clip_group]);

    // 8. Bitmap (exercises EMR_STRETCHDIBITS 81 in EMF and META_STRETCHDIB 0x0F43 in WMF).
    let mut bitmap_doc = doc_with(144.0, 108.0, vec![]);
    let blob = ImageBlob::new("image/png", png_bytes(4, 2, [0, 200, 80, 255]));
    let key = blob.content_key();
    bitmap_doc.images.insert(key.clone(), blob);
    let img_node = Node::new(
        NodeId(0),
        NodeKind::Image(Box::new(ImageObject {
            key,
            width: 4,
            height: 2,
            xf: Affine::translate((18.0, 18.0)) * Affine::scale(9.0),
            link: None,
            placement: Default::default(),
            pixels: None,
        })),
    );
    add(&mut bitmap_doc, img_node);
    add(
        &mut bitmap_doc,
        Node::path(NodeId(0), shapes::rectangle(Rect::new(72.0, 18.0, 126.0, 54.0)), Appearance::basic(solid(80, 40, 200), Paint::None, 0.0)),
    );

    vec![
        ("lines", lines_doc),
        ("polylines", polylines_doc),
        ("beziers", beziers_doc),
        ("rectangles_ellipses", rect_ellipse_doc),
        ("text_outlines", text_doc),
        ("filled_stroked_path", filled_stroked_doc),
        ("clipped_group", clipped_group_doc),
        ("bitmap", bitmap_doc),
    ]
}

fn le32(values: &[i32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn le16(values: &[i16]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// Hand-built EMF with native `EMR_EXTCREATEFONTINDIRECTW` (82) and `EMR_EXTTEXTOUTW` (84) text runs
/// plus `EMR_RECTANGLE` (43), so text-run extraction is differentially checked alongside `export`.
fn native_text_emf() -> Vec<u8> {
    let make_text = |x: i32, y: i32, s: &str| {
        let mut body = vec![0u8; 16];
        body.extend(le32(&[1, 0, 0]));
        let chars = s.encode_utf16().count() as i32;
        body.extend(le32(&[x, y, chars, 76, 0, 0, 0, 0, 0, 0]));
        body.extend(s.encode_utf16().flat_map(u16::to_le_bytes));
        while !body.len().is_multiple_of(4) {
            body.push(0);
        }
        body
    };
    let mut font = le32(&[2, -635, 0, 0, 0, 400]);
    font.extend([0u8; 8]);
    font.extend("Arial".encode_utf16().chain(std::iter::repeat_n(0, 27)).flat_map(u16::to_le_bytes));
    let records: Vec<(u32, Vec<u8>)> = vec![
        (39, le32(&[1, 0, 0x00_00_ff, 0])),
        (37, le32(&[1])),
        (37, le32(&[0x8000_0008u32 as i32])),
        (43, le32(&[635, 635, 1905, 1270])),
        (82, font),
        (37, le32(&[2])),
        (24, le32(&[0x00_20_20])),
        (84, make_text(635, 1905, "Hi")),
        (84, make_text(635, 2540, "World")),
    ];
    let mut out = Vec::new();
    let push_rec = |out: &mut Vec<u8>, kind: u32, body: &[u8]| {
        out.extend(kind.to_le_bytes());
        out.extend((8 + body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
    };
    let mut head = Vec::new();
    head.extend([0u8; 16]);
    for v in [0i32, 0, 5080, 3810] {
        head.extend(v.to_le_bytes());
    }
    head.extend(0x464D_4520u32.to_le_bytes());
    head.extend(0x0001_0000u32.to_le_bytes());
    head.extend([0u8; 8]);
    head.extend(3u16.to_le_bytes());
    head.extend(0u16.to_le_bytes());
    head.extend([0u8; 12]);
    for v in [10000i32, 10000, 100, 100] {
        head.extend(v.to_le_bytes());
    }
    push_rec(&mut out, 1, &head);
    for (k, body) in &records {
        push_rec(&mut out, *k, body);
    }
    push_rec(&mut out, 14, &[0, 0, 0, 0, 16, 0, 0, 0, 20, 0, 0, 0]);
    out
}

/// Hand-built placeable WMF with `META_TEXTOUT` (0x0521), `META_EXTTEXTOUT` (0x0A32), and
/// `META_RECTANGLE` (0x041B).
fn native_text_wmf() -> Vec<u8> {
    let mut text_out = le16(&[2]);
    text_out.extend_from_slice(b"Hi");
    text_out.extend(le16(&[400, 200]));

    let mut ext_text_out = le16(&[800, 200, 5, 0]);
    ext_text_out.extend_from_slice(b"World\0");

    let records: Vec<(u16, Vec<u8>)> = vec![
        (0x02FC, [le16(&[0]), 0x00ff_0000u32.to_le_bytes().to_vec(), le16(&[0])].concat()),
        (0x02FA, [le16(&[5, 0, 0]), 0u32.to_le_bytes().to_vec()].concat()),
        (0x012D, le16(&[0])),
        (0x012D, le16(&[1])),
        (0x041B, le16(&[720, 1440, 180, 180])),
        (0x0521, text_out),
        (0x0A32, ext_text_out),
    ];

    let mut out = Vec::new();
    out.extend(0x9AC6_CDD7u32.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    for v in [0i16, 0, 2880, 1440] {
        out.extend(v.to_le_bytes());
    }
    out.extend(1440u16.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    let sum = out.as_chunks::<2>().0.iter().take(10).fold(0u16, |acc, w| acc ^ u16::from_le_bytes(*w));
    out.extend(sum.to_le_bytes());
    out.extend([1u8, 0, 9, 0, 0, 3]);
    out.extend([0u8; 12]);
    for (func, body) in records.iter().chain(&[(0u16, Vec::new())]) {
        let words = 3 + body.len() as u32 / 2;
        out.extend(words.to_le_bytes());
        out.extend(func.to_le_bytes());
        out.extend_from_slice(body);
    }
    out
}

fn all_fixtures() -> Vec<Fixture> {
    let mut fixtures = Vec::new();
    for (name, doc) in synthesized_documents() {
        let region = doc.artboards[0].rect;
        let emf_out = export(&doc, region, Kind::Emf).unwrap();
        fixtures.push(Fixture { name: Box::leak(format!("{name}.emf").into_boxed_str()), kind: Kind::Emf, bytes: emf_out.bytes });
        let wmf_out = export(&doc, region, Kind::Wmf).unwrap();
        fixtures.push(Fixture { name: Box::leak(format!("{name}.wmf").into_boxed_str()), kind: Kind::Wmf, bytes: wmf_out.bytes });
    }
    fixtures.push(Fixture { name: "native_text.emf", kind: Kind::Emf, bytes: native_text_emf() });
    fixtures.push(Fixture { name: "native_text.wmf", kind: Kind::Wmf, bytes: native_text_wmf() });
    fixtures
}

fn summarize_vc(doc: &Document) -> VcSummary {
    let ab = doc.artboards[0].rect;
    let page_span = ab.width().max(ab.height()).max(1.0);
    let mut drawn_paths = 0usize;
    let mut subpaths = 0usize;
    let mut clip_paths = 0usize;
    let mut bounds: Option<kurbo::Rect> = None;
    let mut texts = Vec::new();
    let mut bitmaps = Vec::new();
    let mut non_default_miter = None;

    for layer in &doc.layers {
        layer.walk(&mut |node| match &node.kind {
            NodeKind::Path { path, clipping, .. } => {
                if *clipping {
                    clip_paths += 1;
                } else {
                    drawn_paths += 1;
                    subpaths += path.subpaths.len();
                    let b = path.to_bezpath().bounding_box();
                    bounds = Some(bounds.map_or(b, |cur| cur.union(b)));
                    if let Some(st) = node.appearance.stroke()
                        && st.join == LineJoin::Miter
                        && (st.miter_limit - 10.0).abs() > 1e-6
                    {
                        non_default_miter = Some(st.miter_limit);
                    }
                }
            }
            NodeKind::Text(t) => texts.push(t.plain_text()),
            NodeKind::Image(im) => bitmaps.push((im.width, im.height)),
            _ => {}
        });
    }

    VcSummary { page_span, drawn_paths, subpaths, clip_paths, bounds: bounds.map(|r| [r.x0, r.y0, r.x1, r.y1]), texts, bitmaps, non_default_miter }
}

fn summarize_cm(pic: &Picture) -> CmSummary {
    let page_span = f64::from(pic.width.max(pic.height)).max(1.0);
    let mut drawn_paths = 0usize;
    let mut subpaths = 0usize;
    let mut bounds: Option<kurbo::Rect> = None;
    let mut texts = Vec::new();
    let mut bitmaps = Vec::new();

    for item in &pic.items {
        match item {
            Item::Path { segs, .. } | Item::Pattern { segs, .. } => {
                drawn_paths += 1;
                let mut bp = BezPath::new();
                for seg in segs {
                    match *seg {
                        Seg::Move(x, y) => {
                            subpaths += 1;
                            bp.move_to((f64::from(x), f64::from(y)));
                        }
                        Seg::Line(x, y) => bp.line_to((f64::from(x), f64::from(y))),
                        Seg::Cubic(x1, y1, x2, y2, x3, y3) => {
                            bp.curve_to((f64::from(x1), f64::from(y1)), (f64::from(x2), f64::from(y2)), (f64::from(x3), f64::from(y3)));
                        }
                        Seg::Close => bp.close_path(),
                    }
                }
                let b = bp.bounding_box();
                bounds = Some(bounds.map_or(b, |cur| cur.union(b)));
            }
            Item::Text(t) => texts.push(t.text.clone()),
            Item::Bitmap { width, height, .. } => bitmaps.push((*width, *height)),
        }
    }

    CmSummary { page_span, drawn_paths, subpaths, bounds: bounds.map(|r| [r.x0, r.y0, r.x1, r.y1]), texts, bitmaps }
}

fn emf_has_record(bytes: &[u8], target: u32) -> bool {
    let mut at = 0usize;
    while let Some(hdr) = bytes.get(at..at + 8) {
        let kind = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
        let size = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
        if size < 8 {
            break;
        }
        if kind == target {
            return true;
        }
        at = at.saturating_add(size);
    }
    false
}

fn compare_fixture(f: &Fixture) -> Vec<GapOrMismatch> {
    let vc_imported = import(&f.bytes).unwrap();
    let cm_picture = craft_metafile::parse(&f.bytes).unwrap();
    let vc = summarize_vc(&vc_imported.document);
    let cm = summarize_cm(&cm_picture);
    let mut out = Vec::new();

    // 1. Drawn path item count (±0) and distinct subpath count (±0).
    if vc.drawn_paths != cm.drawn_paths {
        out.push(GapOrMismatch::Mismatch {
            record: if f.kind == Kind::Emf { "EMF path bracket (59/60/62/64)".into() } else { "WMF polygon/polyline (0x0324/0x0325/0x0538)".into() },
            detail: format!("{}: drawn path item count vc={} vs cm={}", f.name, vc.drawn_paths, cm.drawn_paths),
        });
    }
    if vc.subpaths != cm.subpaths {
        out.push(GapOrMismatch::Mismatch {
            record: if f.kind == Kind::Emf { "EMF MOVETOEX (27)".into() } else { "WMF subpath count".into() },
            detail: format!("{}: subpath count vc={} vs cm={}", f.name, vc.subpaths, cm.subpaths),
        });
    }

    // 2. Total path bounding box: within 1e-6 of page units (and < 1e-6 absolute pt) for EMF,
    // or within 1 unit for WMF's integer coords.
    let tol = match f.kind {
        Kind::Emf => 1e-6 * vc.page_span.min(cm.page_span),
        Kind::Wmf => 1.0,
    };
    match (vc.bounds, cm.bounds) {
        (Some(vb), Some(cb)) => {
            let max_diff = vb.into_iter().zip(cb).map(|(a, b)| (a - b).abs()).fold(0.0f64, f64::max);
            if max_diff > tol {
                out.push(GapOrMismatch::Mismatch {
                    record: if f.kind == Kind::Emf {
                        "EMF path geometry (5/6/27/43/62/64)".into()
                    } else {
                        "WMF path geometry (0x0324/0x0325/0x041B/0x0538)".into()
                    },
                    detail: format!("{}: path bounds vc={vb:?} vs cm={cb:?} (max_diff={max_diff:.3e} > tol={tol:.3e})", f.name),
                });
            }
        }
        (None, None) => {}
        (vb, cb) => {
            out.push(GapOrMismatch::Mismatch {
                record: "path presence".into(),
                detail: format!("{}: path bounds presence mismatch vc={vb:?} vs cm={cb:?}", f.name),
            });
        }
    }

    // 3. Text run strings in order.
    if vc.texts != cm.texts {
        out.push(GapOrMismatch::Mismatch {
            record: if f.kind == Kind::Emf { "EMR_EXTTEXTOUTW (84)".into() } else { "META_TEXTOUT/EXTTEXTOUT (0x0521/0x0A32)".into() },
            detail: format!("{}: text runs vc={:?} vs cm={:?}", f.name, vc.texts, cm.texts),
        });
    }

    // 4. Bitmap dimensions in order.
    if vc.bitmaps != cm.bitmaps {
        out.push(GapOrMismatch::Mismatch {
            record: if f.kind == Kind::Emf { "EMR_STRETCHDIBITS (81)".into() } else { "META_STRETCHDIB (0x0F43)".into() },
            detail: format!("{}: bitmap dimensions vc={:?} vs cm={:?}", f.name, vc.bitmaps, cm.bitmaps),
        });
    }

    // 5. Record COVERAGE.md gaps (EMF 58 SETMITERLIMIT and EMF 67 SELECTCLIPPATH).
    if f.kind == Kind::Emf && vc.non_default_miter.is_some() && emf_has_record(&f.bytes, 58) {
        if let Some(&(rec, rec_name)) = ALLOWLISTED_EMF_GAPS.iter().find(|(r, _)| *r == 58) {
            out.push(GapOrMismatch::AllowlistedGap {
                record: rec,
                record_name: rec_name,
                detail: format!("{}: vc preserves miter_limit={:?}; craft-metafile omits miter limit", f.name, vc.non_default_miter),
            });
        } else {
            out.push(GapOrMismatch::Mismatch {
                record: "58 EMR_SETMITERLIMIT".into(),
                detail: format!("{}: miter limit not preserved by craft-metafile", f.name),
            });
        }
    }

    if f.kind == Kind::Emf && vc.clip_paths > 0 && emf_has_record(&f.bytes, 67) {
        if let Some(&(rec, rec_name)) = ALLOWLISTED_EMF_GAPS.iter().find(|(r, _)| *r == 67) {
            out.push(GapOrMismatch::AllowlistedGap {
                record: rec,
                record_name: rec_name,
                detail: format!("{}: vc imports {} clip path(s); craft-metafile skips SELECTCLIPPATH", f.name, vc.clip_paths),
            });
        } else {
            out.push(GapOrMismatch::Mismatch {
                record: "67 EMR_SELECTCLIPPATH".into(),
                detail: format!("{}: clip path count vc={} vs cm=0", f.name, vc.clip_paths),
            });
        }
    }

    out
}

#[test]
fn vectorcraft_and_craft_metafile_agree_across_exported_and_record_fixtures() {
    let fixtures = all_fixtures();
    assert_eq!(fixtures.len(), 18);

    let mut unexpected_mismatches = Vec::new();
    let mut allowlisted_gaps = Vec::new();

    for fixture in &fixtures {
        for finding in compare_fixture(fixture) {
            match finding {
                GapOrMismatch::AllowlistedGap { .. } => allowlisted_gaps.push(finding),
                GapOrMismatch::Mismatch { .. } => unexpected_mismatches.push(finding),
            }
        }
    }

    assert!(
        unexpected_mismatches.is_empty(),
        "unexpected differential mismatches between vectorcraft-metafile and craft-metafile: {unexpected_mismatches:#?}"
    );
    // Both COVERAGE.md gaps (58 EMR_SETMITERLIMIT and 67 EMR_SELECTCLIPPATH) are exercised and reported.
    assert!(
        allowlisted_gaps.iter().any(|g| matches!(g, GapOrMismatch::AllowlistedGap { record: 58, .. })),
        "expected EMR_SETMITERLIMIT (58) gap to be recorded: {allowlisted_gaps:#?}"
    );
    assert!(
        allowlisted_gaps.iter().any(|g| matches!(g, GapOrMismatch::AllowlistedGap { record: 67, .. })),
        "expected EMR_SELECTCLIPPATH (67) gap to be recorded: {allowlisted_gaps:#?}"
    );
}

#[test]
fn seeded_2000_mutations_never_panic_in_either_reader() {
    let fixtures = all_fixtures();
    let mut seed = 0x92e0_2026_1103_f00du64;
    let tight_limits = Limits { max_records: 8, max_items: 2, max_bitmap_pixels: 16, max_bytes: 8_192 };

    // 2,000 seeded mutations across the exported and record fixtures, mirroring the shared crate's
    // xorshift mutation harness and VectorCraft's mutated_metafiles_never_panic edits + truncations.
    for iteration in 0..2_000usize {
        let base = &fixtures[iteration % fixtures.len()].bytes;
        let mut bytes = base.clone();
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let idx = (seed as usize) % bytes.len();
        if iteration % 2 == 0 {
            bytes.truncate(idx.max(4));
        } else {
            bytes[idx] ^= 1 << (seed % 8);
            let end = (idx + 4).min(bytes.len());
            if iteration % 5 == 1 {
                bytes[idx..end].fill(0xff);
            }
        }
        let res = std::panic::catch_unwind(|| {
            let _ = import(&bytes);
            let _ = craft_metafile::parse(&bytes);
            let _ = craft_metafile::parse_with_limits(&bytes, tight_limits);
        });
        assert!(res.is_ok(), "mutation {iteration} (seed {seed:#x}) panicked");
    }
}
