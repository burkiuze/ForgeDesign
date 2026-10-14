//! Text tool rasterisation (ab_glyph) and system font discovery.

use std::path::PathBuf;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use forge_core::buffer::TiledBuffer;
use forge_core::layer::TextData;

#[derive(Clone)]
pub struct FontEntry {
    pub name: String,
    pub path: Option<PathBuf>,
}

/// Bundled font (from egui) plus `.ttf`/`.otf` files found in the usual
/// system font directories.
pub fn discover_fonts() -> Vec<FontEntry> {
    let mut out = vec![FontEntry { name: "Ubuntu Light (built-in)".into(), path: None }];
    let mut dirs: Vec<PathBuf> = vec![
        "/usr/share/fonts".into(),
        "/usr/local/share/fonts".into(),
        "/System/Library/Fonts".into(),
        "/Library/Fonts".into(),
        "C:\\Windows\\Fonts".into(),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".fonts"));
        dirs.push(home.join(".local/share/fonts"));
        dirs.push(home.join("Library/Fonts"));
    }
    let mut found = Vec::new();
    for d in dirs {
        walk(&d, 0, &mut found);
    }
    found.sort_by_key(|a| a.name.to_lowercase());
    found.dedup_by(|a, b| a.name == b.name);
    out.extend(found);
    out
}

fn walk(dir: &std::path::Path, depth: u32, out: &mut Vec<FontEntry>) {
    if depth > 4 || out.len() > 2000 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, depth + 1, out);
        } else if matches!(
            p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
            Some("ttf" | "otf")
        ) {
            let name = p.file_stem().and_then(|s| s.to_str()).unwrap_or("?").to_string();
            out.push(FontEntry { name, path: Some(p) });
        }
    }
}

pub fn load_font(entry: &FontEntry) -> Option<FontVec> {
    let bytes = match &entry.path {
        Some(p) => std::fs::read(p).ok()?,
        None => {
            let defs = eframe::egui::FontDefinitions::default();
            defs.font_data.get("Ubuntu-Light")?.font.to_vec()
        }
    };
    FontVec::try_from_vec(bytes).ok()
}

fn line_width<F: Font>(sf: &ab_glyph::PxScaleFont<&F>, line: &str, tracking: f32) -> f32 {
    let mut w = 0.0;
    let mut prev = None;
    for ch in line.chars() {
        let id = sf.glyph_id(ch);
        if let Some(p) = prev {
            w += sf.kern(p, id);
        }
        w += sf.h_advance(id) + tracking;
        prev = Some(id);
    }
    w
}

/// Splits text into lines, word-wrapping to `box_width` when set.
fn layout<F: Font>(sf: &ab_glyph::PxScaleFont<&F>, data: &TextData, tracking: f32) -> Vec<String> {
    let mut out = Vec::new();
    for para in data.text.split('\n') {
        match data.box_width {
            Some(bw) if bw > 10.0 => {
                let mut line = String::new();
                for word in para.split(' ') {
                    let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
                    if !line.is_empty() && line_width(sf, &candidate, tracking) > bw {
                        out.push(std::mem::take(&mut line));
                        line = word.to_string();
                    } else {
                        line = candidate;
                    }
                }
                out.push(line);
            }
            _ => out.push(para.to_string()),
        }
    }
    out
}

/// Rasterises an editable type layer.
pub fn render_text_data(font: &FontVec, data: &TextData, w: u32, h: u32) -> TiledBuffer<[u8; 4]> {
    let scale = PxScale::from(data.size.max(1.0));
    let sf = font.as_scaled(scale);
    let tracking = data.tracking / 1000.0 * data.size;
    let mut cov: TiledBuffer<f32> = TiledBuffer::new(w, h);
    let line_h = (sf.ascent() - sf.descent() + sf.line_gap()) * data.leading.max(0.1) / 1.2;
    let bold = if data.bold { (data.size / 28.0).max(1.0) } else { 0.0 };
    let shear = if data.italic { 0.22 } else { 0.0 };
    let lines = layout(&sf, data, tracking);
    let block_w = data.box_width.unwrap_or_else(|| lines.iter().map(|l| line_width(&sf, l, tracking)).fold(0.0, f32::max));
    let mut plot = |px: i32, py: i32, c: f32| {
        let v = cov.get(px, py);
        cov.set(px, py, (v + c).min(1.0));
    };
    let draw_glyph = |ch: char, x: f32, baseline: f32, plot: &mut dyn FnMut(i32, i32, f32)| {
        let id = sf.glyph_id(ch);
        let g = id.with_scale_and_position(scale, ab_glyph::point(x, baseline));
        if let Some(og) = font.outline_glyph(g) {
            let b = og.px_bounds();
            og.draw(|gx, gy, c| {
                let py = b.min.y as i32 + gy as i32;
                let sx = (shear * (baseline - py as f32)).round() as i32;
                let px = b.min.x as i32 + gx as i32 + sx;
                plot(px, py, c);
                if bold > 0.0 {
                    let k = bold.round() as i32;
                    for o in 1..=k {
                        plot(px + o, py, c);
                    }
                }
            });
        }
    };
    if data.vertical {
        // Columns right-to-left, characters top-to-bottom.
        let col_w = data.size * data.leading.max(0.1);
        for (li, line) in lines.iter().enumerate() {
            let cx = data.x - li as f32 * col_w;
            let mut y = data.y + sf.ascent();
            for ch in line.chars() {
                let id = sf.glyph_id(ch);
                let adv = sf.h_advance(id);
                draw_glyph(ch, cx - adv * 0.5, y, &mut plot);
                y += data.size + tracking;
            }
        }
    } else {
        for (li, line) in lines.iter().enumerate() {
            let lw = line_width(&sf, line, tracking);
            let mut x = data.x
                + match data.align {
                    forge_core::layer::TextAlign::Left => 0.0,
                    forge_core::layer::TextAlign::Center => (block_w - lw) * 0.5,
                    forge_core::layer::TextAlign::Right => block_w - lw,
                };
            let baseline = data.y + sf.ascent() + li as f32 * line_h;
            let x_start = x;
            let mut prev = None;
            for ch in line.chars() {
                let id = sf.glyph_id(ch);
                if let Some(p) = prev {
                    x += sf.kern(p, id);
                }
                draw_glyph(ch, x, baseline, &mut plot);
                x += sf.h_advance(id) + tracking;
                prev = Some(id);
            }
            if data.underline {
                let uy = baseline + (data.size * 0.1).max(1.0);
                let th = (data.size / 16.0).max(1.0);
                for yy in uy as i32..(uy + th) as i32 {
                    for xx in x_start as i32..x as i32 {
                        plot(xx, yy, 1.0);
                    }
                }
            }
        }
    }
    // Arc warp: bend the block vertically around its centre.
    let bounds = cov.content_bounds().unwrap_or_default();
    if data.warp.abs() > 0.5 && !bounds.is_empty() {
        let src = cov.clone();
        let cx = (bounds.x0 + bounds.x1) as f32 * 0.5;
        let half = (bounds.width() as f32 * 0.5).max(1.0);
        let amp = data.warp / 100.0 * bounds.height() as f32;
        let out_rect = bounds.expand(amp.abs().ceil() as i32 + 2);
        cov = TiledBuffer::new(w, h);
        cov.par_for_each_in_rect_mut(out_rect, |x, y, v| {
            let t = ((x as f32 + 0.5 - cx) / half).clamp(-1.0, 1.0);
            let off = amp * (1.0 - t * t);
            *v = src.get(x, (y as f32 + off).round() as i32);
        });
    }
    let color = data.color;
    let rgb = color.to_rgba8();
    let mut out = TiledBuffer::new(w, h);
    let bounds = cov.content_bounds().unwrap_or_default();
    let aa = data.antialias;
    out.par_for_each_in_rect_mut(bounds, |x, y, p| {
        let mut c = cov.get(x, y);
        if !aa {
            c = if c >= 0.5 { 1.0 } else { 0.0 };
        }
        if c > 0.0 {
            *p = [rgb[0], rgb[1], rgb[2], (c * color.a * 255.0 + 0.5) as u8];
        }
    });
    out.optimize();
    out
}
