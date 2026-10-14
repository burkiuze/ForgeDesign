//! Content-aware repair: Spot Healing Brush, Remove Tool and
//! Edit > Content-Aware Fill.
//!
//! The algorithm is a fast two-stage approach:
//! 1. **Structure** — a coarse-to-fine membrane (Laplace) fill that smoothly
//!    propagates surrounding colours into the hole.
//! 2. **Texture** — high-frequency detail is copied from the best-matching
//!    nearby source region (searched in several directions and scored by
//!    how well its border matches), so the result is not a smooth blur.

use rayon::prelude::*;

use crate::buffer::TiledBuffer;
use crate::color::Rgba8;
use crate::filters::{box_blur, FImage};
use crate::geom::IRect;
use crate::selection::Mask;

type Px = [f32; 4];

/// Fills masked pixels in `img` (dense, premultiplied) where `known[i] == false`.
fn membrane(img: &mut [Px], known: &[bool], w: usize, h: usize) {
    if w <= 2 || h <= 2 || known.iter().all(|&k| k) {
        return;
    }
    // Build a pyramid of known-weighted averages.
    if w > 16 && h > 16 {
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mut coarse = vec![[0.0f32; 4]; cw * ch];
        let mut ck = vec![false; cw * ch];
        for y in 0..ch {
            for x in 0..cw {
                let mut acc = [0.0; 4];
                let mut n = 0.0;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (sx, sy) = (2 * x + dx, 2 * y + dy);
                    if sx < w && sy < h && known[sy * w + sx] {
                        let p = img[sy * w + sx];
                        for k in 0..4 {
                            acc[k] += p[k];
                        }
                        n += 1.0;
                    }
                }
                if n > 0.0 {
                    coarse[y * cw + x] = acc.map(|v| v / n);
                    ck[y * cw + x] = true;
                }
            }
        }
        membrane(&mut coarse, &ck, cw, ch);
        for y in 0..h {
            for x in 0..w {
                if !known[y * w + x] {
                    img[y * w + x] = coarse[(y / 2) * cw + x / 2];
                }
            }
        }
    } else {
        // Coarsest level: start from the mean of known pixels.
        let mut acc = [0.0; 4];
        let mut n = 0.0;
        for (p, &k) in img.iter().zip(known) {
            if k {
                for c in 0..4 {
                    acc[c] += p[c];
                }
                n += 1.0;
            }
        }
        let mean = if n > 0.0 { acc.map(|v| v / n) } else { acc };
        for (p, &k) in img.iter_mut().zip(known) {
            if !k {
                *p = mean;
            }
        }
    }
    // Relax (Gauss-Seidel) at this level.
    let unknown: Vec<usize> = (0..w * h).filter(|&i| !known[i]).collect();
    for _ in 0..24 {
        for &i in &unknown {
            let (x, y) = (i % w, i / w);
            let mut acc = [0.0; 4];
            let mut n = 0.0;
            for (nx, ny) in [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)] {
                if nx < w && ny < h {
                    let p = img[ny * w + nx];
                    for k in 0..4 {
                        acc[k] += p[k];
                    }
                    n += 1.0;
                }
            }
            img[i] = acc.map(|v| v / n);
        }
    }
}

/// Repairs the pixels covered by `mask` (255 = fully replace) using the
/// surroundings. Returns the modified rectangle.
pub fn content_aware_fill(target: &mut TiledBuffer<Rgba8>, base: &TiledBuffer<Rgba8>, mask: &Mask) -> Option<IRect> {
    let (fw, fh) = (base.width(), base.height());
    let hole = mask.content_bounds()?;
    let size = hole.width().max(hole.height());
    let pad = (size / 2 + 12).max(16);
    let rect = hole.expand(pad).clamp_to(fw, fh);
    let (w, h) = (rect.width() as usize, rect.height() as usize);
    let src = FImage::from_buffer(base, rect);
    let m: Vec<f32> = mask.region_to_dense(rect).iter().map(|&v| v as f32 / 255.0).collect();
    let known: Vec<bool> = m.iter().map(|&v| v < 0.5).collect();

    // 1. Structure.
    let mut smooth = src.data.clone();
    membrane(&mut smooth, &known, w, h);

    // 2. Texture: pick the best source offset by comparing a ring around the hole.
    let mut detail = src.data.clone();
    let mut low = src.data.clone();
    box_blur(&mut low, w, h, 3);
    for (d, l) in detail.iter_mut().zip(&low) {
        for k in 0..4 {
            d[k] -= l[k];
        }
    }
    let (hx0, hy0) = ((hole.x0 - rect.x0) as i32, (hole.y0 - rect.y0) as i32);
    let (hw, hh) = (hole.width(), hole.height());
    let mut candidates = Vec::new();
    for s in [1.0f32, 1.5, 2.0] {
        let (ox, oy) = ((hw as f32 * s) as i32 + 2, (hh as f32 * s) as i32 + 2);
        candidates.extend([(ox, 0), (-ox, 0), (0, oy), (0, -oy), (ox, oy), (-ox, -oy), (ox, -oy), (-ox, oy)]);
    }
    let in_bounds = |dx: i32, dy: i32| {
        hx0 + dx >= 0 && hy0 + dy >= 0 && hx0 + hw + dx <= w as i32 && hy0 + hh + dy <= h as i32
    };
    let score = |dx: i32, dy: i32| -> f32 {
        // Source window must avoid the hole itself.
        let mut err = 0.0;
        let mut n = 0.0;
        for y in (hy0.max(0)..(hy0 + hh).min(h as i32)).step_by(2) {
            for x in (hx0.max(0)..(hx0 + hw).min(w as i32)).step_by(2) {
                let i = y as usize * w + x as usize;
                let j = (y + dy) as usize * w + (x + dx) as usize;
                if !known[j] {
                    return f32::MAX;
                }
                let (a, b) = (smooth[i], low[j]);
                err += (0..3).map(|k| (a[k] - b[k]).abs()).sum::<f32>();
                n += 1.0;
            }
        }
        if n > 0.0 {
            err / n
        } else {
            f32::MAX
        }
    };
    let best = candidates
        .into_par_iter()
        .filter(|&(dx, dy)| in_bounds(dx, dy))
        .map(|(dx, dy)| (score(dx, dy), dx, dy))
        .min_by(|a, b| a.0.total_cmp(&b.0));

    let result: Vec<Px> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            if known[i] && m[i] <= 0.0 {
                return src.data[i];
            }
            let mut p = smooth[i];
            if let Some((s, dx, dy)) = best {
                if s < f32::MAX {
                    let (x, y) = ((i % w) as i32 + dx, (i / w) as i32 + dy);
                    if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
                        let d = detail[y as usize * w + x as usize];
                        for k in 0..4 {
                            p[k] += d[k];
                        }
                    }
                }
            }
            p[3] = p[3].clamp(0.0, 1.0);
            for k in 0..3 {
                p[k] = p[k].clamp(0.0, p[3]);
            }
            // Blend by mask coverage for soft edges.
            let a = m[i];
            let o = src.data[i];
            [o[0] + (p[0] - o[0]) * a, o[1] + (p[1] - o[1]) * a, o[2] + (p[2] - o[2]) * a, o[3] + (p[3] - o[3]) * a]
        })
        .collect();
    let img = FImage { w, h, data: result };
    crate::filters::write_back(target, base, None, hole, rect, &img);
    Some(hole)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_hole_with_surroundings() {
        let mut base = TiledBuffer::solid(80, 80, [40, 120, 200, 255]);
        // A dark spot to remove.
        for y in 35..45 {
            for x in 35..45 {
                base.set(x, y, [0, 0, 0, 255]);
            }
        }
        let mask = crate::selection::rect_mask(80, 80, IRect::new(33, 33, 47, 47));
        let mut t = base.clone();
        content_aware_fill(&mut t, &base, &mask).unwrap();
        let p = t.get(40, 40);
        for (a, b) in p.iter().zip([40u8, 120, 200, 255]) {
            assert!((*a as i32 - b as i32).abs() <= 3, "{p:?}");
        }
    }
}
