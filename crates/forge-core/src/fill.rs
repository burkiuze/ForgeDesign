//! Paint Bucket / Magic Wand flood fills, gradients and shape fills.

use serde::{Deserialize, Serialize};

use crate::blend::{composite, BlendMode};
use crate::buffer::TiledBuffer;
use crate::color::{Color, Rgba8};
use crate::geom::{IRect, Point};
use crate::selection::{self, coverage, Mask};

#[inline]
fn matches(a: Rgba8, b: Rgba8, tol: i32) -> bool {
    (a[0] as i32 - b[0] as i32).abs() <= tol
        && (a[1] as i32 - b[1] as i32).abs() <= tol
        && (a[2] as i32 - b[2] as i32).abs() <= tol
        && (a[3] as i32 - b[3] as i32).abs() <= tol
}

/// Pixels similar to the seed pixel, as a selection-style mask.
/// `contiguous` limits the region to pixels 4-connected to the seed.
pub fn flood_mask(src: &TiledBuffer<Rgba8>, seed_x: i32, seed_y: i32, tolerance: u8, contiguous: bool) -> Mask {
    let (w, h) = (src.width(), src.height());
    let mut out = Mask::new(w, h);
    if seed_x < 0 || seed_y < 0 || seed_x as u32 >= w || seed_y as u32 >= h {
        return out;
    }
    let seed = src.get(seed_x, seed_y);
    let tol = tolerance as i32;
    if !contiguous {
        out.par_map_all(|x, y, _| if matches(src.get(x, y), seed, tol) { 255 } else { 0 });
        out.optimize();
        return out;
    }
    let (wu, hu) = (w as usize, h as usize);
    let data = src.to_dense();
    let mut filled = vec![false; wu * hu];
    let mut stack = vec![(seed_x as usize, seed_y as usize)];
    // Scanline flood fill.
    while let Some((x, y)) = stack.pop() {
        let row = y * wu;
        if filled[row + x] {
            continue;
        }
        let mut l = x;
        while l > 0 && !filled[row + l - 1] && matches(data[row + l - 1], seed, tol) {
            l -= 1;
        }
        let mut r = x;
        while r + 1 < wu && !filled[row + r + 1] && matches(data[row + r + 1], seed, tol) {
            r += 1;
        }
        for i in l..=r {
            filled[row + i] = true;
        }
        for ny in [y.wrapping_sub(1), y + 1] {
            if ny >= hu {
                continue;
            }
            let nrow = ny * wu;
            let mut in_run = false;
            for i in l..=r {
                let ok = !filled[nrow + i] && matches(data[nrow + i], seed, tol);
                if ok && !in_run {
                    stack.push((i, ny));
                }
                in_run = ok;
            }
        }
    }
    let bytes: Vec<u8> = filled.iter().map(|&f| if f { 255 } else { 0 }).collect();
    out = Mask::from_dense(w, h, &bytes);
    out
}

/// Composites a straight colour onto one straight pixel with a blend mode.
#[inline]
pub fn blend_pixel(dst: Rgba8, c: [f32; 3], a: f32, mode: BlendMode, lock_alpha: bool, x: i32, y: i32) -> Rgba8 {
    if a <= 0.0 {
        return dst;
    }
    let da = dst[3] as f32 / 255.0;
    let mut acc = [dst[0] as f32 / 255.0 * da, dst[1] as f32 / 255.0 * da, dst[2] as f32 / 255.0 * da, da];
    composite(&mut acc, c, a, mode, x, y);
    let out_a = if lock_alpha { da } else { acc[3] };
    if acc[3] <= 0.0 || out_a <= 0.0 {
        return if lock_alpha { dst } else { [0, 0, 0, 0] };
    }
    let inv = 1.0 / acc[3];
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    [q(acc[0] * inv), q(acc[1] * inv), q(acc[2] * inv), q(out_a)]
}

/// Paints `color` through `shape` (coverage mask) and the selection.
/// Returns the modified rectangle.
pub fn fill_mask(
    target: &mut TiledBuffer<Rgba8>,
    shape: Option<&Mask>,
    sel: Option<&Mask>,
    color: Color,
    opacity: f32,
    mode: BlendMode,
    lock_alpha: bool,
) -> Option<IRect> {
    let (w, h) = (target.width(), target.height());
    let rect = match shape {
        Some(m) => m.content_bounds()?,
        None => selection::work_rect(sel, w, h),
    };
    let rect = match sel {
        Some(s) => rect.intersect(&s.content_bounds()?),
        None => rect,
    };
    if rect.is_empty() {
        return None;
    }
    let c = color.rgb_array();
    let ca = color.a * opacity;
    target.par_for_each_in_rect_mut(rect, |x, y, p| {
        let a = ca * coverage(shape, x, y) * coverage(sel, x, y);
        *p = blend_pixel(*p, c, a, mode, lock_alpha, x, y);
    });
    target.optimize();
    Some(rect)
}

/// Deletes (makes transparent) the selected pixels.
pub fn clear(target: &mut TiledBuffer<Rgba8>, sel: Option<&Mask>) -> Option<IRect> {
    let rect = selection::work_rect(sel, target.width(), target.height());
    if rect.is_empty() {
        return None;
    }
    target.par_for_each_in_rect_mut(rect, |x, y, p| {
        let s = coverage(sel, x, y);
        p[3] = (p[3] as f32 * (1.0 - s) + 0.5) as u8;
    });
    target.optimize();
    Some(rect)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GradientKind {
    #[default]
    Linear,
    Radial,
    Angle,
    Reflected,
    Diamond,
}

impl GradientKind {
    pub const ALL: [GradientKind; 5] =
        [GradientKind::Linear, GradientKind::Radial, GradientKind::Angle, GradientKind::Reflected, GradientKind::Diamond];

    pub fn name(self) -> &'static str {
        match self {
            GradientKind::Linear => "Linear",
            GradientKind::Radial => "Radial",
            GradientKind::Angle => "Angle",
            GradientKind::Reflected => "Reflected",
            GradientKind::Diamond => "Diamond",
        }
    }

    /// Gradient parameter `t` (0..1) at `p` for a drag from `a` to `b`.
    pub fn t_at(self, a: Point, b: Point, p: Point) -> f32 {
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let len2 = (dx * dx + dy * dy).max(1e-6);
        let len = len2.sqrt();
        let (px, py) = (p.x - a.x, p.y - a.y);
        let along = (px * dx + py * dy) / len2;
        let t = match self {
            GradientKind::Linear => along,
            GradientKind::Radial => (px * px + py * py).sqrt() / len,
            GradientKind::Angle => {
                let ang = py.atan2(px) - dy.atan2(dx);
                ang.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU
            }
            GradientKind::Reflected => along.abs(),
            GradientKind::Diamond => {
                let across = (px * -dy + py * dx) / len2;
                along.abs() + across.abs()
            }
        };
        t.clamp(0.0, 1.0)
    }
}

/// Draws a two-colour gradient into the selection.
#[allow(clippy::too_many_arguments)]
pub fn gradient(
    target: &mut TiledBuffer<Rgba8>,
    sel: Option<&Mask>,
    kind: GradientKind,
    a: Point,
    b: Point,
    c0: Color,
    c1: Color,
    opacity: f32,
    mode: BlendMode,
    lock_alpha: bool,
) -> Option<IRect> {
    let rect = selection::work_rect(sel, target.width(), target.height());
    if rect.is_empty() {
        return None;
    }
    target.par_for_each_in_rect_mut(rect, |x, y, p| {
        let t = kind.t_at(a, b, Point::new(x as f32 + 0.5, y as f32 + 0.5));
        // Ordered dithering hides banding in 8-bit output.
        let t = (t + (crate::blend::pixel_noise(x, y) - 0.5) / 255.0).clamp(0.0, 1.0);
        let c = c0.lerp(c1, t);
        let s = coverage(sel, x, y);
        *p = blend_pixel(*p, c.rgb_array(), c.a * opacity * s, mode, lock_alpha, x, y);
    });
    Some(rect)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeKind {
    #[default]
    Rectangle,
    Ellipse,
    Line,
}

impl ShapeKind {
    pub const ALL: [ShapeKind; 3] = [ShapeKind::Rectangle, ShapeKind::Ellipse, ShapeKind::Line];
    pub fn name(self) -> &'static str {
        match self {
            ShapeKind::Rectangle => "Rectangle",
            ShapeKind::Ellipse => "Ellipse",
            ShapeKind::Line => "Line",
        }
    }
}

/// Coverage mask of a shape dragged from `a` to `b`.
pub fn shape_mask(w: u32, h: u32, kind: ShapeKind, a: Point, b: Point, line_width: f32) -> Mask {
    match kind {
        ShapeKind::Rectangle => selection::polygon_mask(
                w,
                h,
                &[
                    Point::new(a.x.min(b.x), a.y.min(b.y)),
                    Point::new(a.x.max(b.x), a.y.min(b.y)),
                    Point::new(a.x.max(b.x), a.y.max(b.y)),
                    Point::new(a.x.min(b.x), a.y.max(b.y)),
                ],
            ),
        ShapeKind::Ellipse => selection::ellipse_mask(w, h, IRect::from_points(a, b)),
        ShapeKind::Line => {
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            let len = (dx * dx + dy * dy).sqrt().max(1e-6);
            let (nx, ny) = (-dy / len * line_width * 0.5, dx / len * line_width * 0.5);
            selection::polygon_mask(
                w,
                h,
                &[
                    Point::new(a.x + nx, a.y + ny),
                    Point::new(b.x + nx, b.y + ny),
                    Point::new(b.x - nx, b.y - ny),
                    Point::new(a.x - nx, a.y - ny),
                ],
            )
        }
    }
}

/// Outline ("Edit > Stroke") of the selection, `width` pixels, centred on the edge.
pub fn stroke_selection_mask(sel: &Mask, width: f32) -> Mask {
    let half = (width * 0.5).ceil().max(1.0) as i32;
    let outer = selection::grow(sel, half);
    let inner = selection::grow(sel, -half);
    let mut out = outer;
    out.par_map_all(|x, y, a| ((a as u32 * (255 - inner.get(x, y) as u32) + 127) / 255) as u8);
    out.optimize();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flood_contiguous_vs_global() {
        let mut img = TiledBuffer::solid(100, 100, [255, 255, 255, 255]);
        // vertical black wall splitting the image
        for y in 0..100 {
            img.set(50, y, [0, 0, 0, 255]);
        }
        let m = flood_mask(&img, 10, 10, 10, true);
        assert_eq!(m.content_bounds(), Some(IRect::new(0, 0, 50, 100)));
        let g = flood_mask(&img, 10, 10, 10, false);
        assert_eq!(g.get(80, 10), 255);
        assert_eq!(g.get(50, 10), 0);
    }

    #[test]
    fn gradient_endpoints() {
        let mut img = TiledBuffer::new(100, 10);
        gradient(
            &mut img,
            None,
            GradientKind::Linear,
            Point::new(0.0, 5.0),
            Point::new(100.0, 5.0),
            Color::BLACK,
            Color::WHITE,
            1.0,
            BlendMode::Normal,
            false,
        );
        assert!(img.get(0, 5)[0] < 5);
        assert!(img.get(99, 5)[0] > 250);
        assert_eq!(img.get(50, 5)[3], 255);
    }

    #[test]
    fn fill_selection() {
        let mut img = TiledBuffer::new(100, 100);
        let sel = selection::rect_mask(100, 100, IRect::new(10, 10, 20, 20));
        let r = fill_mask(&mut img, None, Some(&sel), Color::rgb(1.0, 0.0, 0.0), 1.0, BlendMode::Normal, false);
        assert_eq!(r, Some(IRect::new(10, 10, 20, 20)));
        assert_eq!(img.get(15, 15), [255, 0, 0, 255]);
        assert_eq!(img.get(25, 15), [0, 0, 0, 0]);
    }
}
