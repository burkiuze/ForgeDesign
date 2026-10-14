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

/// Free Transform of the selected pixels (or whole layer).
pub fn transform_pixels(base: &TiledBuffer<Rgba8>, sel: Option<&Mask>, m: Affine, method: Resample) -> (TiledBuffer<Rgba8>, Option<Mask>) {
    let (w, h) = (base.width(), base.height());
    let Some(inv) = m.inverse() else { return (base.clone(), sel.cloned()) };
    let src_bounds = match sel {
        Some(s) => s.content_bounds().unwrap_or_default(),
        None => base.content_bounds().unwrap_or_default(),
    };
    if src_bounds.is_empty() {
        return (base.clone(), sel.cloned());
    }
    // Floating content (premultiplied, selection applied).
    let pad = 2;
    let src_rect = src_bounds.expand(pad);
    let mut img = FImage::from_buffer(base, src_rect);
    if let Some(s) = sel {
        img.data.par_chunks_mut(img.w).enumerate().for_each(|(y, row)| {
            for (x, p) in row.iter_mut().enumerate() {
                let c = coverage(Some(s), src_rect.x0 + x as i32, src_rect.y0 + y as i32);
                *p = p.map(|v| v * c);
            }
        });
    }
    // Destination bounding box.
    let corners = [
        m.apply(src_bounds.x0 as f32, src_bounds.y0 as f32),
        m.apply(src_bounds.x1 as f32, src_bounds.y0 as f32),
        m.apply(src_bounds.x0 as f32, src_bounds.y1 as f32),
        m.apply(src_bounds.x1 as f32, src_bounds.y1 as f32),
    ];
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (x, y) in corners {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    let dst = IRect::new(x0.floor() as i32 - 1, y0.floor() as i32 - 1, x1.ceil() as i32 + 1, y1.ceil() as i32 + 1).clamp_to(w, h);

    // Base with the selection removed (the "hole").
    let mut out = base.clone();
    out.par_for_each_in_rect_mut(src_bounds, |x, y, p| {
        let s = coverage(sel, x, y);
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
        let (sx, sy) = inv.apply(x as f32 + 0.5, y as f32 + 0.5);
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
            let (sx, sy) = inv.apply(x as f32 + 0.5, y as f32 + 0.5);
            *v = s.get(sx.floor() as i32, sy.floor() as i32);
        });
        ns.optimize();
        ns
    });
    (out, new_sel)
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
