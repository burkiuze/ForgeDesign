//! Selections are 8-bit masks (`TiledBuffer<u8>`, 0 = unselected,
//! 255 = selected). `None` in the document means "no selection", i.e. tools
//! affect the whole layer.

use serde::{Deserialize, Serialize};

use crate::buffer::TiledBuffer;
use crate::geom::{IRect, Point};

pub type Mask = TiledBuffer<u8>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectionOp {
    #[default]
    Replace,
    Add,
    Subtract,
    Intersect,
}

impl SelectionOp {
    pub const ALL: [SelectionOp; 4] =
        [SelectionOp::Replace, SelectionOp::Add, SelectionOp::Subtract, SelectionOp::Intersect];

    pub fn name(self) -> &'static str {
        match self {
            SelectionOp::Replace => "New",
            SelectionOp::Add => "Add",
            SelectionOp::Subtract => "Subtract",
            SelectionOp::Intersect => "Intersect",
        }
    }
}

/// Selection coverage at a pixel as `0.0..=1.0` (1.0 when nothing is selected).
#[inline]
pub fn coverage(sel: Option<&Mask>, x: i32, y: i32) -> f32 {
    match sel {
        Some(m) => m.get(x, y) as f32 * (1.0 / 255.0),
        None => 1.0,
    }
}

/// Rectangle that operations need to touch: the selection bounds, or the
/// whole canvas when nothing is selected.
pub fn work_rect(sel: Option<&Mask>, w: u32, h: u32) -> IRect {
    match sel {
        Some(m) => m.content_bounds().unwrap_or_default(),
        None => IRect::from_size(w, h),
    }
}

pub fn select_all(w: u32, h: u32) -> Mask {
    Mask::solid(w, h, 255)
}

pub fn rect_mask(w: u32, h: u32, r: IRect) -> Mask {
    let mut m = Mask::new(w, h);
    m.par_for_each_in_rect_mut(r, |_, _, p| *p = 255);
    m
}

/// Antialiased ellipse inscribed in `r` (4×4 supersampling).
pub fn ellipse_mask(w: u32, h: u32, r: IRect) -> Mask {
    let mut m = Mask::new(w, h);
    if r.is_empty() {
        return m;
    }
    let cx = (r.x0 + r.x1) as f32 * 0.5;
    let cy = (r.y0 + r.y1) as f32 * 0.5;
    let rx = r.width() as f32 * 0.5;
    let ry = r.height() as f32 * 0.5;
    m.par_for_each_in_rect_mut(r, |x, y, p| {
        let mut n = 0u32;
        for sy in 0..4 {
            let dy = (y as f32 + (sy as f32 + 0.5) * 0.25 - cy) / ry;
            for sx in 0..4 {
                let dx = (x as f32 + (sx as f32 + 0.5) * 0.25 - cx) / rx;
                n += (dx * dx + dy * dy <= 1.0) as u32;
            }
        }
        *p = ((n * 255 + 8) / 16) as u8;
    });
    m.optimize();
    m
}

/// Antialiased polygon fill (even-odd rule) with 4 sub-scanlines per pixel
/// and exact horizontal coverage. Used by the lasso and shape tools.
pub fn polygon_mask(w: u32, h: u32, pts: &[Point]) -> Mask {
    let mut m = Mask::new(w, h);
    if pts.len() < 3 {
        return m;
    }
    let (mut minx, mut miny, mut maxx, mut maxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in pts {
        minx = minx.min(p.x);
        miny = miny.min(p.y);
        maxx = maxx.max(p.x);
        maxy = maxy.max(p.y);
    }
    let bounds = IRect::new(minx.floor() as i32, miny.floor() as i32, maxx.ceil() as i32 + 1, maxy.ceil() as i32 + 1)
        .clamp_to(w, h);
    if bounds.is_empty() {
        return m;
    }
    let bw = bounds.width() as usize;
    let mut rows = vec![0u8; bw * bounds.height() as usize];
    const SUB: usize = 4;
    {
        use rayon::prelude::*;
        rows.par_chunks_mut(bw).enumerate().for_each(|(ry, row)| {
            let y = bounds.y0 + ry as i32;
            let mut acc = vec![0.0f32; bw + 1];
            let mut xs: Vec<f32> = Vec::with_capacity(16);
            for s in 0..SUB {
                let sy = y as f32 + (s as f32 + 0.5) / SUB as f32;
                xs.clear();
                for i in 0..pts.len() {
                    let a = pts[i];
                    let b = pts[(i + 1) % pts.len()];
                    if (a.y <= sy && b.y > sy) || (b.y <= sy && a.y > sy) {
                        xs.push(a.x + (sy - a.y) / (b.y - a.y) * (b.x - a.x));
                    }
                }
                xs.sort_by(f32::total_cmp);
                for pair in xs.chunks_exact(2) {
                    let x0 = (pair[0] - bounds.x0 as f32).clamp(0.0, bw as f32);
                    let x1 = (pair[1] - bounds.x0 as f32).clamp(0.0, bw as f32);
                    if x1 <= x0 {
                        continue;
                    }
                    let (i0, i1) = (x0.floor() as usize, x1.floor() as usize);
                    if i0 == i1 {
                        acc[i0] += x1 - x0;
                    } else {
                        acc[i0] += (i0 + 1) as f32 - x0;
                        for v in &mut acc[i0 + 1..i1] {
                            *v += 1.0;
                        }
                        acc[i1] += x1 - i1 as f32;
                    }
                }
            }
            for (o, a) in row.iter_mut().zip(&acc) {
                *o = ((a / SUB as f32).min(1.0) * 255.0 + 0.5) as u8;
            }
        });
    }
    m.write_dense(bounds, &rows);
    m.optimize();
    m
}

/// Combines an existing selection with a new shape.
/// Returns `None` when the result selects nothing.
pub fn combine(existing: Option<&Mask>, new: Mask, op: SelectionOp) -> Option<Mask> {
    let result = match (op, existing) {
        (SelectionOp::Subtract, None) => return None,
        (SelectionOp::Replace, _) | (_, None) => new,
        (op, Some(old)) => {
            let mut out = old.clone();
            out.par_map_all(|x, y, a| {
                let b = new.get(x, y);
                match op {
                    SelectionOp::Add => a.max(b),
                    SelectionOp::Subtract => ((a as u32 * (255 - b as u32) + 127) / 255) as u8,
                    SelectionOp::Intersect => a.min(b),
                    SelectionOp::Replace => b,
                }
            });
            out
        }
    };
    let mut result = result;
    result.optimize();
    result.content_bounds().map(|_| result)
}

pub fn invert(sel: Option<&Mask>, w: u32, h: u32) -> Option<Mask> {
    match sel {
        None => None,
        Some(m) => {
            let mut out = Mask::solid(w, h, 255);
            out.par_map_all(|x, y, _| 255 - m.get(x, y));
            out.optimize();
            out.content_bounds().map(|_| out)
        }
    }
}

/// Softens the selection edge with a Gaussian blur of `radius` pixels.
pub fn feather(m: &Mask, radius: f32) -> Mask {
    if radius <= 0.0 {
        return m.clone();
    }
    let Some(b) = m.content_bounds() else { return m.clone() };
    let pad = (radius * 3.0).ceil() as i32 + 1;
    let rect = b.expand(pad).clamp_to(m.width(), m.height());
    let data: Vec<f32> = m.region_to_dense(rect).iter().map(|&v| v as f32).collect();
    let blurred = crate::filters::gaussian_blur_gray(&data, rect.width() as usize, rect.height() as usize, radius);
    let bytes: Vec<u8> = blurred.iter().map(|v| v.round().clamp(0.0, 255.0) as u8).collect();
    let mut out = m.clone();
    out.write_dense(rect, &bytes);
    out.optimize();
    out
}

/// Grows (`amount > 0`) or shrinks (`amount < 0`) the selection with a
/// separable square min/max filter.
pub fn grow(m: &Mask, amount: i32) -> Mask {
    if amount == 0 {
        return m.clone();
    }
    let Some(b) = m.content_bounds() else { return m.clone() };
    let rect = if amount > 0 { b.expand(amount + 1) } else { b.expand(1) }.clamp_to(m.width(), m.height());
    let (w, h) = (rect.width() as usize, rect.height() as usize);
    let mut data = m.region_to_dense(rect);
    let r = amount.unsigned_abs() as usize;
    let pick = |a: u8, b: u8| if amount > 0 { a.max(b) } else { a.min(b) };
    let mut tmp = data.clone();
    for y in 0..h {
        for x in 0..w {
            let mut v = data[y * w + x];
            for k in x.saturating_sub(r)..(x + r + 1).min(w) {
                v = pick(v, data[y * w + k]);
            }
            tmp[y * w + x] = v;
        }
    }
    for y in 0..h {
        for x in 0..w {
            let mut v = tmp[y * w + x];
            for k in y.saturating_sub(r)..(y + r + 1).min(h) {
                v = pick(v, tmp[k * w + x]);
            }
            data[y * w + x] = v;
        }
    }
    let mut out = m.clone();
    out.write_dense(rect, &data);
    out.optimize();
    out
}

/// Boundary of the selection (at 50% coverage) as merged axis-aligned
/// segments in image coordinates, for drawing "marching ants".
pub fn outline(m: &Mask) -> Vec<(Point, Point)> {
    let Some(b) = m.content_bounds() else { return Vec::new() };
    let rect = b.expand(1);
    let (w, h) = (rect.width() as usize, rect.height() as usize);
    let data = m.region_to_dense(rect);
    let inside = |x: isize, y: isize| -> bool {
        x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && data[y as usize * w + x as usize] >= 128
    };
    let mut segs = Vec::new();
    let ox = rect.x0 as f32;
    let oy = rect.y0 as f32;
    // Horizontal edges: between row y-1 and y.
    for y in 0..=h as isize {
        let mut run: Option<isize> = None;
        for x in 0..=w as isize {
            let edge = x < w as isize && inside(x, y) != inside(x, y - 1);
            match (edge, run) {
                (true, None) => run = Some(x),
                (false, Some(s)) => {
                    segs.push((Point::new(ox + s as f32, oy + y as f32), Point::new(ox + x as f32, oy + y as f32)));
                    run = None;
                }
                _ => {}
            }
        }
    }
    // Vertical edges: between column x-1 and x.
    for x in 0..=w as isize {
        let mut run: Option<isize> = None;
        for y in 0..=h as isize {
            let edge = y < h as isize && inside(x, y) != inside(x - 1, y);
            match (edge, run) {
                (true, None) => run = Some(y),
                (false, Some(s)) => {
                    segs.push((Point::new(ox + x as f32, oy + s as f32), Point::new(ox + x as f32, oy + y as f32)));
                    run = None;
                }
                _ => {}
            }
        }
    }
    segs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_outline_is_four_segments() {
        let m = rect_mask(100, 100, IRect::new(10, 10, 30, 20));
        assert_eq!(outline(&m).len(), 4);
    }

    #[test]
    fn combine_ops() {
        let a = rect_mask(100, 100, IRect::new(0, 0, 50, 50));
        let b = rect_mask(100, 100, IRect::new(25, 25, 75, 75));
        let add = combine(Some(&a), b.clone(), SelectionOp::Add).unwrap();
        assert_eq!(add.content_bounds(), Some(IRect::new(0, 0, 75, 75)));
        let inter = combine(Some(&a), b.clone(), SelectionOp::Intersect).unwrap();
        assert_eq!(inter.content_bounds(), Some(IRect::new(25, 25, 50, 50)));
        let sub = combine(Some(&a), b.clone(), SelectionOp::Subtract).unwrap();
        assert_eq!(sub.get(30, 30), 0);
        assert_eq!(sub.get(10, 10), 255);
        assert!(combine(Some(&a), Mask::new(100, 100), SelectionOp::Replace).is_none());
    }

    #[test]
    fn polygon_coverage() {
        // A triangle covering half of a 10x10 square.
        let m = polygon_mask(20, 20, &[Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(0.0, 10.0)]);
        let total: u32 = m.to_dense().iter().map(|&v| v as u32).sum();
        let area = total as f32 / 255.0;
        assert!((area - 50.0).abs() < 1.5, "area {area}");
        let e = ellipse_mask(64, 64, IRect::new(0, 0, 64, 64));
        assert_eq!(e.get(32, 32), 255);
        assert_eq!(e.get(0, 0), 0);
    }

    #[test]
    fn grow_shrink() {
        let a = rect_mask(100, 100, IRect::new(40, 40, 60, 60));
        assert_eq!(grow(&a, 5).content_bounds(), Some(IRect::new(35, 35, 65, 65)));
        assert_eq!(grow(&a, -5).content_bounds(), Some(IRect::new(45, 45, 55, 55)));
    }
}
