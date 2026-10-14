//! Layered Photoshop (.psd) writer: 8-bit RGB, RLE (PackBits) compressed,
//! with layer names (incl. Unicode), opacity, visibility, blend modes,
//! clipping masks, layer masks and groups.

use forge_core::composite::{self, Background};
use forge_core::document::DocState;
use forge_core::layer::Layer;
use forge_core::{BlendMode, IRect};

use crate::Result;

fn blend_key(m: BlendMode) -> &'static [u8; 4] {
    use BlendMode::*;
    match m {
        Normal => b"norm",
        Dissolve => b"diss",
        Darken => b"dark",
        Multiply => b"mul ",
        ColorBurn => b"idiv",
        LinearBurn => b"lbrn",
        DarkerColor => b"dkCl",
        Lighten => b"lite",
        Screen => b"scrn",
        ColorDodge => b"div ",
        LinearDodge => b"lddg",
        LighterColor => b"lgCl",
        Overlay => b"over",
        SoftLight => b"sLit",
        HardLight => b"hLit",
        VividLight => b"vLit",
        LinearLight => b"lLit",
        PinLight => b"pLit",
        HardMix => b"hMix",
        Difference => b"diff",
        Exclusion => b"smud",
        Subtract => b"fsub",
        Divide => b"fdiv",
        Hue => b"hue ",
        Saturation => b"sat ",
        Color => b"colr",
        Luminosity => b"lum ",
    }
}

/// PackBits compression of one row.
fn packbits(row: &[u8], out: &mut Vec<u8>) {
    let n = row.len();
    let mut i = 0;
    while i < n {
        // Run of identical bytes?
        let mut run = 1;
        while i + run < n && run < 128 && row[i + run] == row[i] {
            run += 1;
        }
        if run >= 2 {
            out.push((257 - run) as u8);
            out.push(row[i]);
            i += run;
            continue;
        }
        // Literal run until the next repeat of 2+.
        let start = i;
        let mut len = 0;
        while i < n && len < 128 {
            if i + 1 < n && row[i] == row[i + 1] {
                break;
            }
            i += 1;
            len += 1;
        }
        out.push((len - 1) as u8);
        out.extend_from_slice(&row[start..start + len]);
    }
}

/// RLE-compresses planar channel data (`rows` rows of `width` bytes):
/// returns (row byte counts, packed data).
fn rle_channel(data: &[u8], width: usize, rows: usize) -> (Vec<u16>, Vec<u8>) {
    let mut counts = Vec::with_capacity(rows);
    let mut packed = Vec::new();
    for r in 0..rows {
        let before = packed.len();
        packbits(&data[r * width..(r + 1) * width], &mut packed);
        counts.push((packed.len() - before) as u16);
    }
    (counts, packed)
}

struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn i16(&mut self, v: i16) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
}

/// One entry in the PSD layer list (groups expand to divider + folder).
struct Entry<'a> {
    layer: Option<&'a Layer>,
    name: String,
    /// 0 = normal layer, 1/2 = open/closed folder, 3 = bounding divider.
    section: u32,
    opacity: f32,
    visible: bool,
    blend: BlendMode,
    clipped: bool,
}

fn order<'a>(state: &'a DocState, parent: Option<u64>, out: &mut Vec<Entry<'a>>) {
    for l in state.layers.iter().filter(|l| l.parent == parent) {
        if l.is_group() {
            out.push(Entry { layer: None, name: "</Layer group>".into(), section: 3, opacity: 1.0, visible: true, blend: BlendMode::Normal, clipped: false });
            order(state, Some(l.id), out);
            out.push(Entry {
                layer: None,
                name: l.name.clone(),
                section: if l.collapsed { 2 } else { 1 },
                opacity: l.opacity,
                visible: l.visible,
                blend: l.blend,
                clipped: false,
            });
        } else {
            out.push(Entry { layer: Some(l), name: l.name.clone(), section: 0, opacity: l.opacity * l.fill, visible: l.visible, blend: l.blend, clipped: l.clipped });
        }
    }
}

pub fn write_psd(state: &DocState) -> Result<Vec<u8>> {
    let (w, h) = (state.width, state.height);
    let mut f = W(Vec::new());
    f.bytes(b"8BPS");
    f.u16(1);
    f.bytes(&[0; 6]);
    f.u16(4);
    f.u32(h);
    f.u32(w);
    f.u16(8);
    f.u16(3);
    f.u32(0); // colour mode data
    f.u32(0); // image resources

    let mut entries = Vec::new();
    order(state, None, &mut entries);
    // Orphans whose parent group is missing.
    for l in &state.layers {
        if let Some(p) = l.parent {
            if !state.layers.iter().any(|o| o.id == p) && !l.is_group() {
                entries.push(Entry { layer: Some(l), name: l.name.clone(), section: 0, opacity: l.opacity, visible: l.visible, blend: l.blend, clipped: l.clipped });
            }
        }
    }

    // ---- layer records + channel data
    let mut records = W(Vec::new());
    let mut chan_data = W(Vec::new());
    records.i16(-(entries.len() as i16)); // negative: first alpha is merged transparency
    for e in &entries {
        let bounds = e.layer.and_then(|l| l.pixels.content_bounds()).unwrap_or(IRect::new(0, 0, 0, 0));
        let (bw, bh) = (bounds.width() as usize, bounds.height() as usize);
        let mask = e.layer.and_then(|l| l.mask.as_ref());
        records.i32(bounds.y0);
        records.i32(bounds.x0);
        records.i32(bounds.y1);
        records.i32(bounds.x1);
        let nch: u16 = if mask.is_some() { 5 } else { 4 };
        records.u16(nch);
        // Channel data (planar, RLE).
        let px = e.layer.map(|l| l.pixels.region_to_dense(bounds)).unwrap_or_default();
        let mut channel_blobs: Vec<(i16, Vec<u8>)> = Vec::new();
        for (id, idx) in [(-1i16, 3usize), (0, 0), (1, 1), (2, 2)] {
            let plane: Vec<u8> = px.iter().map(|p| p[idx]).collect();
            let mut blob = W(Vec::new());
            if bw == 0 || bh == 0 {
                blob.u16(0);
            } else {
                blob.u16(1);
                let (counts, packed) = rle_channel(&plane, bw, bh);
                for c in counts {
                    blob.u16(c);
                }
                blob.bytes(&packed);
            }
            channel_blobs.push((id, blob.0));
        }
        if let Some(m) = mask {
            let plane = m.to_dense();
            let mut blob = W(Vec::new());
            blob.u16(1);
            let (counts, packed) = rle_channel(&plane, w as usize, h as usize);
            for c in counts {
                blob.u16(c);
            }
            blob.bytes(&packed);
            channel_blobs.push((-2, blob.0));
        }
        for (id, blob) in &channel_blobs {
            records.i16(*id);
            records.u32(blob.len() as u32);
        }
        records.bytes(b"8BIM");
        let key = if e.section == 1 || e.section == 2 {
            if e.blend == BlendMode::Normal {
                b"pass"
            } else {
                blend_key(e.blend)
            }
        } else {
            blend_key(e.blend)
        };
        records.bytes(key);
        records.u8((e.opacity.clamp(0.0, 1.0) * 255.0).round() as u8);
        records.u8(e.clipped as u8);
        records.u8(if e.visible { 0 } else { 0x02 });
        records.u8(0);
        // Extra data.
        let mut extra = W(Vec::new());
        if mask.is_some() {
            extra.u32(20);
            extra.i32(0);
            extra.i32(0);
            extra.i32(h as i32);
            extra.i32(w as i32);
            extra.u8(255); // default colour
            let enabled = e.layer.is_some_and(|l| l.mask_enabled);
            extra.u8(if enabled { 0 } else { 0x02 });
            extra.u16(0);
        } else {
            extra.u32(0);
        }
        extra.u32(0); // blending ranges
        // Pascal name padded to 4.
        let ascii: Vec<u8> = e.name.bytes().map(|b| if b.is_ascii() { b } else { b'?' }).take(255).collect();
        extra.u8(ascii.len() as u8);
        extra.bytes(&ascii);
        let mut pad = (ascii.len() + 1) % 4;
        while pad != 0 && pad < 4 {
            extra.u8(0);
            pad += 1;
        }
        // Unicode name.
        let utf16: Vec<u16> = e.name.encode_utf16().collect();
        let mut luni = W(Vec::new());
        luni.u32(utf16.len() as u32);
        for c in &utf16 {
            luni.u16(*c);
        }
        if luni.0.len() % 4 != 0 {
            luni.bytes(&vec![0; 4 - luni.0.len() % 4]);
        }
        extra.bytes(b"8BIMluni");
        extra.u32(luni.0.len() as u32);
        extra.bytes(&luni.0);
        if e.section != 0 {
            extra.bytes(b"8BIMlsct");
            extra.u32(4);
            extra.u32(e.section);
        }
        records.u32(extra.0.len() as u32);
        records.bytes(&extra.0);
        for (_, blob) in channel_blobs {
            chan_data.bytes(&blob);
        }
    }
    let mut layer_info = records.0;
    layer_info.extend_from_slice(&chan_data.0);
    if layer_info.len() % 2 != 0 {
        layer_info.push(0);
    }
    f.u32((4 + layer_info.len() + 4) as u32);
    f.u32(layer_info.len() as u32);
    f.bytes(&layer_info);
    f.u32(0); // global layer mask info

    // ---- merged composite image (planar RGBA, RLE)
    let merged = composite::composite_rect(state, state.bounds(), Background::Transparent);
    f.u16(1);
    let mut all_counts = Vec::new();
    let mut all_data = Vec::new();
    for c in 0..4 {
        let plane: Vec<u8> = merged.iter().map(|p| p[c]).collect();
        let (counts, packed) = rle_channel(&plane, w as usize, h as usize);
        all_counts.extend(counts);
        all_data.extend(packed);
    }
    for c in all_counts {
        f.u16(c);
    }
    f.bytes(&all_data);
    Ok(f.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packbits_roundtrip() {
        let row: Vec<u8> = vec![1, 1, 1, 1, 2, 3, 4, 4, 5, 6, 6, 6];
        let mut out = Vec::new();
        packbits(&row, &mut out);
        // Decode.
        let mut dec = Vec::new();
        let mut i = 0;
        while i < out.len() {
            let n = out[i] as i8;
            i += 1;
            if n >= 0 {
                dec.extend_from_slice(&out[i..i + n as usize + 1]);
                i += n as usize + 1;
            } else {
                dec.extend(std::iter::repeat_n(out[i], (1 - n as i32) as usize));
                i += 1;
            }
        }
        assert_eq!(dec, row);
    }
}
