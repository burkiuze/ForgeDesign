//! Text tool rasterisation (ab_glyph) and system font discovery.

use std::path::PathBuf;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use forge_core::buffer::TiledBuffer;
use forge_core::{Color, Point};

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
    found.sort_by(|a: &FontEntry, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
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

/// Rasterises `text` with its top-left corner at `pos` into a new buffer.
pub fn render_text(
    font: &FontVec,
    text: &str,
    size: f32,
    color: Color,
    pos: Point,
    w: u32,
    h: u32,
    antialias: bool,
) -> TiledBuffer<[u8; 4]> {
    let scale = PxScale::from(size.max(1.0));
    let sf = font.as_scaled(scale);
    let mut cov: TiledBuffer<f32> = TiledBuffer::new(w, h);
    let line_h = sf.ascent() - sf.descent() + sf.line_gap();
    for (li, line) in text.lines().enumerate() {
        let mut x = pos.x;
        let baseline = pos.y + sf.ascent() + li as f32 * line_h;
        let mut prev = None;
        for ch in line.chars() {
            let id = sf.glyph_id(ch);
            if let Some(p) = prev {
                x += sf.kern(p, id);
            }
            let g = id.with_scale_and_position(scale, ab_glyph::point(x, baseline));
            x += sf.h_advance(id);
            prev = Some(id);
            if let Some(og) = font.outline_glyph(g) {
                let b = og.px_bounds();
                og.draw(|gx, gy, c| {
                    let px = b.min.x as i32 + gx as i32;
                    let py = b.min.y as i32 + gy as i32;
                    let v = cov.get(px, py);
                    cov.set(px, py, (v + c).min(1.0));
                });
            }
        }
    }
    let rgb = color.to_rgba8();
    let mut out = TiledBuffer::new(w, h);
    let bounds = cov.content_bounds().unwrap_or_default();
    out.par_for_each_in_rect_mut(bounds, |x, y, p| {
        let mut c = cov.get(x, y);
        if !antialias {
            c = if c >= 0.5 { 1.0 } else { 0.0 };
        }
        if c > 0.0 {
            *p = [rgb[0], rgb[1], rgb[2], (c * color.a * 255.0 + 0.5) as u8];
        }
    });
    out.optimize();
    out
}
