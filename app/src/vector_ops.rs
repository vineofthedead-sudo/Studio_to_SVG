//! Vector operations for Craft Anything papercraft design studio.
//! All functions are panic-free and use safe bounds checking.

use egui::{Pos2, Vec2};

// ---------------------------------------------------------------------------
// 1. Polygon Offset
// ---------------------------------------------------------------------------

/// Compute signed area of a polygon.
/// In screen coordinates (where y is down):
/// Positive area means clockwise winding; negative area means counter-clockwise.
pub fn polygon_signed_area(pts: &[Pos2]) -> f32 {
    let n = pts.len();
    if n < 3 {
        return 0.0;
    }
    let mut area = 0.0;
    for i in 0..n {
        let j = (i + 1) % n;
        area += pts[i].x * pts[j].y - pts[j].x * pts[i].y;
    }
    area * 0.5
}

/// Offset a polygon outward (+dist) or inward (-dist) using per-vertex normal bisectors.
///
/// Returns a `Vec<Pos2>` of the same length as `pts`, or an empty `Vec` on
/// degenerate input (fewer than 3 points, or non-finite `dist`).
pub fn offset_polygon(pts: &[Pos2], dist: f32) -> Vec<Pos2> {
    if pts.len() < 3 || !dist.is_finite() {
        return Vec::new();
    }

    let n = pts.len();
    let area = polygon_signed_area(pts);
    let is_cw = area >= 0.0;
    let mut result = Vec::with_capacity(n);

    for i in 0..n {
        let prev = pts[(i + n - 1) % n];
        let curr = pts[i];
        let next = pts[(i + 1) % n];

        // Edge vectors (from prev→curr and curr→next)
        let edge_a_raw = curr - prev;
        let edge_b_raw = next - curr;

        // Normalize — fall back to zero-length vec if degenerate
        let edge_a = normalize_vec2(edge_a_raw);
        let edge_b = normalize_vec2(edge_b_raw);

        // Outward-pointing normals depending on winding:
        // In screen coordinates (y down), CW winding has outward normal (edge.y, -edge.x)
        // CCW winding has outward normal (-edge.y, edge.x)
        let normal_a = if is_cw {
            Vec2::new(edge_a.y, -edge_a.x)
        } else {
            Vec2::new(-edge_a.y, edge_a.x)
        };
        let normal_b = if is_cw {
            Vec2::new(edge_b.y, -edge_b.x)
        } else {
            Vec2::new(-edge_b.y, edge_b.x)
        };

        // Bisector of the two normals
        let bisector_raw = normal_a + normal_b;
        let bisector = normalize_vec2(bisector_raw);

        // Miter length: dist / cos(half-angle) = dist / (bisector·normal_a)
        // Clamp to avoid extreme spikes at near-parallel or sharp corners.
        let dot = bisector.dot(normal_a);
        let miter_len = if dot.abs() < 0.1 {
            dist.signum() * 5.0 * dist.abs()
        } else {
            let len = dist / dot;
            let max_miter = 5.0 * dist.abs();
            len.clamp(-max_miter, max_miter)
        };

        result.push(Pos2::new(
            curr.x + bisector.x * miter_len,
            curr.y + bisector.y * miter_len,
        ));
    }

    result
}

/// Normalize a `Vec2`; returns `Vec2::ZERO` if the length is too small.
fn normalize_vec2(v: Vec2) -> Vec2 {
    let len = v.length();
    if len < 1e-10 {
        Vec2::ZERO
    } else {
        v / len
    }
}

// ---------------------------------------------------------------------------
// 2. Convex Hull (Graham scan) — CCW order
// ---------------------------------------------------------------------------

/// Compute the convex hull of a set of points using a Graham scan.
///
/// Returns the hull vertices in counter-clockwise order.
/// Returns an empty `Vec` for empty input, a single point for one point, etc.
pub fn convex_hull(pts: &[Pos2]) -> Vec<Pos2> {
    if pts.is_empty() {
        return Vec::new();
    }
    if pts.len() == 1 {
        return vec![pts[0]];
    }
    if pts.len() == 2 {
        // Two distinct points: return both (or one if coincident)
        if pts[0] == pts[1] {
            return vec![pts[0]];
        }
        return vec![pts[0], pts[1]];
    }

    // Find the pivot: lowest y, then leftmost x.
    let pivot_idx = pts
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            a.y.partial_cmp(&b.y)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
        })
        .map(|(i, _)| i)
        .unwrap_or(0);

    let pivot = pts[pivot_idx];

    // Sort all other points by polar angle relative to pivot, then by distance.
    let mut others: Vec<Pos2> = pts
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != pivot_idx)
        .map(|(_, &p)| p)
        .collect();

    others.sort_by(|a, b| {
        let angle_a = (a.y - pivot.y).atan2(a.x - pivot.x);
        let angle_b = (b.y - pivot.y).atan2(b.x - pivot.x);
        angle_a
            .partial_cmp(&angle_b)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                let da = (a.x - pivot.x).hypot(a.y - pivot.y);
                let db = (b.x - pivot.x).hypot(b.y - pivot.y);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    // Graham scan
    let mut stack: Vec<Pos2> = vec![pivot];
    for &p in &others {
        while stack.len() >= 2 {
            let o = stack[stack.len() - 2];
            let a = stack[stack.len() - 1];
            // Cross product of (a - o) × (p - o); negative or zero → not CCW turn
            let cross = (a.x - o.x) * (p.y - o.y) - (a.y - o.y) * (p.x - o.x);
            if cross <= 0.0 {
                stack.pop();
            } else {
                break;
            }
        }
        stack.push(p);
    }

    stack
}

// ---------------------------------------------------------------------------
// 3. Alignment
// ---------------------------------------------------------------------------

/// Alignment anchor for `compute_align_offsets`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AlignAnchor {
    Left,
    CenterH,
    Right,
    Top,
    CenterV,
    Bottom,
    CenterToMatH,
    CenterToMatV,
}

/// Compute per-shape translation offsets (dx, dy) in mat-inches.
///
/// `sel_rects` is a slice of `(min_x, min_y, max_x, max_y)` for each selected shape.
/// `mat_w` and `mat_h` are the mat dimensions in inches.
///
/// Returns a `Vec<(f32, f32)>` of the same length as `sel_rects`.
pub fn compute_align_offsets(
    sel_rects: &[(f32, f32, f32, f32)],
    anchor: AlignAnchor,
    mat_w: f32,
    mat_h: f32,
) -> Vec<(f32, f32)> {
    if sel_rects.is_empty() {
        return Vec::new();
    }

    // Compute the overall bounding box of all selected shapes.
    let all_min_x = sel_rects.iter().map(|r| r.0).fold(f32::INFINITY, f32::min);
    let all_min_y = sel_rects.iter().map(|r| r.1).fold(f32::INFINITY, f32::min);
    let all_max_x = sel_rects.iter().map(|r| r.2).fold(f32::NEG_INFINITY, f32::max);
    let all_max_y = sel_rects.iter().map(|r| r.3).fold(f32::NEG_INFINITY, f32::max);

    sel_rects
        .iter()
        .map(|&(min_x, min_y, max_x, max_y)| {
            let _w = max_x - min_x;
            let _h = max_y - min_y;
            let cx = (min_x + max_x) * 0.5;
            let cy = (min_y + max_y) * 0.5;

            match anchor {
                AlignAnchor::Left => (all_min_x - min_x, 0.0),
                AlignAnchor::Right => (all_max_x - max_x, 0.0),
                AlignAnchor::CenterH => {
                    let group_cx = (all_min_x + all_max_x) * 0.5;
                    (group_cx - cx, 0.0)
                }
                AlignAnchor::Top => (0.0, all_min_y - min_y),
                AlignAnchor::Bottom => (0.0, all_max_y - max_y),
                AlignAnchor::CenterV => {
                    let group_cy = (all_min_y + all_max_y) * 0.5;
                    (0.0, group_cy - cy)
                }
                AlignAnchor::CenterToMatH => {
                    // Center this shape's own midpoint to the mat's horizontal center.
                    let mat_cx = mat_w * 0.5;
                    (mat_cx - cx, 0.0)
                }
                AlignAnchor::CenterToMatV => {
                    // Center this shape's own midpoint to the mat's vertical center.
                    let mat_cy = mat_h * 0.5;
                    (0.0, mat_cy - cy)
                }
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 4. Registration Marks
// ---------------------------------------------------------------------------

/// Generate three registration marks in mat-inch coordinates.
///
/// Returns `Vec<(Vec<Pos2>, bool)>` where the `bool` indicates `is_closed`:
/// - Index 0: top-left filled square (closed)
/// - Index 1: top-right L-bracket (open)
/// - Index 2: bottom-left L-bracket (open)
pub fn registration_marks(
    mat_w: f32,
    mat_h: f32,
    inset: f32,
    mark_size: f32,
) -> Vec<(Vec<Pos2>, bool)> {
    let mut marks = Vec::with_capacity(3);

    // Top-left: closed square
    let tl = vec![
        Pos2::new(inset, inset),
        Pos2::new(inset + mark_size, inset),
        Pos2::new(inset + mark_size, inset + mark_size),
        Pos2::new(inset, inset + mark_size),
    ];
    marks.push((tl, true));

    // Top-right: L-bracket (open polyline)
    let tr = vec![
        Pos2::new(mat_w - inset - mark_size, inset),
        Pos2::new(mat_w - inset, inset),
        Pos2::new(mat_w - inset, inset + mark_size),
    ];
    marks.push((tr, false));

    // Bottom-left: L-bracket (open polyline)
    let bl = vec![
        Pos2::new(inset, mat_h - inset - mark_size),
        Pos2::new(inset, mat_h - inset),
        Pos2::new(inset + mark_size, mat_h - inset),
    ];
    marks.push((bl, false));

    marks
}

// ---------------------------------------------------------------------------
// Unit Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    // 1. offset_polygon: unit square offset by 0.1 — each vertex ~0.1 from original
    #[test]
    fn test_offset_polygon_square() {
        // CCW unit square
        let square = vec![
            Pos2::new(0.0, 0.0),
            Pos2::new(1.0, 0.0),
            Pos2::new(1.0, 1.0),
            Pos2::new(0.0, 1.0),
        ];
        let dist = 0.1_f32;
        let result = offset_polygon(&square, dist);
        assert_eq!(result.len(), 4, "offset_polygon should return same number of vertices");
        for (orig, off) in square.iter().zip(result.iter()) {
            let d = ((off.x - orig.x).powi(2) + (off.y - orig.y).powi(2)).sqrt();
            assert!(
                approx_eq(d, dist, 0.05),
                "vertex distance from original should be ~{dist}, got {d} for orig={orig:?} off={off:?}"
            );
        }
    }

    // 2. convex_hull: 3 non-collinear points → hull has 3 points
    #[test]
    fn test_convex_hull_triangle() {
        let pts = vec![
            Pos2::new(0.0, 0.0),
            Pos2::new(1.0, 0.0),
            Pos2::new(0.5, 1.0),
        ];
        let hull = convex_hull(&pts);
        assert_eq!(hull.len(), 3, "triangle hull should have 3 points, got {}", hull.len());
    }

    // 3. convex_hull: all collinear → hull has ≤ 2 points
    #[test]
    fn test_convex_hull_collinear() {
        let pts = vec![
            Pos2::new(0.0, 0.0),
            Pos2::new(1.0, 0.0),
            Pos2::new(2.0, 0.0),
            Pos2::new(3.0, 0.0),
        ];
        let hull = convex_hull(&pts);
        assert!(
            hull.len() <= 2,
            "collinear points hull should have ≤ 2 points, got {}",
            hull.len()
        );
    }

    // 4. align_left: 3 rects at x=1,2,3 → after align all have same leftmost x
    #[test]
    fn test_align_left() {
        let rects = vec![
            (1.0_f32, 0.0, 2.0, 1.0),
            (2.0_f32, 0.0, 3.0, 1.0),
            (3.0_f32, 0.0, 4.0, 1.0),
        ];
        let offsets = compute_align_offsets(&rects, AlignAnchor::Left, 12.0, 12.0);
        assert_eq!(offsets.len(), 3);
        // After applying dx, each rect's min_x should equal 1.0 (the leftmost)
        for ((min_x, _, _, _), (dx, _)) in rects.iter().zip(offsets.iter()) {
            let new_min_x = min_x + dx;
            assert!(
                approx_eq(new_min_x, 1.0, 1e-5),
                "after Left align, min_x should be 1.0, got {new_min_x}"
            );
        }
    }

    // 5. align_center_to_mat: rect at x=0, w=2, mat_w=10 → dx=4
    #[test]
    fn test_align_center_to_mat() {
        // rect: min_x=0, max_x=2 → cx=1; mat_cx=5 → dx=4
        let rects = vec![(0.0_f32, 0.0, 2.0, 2.0)];
        let offsets = compute_align_offsets(&rects, AlignAnchor::CenterToMatH, 10.0, 10.0);
        assert_eq!(offsets.len(), 1);
        let (dx, _dy) = offsets[0];
        assert!(
            approx_eq(dx, 4.0, 1e-5),
            "CenterToMatH dx should be 4.0, got {dx}"
        );
    }

    // 6. registration_marks_bounds: all points within 8.5×11 mat
    #[test]
    fn test_registration_marks_bounds() {
        let mat_w = 8.5_f32;
        let mat_h = 11.0_f32;
        let inset = 0.25_f32;
        let mark_size = 0.2_f32;
        let marks = registration_marks(mat_w, mat_h, inset, mark_size);
        assert_eq!(marks.len(), 3, "should return 3 marks");
        for (i, (pts, _closed)) in marks.iter().enumerate() {
            for p in pts {
                assert!(
                    p.x >= 0.0 && p.x <= mat_w,
                    "mark {i} point x={} out of [0, {mat_w}]",
                    p.x
                );
                assert!(
                    p.y >= 0.0 && p.y <= mat_h,
                    "mark {i} point y={} out of [0, {mat_h}]",
                    p.y
                );
            }
        }
    }
}
