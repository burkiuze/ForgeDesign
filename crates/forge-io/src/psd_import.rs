//! Layered Photoshop (.psd) import.

use forge_core::buffer::TiledBuffer;
use forge_core::document::DocState;
use forge_core::layer::Layer;
use forge_core::{BlendMode, Rgba8};

use crate::{IoError, Result};

fn to_buffer(rgba: &[u8], w: u32, h: u32) -> TiledBuffer<Rgba8> {
    let px: Vec<Rgba8> = rgba.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
    TiledBuffer::from_dense(w, h, &px)
}

/// Maps the psd crate's blend mode (by variant name, which matches ours).
fn map_blend(debug_name: &str) -> BlendMode {
    BlendMode::ALL.into_iter().find(|m| format!("{m:?}") == debug_name).unwrap_or_default()
}

pub fn read_psd(bytes: &[u8]) -> Result<DocState> {
    let psd = psd::Psd::from_bytes(bytes).map_err(|e| IoError::Format(format!("PSD: {e}")))?;
    let (w, h) = (psd.width(), psd.height());
    let mut state = DocState::new(w, h);
    // Groups: allocate our ids first so layers can reference them.
    let mut gmap = std::collections::HashMap::new();
    for gid in psd.group_ids_in_order() {
        gmap.insert(*gid, state.alloc_id());
    }
    // psd lists layers bottom-most first, like we do.
    for l in psd.layers() {
        let rgba = l.rgba();
        if rgba.len() != (w * h * 4) as usize {
            continue;
        }
        let id = state.alloc_id();
        let mut layer = Layer::with_pixels(id, l.name(), to_buffer(&rgba, w, h));
        layer.visible = l.visible();
        layer.opacity = l.opacity() as f32 / 255.0;
        layer.blend = map_blend(&format!("{:?}", l.blend_mode()));
        layer.clipped = l.is_clipping_mask();
        layer.parent = l.parent_id().and_then(|p| gmap.get(&p).copied());
        layer.pixels.optimize();
        state.layers.push(layer);
    }
    // Insert group layers above their contents, innermost first.
    let groups = psd.groups();
    let depth = |mut id: u32| {
        let mut d = 0;
        while let Some(p) = groups.get(&id).and_then(|g| g.parent_id()) {
            id = p;
            d += 1;
            if d > 64 {
                break;
            }
        }
        d
    };
    let mut order: Vec<u32> = psd.group_ids_in_order().clone();
    order.sort_by_key(|g| std::cmp::Reverse(depth(*g)));
    for gid in order {
        let Some(g) = groups.get(&gid) else { continue };
        let our = gmap[&gid];
        let mut gl = Layer::new_group(our, g.name(), w, h);
        gl.visible = g.visible();
        gl.opacity = g.opacity() as f32 / 255.0;
        let bm = format!("{:?}", g.blend_mode());
        gl.blend = if bm == "PassThrough" { BlendMode::Normal } else { map_blend(&bm) };
        gl.parent = g.parent_id().and_then(|p| gmap.get(&p).copied());
        let at = state.descendants(our).into_iter().max().map_or(state.layers.len(), |i| i + 1);
        state.layers.insert(at, gl);
    }
    if state.layers.is_empty() {
        // No layer section (e.g. flattened PSD): use the composite image.
        let id = state.alloc_id();
        state.layers.push(Layer::with_pixels(id, "Background", to_buffer(&psd.rgba(), w, h)));
    }
    state.active = state.layers.len() - 1;
    Ok(state)
}
