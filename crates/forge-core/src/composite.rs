//! Layer compositing (the "render" of a document).
//!
//! Work is split by tile and run on all cores. Sparse tiles (absent in a
//! layer) are skipped entirely, so empty areas of big documents cost nothing.

use rayon::prelude::*;

use std::sync::Arc;

use crate::blend::{composite, BlendMode};
use crate::buffer::{TiledBuffer, TILE_SIZE};
use crate::color::Rgba8;
use crate::document::DocState;
use crate::geom::IRect;
use crate::layer::{Layer, LayerId, LayerMask};
use crate::styles::RenderedStyle;

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

/// One thing to blend: a layer, a rendered group, or a layer-style effect.
type AdjustFn = dyn Fn(Rgba8) -> Rgba8 + Send + Sync;

struct Item<'a> {
    pixels: &'a TiledBuffer<Rgba8>,
    /// Adjustment layers recolour everything below instead of adding pixels.
    adjust: Option<&'a AdjustFn>,
    mask: Option<&'a LayerMask>,
    opacity: f32,
    mode: BlendMode,
    /// Multiplied by the alpha of the clipping base below.
    clipped: bool,
    /// Effects never act as a clipping base.
    effect: bool,
}

/// Owned buffers that items borrow from (group renders, style caches).
enum Owned {
    Buffer(TiledBuffer<Rgba8>),
    Style(Arc<RenderedStyle>),
    Adjust(Box<AdjustFn>),
}

static EMPTY: std::sync::LazyLock<TiledBuffer<Rgba8>> = std::sync::LazyLock::new(|| TiledBuffer::new(1, 1));

fn is_top_level(l: &Layer, layers: &[&Layer], parent: Option<LayerId>) -> bool {
    match parent {
        Some(p) => l.parent == Some(p),
        // Orphans (parent not in this stack) count as top level.
        None => l.parent.is_none_or(|p| !layers.iter().any(|o| o.id == p && o.is_group())),
    }
}

/// Composites `layers` (bottom first) for `rect` and returns a dense,
/// row-major RGBA buffer of `rect.width() * rect.height()` pixels.
/// Groups (`parent` links), clipping masks, fill opacity and layer styles
/// are honoured.
pub fn composite_layers(layers: &[&Layer], w: u32, h: u32, rect: IRect, bg: Background) -> Vec<Rgba8> {
    composite_level(layers, None, w, h, rect, bg)
}

fn composite_level(layers: &[&Layer], parent: Option<LayerId>, w: u32, h: u32, rect: IRect, bg: Background) -> Vec<Rgba8> {
    let rect = rect.clamp_to(w, h);
    // First pass: produce owned buffers for groups and styles.
    let level: Vec<&Layer> = layers
        .iter()
        .copied()
        .filter(|l| is_top_level(l, layers, parent) && l.visible && l.opacity > 0.0)
        .collect();
    let mut owned: Vec<(usize, Owned)> = Vec::new();
    for (i, l) in level.iter().enumerate() {
        if l.is_group() {
            let px = composite_level(layers, Some(l.id), w, h, rect, Background::Transparent);
            let mut buf = TiledBuffer::new(w, h);
            if !rect.is_empty() {
                buf.write_dense(rect, &px);
                buf.optimize();
            }
            owned.push((i, Owned::Buffer(buf)));
        }
        if let Some(st) = crate::styles::rendered(l) {
            owned.push((i, Owned::Style(st)));
        }
        if let Some(adj) = l.adjustment.as_ref().filter(|_| l.is_adjustment()) {
            owned.push((i, Owned::Adjust(adj.compile())));
        }
    }
    let mut items: Vec<Item> = Vec::new();
    for (i, l) in level.iter().enumerate() {
        let style = owned.iter().find_map(|(j, o)| match o {
            Owned::Style(s) if *j == i => Some(s),
            _ => None,
        });
        if let Some(st) = style {
            for e in &st.below {
                items.push(Item { pixels: &e.pixels, adjust: None, mask: None, opacity: e.opacity * l.opacity, mode: e.mode, clipped: false, effect: true });
            }
        }
        if l.is_adjustment() {
            let f = owned.iter().find_map(|(j, o)| match o {
                Owned::Adjust(f) if *j == i => Some(f.as_ref()),
                _ => None,
            });
            if let Some(f) = f {
                items.push(Item {
                    pixels: &EMPTY,
                    adjust: Some(f),
                    mask: l.active_mask(),
                    opacity: l.opacity,
                    mode: l.blend,
                    clipped: l.clipped,
                    effect: true,
                });
            }
            continue;
        }
        let pixels = if l.is_group() {
            owned.iter().find_map(|(j, o)| match o {
                Owned::Buffer(b) if *j == i => Some(b),
                _ => None,
            })
        } else {
            Some(&l.pixels)
        };
        if let Some(pixels) = pixels {
            items.push(Item {
                pixels,
                adjust: None,
                mask: l.active_mask(),
                opacity: l.opacity * l.fill,
                mode: l.blend,
                clipped: l.clipped,
                effect: false,
            });
        }
        if let Some(st) = style {
            for e in &st.above {
                items.push(Item { pixels: &e.pixels, adjust: None, mask: None, opacity: e.opacity * l.opacity, mode: e.mode, clipped: l.clipped, effect: true });
            }
        }
    }
    composite_items(&items, rect, bg)
}

fn composite_items(items: &[Item], rect: IRect, bg: Background) -> Vec<Rgba8> {
    let rw = rect.width() as usize;
    let mut out = vec![[0u8; 4]; rw * rect.height() as usize];
    if out.is_empty() {
        return out;
    }
    let lut = &*U8_TO_F32;
    let any_clip = items.iter().any(|i| i.clipped);

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
        let mut clip: Vec<f32> = Vec::new();
        let mut x = rect.x0;
        while x < rect.x1 {
            let tx = x as u32 / TILE_SIZE;
            let tile_x1 = (((tx + 1) * TILE_SIZE) as i32).min(rect.x1);
            let tw = (tile_x1 - x) as usize;
            acc.clear();
            acc.resize(tw * band_h, [0.0; 4]);
            if any_clip {
                clip.clear();
                clip.resize(tw * band_h, 0.0);
            }
            let base_x = (tx * TILE_SIZE) as i32;
            let base_y = (ty * TILE_SIZE) as i32;

            for item in items {
                if let Some(f) = item.adjust {
                    let mask_tile = item.mask.and_then(|m| m.tile(tx, ty));
                    let mask_fill = item.mask.map_or(255, |m| m.fill_value());
                    for ly in 0..band_h {
                        let py = band_y0 + ly as i32;
                        let trow = ((py - base_y) as u32 * TILE_SIZE) as usize;
                        for lx in 0..tw {
                            let ti = trow + (x + lx as i32 - base_x) as usize;
                            let ai = ly * tw + lx;
                            let m = mask_tile.map_or(mask_fill, |mt| mt[ti]);
                            let mut a = lut[m as usize] * item.opacity;
                            if item.clipped {
                                a *= clip[ai];
                            }
                            let c = &mut acc[ai];
                            if a <= 0.0 || c[3] <= 0.0 {
                                continue;
                            }
                            let inv = 1.0 / c[3];
                            let o = f([q(c[0] * inv), q(c[1] * inv), q(c[2] * inv), 255]);
                            for k in 0..3 {
                                let target = lut[o[k] as usize] * c[3];
                                c[k] += (target - c[k]) * a;
                            }
                        }
                    }
                    continue;
                }
                let tile = item.pixels.tile(tx, ty);
                let is_base = any_clip && !item.clipped && !item.effect;
                if tile.is_none() {
                    if is_base {
                        clip.iter_mut().for_each(|c| *c = 0.0);
                    }
                    continue;
                }
                let tile = tile.unwrap();
                let mask_tile = item.mask.and_then(|m| m.tile(tx, ty));
                let mask_fill = item.mask.map_or(255, |m| m.fill_value());
                if mask_tile.is_none() && mask_fill == 0 {
                    if is_base {
                        clip.iter_mut().for_each(|c| *c = 0.0);
                    }
                    continue;
                }
                let opacity = item.opacity;
                let mode = item.mode;
                for ly in 0..band_h {
                    let py = band_y0 + ly as i32;
                    let trow = ((py - base_y) as u32 * TILE_SIZE) as usize;
                    for lx in 0..tw {
                        let px = x + lx as i32;
                        let ti = trow + (px - base_x) as usize;
                        let p = tile[ti];
                        let m = match mask_tile {
                            Some(mt) => mt[ti],
                            None => mask_fill,
                        };
                        let mut a = lut[p[3] as usize] * lut[m as usize];
                        let ai = ly * tw + lx;
                        if is_base {
                            clip[ai] = a;
                        } else if item.clipped {
                            a *= clip[ai];
                        }
                        if a <= 0.0 {
                            continue;
                        }
                        composite(
                            &mut acc[ai],
                            [lut[p[0] as usize], lut[p[1] as usize], lut[p[2] as usize]],
                            a * opacity,
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
    fn groups_and_clipping() {
        let mut bg = Layer::new(1, "bg", 10, 10);
        bg.pixels = TiledBuffer::solid(10, 10, [255, 255, 255, 255]);
        // Base shape covers only the left half; the clipped layer is red everywhere.
        let mut base = Layer::new(2, "base", 10, 10);
        for y in 0..10 {
            for x in 0..5 {
                base.pixels.set(x, y, [0, 0, 255, 255]);
            }
        }
        let mut clipped = Layer::new(3, "clip", 10, 10);
        clipped.pixels = TiledBuffer::solid(10, 10, [255, 0, 0, 255]);
        clipped.clipped = true;
        let out = composite_layers(&[&bg, &base, &clipped], 10, 10, IRect::from_size(10, 10), Background::Transparent);
        assert_eq!(out[2], [255, 0, 0, 255]);
        assert_eq!(out[7], [255, 255, 255, 255], "clipped layer hidden outside base");

        // A group at 50% opacity containing an opaque black layer.
        let mut g = Layer::new_group(4, "group", 10, 10);
        g.opacity = 0.5;
        let mut child = Layer::new(5, "child", 10, 10);
        child.pixels = TiledBuffer::solid(10, 10, [0, 0, 0, 255]);
        child.parent = Some(4);
        let out = composite_layers(&[&bg, &child, &g], 10, 10, IRect::from_size(10, 10), Background::Transparent);
        assert_eq!(out[0], [128, 128, 128, 255]);
        g.visible = false;
        let out = composite_layers(&[&bg, &child, &g], 10, 10, IRect::from_size(10, 10), Background::Transparent);
        assert_eq!(out[0], [255, 255, 255, 255], "hidden group hides children");
    }

    #[test]
    fn adjustment_layer() {
        let mut bg = Layer::new(1, "bg", 10, 10);
        bg.pixels = TiledBuffer::solid(10, 10, [200, 100, 50, 255]);
        let mut adj = Layer::new_adjustment(2, crate::adjust::Adjustment::Invert, 10, 10);
        adj.mask.as_mut().unwrap().set(1, 0, 0);
        let out = composite_layers(&[&bg, &adj], 10, 10, IRect::new(0, 0, 2, 1), Background::Transparent);
        assert_eq!(out[0], [55, 155, 205, 255]);
        assert_eq!(out[1], [200, 100, 50, 255], "masked out");
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
