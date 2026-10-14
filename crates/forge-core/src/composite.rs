//! Layer compositing (the "render" of a document).
//!
//! Work is split by tile and run on all cores. Sparse tiles (absent in a
//! layer) are skipped entirely, so empty areas of big documents cost nothing.

use rayon::prelude::*;

use crate::blend::composite;
use crate::buffer::{TiledBuffer, TILE_SIZE};
use crate::color::Rgba8;
use crate::document::DocState;
use crate::geom::IRect;
use crate::layer::Layer;

/// How transparent areas are rendered.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Background {
    /// Keep transparency (straight alpha output).
    Transparent,
    /// Photoshop-style grey/white checkerboard (opaque output, for display).
    Checker,
    /// Flatten onto a solid colour (opaque output, e.g. JPEG export).
    Solid(Rgba8),
}

static U8_TO_F32: std::sync::LazyLock<[f32; 256]> =
    std::sync::LazyLock::new(|| std::array::from_fn(|i| i as f32 / 255.0));

#[inline]
fn checker(x: i32, y: i32) -> f32 {
    if ((x >> 3) + (y >> 3)) & 1 == 0 {
        1.0
    } else {
        0.8
    }
}

/// Composites `layers` (bottom first) for `rect` and returns a dense,
/// row-major RGBA buffer of `rect.width() * rect.height()` pixels.
pub fn composite_layers(layers: &[&Layer], w: u32, h: u32, rect: IRect, bg: Background) -> Vec<Rgba8> {
    let rect = rect.clamp_to(w, h);
    let rw = rect.width() as usize;
    let mut out = vec![[0u8; 4]; rw * rect.height() as usize];
    if out.is_empty() {
        return out;
    }
    let visible: Vec<&Layer> = layers.iter().copied().filter(|l| l.visible && l.opacity > 0.0).collect();
    let lut = &*U8_TO_F32;

    // Split the output into horizontal bands aligned to tile rows.
    let mut bands: Vec<(i32, &mut [Rgba8])> = Vec::new();
    let mut rest: &mut [Rgba8] = &mut out;
    let mut y = rect.y0;
    while y < rect.y1 {
        let band_end = (((y as u32 / TILE_SIZE) + 1) * TILE_SIZE).min(rect.y1 as u32) as i32;
        let (band, tail) = rest.split_at_mut((band_end - y) as usize * rw);
        bands.push((y, band));
        rest = tail;
        y = band_end;
    }

    bands.into_par_iter().for_each(|(band_y0, band)| {
        let band_h = band.len() / rw;
        let ty = band_y0 as u32 / TILE_SIZE;
        let mut acc: Vec<[f32; 4]> = Vec::with_capacity((TILE_SIZE * TILE_SIZE) as usize);
        let mut x = rect.x0;
        while x < rect.x1 {
            let tx = x as u32 / TILE_SIZE;
            let tile_x1 = (((tx + 1) * TILE_SIZE) as i32).min(rect.x1);
            let tw = (tile_x1 - x) as usize;
            acc.clear();
            acc.resize(tw * band_h, [0.0; 4]);
            let base_x = (tx * TILE_SIZE) as i32;
            let base_y = (ty * TILE_SIZE) as i32;

            for layer in &visible {
                let Some(tile) = layer.pixels.tile(tx, ty) else { continue };
                let mask = layer.active_mask();
                let mask_tile = mask.and_then(|m| m.tile(tx, ty));
                let mask_fill = mask.map_or(255, |m| m.fill_value());
                if mask_tile.is_none() && mask_fill == 0 {
                    continue;
                }
                let opacity = layer.opacity;
                let mode = layer.blend;
                for ly in 0..band_h {
                    let py = band_y0 + ly as i32;
                    let trow = ((py - base_y) as u32 * TILE_SIZE) as usize;
                    for lx in 0..tw {
                        let px = x + lx as i32;
                        let ti = trow + (px - base_x) as usize;
                        let p = tile[ti];
                        if p[3] == 0 {
                            continue;
                        }
                        let m = match mask_tile {
                            Some(mt) => mt[ti],
                            None => mask_fill,
                        };
                        if m == 0 {
                            continue;
                        }
                        let a = lut[p[3] as usize] * lut[m as usize] * opacity;
                        composite(
                            &mut acc[ly * tw + lx],
                            [lut[p[0] as usize], lut[p[1] as usize], lut[p[2] as usize]],
                            a,
                            mode,
                            px,
                            py,
                        );
                    }
                }
            }

            for ly in 0..band_h {
                let py = band_y0 + ly as i32;
                let dst = &mut band[ly * rw + (x - rect.x0) as usize..][..tw];
                for (lx, d) in dst.iter_mut().enumerate() {
                    let c = acc[ly * tw + lx];
                    *d = match bg {
                        Background::Transparent => {
                            if c[3] <= 0.0 {
                                [0, 0, 0, 0]
                            } else {
                                let inv = 1.0 / c[3];
                                [q(c[0] * inv), q(c[1] * inv), q(c[2] * inv), q(c[3])]
                            }
                        }
                        Background::Checker => {
                            let b = checker(x + lx as i32, py) * (1.0 - c[3]);
                            [q(c[0] + b), q(c[1] + b), q(c[2] + b), 255]
                        }
                        Background::Solid(s) => {
                            let ia = 1.0 - c[3];
                            [
                                q(c[0] + lut[s[0] as usize] * ia),
                                q(c[1] + lut[s[1] as usize] * ia),
                                q(c[2] + lut[s[2] as usize] * ia),
                                255,
                            ]
                        }
                    };
                }
            }
            x = tile_x1;
        }
    });
    out
}

#[inline]
fn q(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Composites the visible document for `rect`.
pub fn composite_rect(state: &DocState, rect: IRect, bg: Background) -> Vec<Rgba8> {
    let layers: Vec<&Layer> = state.layers.iter().collect();
    composite_layers(&layers, state.width, state.height, rect, bg)
}

/// Flattens layers into a single buffer (used by merge/flatten/export).
pub fn flatten_layers(layers: &[&Layer], w: u32, h: u32) -> TiledBuffer<Rgba8> {
    let full = IRect::from_size(w, h);
    TiledBuffer::from_dense(w, h, &composite_layers(layers, w, h, full, Background::Transparent))
}

/// The merged image of the whole document ("Merged" in Photoshop terms).
pub fn merged(state: &DocState) -> TiledBuffer<Rgba8> {
    let layers: Vec<&Layer> = state.layers.iter().collect();
    flatten_layers(&layers, state.width, state.height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::BlendMode;

    #[test]
    fn two_layers_multiply() {
        let mut bottom = Layer::new(1, "bg", 70, 70);
        bottom.pixels = TiledBuffer::solid(70, 70, [200, 100, 50, 255]);
        let mut top = Layer::new(2, "top", 70, 70);
        top.pixels.set(65, 65, [128, 255, 0, 255]);
        top.blend = BlendMode::Multiply;
        let out = composite_layers(&[&bottom, &top], 70, 70, IRect::from_size(70, 70), Background::Transparent);
        assert_eq!(out[0], [200, 100, 50, 255]);
        assert_eq!(out[65 * 70 + 65], [100, 100, 0, 255]);
    }

    #[test]
    fn masks_and_opacity() {
        let mut l = Layer::new(1, "l", 10, 10);
        l.pixels = TiledBuffer::solid(10, 10, [255, 0, 0, 255]);
        l.opacity = 0.5;
        l.add_mask();
        l.mask.as_mut().unwrap().set(1, 0, 0);
        let out = composite_layers(&[&l], 10, 10, IRect::new(0, 0, 2, 1), Background::Transparent);
        assert_eq!(out[0], [255, 0, 0, 128]);
        assert_eq!(out[1], [0, 0, 0, 0]);
        let out = composite_layers(&[&l], 10, 10, IRect::new(0, 0, 1, 1), Background::Solid([255, 255, 255, 255]));
        assert_eq!(out[0], [255, 128, 128, 255]);
    }
}
