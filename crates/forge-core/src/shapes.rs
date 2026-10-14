//! Shape tools (Rectangle, Rounded Rectangle, Ellipse, Triangle, Polygon,
//! Star, Line, Arrow, Heart, Speech Bubble) and anti-aliased, optionally
//! dashed polyline strokes (also used for Pen paths and Edit > Stroke).

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::geom::{IRect, Point};
use crate::selection::{self, Mask};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeKind {
    #[default]
    Rectangle,
    RoundedRectangle,
    Ellipse,
    Triangle,
    Polygon,
    Star,
    Line,
    Arrow,
    Heart,
    SpeechBubble,
}

impl ShapeKind {
    pub const ALL: [ShapeKind; 10] = [
        ShapeKind::Rectangle,
        ShapeKind::RoundedRectangle,
        ShapeKind::Ellipse,
        ShapeKind::Triangle,
        ShapeKind::Polygon,
        ShapeKind::Star,
        ShapeKind::Line,
        ShapeKind::Arrow,
        ShapeKind::Heart,
        ShapeKind::SpeechBubble,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ShapeKind::Rectangle => "Rectangle",
            ShapeKind::RoundedRectangle => "Rounded Rectangle",
            ShapeKind::Ellipse => "Ellipse",
            ShapeKind::Triangle => "Triangle",
            ShapeKind::Polygon => "Polygon",
            ShapeKind::Star => "Star",
            ShapeKind::Line => "Line",
            ShapeKind::Arrow => "Arrow",
            ShapeKind::Heart => "Heart",
            ShapeKind::SpeechBubble => "Speech Bubble",
        }
    }

    /// Lines have no interior; the stroke is their only paint.
    pub fn is_open(self) -> bool {
        self == ShapeKind::Line
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShapeParams {
    pub corner_radius: f32,
    pub sides: u32,
    /// Star inner radius as a fraction of the outer one.
    pub star_inner: f32,
    pub line_width: f32,
}

impl Default for ShapeParams {
    fn default() -> Self {
        Self { corner_radius: 20.0, sides: 5, star_inner: 0.45, line_width: 4.0 }
    }
}

fn ellipse_pts(cx: f32, cy: f32, rx: f32, ry: f32, n: usize) -> Vec<Point> {
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32 * std::f32::consts::TAU;
            Point::new(cx + rx * t.cos(), cy + ry * t.sin())
        })
        .collect()
}

/// Closed outline(s) of a shape dragged from `a` to `b` (Line: the two endpoints).
pub fn outline(kind: ShapeKind, a: Point, b: Point, p: &ShapeParams) -> Vec<Point> {
    let (x0, y0, x1, y1) = (a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y));
    let (cx, cy) = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
    let (rx, ry) = ((x1 - x0) * 0.5, (y1 - y0) * 0.5);
    let segs = ((rx + ry) * 0.5).clamp(24.0, 256.0) as usize;
    match kind {
        ShapeKind::Rectangle => vec![Point::new(x0, y0), Point::new(x1, y0), Point::new(x1, y1), Point::new(x0, y1)],
        ShapeKind::RoundedRectangle => {
            let r = p.corner_radius.min(rx).min(ry).max(0.0);
            let mut pts = Vec::new();
            let corners = [(x1 - r, y0 + r, -90.0f32), (x1 - r, y1 - r, 0.0), (x0 + r, y1 - r, 90.0), (x0 + r, y0 + r, 180.0)];
            for (ccx, ccy, start) in corners {
                for i in 0..=8 {
                    let t = (start + i as f32 * 90.0 / 8.0).to_radians();
                    pts.push(Point::new(ccx + r * t.cos(), ccy + r * t.sin()));
                }
            }
            pts
        }
        ShapeKind::Ellipse => ellipse_pts(cx, cy, rx, ry, segs),
        ShapeKind::Triangle => vec![Point::new(cx, y0), Point::new(x1, y1), Point::new(x0, y1)],
        ShapeKind::Polygon => {
            let n = p.sides.clamp(3, 100) as usize;
            (0..n)
                .map(|i| {
                    let t = (i as f32 / n as f32) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                    Point::new(cx + rx * t.cos(), cy + ry * t.sin())
                })
                .collect()
        }
        ShapeKind::Star => {
            let n = p.sides.clamp(3, 100) as usize;
            (0..n * 2)
                .map(|i| {
                    let t = (i as f32 / (n * 2) as f32) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                    let k = if i % 2 == 0 { 1.0 } else { p.star_inner.clamp(0.05, 1.0) };
                    Point::new(cx + rx * k * t.cos(), cy + ry * k * t.sin())
                })
                .collect()
        }
        ShapeKind::Line => vec![a, b],
        ShapeKind::Arrow => {
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            let len = (dx * dx + dy * dy).sqrt().max(1e-3);
            let (ux, uy) = (dx / len, dy / len);
            let (nx, ny) = (-uy, ux);
            let shaft = p.line_width.max(1.0) * 0.5;
            let head_w = (shaft * 3.0).max(len * 0.15);
            let head_l = (head_w * 1.5).min(len * 0.6);
            let base = Point::new(b.x - ux * head_l, b.y - uy * head_l);
            vec![
                Point::new(a.x + nx * shaft, a.y + ny * shaft),
                Point::new(base.x + nx * shaft, base.y + ny * shaft),
                Point::new(base.x + nx * head_w, base.y + ny * head_w),
                b,
                Point::new(base.x - nx * head_w, base.y - ny * head_w),
                Point::new(base.x - nx * shaft, base.y - ny * shaft),
                Point::new(a.x - nx * shaft, a.y - ny * shaft),
            ]
        }
        ShapeKind::Heart => (0..segs)
            .map(|i| {
                let t = i as f32 / segs as f32 * std::f32::consts::TAU;
                let hx = 16.0 * t.sin().powi(3);
                let hy = 13.0 * t.cos() - 5.0 * (2.0 * t).cos() - 2.0 * (3.0 * t).cos() - (4.0 * t).cos();
                Point::new(cx + hx / 17.0 * rx, cy - (hy + 2.5) / 17.0 * ry)
            })
            .collect(),
        ShapeKind::SpeechBubble => {
            let body_y1 = y0 + (y1 - y0) * 0.78;
            let (bcy, bry) = ((y0 + body_y1) * 0.5, (body_y1 - y0) * 0.5);
            let mut pts = Vec::new();
            for i in 0..segs {
                let t = i as f32 / segs as f32 * std::f32::consts::TAU;
                let pt = Point::new(cx + rx * t.cos(), bcy + bry * t.sin());
                pts.push(pt);
                // Insert the tail at the lower-left of the ellipse.
                if (t - 2.0).abs() < std::f32::consts::TAU / segs as f32 * 0.5 {
                    pts.push(Point::new(x0 + (x1 - x0) * 0.2, y1));
                }
            }
            pts
        }
    }
}

/// Filled coverage mask of a shape.
pub fn shape_mask(w: u32, h: u32, kind: ShapeKind, a: Point, b: Point, p: &ShapeParams) -> Mask {
    match kind {
        ShapeKind::Ellipse => selection::ellipse_mask(w, h, IRect::from_points(a, b)),
        ShapeKind::Line => stroke_polyline(w, h, &[a, b], false, p.line_width, None),
        _ => selection::polygon_mask(w, h, &outline(kind, a, b, p)),
    }
}

/// Anti-aliased stroke of a polyline with round joins/caps. `dash` =
/// (dash length, gap length) in pixels.
pub fn stroke_polyline(w: u32, h: u32, pts: &[Point], closed: bool, width: f32, dash: Option<(f32, f32)>) -> Mask {
    let mut m = Mask::new(w, h);
    if pts.len() < 2 || width <= 0.0 {
        return m;
    }
    let mut segs: Vec<(Point, Point, f32)> = Vec::new(); // (a, b, arc length at a)
    let mut acc = 0.0;
    let n = pts.len();
    let count = if closed { n } else { n - 1 };
    for i in 0..count {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        segs.push((a, b, acc));
        acc += a.distance(b);
    }
    let half = width * 0.5;
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in pts {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    let rect = IRect::new((x0 - half - 2.0).floor() as i32, (y0 - half - 2.0).floor() as i32, (x1 + half + 2.0).ceil() as i32, (y1 + half + 2.0).ceil() as i32)
        .clamp_to(w, h);
    if rect.is_empty() {
        return m;
    }
    // Spatial bucketing of segments by row bands for speed.
    let band = 32i32;
    let nb = ((rect.height() + band - 1) / band) as usize;
    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); nb];
    for (si, (a, b, _)) in segs.iter().enumerate() {
        let lo = ((a.y.min(b.y) - half - 1.0).floor() as i32 - rect.y0).max(0) / band;
        let hi = ((a.y.max(b.y) + half + 1.0).ceil() as i32 - rect.y0).min(rect.height() - 1).max(0) / band;
        for bi in lo..=hi {
            if (bi as usize) < nb {
                buckets[bi as usize].push(si);
            }
        }
    }
    let rw = rect.width() as usize;
    let mut data = vec![0u8; rw * rect.height() as usize];
    data.par_chunks_mut(rw).enumerate().for_each(|(ry, row)| {
        let py = rect.y0 as f32 + ry as f32 + 0.5;
        let list = &buckets[ry / band as usize];
        if list.is_empty() {
            return;
        }
        for (rx, o) in row.iter_mut().enumerate() {
            let px = rect.x0 as f32 + rx as f32 + 0.5;
            let mut best = f32::MAX;
            let mut best_s = 0.0;
            for &si in list {
                let (a, b, s0) = segs[si];
                let (dx, dy) = (b.x - a.x, b.y - a.y);
                let l2 = dx * dx + dy * dy;
                let t = if l2 > 0.0 { (((px - a.x) * dx + (py - a.y) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
                let (qx, qy) = (a.x + dx * t, a.y + dy * t);
                let d = ((px - qx).powi(2) + (py - qy).powi(2)).sqrt();
                if d < best {
                    best = d;
                    best_s = s0 + t * l2.sqrt();
                }
            }
            let mut cov = (half - best + 0.5).clamp(0.0, 1.0);
            if let Some((dl, gl)) = dash {
                let period = (dl + gl).max(1.0);
                let ph = best_s.rem_euclid(period);
                let on = (dl - ph + 0.5).clamp(0.0, 1.0).min((ph + 0.5).clamp(0.0, 1.0));
                cov *= on;
            }
            *o = (cov * 255.0 + 0.5) as u8;
        }
    });
    m.write_dense(rect, &data);
    m.optimize();
    m
}

/// Stroke of a shape's outline.
pub fn shape_stroke(w: u32, h: u32, kind: ShapeKind, a: Point, b: Point, p: &ShapeParams, width: f32, dash: Option<(f32, f32)>) -> Mask {
    let pts = outline(kind, a, b, p);
    stroke_polyline(w, h, &pts, !kind.is_open(), width, dash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_have_area() {
        for k in ShapeKind::ALL {
            let m = shape_mask(100, 100, k, Point::new(10.0, 10.0), Point::new(90.0, 90.0), &ShapeParams::default());
            assert!(m.content_bounds().is_some(), "{k:?}");
        }
    }

    #[test]
    fn dashed_stroke() {
        let solid = stroke_polyline(100, 20, &[Point::new(0.0, 10.0), Point::new(100.0, 10.0)], false, 4.0, None);
        let dashed = stroke_polyline(100, 20, &[Point::new(0.0, 10.0), Point::new(100.0, 10.0)], false, 4.0, Some((10.0, 10.0)));
        let sum = |m: &Mask| m.to_dense().iter().map(|&v| v as u32).sum::<u32>();
        let ratio = sum(&dashed) as f32 / sum(&solid) as f32;
        assert!((ratio - 0.5).abs() < 0.08, "{ratio}");
        assert_eq!(solid.get(50, 10), 255);
        assert_eq!(solid.get(50, 15), 0);
    }
}
