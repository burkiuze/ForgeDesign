//! Vector paths (Pen tool): cubic Bézier sub-paths that can become a
//! selection, be filled or stroked.

use serde::{Deserialize, Serialize};

use crate::geom::{IRect, Point};
use crate::selection::{self, Mask};

/// An anchor with absolute in/out control handles.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    pub p: Point,
    pub h_in: Point,
    pub h_out: Point,
}

impl Anchor {
    pub fn corner(p: Point) -> Self {
        Self { p, h_in: p, h_out: p }
    }

    /// Smooth anchor with a symmetric handle pointing at `out`.
    pub fn smooth(p: Point, out: Point) -> Self {
        Self { p, h_in: Point::new(2.0 * p.x - out.x, 2.0 * p.y - out.y), h_out: out }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SubPath {
    pub anchors: Vec<Anchor>,
    pub closed: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VectorPath {
    pub name: String,
    pub subpaths: Vec<SubPath>,
}

fn cubic(a: Point, b: Point, c: Point, d: Point, t: f32) -> Point {
    let u = 1.0 - t;
    let (w0, w1, w2, w3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    Point::new(a.x * w0 + b.x * w1 + c.x * w2 + d.x * w3, a.y * w0 + b.y * w1 + c.y * w2 + d.y * w3)
}

impl SubPath {
    /// Polyline approximation.
    pub fn flatten(&self) -> Vec<Point> {
        let n = self.anchors.len();
        if n == 0 {
            return Vec::new();
        }
        let mut out = vec![self.anchors[0].p];
        let count = if self.closed { n } else { n - 1 };
        for i in 0..count {
            let (a, b) = (self.anchors[i], self.anchors[(i + 1) % n]);
            let straight = a.h_out == a.p && b.h_in == b.p;
            if straight {
                out.push(b.p);
                continue;
            }
            let len = a.p.distance(a.h_out) + a.h_out.distance(b.h_in) + b.h_in.distance(b.p);
            let steps = (len / 3.0).clamp(4.0, 200.0) as usize;
            for s in 1..=steps {
                out.push(cubic(a.p, a.h_out, b.h_in, b.p, s as f32 / steps as f32));
            }
        }
        if self.closed && out.len() > 1 && out.first() == out.last() {
            out.pop();
        }
        out
    }
}

impl VectorPath {
    pub fn flatten(&self) -> Vec<(Vec<Point>, bool)> {
        self.subpaths.iter().map(|s| (s.flatten(), s.closed)).collect()
    }

    pub fn bounds(&self) -> Option<IRect> {
        let mut r: Option<IRect> = None;
        for (pts, _) in self.flatten() {
            for p in pts {
                let pr = IRect::new(p.x.floor() as i32, p.y.floor() as i32, p.x.ceil() as i32 + 1, p.y.ceil() as i32 + 1);
                r = Some(r.map_or(pr, |r| r.union(&pr)));
            }
        }
        r
    }

    /// Filled area (even-odd across sub-paths; open sub-paths are closed implicitly).
    pub fn to_mask(&self, w: u32, h: u32) -> Mask {
        let rings: Vec<Vec<Point>> = self.flatten().into_iter().map(|(p, _)| p).filter(|p| p.len() >= 3).collect();
        selection::polygon_mask_multi(w, h, &rings)
    }

    pub fn stroke_mask(&self, w: u32, h: u32, width: f32, dash: Option<(f32, f32)>) -> Mask {
        let mut out = Mask::new(w, h);
        for (pts, closed) in self.flatten() {
            let m = crate::shapes::stroke_polyline(w, h, &pts, closed, width, dash);
            if let Some(r) = m.content_bounds() {
                out.par_for_each_in_rect_mut(r, |x, y, v| *v = (*v).max(m.get(x, y)));
            }
        }
        out
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        for s in &mut self.subpaths {
            for a in &mut s.anchors {
                for p in [&mut a.p, &mut a.h_in, &mut a.h_out] {
                    p.x += dx;
                    p.y += dy;
                }
            }
        }
    }

    /// Converts a selection outline into a path (Make Work Path).
    pub fn from_selection(sel: &Mask) -> VectorPath {
        // Trace boundary segments and chain them into loops.
        let segs = selection::outline(sel);
        let mut remaining: Vec<(Point, Point)> = segs;
        let mut subpaths = Vec::new();
        while let Some((a, b)) = remaining.pop() {
            let mut ring = vec![a, b];
            loop {
                let last = *ring.last().unwrap();
                let Some(i) = remaining.iter().position(|(p, q)| *p == last || *q == last) else { break };
                let (p, q) = remaining.swap_remove(i);
                ring.push(if p == last { q } else { p });
                if *ring.last().unwrap() == ring[0] {
                    break;
                }
            }
            if ring.len() >= 4 {
                ring.pop();
                // Drop collinear points.
                let mut simple: Vec<Point> = Vec::new();
                for (i, p) in ring.iter().enumerate() {
                    let prev = ring[(i + ring.len() - 1) % ring.len()];
                    let next = ring[(i + 1) % ring.len()];
                    let cross = (p.x - prev.x) * (next.y - p.y) - (p.y - prev.y) * (next.x - p.x);
                    if cross.abs() > 1e-6 {
                        simple.push(*p);
                    }
                }
                subpaths.push(SubPath { anchors: simple.into_iter().map(Anchor::corner).collect(), closed: true });
            }
        }
        VectorPath { name: "Work Path".into(), subpaths }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_to_mask_and_back() {
        let sp = SubPath {
            anchors: vec![
                Anchor::corner(Point::new(10.0, 10.0)),
                Anchor::corner(Point::new(50.0, 10.0)),
                Anchor::corner(Point::new(50.0, 40.0)),
                Anchor::corner(Point::new(10.0, 40.0)),
            ],
            closed: true,
        };
        let p = VectorPath { name: "p".into(), subpaths: vec![sp] };
        let m = p.to_mask(64, 64);
        assert_eq!(m.content_bounds(), Some(IRect::new(10, 10, 50, 40)));
        let back = VectorPath::from_selection(&m);
        assert_eq!(back.subpaths.len(), 1);
        assert_eq!(back.subpaths[0].anchors.len(), 4);
        let s = p.stroke_mask(64, 64, 2.0, None);
        assert_eq!(s.get(30, 10), 255);
        assert_eq!(s.get(30, 25), 0);
    }

    #[test]
    fn curves_flatten() {
        let sp = SubPath {
            anchors: vec![Anchor::smooth(Point::new(0.0, 0.0), Point::new(10.0, -10.0)), Anchor::corner(Point::new(20.0, 0.0))],
            closed: false,
        };
        let pts = sp.flatten();
        assert!(pts.len() > 4);
        assert!(pts.iter().any(|p| p.y < -2.0), "curve bends upwards");
    }
}
