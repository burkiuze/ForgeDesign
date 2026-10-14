//! Layer Styles (non-destructive effects): Drop Shadow, Inner Shadow,
//! Outer Glow, Inner Glow, Bevel & Emboss, Satin, Color/Gradient/Pattern
//! Overlay and Stroke.
//!
//! Effects are rendered from the layer's alpha into separate RGBA buffers
//! that the compositor blends below (shadow, outer glow) or above (the rest)
//! the layer. Rendering is cached per layer content + style, so it only
//! re-runs when either changes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::blend::BlendMode;
use crate::buffer::TiledBuffer;
use crate::color::{Color, Rgba8};
use crate::fill::GradientKind;
use crate::filters::gaussian_blur;
use crate::geom::{IRect, Point};
use crate::layer::{Layer, LayerId};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shadow {
    pub enabled: bool,
    pub color: Color,
    pub mode: BlendMode,
    pub opacity: f32,
    /// Light angle in degrees (Photoshop: 120°).
    pub angle: f32,
    pub distance: f32,
    /// Choke (inner) / spread (drop), 0..1.
    pub spread: f32,
    pub size: f32,
}

impl Shadow {
    pub fn drop() -> Self {
        Self { enabled: true, color: Color::BLACK, mode: BlendMode::Multiply, opacity: 0.75, angle: 120.0, distance: 5.0, spread: 0.0, size: 5.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Glow {
    pub enabled: bool,
    pub color: Color,
    pub mode: BlendMode,
    pub opacity: f32,
    pub spread: f32,
    pub size: f32,
}

impl Glow {
    pub fn outer() -> Self {
        Self { enabled: true, color: Color::rgb(1.0, 1.0, 0.75), mode: BlendMode::Screen, opacity: 0.75, spread: 0.0, size: 10.0 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StrokePosition {
    #[default]
    Outside,
    Inside,
    Center,
}

impl StrokePosition {
    pub const ALL: [StrokePosition; 3] = [StrokePosition::Outside, StrokePosition::Inside, StrokePosition::Center];
    pub fn name(self) -> &'static str {
        match self {
            StrokePosition::Outside => "Outside",
            StrokePosition::Inside => "Inside",
            StrokePosition::Center => "Center",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrokeStyle {
    pub enabled: bool,
    pub size: f32,
    pub position: StrokePosition,
    pub color: Color,
    pub mode: BlendMode,
    pub opacity: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Overlay {
    pub enabled: bool,
    pub color: Color,
    pub mode: BlendMode,
    pub opacity: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradientOverlay {
    pub enabled: bool,
    pub from: Color,
    pub to: Color,
    pub kind: GradientKind,
    pub angle: f32,
    pub reverse: bool,
    pub mode: BlendMode,
    pub opacity: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PatternOverlay {
    pub enabled: bool,
    /// Checker cell size of the built-in pattern.
    pub scale: f32,
    pub color_a: Color,
    pub color_b: Color,
    pub mode: BlendMode,
    pub opacity: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bevel {
    pub enabled: bool,
    pub size: f32,
    /// Depth multiplier.
    pub depth: f32,
    pub angle: f32,
    pub altitude: f32,
    pub soften: f32,
    pub highlight: Color,
    pub highlight_opacity: f32,
    pub shadow: Color,
    pub shadow_opacity: f32,
    /// Raised (true) or sunken.
    pub up: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Satin {
    pub enabled: bool,
    pub color: Color,
    pub mode: BlendMode,
    pub opacity: f32,
    pub angle: f32,
    pub distance: f32,
    pub size: f32,
    pub invert: bool,
}

/// All effects of one layer. `None` = effect never added.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LayerStyle {
    pub enabled: bool,
    pub drop_shadow: Option<Shadow>,
    pub inner_shadow: Option<Shadow>,
    pub outer_glow: Option<Glow>,
    pub inner_glow: Option<Glow>,
    pub bevel: Option<Bevel>,
    pub satin: Option<Satin>,
    pub color_overlay: Option<Overlay>,
    pub gradient_overlay: Option<GradientOverlay>,
    pub pattern_overlay: Option<PatternOverlay>,
    pub stroke: Option<StrokeStyle>,
}

impl LayerStyle {
    pub fn is_active(&self) -> bool {
        self.enabled
            && (self.drop_shadow.as_ref().is_some_and(|e| e.enabled)
                || self.inner_shadow.as_ref().is_some_and(|e| e.enabled)
                || self.outer_glow.as_ref().is_some_and(|e| e.enabled)
                || self.inner_glow.as_ref().is_some_and(|e| e.enabled)
                || self.bevel.as_ref().is_some_and(|e| e.enabled)
                || self.satin.as_ref().is_some_and(|e| e.enabled)
                || self.color_overlay.as_ref().is_some_and(|e| e.enabled)
                || self.gradient_overlay.as_ref().is_some_and(|e| e.enabled)
                || self.pattern_overlay.as_ref().is_some_and(|e| e.enabled)
                || self.stroke.as_ref().is_some_and(|e| e.enabled))
    }

    /// How far effects can reach outside the layer's pixels.
    fn reach(&self) -> f32 {
        let mut r: f32 = 0.0;
        if let Some(s) = self.drop_shadow.as_ref().filter(|e| e.enabled) {
            r = r.max(s.distance + s.size * 2.0);
        }
        if let Some(g) = self.outer_glow.as_ref().filter(|e| e.enabled) {
            r = r.max(g.size * 2.0);
        }
        if let Some(s) = self.stroke.as_ref().filter(|e| e.enabled) {
            r = r.max(s.size + 1.0);
        }
        r
    }
}

/// One rendered effect, ready to composite.
pub struct EffectLayer {
    pub pixels: TiledBuffer<Rgba8>,
    pub mode: BlendMode,
    pub opacity: f32,
}

pub struct RenderedStyle {
    pub below: Vec<EffectLayer>,
    pub above: Vec<EffectLayer>,
}

// --------------------------------------------------------------- distance

/// 1D squared Euclidean distance transform (Felzenszwalb & Huttenlocher).
fn edt_1d(f: &[f64], out: &mut [f32]) {
    let n = f.len();
    let mut v = vec![0usize; n];
    let mut z = vec![0f64; n + 1];
    let mut k = 0usize;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in 1..n {
        let qf = q as f64;
        let mut s;
        loop {
            let p = v[k] as f64;
            s = ((f[q] + qf * qf) - (f[v[k]] + p * p)) / (2.0 * qf - 2.0 * p);
            if s <= z[k] {
                // z[0] is -inf, so this never underflows.
                k -= 1;
            } else {
                break;
            }
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    k = 0;
    for (q, o) in out.iter_mut().enumerate() {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let d = q as f64 - v[k] as f64;
        *o = (d * d + f[v[k]]) as f32;
    }
}

/// Euclidean distance from every pixel to the nearest `inside` pixel.
pub fn distance_field(inside: &[bool], w: usize, h: usize) -> Vec<f32> {
    let inf = ((w + h) * (w + h)) as f64 * 4.0 + 1.0;
    // Columns.
    let mut cols = vec![0.0f32; w * h];
    cols.par_chunks_mut(h).enumerate().for_each(|(x, col)| {
        let f: Vec<f64> = (0..h).map(|y| if inside[y * w + x] { 0.0 } else { inf }).collect();
        edt_1d(&f, col);
    });
    // Rows.
    let mut d = vec![0.0f32; w * h];
    d.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let f: Vec<f64> = (0..w).map(|x| cols[x * h + y] as f64).collect();
        edt_1d(&f, row);
    });
    d.par_iter_mut().for_each(|v| *v = v.sqrt());
    d
}

// ---------------------------------------------------------------- render

fn shift(a: &[f32], w: usize, h: usize, dx: i32, dy: i32) -> Vec<f32> {
    let mut out = vec![0.0; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let sy = y as i32 - dy;
        if sy < 0 || sy >= h as i32 {
            return;
        }
        for (x, o) in row.iter_mut().enumerate() {
            let sx = x as i32 - dx;
            if sx >= 0 && sx < w as i32 {
                *o = a[sy as usize * w + sx as usize];
            }
        }
    });
    out
}

fn blur(a: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let mut d: Vec<[f32; 1]> = a.iter().map(|&v| [v]).collect();
    gaussian_blur(&mut d, w, h, sigma.max(0.01));
    d.into_iter().map(|[v]| v).collect()
}

/// Morphological spread/choke via the distance field.
fn grow_alpha(a: &[f32], w: usize, h: usize, amount: f32) -> Vec<f32> {
    if amount <= 0.0 {
        return a.to_vec();
    }
    let inside: Vec<bool> = a.iter().map(|&v| v >= 0.5).collect();
    let d = distance_field(&inside, w, h);
    a.iter().zip(&d).map(|(&v, &dist)| v.max((amount - dist + 0.5).clamp(0.0, 1.0))).collect()
}

fn effect(mask: &[f32], rect: IRect, full: (u32, u32), color: Color, mode: BlendMode, opacity: f32) -> EffectLayer {
    let c = color.to_rgba8();
    let mut pixels = TiledBuffer::new(full.0, full.1);
    let w = rect.width() as usize;
    pixels.par_for_each_in_rect_mut(rect, |x, y, p| {
        let m = mask[(y - rect.y0) as usize * w + (x - rect.x0) as usize];
        if m > 0.0 {
            *p = [c[0], c[1], c[2], (m.clamp(0.0, 1.0) * 255.0 * color.a + 0.5) as u8];
        }
    });
    pixels.optimize();
    EffectLayer { pixels, mode, opacity }
}

fn dir(angle: f32, distance: f32) -> (i32, i32) {
    // Photoshop angles point *towards* the light, shadows fall away from it.
    let a = angle.to_radians();
    ((-a.cos() * distance).round() as i32, (a.sin() * distance).round() as i32)
}

/// Renders all enabled effects of `layer`.
pub fn render(layer: &Layer, style: &LayerStyle) -> RenderedStyle {
    let (fw, fh) = (layer.width(), layer.height());
    let mut out = RenderedStyle { below: Vec::new(), above: Vec::new() };
    let Some(content) = layer.pixels.content_bounds() else { return out };
    let pad = style.reach().ceil() as i32 + 4;
    let rect = content.expand(pad).clamp_to(fw, fh);
    let (w, h) = (rect.width() as usize, rect.height() as usize);
    let mask_buf = layer.active_mask();
    let alpha: Vec<f32> = layer
        .pixels
        .region_to_dense(rect)
        .par_iter()
        .enumerate()
        .map(|(i, p)| {
            let m = mask_buf.map_or(1.0, |m| m.get(rect.x0 + (i % w) as i32, rect.y0 + (i / w) as i32) as f32 / 255.0);
            p[3] as f32 / 255.0 * m
        })
        .collect();
    let full = (fw, fh);

    if let Some(s) = style.drop_shadow.as_ref().filter(|e| e.enabled) {
        let (dx, dy) = dir(s.angle, s.distance);
        let spread_px = s.spread * s.size;
        let a = grow_alpha(&alpha, w, h, spread_px);
        let a = blur(&shift(&a, w, h, dx, dy), w, h, (s.size - spread_px).max(0.0) * 0.5);
        out.below.push(effect(&a, rect, full, s.color, s.mode, s.opacity));
    }
    if let Some(g) = style.outer_glow.as_ref().filter(|e| e.enabled) {
        let spread_px = g.spread * g.size;
        let a = grow_alpha(&alpha, w, h, spread_px);
        let a = blur(&a, w, h, (g.size - spread_px).max(0.0) * 0.5);
        out.below.push(effect(&a, rect, full, g.color, g.mode, g.opacity));
    }
    // "Above" effects are clipped to the layer's own alpha.
    if let Some(s) = style.satin.as_ref().filter(|e| e.enabled) {
        let (dx, dy) = dir(s.angle, s.distance);
        let a1 = blur(&shift(&alpha, w, h, dx, dy), w, h, s.size * 0.5);
        let a2 = blur(&shift(&alpha, w, h, -dx, -dy), w, h, s.size * 0.5);
        let m: Vec<f32> = (0..w * h)
            .map(|i| {
                let v = (a1[i] - a2[i]).abs();
                (if s.invert { 1.0 - v } else { v }) * alpha[i]
            })
            .collect();
        out.above.push(effect(&m, rect, full, s.color, s.mode, s.opacity));
    }
    if let Some(o) = style.color_overlay.as_ref().filter(|e| e.enabled) {
        out.above.push(effect(&alpha, rect, full, o.color, o.mode, o.opacity));
    }
    if let Some(g) = style.gradient_overlay.as_ref().filter(|e| e.enabled) {
        let c = content;
        let ctr = Point::new((c.x0 + c.x1) as f32 * 0.5, (c.y0 + c.y1) as f32 * 0.5);
        let half = (c.width().max(c.height()) as f32) * 0.5;
        let a = g.angle.to_radians();
        let (ux, uy) = (a.cos(), -a.sin());
        let p0 = Point::new(ctr.x - ux * half, ctr.y - uy * half);
        let p1 = Point::new(ctr.x + ux * half, ctr.y + uy * half);
        let (c0, c1) = if g.reverse { (g.to, g.from) } else { (g.from, g.to) };
        let mut pixels = TiledBuffer::new(fw, fh);
        pixels.par_for_each_in_rect_mut(rect, |x, y, p| {
            let m = alpha[(y - rect.y0) as usize * w + (x - rect.x0) as usize];
            if m > 0.0 {
                let t = g.kind.t_at(if g.kind == GradientKind::Radial { ctr } else { p0 }, p1, Point::new(x as f32 + 0.5, y as f32 + 0.5));
                let col = c0.lerp(c1, t);
                let c8 = col.to_rgba8();
                *p = [c8[0], c8[1], c8[2], (m * col.a * 255.0 + 0.5) as u8];
            }
        });
        pixels.optimize();
        out.above.push(EffectLayer { pixels, mode: g.mode, opacity: g.opacity });
    }
    if let Some(pt) = style.pattern_overlay.as_ref().filter(|e| e.enabled) {
        let cell = pt.scale.max(1.0);
        let (ca, cb) = (pt.color_a.to_rgba8(), pt.color_b.to_rgba8());
        let mut pixels = TiledBuffer::new(fw, fh);
        pixels.par_for_each_in_rect_mut(rect, |x, y, p| {
            let m = alpha[(y - rect.y0) as usize * w + (x - rect.x0) as usize];
            if m > 0.0 {
                let even = ((x as f32 / cell).floor() as i32 + (y as f32 / cell).floor() as i32) % 2 == 0;
                let c = if even { ca } else { cb };
                *p = [c[0], c[1], c[2], (m * 255.0 + 0.5) as u8];
            }
        });
        pixels.optimize();
        out.above.push(EffectLayer { pixels, mode: pt.mode, opacity: pt.opacity });
    }
    if let Some(g) = style.inner_glow.as_ref().filter(|e| e.enabled) {
        let inv: Vec<f32> = alpha.iter().map(|v| 1.0 - v).collect();
        let a = blur(&grow_alpha(&inv, w, h, g.spread * g.size), w, h, g.size * 0.5);
        let m: Vec<f32> = a.iter().zip(&alpha).map(|(v, al)| (v * al).clamp(0.0, 1.0)).collect();
        out.above.push(effect(&m, rect, full, g.color, g.mode, g.opacity));
    }
    if let Some(s) = style.inner_shadow.as_ref().filter(|e| e.enabled) {
        let (dx, dy) = dir(s.angle, s.distance);
        let inv: Vec<f32> = alpha.iter().map(|v| 1.0 - v).collect();
        // Outside the computed region counts as "outside the shape".
        let shifted = shift_fill(&inv, w, h, dx, dy, 1.0);
        let a = blur(&grow_alpha(&shifted, w, h, s.spread * s.size), w, h, s.size * 0.5);
        let m: Vec<f32> = a.iter().zip(&alpha).map(|(v, al)| (v * al).clamp(0.0, 1.0)).collect();
        out.above.push(effect(&m, rect, full, s.color, s.mode, s.opacity));
    }
    if let Some(b) = style.bevel.as_ref().filter(|e| e.enabled) {
        let height = blur(&alpha, w, h, b.size * 0.5);
        let (la, alt) = (b.angle.to_radians(), b.altitude.to_radians());
        let light = [la.cos() * alt.cos(), -la.sin() * alt.cos(), alt.sin()];
        let depth = b.depth * b.size.max(1.0) * if b.up { 1.0 } else { -1.0 };
        let mut hi = vec![0.0f32; w * h];
        let mut sh = vec![0.0f32; w * h];
        hi.par_chunks_mut(w).zip(sh.par_chunks_mut(w)).enumerate().for_each(|(y, (hrow, srow))| {
            for x in 0..w {
                let at = |xx: isize, yy: isize| height[(yy.clamp(0, h as isize - 1) as usize) * w + xx.clamp(0, w as isize - 1) as usize];
                let gx = (at(x as isize + 1, y as isize) - at(x as isize - 1, y as isize)) * 0.5 * depth;
                let gy = (at(x as isize, y as isize + 1) - at(x as isize, y as isize - 1)) * 0.5 * depth;
                let n = [-gx, -gy, 1.0];
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                let d = (n[0] * light[0] + n[1] * light[1] + n[2] * light[2]) / len - light[2];
                let a = alpha[y * w + x];
                if d > 0.0 {
                    hrow[x] = (d * 3.0).min(1.0) * a;
                } else {
                    srow[x] = (-d * 3.0).min(1.0) * a;
                }
            }
        });
        if b.soften > 0.0 {
            hi = blur(&hi, w, h, b.soften);
            sh = blur(&sh, w, h, b.soften);
        }
        out.above.push(effect(&hi, rect, full, b.highlight, BlendMode::Screen, b.highlight_opacity));
        out.above.push(effect(&sh, rect, full, b.shadow, BlendMode::Multiply, b.shadow_opacity));
    }
    if let Some(s) = style.stroke.as_ref().filter(|e| e.enabled) {
        let inside: Vec<bool> = alpha.iter().map(|&v| v >= 0.5).collect();
        let outside: Vec<bool> = inside.iter().map(|b| !b).collect();
        let d_out = distance_field(&inside, w, h);
        let d_in = distance_field(&outside, w, h);
        let m: Vec<f32> = (0..w * h)
            .map(|i| {
                let (o, n) = (d_out[i], d_in[i]);
                match s.position {
                    StrokePosition::Outside => {
                        if inside[i] {
                            0.0
                        } else {
                            (s.size - o + 0.5).clamp(0.0, 1.0) * (1.0 - alpha[i]) + 0.0
                        }
                    }
                    StrokePosition::Inside => {
                        if inside[i] {
                            (s.size - n + 0.5).clamp(0.0, 1.0) * alpha[i]
                        } else {
                            0.0
                        }
                    }
                    StrokePosition::Center => {
                        let half = s.size * 0.5;
                        if inside[i] {
                            (half - n + 0.5).clamp(0.0, 1.0)
                        } else {
                            (half - o + 0.5).clamp(0.0, 1.0)
                        }
                    }
                }
            })
            .collect();
        out.above.push(effect(&m, rect, full, s.color, s.mode, s.opacity));
    }
    out
}

fn shift_fill(a: &[f32], w: usize, h: usize, dx: i32, dy: i32, fill: f32) -> Vec<f32> {
    let mut out = vec![fill; w * h];
    for y in 0..h as i32 {
        let sy = y - dy;
        if sy < 0 || sy >= h as i32 {
            continue;
        }
        for x in 0..w as i32 {
            let sx = x - dx;
            if sx >= 0 && sx < w as i32 {
                out[y as usize * w + x as usize] = a[sy as usize * w + sx as usize];
            }
        }
    }
    out
}

type CacheKey = LayerId;
type CacheVal = (usize, LayerStyle, (u32, u32), Arc<RenderedStyle>);

static CACHE: std::sync::LazyLock<Mutex<HashMap<CacheKey, CacheVal>>> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Cached rendering of a layer's style (re-renders only when the pixels,
/// mask or style changed).
pub fn rendered(layer: &Layer) -> Option<Arc<RenderedStyle>> {
    let style = layer.style.as_ref().filter(|s| s.is_active())?;
    let fp = layer.pixels.fingerprint() ^ layer.active_mask().map_or(0, |m| m.fingerprint().rotate_left(7));
    let dims = (layer.width(), layer.height());
    if let Ok(cache) = CACHE.lock() {
        if let Some((f, s, d, r)) = cache.get(&layer.id) {
            if *f == fp && s == style && *d == dims {
                return Some(r.clone());
            }
        }
    }
    let r = Arc::new(render(layer, style));
    if let Ok(mut cache) = CACHE.lock() {
        if cache.len() > 256 {
            cache.clear();
        }
        cache.insert(layer.id, (fp, style.clone(), dims, r.clone()));
    }
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_field_basic() {
        let w = 7;
        let mut inside = vec![false; w * w];
        inside[3 * w + 3] = true;
        let d = distance_field(&inside, w, w);
        assert_eq!(d[3 * w + 3], 0.0);
        assert!((d[3 * w + 6] - 3.0).abs() < 1e-4);
        assert!((d[0] - (18.0f32).sqrt()).abs() < 1e-4);
    }

    #[test]
    fn stroke_and_shadow() {
        let mut l = Layer::new(1, "l", 64, 64);
        for y in 20..40 {
            for x in 20..40 {
                l.pixels.set(x, y, [255, 0, 0, 255]);
            }
        }
        let style = LayerStyle {
            enabled: true,
            stroke: Some(StrokeStyle { enabled: true, size: 3.0, position: StrokePosition::Outside, color: Color::BLACK, mode: BlendMode::Normal, opacity: 1.0 }),
            drop_shadow: Some(Shadow::drop()),
            ..Default::default()
        };
        let r = render(&l, &style);
        assert_eq!(r.below.len(), 1);
        assert_eq!(r.above.len(), 1);
        let stroke = &r.above[0].pixels;
        assert_eq!(stroke.get(18, 30)[3], 255, "outside stroke");
        assert_eq!(stroke.get(30, 30)[3], 0, "not inside");
        assert_eq!(stroke.get(10, 30)[3], 0, "not too far");
        let shadow = &r.below[0].pixels;
        assert!(shadow.get(42, 42)[3] > 0, "shadow falls to the lower right");
    }
}
