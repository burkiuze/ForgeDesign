//! Geometric operations: resampling (Image Size), Canvas Size, Crop,
//! rotation/flip, Move and Free Transform.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::buffer::{Pixel, TiledBuffer};
use crate::color::Rgba8;
use crate::document::DocState;
use crate::filters::FImage;
use crate::geom::IRect;
use crate::selection::{coverage, Mask};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resample {
    Nearest,
    Bilinear,
    #[default]
    Bicubic,
    Lanczos3,
}

impl Resample {
    pub const ALL: [Resample; 4] = [Resample::Nearest, Resample::Bilinear, Resample::Bicubic, Resample::Lanczos3];

    pub fn name(self) -> &'static str {
        match self {
            Resample::Nearest => "Nearest Neighbor",
            Resample::Bilinear => "Bilinear",
            Resample::Bicubic => "Bicubic",
            Resample::Lanczos3 => "Lanczos-3",
        }
    }

    fn support(self) -> f32 {
        match self {
            Resample::Nearest => 0.5,
            Resample::Bilinear => 1.0,
            Resample::Bicubic => 2.0,
            Resample::Lanczos3 => 3.0,
        }
    }

    fn kernel(self, x: f32) -> f32 {
        let x = x.abs();
        match self {
            Resample::Nearest => (x < 0.5) as u8 as f32,
            Resample::Bilinear => (1.0 - x).max(0.0),
            Resample::Bicubic => {
                // Catmull-Rom (a = -0.5)
                let a = -0.5;
                if x < 1.0 {
                    (a + 2.0) * x * x * x - (a + 3.0) * x * x + 1.0
                } else if x < 2.0 {
                    a * x * x * x - 5.0 * a * x * x + 8.0 * a * x - 4.0 * a
                } else {
                    0.0
                }
            }
            Resample::Lanczos3 => {
                if x < 1e-6 {
                    1.0
                } else if x < 3.0 {
                    let px = std::f32::consts::PI * x;
                    3.0 * px.sin() * (px / 3.0).sin() / (px * px)
                } else {
                    0.0
                }
            }
        }
    }
}

/// Precomputed filter taps for one output coordinate.
struct Taps {
    start: usize,
    weights: Vec<f32>,
}

fn compute_taps(src: usize, dst: usize, method: Resample) -> Vec<Taps> {
    let scale = dst as f32 / src as f32;
    // When minifying, stretch the kernel to low-pass filter (no aliasing).
    let fscale = scale.min(1.0);
    let support = method.support() / fscale;
    (0..dst)
        .map(|i| {
            let center = (i as f32 + 0.5) / scale;
            let start = ((center - support).floor() as isize).max(0) as usize;
            let end = ((center + support).ceil() as usize).min(src);
            let mut weights: Vec<f32> =
                (start..end).map(|j| method.kernel((j as f32 + 0.5 - center) * fscale)).collect();
            let sum: f32 = weights.iter().sum();
            if sum.abs() > 1e-6 {
                weights.iter_mut().for_each(|w| *w /= sum);
            } else if !weights.is_empty() {
                // Degenerate (nearest at exact half): take the closest sample.
                let k = ((center - 0.5).round() as usize).clamp(start, end - 1) - start;
                weights.iter_mut().for_each(|w| *w = 0.0);
                weights[k] = 1.0;
            }
            Taps { start, weights }
        })
        .collect()
}

/// Separable resampling of an N-channel dense image.
pub fn resample_dense<const N: usize>(
    data: &[[f32; N]],
    w: usize,
    h: usize,
    nw: usize,
    nh: usize,
    method: Resample,
) -> Vec<[f32; N]> {
    let xt = compute_taps(w, nw, method);
    let mut tmp = vec![[0.0f32; N]; nw * h];
    tmp.par_chunks_mut(nw).enumerate().for_each(|(y, row)| {
        let src = &data[y * w..(y + 1) * w];
        for (x, o) in row.iter_mut().enumerate() {
            let t = &xt[x];
            let mut acc = [0.0; N];
            for (k, wgt) in t.weights.iter().enumerate() {
                let s = src[t.start + k];
                for c in 0..N {
                    acc[c] += s[c] * wgt;
                }
            }
            *o = acc;
        }
    });
    let yt = compute_taps(h, nh, method);
    let mut out = vec![[0.0f32; N]; nw * nh];
    out.par_chunks_mut(nw).enumerate().for_each(|(y, row)| {
        let t = &yt[y];
        for (k, wgt) in t.weights.iter().enumerate() {
            let src = &tmp[(t.start + k) * nw..][..nw];
            for (o, s) in row.iter_mut().zip(src) {
                for c in 0..N {
                    o[c] += s[c] * wgt;
                }
            }
        }
    });
    out
}

/// Resamples an RGBA buffer (premultiplied internally, so transparent
/// edges don't create dark fringes).
pub fn resample_rgba(buf: &TiledBuffer<Rgba8>, nw: u32, nh: u32, method: Resample) -> TiledBuffer<Rgba8> {
    let img = FImage::from_buffer(buf, buf.bounds());
    let out = resample_dense(&img.data, img.w, img.h, nw as usize, nh as usize, method);
    let px: Vec<Rgba8> = out.par_iter().map(|p| premul_to_rgba8(*p)).collect();
    TiledBuffer::from_dense(nw, nh, &px)
}

pub fn resample_mask(buf: &TiledBuffer<u8>, nw: u32, nh: u32, method: Resample) -> TiledBuffer<u8> {
    let data: Vec<[f32; 1]> = buf.to_dense().iter().map(|&v| [v as f32]).collect();
    let out = resample_dense(&data, buf.width() as usize, buf.height() as usize, nw as usize, nh as usize, method);
    let px: Vec<u8> = out.iter().map(|[v]| v.round().clamp(0.0, 255.0) as u8).collect();
    let mut m = TiledBuffer::new_filled(nw, nh, buf.fill_value());
    m.write_dense(IRect::from_size(nw, nh), &px);
    m.optimize();
    m
}

#[inline]
fn premul_to_rgba8(p: [f32; 4]) -> Rgba8 {
    let a = p[3].clamp(0.0, 1.0);
    if a <= 1e-6 {
        return [0, 0, 0, 0];
    }
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    [q(p[0] / a), q(p[1] / a), q(p[2] / a), q(a)]
}

/// Image > Image Size.
pub fn resize_document(state: &mut DocState, nw: u32, nh: u32, method: Resample) {
    let (nw, nh) = (nw.max(1), nh.max(1));
    state.layers.par_iter_mut().for_each(|l| {
        l.pixels = resample_rgba(&l.pixels, nw, nh, method);
        if let Some(m) = &l.mask {
            l.mask = Some(resample_mask(m, nw, nh, method));
        }
    });
    state.selection = state.selection.as_ref().map(|m| resample_mask(m, nw, nh, method));
    state.width = nw;
    state.height = nh;
}

/// Copies `buf` into a new `nw × nh` buffer, shifted by `(dx, dy)`.
pub fn translate<P: Pixel>(buf: &TiledBuffer<P>, dx: i32, dy: i32, nw: u32, nh: u32) -> TiledBuffer<P> {
    let mut out = TiledBuffer::new_filled(nw, nh, buf.fill_value());
    let src = buf.bounds().translate(dx, dy).clamp_to(nw, nh);
    out.par_for_each_in_rect_mut(src, |x, y, p| *p = buf.get(x - dx, y - dy));
    out.optimize();
    out
}

/// Image > Canvas Size. `anchor` is (0..1, 0..1): (0.5, 0.5) keeps the image centred.
pub fn canvas_size(state: &mut DocState, nw: u32, nh: u32, anchor: (f32, f32)) {
    let dx = ((nw as f32 - state.width as f32) * anchor.0).round() as i32;
    let dy = ((nh as f32 - state.height as f32) * anchor.1).round() as i32;
    offset_document(state, dx, dy, nw.max(1), nh.max(1));
}

/// Image > Crop.
pub fn crop(state: &mut DocState, r: IRect) {
    let r = r.clamp_to(state.width, state.height);
    if r.is_empty() {
        return;
    }
    offset_document(state, -r.x0, -r.y0, r.width() as u32, r.height() as u32);
}

fn offset_document(state: &mut DocState, dx: i32, dy: i32, nw: u32, nh: u32) {
    state.layers.par_iter_mut().for_each(|l| {
        l.pixels = translate(&l.pixels, dx, dy, nw, nh);
        if let Some(m) = &l.mask {
            l.mask = Some(translate(m, dx, dy, nw, nh));
        }
    });
    state.selection = state.selection.as_ref().map(|m| translate(m, dx, dy, nw, nh)).filter(|m| m.content_bounds().is_some());
    state.width = nw;
    state.height = nh;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Orientation {
    Rotate90Cw,
    Rotate90Ccw,
    Rotate180,
    FlipHorizontal,
    FlipVertical,
}

impl Orientation {
    pub fn name(self) -> &'static str {
        match self {
            Orientation::Rotate90Cw => "Rotate 90° Clockwise",
            Orientation::Rotate90Ccw => "Rotate 90° Counter Clockwise",
            Orientation::Rotate180 => "Rotate 180°",
            Orientation::FlipHorizontal => "Flip Horizontal",
            Orientation::FlipVertical => "Flip Vertical",
        }
    }

    fn swaps_axes(self) -> bool {
        matches!(self, Orientation::Rotate90Cw | Orientation::Rotate90Ccw)
    }
}

/// Re-orients a whole buffer.
pub fn reorient<P: Pixel>(buf: &TiledBuffer<P>, o: Orientation) -> TiledBuffer<P> {
    let (w, h) = (buf.width() as i32, buf.height() as i32);
    let (nw, nh) = if o.swaps_axes() { (h as u32, w as u32) } else { (w as u32, h as u32) };
    let mut out = TiledBuffer::new_filled(nw, nh, buf.fill_value());
    out.par_for_each_in_rect_mut(IRect::from_size(nw, nh), |x, y, p| {
        *p = match o {
            Orientation::Rotate90Cw => buf.get(y, h - 1 - x),
            Orientation::Rotate90Ccw => buf.get(w - 1 - y, x),
            Orientation::Rotate180 => buf.get(w - 1 - x, h - 1 - y),
            Orientation::FlipHorizontal => buf.get(w - 1 - x, y),
            Orientation::FlipVertical => buf.get(x, h - 1 - y),
        }
    });
    out.optimize();
    out
}

/// Image > Image Rotation / Flip Canvas.
pub fn reorient_document(state: &mut DocState, o: Orientation) {
    state.layers.par_iter_mut().for_each(|l| {
        l.pixels = reorient(&l.pixels, o);
        if let Some(m) = &l.mask {
            l.mask = Some(reorient(m, o));
        }
    });
    state.selection = state.selection.as_ref().map(|m| reorient(m, o));
    if o.swaps_axes() {
        std::mem::swap(&mut state.width, &mut state.height);
    }
}

/// Move tool: shifts the selected pixels of `base` by `(dx, dy)`. Without a
/// selection the whole layer moves. Returns the new pixels and the moved
/// selection.
pub fn move_pixels(base: &TiledBuffer<Rgba8>, sel: Option<&Mask>, dx: i32, dy: i32) -> (TiledBuffer<Rgba8>, Option<Mask>) {
    let (w, h) = (base.width(), base.height());
    let Some(sel) = sel else {
        return (translate(base, dx, dy, w, h), None);
    };
    let moved_sel = translate(sel, dx, dy, w, h);
    let mut out = base.clone();
    let src_bounds = sel.content_bounds().unwrap_or_default();
    let affected = src_bounds.union(&src_bounds.translate(dx, dy)).clamp_to(w, h);
    out.par_for_each_in_rect_mut(affected, |x, y, p| {
        // Hole left behind.
        let b = base.get(x, y);
        let s = coverage(Some(sel), x, y);
        let mut hole = b;
        hole[3] = (b[3] as f32 * (1.0 - s) + 0.5) as u8;
        // Floating content on top.
        let f = base.get(x - dx, y - dy);
        let fs = coverage(Some(sel), x - dx, y - dy);
        let fa = f[3] as f32 / 255.0 * fs;
        *p = crate::brush::paint_over(hole, [f[0] as f32 / 255.0, f[1] as f32 / 255.0, f[2] as f32 / 255.0], fa, false);
    });
    out.optimize();
    (out, Some(moved_sel))
}

/// 2D affine matrix: `x' = a·x + c·y + e`, `y' = b·x + d·y + f`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Affine {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Affine {
    pub const IDENTITY: Affine = Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    /// Scale + rotate (degrees) around `(cx, cy)`, then translate.
    pub fn scale_rotate(cx: f32, cy: f32, sx: f32, sy: f32, deg: f32, tx: f32, ty: f32) -> Affine {
        let (s, c) = deg.to_radians().sin_cos();
        let a = c * sx;
        let b = s * sx;
        let cc = -s * sy;
        let d = c * sy;
        Affine { a, b, c: cc, d, e: cx + tx - (a * cx + cc * cy), f: cy + ty - (b * cx + d * cy) }
    }

    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (self.a * x + self.c * y + self.e, self.b * x + self.d * y + self.f)
    }

    pub fn inverse(&self) -> Option<Affine> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-9 {
            return None;
        }
        let id = 1.0 / det;
        let a = self.d * id;
        let b = -self.b * id;
        let c = -self.c * id;
        let d = self.a * id;
        Some(Affine { a, b, c, d, e: -(a * self.e + c * self.f), f: -(b * self.e + d * self.f) })
    }
}

/// Bounds of what a transform moves: the selection, or the layer content.
pub fn transform_source_bounds(base: &TiledBuffer<Rgba8>, sel: Option<&Mask>) -> Option<IRect> {
    match sel {
        Some(s) => s.content_bounds(),
        None => base.content_bounds(),
    }
}

/// Free Transform of the selected pixels (or whole layer).
pub fn transform_pixels(base: &TiledBuffer<Rgba8>, sel: Option<&Mask>, m: Affine, method: Resample) -> (TiledBuffer<Rgba8>, Option<Mask>) {
    let Some(inv) = m.inverse() else { return (base.clone(), sel.cloned()) };
    let Some(src_bounds) = transform_source_bounds(base, sel) else { return (base.clone(), sel.cloned()) };
    let corners = [
        m.apply(src_bounds.x0 as f32, src_bounds.y0 as f32),
        m.apply(src_bounds.x1 as f32, src_bounds.y0 as f32),
        m.apply(src_bounds.x0 as f32, src_bounds.y1 as f32),
        m.apply(src_bounds.x1 as f32, src_bounds.y1 as f32),
    ];
    map_pixels(base, sel, src_bounds, &corners, method, |x, y| Some(inv.apply(x, y)))
}

/// Perspective / Distort / Skew: maps the source bounds' corners
/// (TL, TR, BR, BL) onto `dst` corners.
pub fn transform_pixels_quad(base: &TiledBuffer<Rgba8>, sel: Option<&Mask>, dst: [(f32, f32); 4], method: Resample) -> (TiledBuffer<Rgba8>, Option<Mask>) {
    let Some(b) = transform_source_bounds(base, sel) else { return (base.clone(), sel.cloned()) };
    let src = [(b.x0 as f32, b.y0 as f32), (b.x1 as f32, b.y0 as f32), (b.x1 as f32, b.y1 as f32), (b.x0 as f32, b.y1 as f32)];
    let Some(hinv) = Homography::from_quads(dst, src) else { return (base.clone(), sel.cloned()) };
    map_pixels(base, sel, b, &dst, method, |x, y| hinv.apply(x, y))
}

/// Shared implementation: lifts the selected pixels, then fills the
/// destination region by inverse-mapping each pixel with `inv`.
fn map_pixels(
    base: &TiledBuffer<Rgba8>,
    sel: Option<&Mask>,
    src_bounds: IRect,
    dst_corners: &[(f32, f32)],
    method: Resample,
    inv: impl Fn(f32, f32) -> Option<(f32, f32)> + Sync,
) -> (TiledBuffer<Rgba8>, Option<Mask>) {
    let (w, h) = (base.width(), base.height());
    let src_rect = src_bounds.expand(2);
    let mut img = FImage::from_buffer(base, src_rect);
    if let Some(s) = sel {
        img.data.par_chunks_mut(img.w).enumerate().for_each(|(y, row)| {
            for (x, p) in row.iter_mut().enumerate() {
                let c = coverage(Some(s), src_rect.x0 + x as i32, src_rect.y0 + y as i32);
                *p = p.map(|v| v * c);
            }
        });
    } else {
        // Without a selection only the layer content (src_bounds) moves.
        img.data.par_chunks_mut(img.w).enumerate().for_each(|(y, row)| {
            for (x, p) in row.iter_mut().enumerate() {
                if !src_bounds.contains(src_rect.x0 + x as i32, src_rect.y0 + y as i32) {
                    *p = [0.0; 4];
                }
            }
        });
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for &(x, y) in dst_corners {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    let dst = IRect::new(x0.floor() as i32 - 1, y0.floor() as i32 - 1, x1.ceil() as i32 + 1, y1.ceil() as i32 + 1).clamp_to(w, h);

    let mut out = base.clone();
    out.par_for_each_in_rect_mut(src_bounds, |x, y, p| {
        let s = if sel.is_some() { coverage(sel, x, y) } else { 1.0 };
        p[3] = (p[3] as f32 * (1.0 - s) + 0.5) as u8;
    });
    let sample = |sx: f32, sy: f32| -> [f32; 4] {
        let lx = sx - src_rect.x0 as f32;
        let ly = sy - src_rect.y0 as f32;
        if lx < -1.0 || ly < -1.0 || lx > img.w as f32 + 1.0 || ly > img.h as f32 + 1.0 {
            return [0.0; 4];
        }
        match method {
            Resample::Nearest => {
                let (ix, iy) = (lx.floor() as isize, ly.floor() as isize);
                if ix < 0 || iy < 0 || ix >= img.w as isize || iy >= img.h as isize {
                    [0.0; 4]
                } else {
                    img.data[iy as usize * img.w + ix as usize]
                }
            }
            _ => img.sample(lx, ly),
        }
    };
    out.par_for_each_in_rect_mut(dst, |x, y, p| {
        let Some((sx, sy)) = inv(x as f32 + 0.5, y as f32 + 0.5) else { return };
        let f = sample(sx, sy);
        if f[3] <= 0.0 {
            return;
        }
        let rgb = [f[0] / f[3], f[1] / f[3], f[2] / f[3]];
        *p = crate::brush::paint_over(*p, rgb, f[3], false);
    });
    out.optimize();
    let new_sel = sel.map(|s| {
        let mut ns = Mask::new(w, h);
        ns.par_for_each_in_rect_mut(dst, |x, y, v| {
            if let Some((sx, sy)) = inv(x as f32 + 0.5, y as f32 + 0.5) {
                *v = s.get(sx.floor() as i32, sy.floor() as i32);
            }
        });
        ns.optimize();
        ns
    });
    (out, new_sel)
}

/// Projective 3×3 transform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Homography(pub [f64; 9]);

impl Homography {
    /// Solves for the homography mapping `src[i]` → `dst[i]`.
    pub fn from_quads(src: [(f32, f32); 4], dst: [(f32, f32); 4]) -> Option<Homography> {
        let mut a = [[0.0f64; 9]; 8];
        for i in 0..4 {
            let (x, y) = (src[i].0 as f64, src[i].1 as f64);
            let (u, v) = (dst[i].0 as f64, dst[i].1 as f64);
            a[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
            a[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v];
        }
        // Gaussian elimination with partial pivoting.
        for col in 0..8 {
            let piv = (col..8).max_by(|&r1, &r2| a[r1][col].abs().total_cmp(&a[r2][col].abs()))?;
            if a[piv][col].abs() < 1e-12 {
                return None;
            }
            a.swap(col, piv);
            for r in 0..8 {
                if r != col {
                    let f = a[r][col] / a[col][col];
                    for c in col..9 {
                        a[r][c] -= f * a[col][c];
                    }
                }
            }
        }
        let mut h = [0.0f64; 9];
        for i in 0..8 {
            h[i] = a[i][8] / a[i][i];
        }
        h[8] = 1.0;
        Some(Homography(h))
    }

    pub fn apply(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let h = &self.0;
        let (x, y) = (x as f64, y as f64);
        let d = h[6] * x + h[7] * y + h[8];
        if d.abs() < 1e-12 {
            return None;
        }
        Some((((h[0] * x + h[1] * y + h[2]) / d) as f32, ((h[3] * x + h[4] * y + h[5]) / d) as f32))
    }
}

/// Edit > Transform > Warp: a 4×4 bicubic Bézier control grid (row-major,
/// 16 points) deforms the source bounds. Rendered by rasterising a fine
/// triangle mesh with per-triangle affine texture mapping.
pub fn warp_pixels(base: &TiledBuffer<Rgba8>, sel: Option<&Mask>, grid: &[(f32, f32); 16]) -> (TiledBuffer<Rgba8>, Option<Mask>) {
    let (w, h) = (base.width(), base.height());
    let Some(b) = transform_source_bounds(base, sel) else { return (base.clone(), sel.cloned()) };
    let bern = |t: f32| [(1.0 - t).powi(3), 3.0 * t * (1.0 - t).powi(2), 3.0 * t * t * (1.0 - t), t.powi(3)];
    let eval = |u: f32, v: f32| {
        let (bu, bv) = (bern(u), bern(v));
        let mut p = (0.0, 0.0);
        for j in 0..4 {
            for i in 0..4 {
                let wgt = bu[i] * bv[j];
                p.0 += grid[j * 4 + i].0 * wgt;
                p.1 += grid[j * 4 + i].1 * wgt;
            }
        }
        p
    };
    const N: usize = 32;
    let mut verts = vec![(0.0f32, 0.0f32); (N + 1) * (N + 1)];
    for j in 0..=N {
        for i in 0..=N {
            verts[j * (N + 1) + i] = eval(i as f32 / N as f32, j as f32 / N as f32);
        }
    }
    let src_at = |i: usize, j: usize| {
        (b.x0 as f32 + b.width() as f32 * i as f32 / N as f32, b.y0 as f32 + b.height() as f32 * j as f32 / N as f32)
    };
    // Inverse map: find the triangle containing the destination pixel.
    // Build a coarse bucket grid of triangles for fast lookup.
    let mut tris: Vec<[(f32, f32); 6]> = Vec::with_capacity(N * N * 2);
    for j in 0..N {
        for i in 0..N {
            let d00 = verts[j * (N + 1) + i];
            let d10 = verts[j * (N + 1) + i + 1];
            let d01 = verts[(j + 1) * (N + 1) + i];
            let d11 = verts[(j + 1) * (N + 1) + i + 1];
            let (s00, s10, s01, s11) = (src_at(i, j), src_at(i + 1, j), src_at(i, j + 1), src_at(i + 1, j + 1));
            tris.push([d00, d10, d11, s00, s10, s11]);
            tris.push([d00, d11, d01, s00, s11, s01]);
        }
    }
    let corners: Vec<(f32, f32)> = verts.clone();
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for &(x, y) in &corners {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    const G: usize = 64;
    let cell = ((x1 - x0).max(1.0) / G as f32, (y1 - y0).max(1.0) / G as f32);
    let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); G * G];
    for (ti, t) in tris.iter().enumerate() {
        let (mut a0, mut b0, mut a1, mut b1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in &t[..3] {
            a0 = a0.min(p.0);
            b0 = b0.min(p.1);
            a1 = a1.max(p.0);
            b1 = b1.max(p.1);
        }
        let gx0 = (((a0 - x0) / cell.0) as usize).min(G - 1);
        let gx1 = (((a1 - x0) / cell.0) as usize).min(G - 1);
        let gy0 = (((b0 - y0) / cell.1) as usize).min(G - 1);
        let gy1 = (((b1 - y0) / cell.1) as usize).min(G - 1);
        for gy in gy0..=gy1 {
            for gx in gx0..=gx1 {
                buckets[gy * G + gx].push(ti as u32);
            }
        }
    }
    let inv = |x: f32, y: f32| -> Option<(f32, f32)> {
        if x < x0 || y < y0 || x > x1 || y > y1 {
            return None;
        }
        let gx = (((x - x0) / cell.0) as usize).min(G - 1);
        let gy = (((y - y0) / cell.1) as usize).min(G - 1);
        for &ti in &buckets[gy * G + gx] {
            let t = &tris[ti as usize];
            let (a, bb, c) = (t[0], t[1], t[2]);
            let det = (bb.1 - c.1) * (a.0 - c.0) + (c.0 - bb.0) * (a.1 - c.1);
            if det.abs() < 1e-9 {
                continue;
            }
            let l1 = ((bb.1 - c.1) * (x - c.0) + (c.0 - bb.0) * (y - c.1)) / det;
            let l2 = ((c.1 - a.1) * (x - c.0) + (a.0 - c.0) * (y - c.1)) / det;
            let l3 = 1.0 - l1 - l2;
            if l1 >= -1e-4 && l2 >= -1e-4 && l3 >= -1e-4 {
                return Some((l1 * t[3].0 + l2 * t[4].0 + l3 * t[5].0, l1 * t[3].1 + l2 * t[4].1 + l3 * t[5].1));
            }
        }
        None
    };
    let _ = (w, h);
    map_pixels(base, sel, b, &corners, Resample::Bilinear, inv)
}

/// Perspective Crop: maps the quad (TL, TR, BR, BL) to a rectangle and
/// makes that the new canvas.
pub fn perspective_crop(state: &mut DocState, quad: [(f32, f32); 4]) {
    let d = |a: (f32, f32), b: (f32, f32)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
    let nw = ((d(quad[0], quad[1]) + d(quad[3], quad[2])) * 0.5).round().max(1.0) as u32;
    let nh = ((d(quad[0], quad[3]) + d(quad[1], quad[2])) * 0.5).round().max(1.0) as u32;
    let rect = [(0.0, 0.0), (nw as f32, 0.0), (nw as f32, nh as f32), (0.0, nh as f32)];
    let Some(hm) = Homography::from_quads(rect, quad) else { return };
    state.layers.par_iter_mut().for_each(|l| {
        let src = l.pixels.clone();
        l.pixels = TiledBuffer::from_fn_par(nw, nh, [0, 0, 0, 0], |x, y| match hm.apply(x as f32 + 0.5, y as f32 + 0.5) {
            Some((sx, sy)) => crate::brush::sample_bilinear(&src, sx, sy),
            None => [0, 0, 0, 0],
        });
        if let Some(m) = &l.mask {
            let m = m.clone();
            l.mask = Some(TiledBuffer::from_fn_par(nw, nh, 255, |x, y| match hm.apply(x as f32 + 0.5, y as f32 + 0.5) {
                Some((sx, sy)) => m.get(sx.floor() as i32, sy.floor() as i32),
                None => 255,
            }));
        }
    });
    state.selection = None;
    state.width = nw;
    state.height = nh;
}

/// Rotates a layer's pixels by `deg` around the canvas centre
/// (Ruler tool "Straighten Layer").
pub fn rotate_layer(buf: &TiledBuffer<Rgba8>, deg: f32) -> TiledBuffer<Rgba8> {
    let (cx, cy) = (buf.width() as f32 * 0.5, buf.height() as f32 * 0.5);
    transform_pixels(buf, None, Affine::scale_rotate(cx, cy, 1.0, 1.0, deg, 0.0, 0.0), Resample::Bilinear).0
}

/// The identity warp grid for `bounds` (4×4 points evenly spaced).
pub fn warp_identity_grid(b: IRect) -> [(f32, f32); 16] {
    std::array::from_fn(|k| {
        let (i, j) = (k % 4, k / 4);
        (b.x0 as f32 + b.width() as f32 * i as f32 / 3.0, b.y0 as f32 + b.height() as f32 * j as f32 / 3.0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;

    #[test]
    fn resample_solid_stays_solid() {
        let b = TiledBuffer::solid(100, 80, [10, 200, 30, 255]);
        for m in Resample::ALL {
            let r = resample_rgba(&b, 37, 151, m);
            assert_eq!((r.width(), r.height()), (37, 151));
            for (x, y) in [(0, 0), (36, 150), (18, 75)] {
                let p = r.get(x, y);
                for c in 0..4 {
                    assert!((p[c] as i32 - b.get(0, 0)[c] as i32).abs() <= 1, "{m:?} {p:?}");
                }
            }
        }
    }

    #[test]
    fn rotate_and_flip() {
        let mut b: TiledBuffer<u8> = TiledBuffer::new(3, 2);
        b.set(0, 0, 1);
        let r = reorient(&b, Orientation::Rotate90Cw);
        assert_eq!((r.width(), r.height()), (2, 3));
        assert_eq!(r.get(1, 0), 1);
        let f = reorient(&b, Orientation::FlipHorizontal);
        assert_eq!(f.get(2, 0), 1);
        let back = reorient(&r, Orientation::Rotate90Ccw);
        assert_eq!(back.to_dense(), b.to_dense());
    }

    #[test]
    fn crop_and_canvas() {
        let mut d = Document::new("t", 100, 100, None);
        d.state.layers[0].pixels.set(50, 50, [1, 2, 3, 255]);
        crop(&mut d.state, IRect::new(40, 40, 60, 70));
        assert_eq!((d.state.width, d.state.height), (20, 30));
        assert_eq!(d.state.layers[0].pixels.get(10, 10), [1, 2, 3, 255]);
        canvas_size(&mut d.state, 40, 30, (0.5, 0.5));
        assert_eq!(d.state.layers[0].pixels.get(20, 10), [1, 2, 3, 255]);
    }

    #[test]
    fn move_with_selection() {
        let mut b = TiledBuffer::new(50, 50);
        b.set(10, 10, [255, 0, 0, 255]);
        b.set(30, 30, [0, 255, 0, 255]);
        let sel = crate::selection::rect_mask(50, 50, IRect::new(5, 5, 15, 15));
        let (out, ns) = move_pixels(&b, Some(&sel), 5, 0);
        assert_eq!(out.get(10, 10)[3], 0);
        assert_eq!(out.get(15, 10), [255, 0, 0, 255]);
        assert_eq!(out.get(30, 30), [0, 255, 0, 255]);
        assert_eq!(ns.unwrap().content_bounds(), Some(IRect::new(10, 5, 20, 15)));
    }

    #[test]
    fn homography_and_warp() {
        let src = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let dst = [(2.0, 1.0), (20.0, 3.0), (18.0, 15.0), (1.0, 12.0)];
        let hm = Homography::from_quads(src, dst).unwrap();
        for i in 0..4 {
            let (x, y) = hm.apply(src[i].0, src[i].1).unwrap();
            assert!((x - dst[i].0).abs() < 1e-3 && (y - dst[i].1).abs() < 1e-3);
        }
        let mut b = TiledBuffer::new(60, 60);
        b.par_for_each_in_rect_mut(IRect::new(10, 10, 30, 30), |_, _, p| *p = [255, 0, 0, 255]);
        // Identity warp keeps pixels in place.
        let grid = warp_identity_grid(IRect::new(10, 10, 30, 30));
        let (out, _) = warp_pixels(&b, None, &grid);
        assert_eq!(out.get(20, 20), [255, 0, 0, 255]);
        assert_eq!(out.get(40, 40)[3], 0);
        // Perspective onto a bigger quad.
        let (out, _) = transform_pixels_quad(&b, None, [(5.0, 5.0), (50.0, 5.0), (50.0, 50.0), (5.0, 50.0)], Resample::Bilinear);
        assert_eq!(out.get(45, 45), [255, 0, 0, 255]);
    }

    #[test]
    fn affine_inverse() {
        let m = Affine::scale_rotate(10.0, 20.0, 2.0, 0.5, 30.0, 3.0, -4.0);
        let inv = m.inverse().unwrap();
        let (x, y) = m.apply(7.0, 9.0);
        let (bx, by) = inv.apply(x, y);
        assert!((bx - 7.0).abs() < 1e-3 && (by - 9.0).abs() < 1e-3);
        let b = TiledBuffer::solid(40, 40, [9, 9, 9, 255]);
        let (out, _) = transform_pixels(&b, None, Affine::scale_rotate(20.0, 20.0, 0.5, 0.5, 0.0, 0.0, 0.0), Resample::Bilinear);
        assert_eq!(out.get(20, 20), [9, 9, 9, 255]);
        assert_eq!(out.get(2, 2)[3], 0);
    }
}
