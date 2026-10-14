//! # forge-io
//!
//! File formats for Forge Design:
//!
//! * **OpenRaster (`.ora`)** — the native, layered format. It is an open
//!   standard (also read by GIMP, Krita and MyPaint). Forge-only data such as
//!   layer masks, extra blend modes and the selection are stored in a
//!   `forge:` XML namespace and as extra PNGs, so other apps still open the
//!   file.
//! * **Photoshop (`.psd`)** — layered import (pixels, names, opacity,
//!   visibility, blend modes).
//! * **Raster formats** — PNG, JPEG, WebP, BMP, TIFF, TGA, GIF, ICO, QOI,
//!   PNM, … via the `image` crate (import and flattened export).

mod ora;
mod psd_export;
mod psd_import;

use std::path::Path;

use forge_core::buffer::TiledBuffer;
use forge_core::composite::{self, Background};
use forge_core::document::{DocState, Document};
use forge_core::layer::Layer;
use forge_core::{IRect, Rgba8};

pub use ora::{read_ora, write_ora};
pub use psd_export::write_psd;

#[derive(Debug, thiserror::Error)]
pub enum IoError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("image error: {0}")]
    Image(#[from] image::ImageError),
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("invalid XML: {0}")]
    Xml(#[from] roxmltree::Error),
    #[error("unsupported or corrupt file: {0}")]
    Format(String),
}

pub type Result<T> = std::result::Result<T, IoError>;

/// Extensions accepted by [`open`].
pub const OPEN_EXTENSIONS: &[&str] = &[
    "ora", "psd", "png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "tga", "gif", "ico", "qoi", "pnm", "pbm", "pgm",
    "ppm", "hdr", "exr", "dds", "ff",
];

/// Extensions accepted by [`export`].
pub const EXPORT_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "tga", "gif", "ico", "qoi", "pdf", "psd", "svg"];

#[derive(Clone, Copy, Debug)]
pub struct ExportOptions {
    /// JPEG quality 1..=100.
    pub jpeg_quality: u8,
    /// Background used for formats without alpha (JPEG).
    pub matte: Rgba8,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self { jpeg_quality: 92, matte: [255, 255, 255, 255] }
    }
}

fn extension(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

fn file_stem(path: &Path) -> String {
    path.file_name().and_then(|s| s.to_str()).unwrap_or("Untitled").to_string()
}

/// Opens any supported file as a document.
pub fn open(path: &Path) -> Result<Document> {
    let bytes = std::fs::read(path)?;
    let mut doc = open_bytes(&bytes, &extension(path), &file_stem(path))?;
    // Layered formats keep their path; flat images must be "Saved As".
    if matches!(extension(path).as_str(), "ora" | "psd") {
        doc.path = Some(path.to_path_buf());
    }
    Ok(doc)
}

/// Opens a document from memory. `ext` selects the decoder (case-insensitive);
/// unknown extensions fall back to content sniffing.
pub fn open_bytes(bytes: &[u8], ext: &str, name: &str) -> Result<Document> {
    let state = match ext.to_ascii_lowercase().as_str() {
        "ora" => read_ora(bytes)?,
        "psd" => psd_import::read_psd(bytes)?,
        _ => {
            let px = decode_image(bytes)?;
            let mut state = DocState::new(px.width(), px.height());
            let id = state.alloc_id();
            state.layers.push(Layer::with_pixels(id, "Background", px));
            state
        }
    };
    Ok(Document::from_state(name, state, None))
}

/// Decodes a raster image into a tiled RGBA buffer.
pub fn decode_image(bytes: &[u8]) -> Result<TiledBuffer<Rgba8>> {
    let img = image::load_from_memory(bytes)?.to_rgba8();
    let (w, h) = img.dimensions();
    let raw = img.into_raw();
    // SAFETY-free reinterpretation: chunk into [u8; 4].
    let px: Vec<Rgba8> = raw.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
    Ok(TiledBuffer::from_dense(w, h, &px))
}

/// Loads an image file as a single buffer (File > Place).
pub fn load_image(path: &Path) -> Result<TiledBuffer<Rgba8>> {
    decode_image(&std::fs::read(path)?)
}

/// Saves the document with layers: `.psd` writes Photoshop format,
/// anything else OpenRaster (the native format).
pub fn save(state: &DocState, path: &Path) -> Result<()> {
    let bytes = match extension(path).as_str() {
        "psd" => write_psd(state)?,
        _ => write_ora(state)?,
    };
    // Write atomically so a crash never leaves a truncated file behind.
    let tmp = path.with_extension("forge-tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Exports the flattened image; the format is picked from the extension.
pub fn export(state: &DocState, path: &Path, opts: ExportOptions) -> Result<()> {
    let bytes = match extension(path).as_str() {
        "pdf" => export_pdf(state, 300.0, opts)?,
        "svg" => export_svg(state)?,
        "psd" => write_psd(state)?,
        "ora" => write_ora(state)?,
        ext => export_bytes(state, ext, opts)?,
    };
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Exports a single-page PDF with the flattened image embedded as JPEG.
/// `dpi` sets the physical page size.
pub fn export_pdf(state: &DocState, dpi: f32, opts: ExportOptions) -> Result<Vec<u8>> {
    let (w, h) = (state.width, state.height);
    let jpeg = export_bytes(state, "jpg", opts)?;
    let (pw, ph) = (w as f32 * 72.0 / dpi.max(1.0), h as f32 * 72.0 / dpi.max(1.0));
    let content = format!("q {pw:.2} 0 0 {ph:.2} 0 0 cm /Im0 Do Q");
    let mut out: Vec<u8> = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::new();
    let mut obj = |out: &mut Vec<u8>, body: Vec<u8>| {
        offsets.push(out.len());
        let n = offsets.len();
        out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
        out.extend_from_slice(&body);
        out.extend_from_slice(b"\nendobj\n");
    };
    obj(&mut out, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    obj(&mut out, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec());
    obj(
        &mut out,
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.2} {ph:.2}] /Resources << /XObject << /Im0 4 0 R >> >> /Contents 5 0 R >>")
            .into_bytes(),
    );
    let mut img = format!("<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n", jpeg.len()).into_bytes();
    img.extend_from_slice(&jpeg);
    img.extend_from_slice(b"\nendstream");
    obj(&mut out, img);
    let mut c = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
    c.extend_from_slice(content.as_bytes());
    c.extend_from_slice(b"\nendstream");
    obj(&mut out, c);
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
    for o in &offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", offsets.len() + 1).as_bytes());
    Ok(out)
}

/// SVG export: the flattened image embedded as PNG plus every vector path
/// as a real `<path>` (useful for handing artwork to vector tools / the web).
pub fn export_svg(state: &DocState) -> Result<Vec<u8>> {
    let (w, h) = (state.width, state.height);
    let png = export_bytes(state, "png", ExportOptions::default())?;
    let mut svg = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">\n"
    );
    svg.push_str(&format!("  <image width=\"{w}\" height=\"{h}\" xlink:href=\"data:image/png;base64,{}\"/>\n", base64(&png)));
    for p in &state.paths {
        let mut d = String::new();
        for sp in &p.subpaths {
            let n = sp.anchors.len();
            if n == 0 {
                continue;
            }
            d.push_str(&format!("M{:.2} {:.2} ", sp.anchors[0].p.x, sp.anchors[0].p.y));
            let count = if sp.closed { n } else { n - 1 };
            for i in 0..count {
                let (a, b) = (sp.anchors[i], sp.anchors[(i + 1) % n]);
                d.push_str(&format!(
                    "C{:.2} {:.2} {:.2} {:.2} {:.2} {:.2} ",
                    a.h_out.x, a.h_out.y, b.h_in.x, b.h_in.y, b.p.x, b.p.y
                ));
            }
            if sp.closed {
                d.push('Z');
            }
        }
        let name: String = p.name.chars().filter(|c| c.is_alphanumeric() || *c == ' ').collect();
        svg.push_str(&format!("  <path id=\"{name}\" d=\"{}\" fill=\"none\" stroke=\"#000\" stroke-width=\"1\"/>\n", d.trim()));
    }
    svg.push_str("</svg>\n");
    Ok(svg.into_bytes())
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Frame animation: each visible pixel layer (bottom first) becomes one
/// GIF frame. When `with_background` is set, the bottom layer is used as a
/// backdrop for every frame instead of being a frame itself.
pub fn export_gif_animation(state: &DocState, delay_ms: u32, with_background: bool) -> Result<Vec<u8>> {
    use image::codecs::gif::{GifEncoder, Repeat};
    use image::{Delay, Frame, RgbaImage};
    let (w, h) = (state.width, state.height);
    let frames: Vec<&Layer> = state.layers.iter().filter(|l| !l.is_group() && l.visible).collect();
    let (bg, frames) = if with_background && frames.len() > 1 { (Some(frames[0]), &frames[1..]) } else { (None, &frames[..]) };
    let mut out = Vec::new();
    {
        let mut enc = GifEncoder::new_with_speed(&mut out, 10);
        enc.set_repeat(Repeat::Infinite)?;
        for l in frames {
            let mut stack: Vec<&Layer> = Vec::new();
            if let Some(b) = bg {
                stack.push(b);
            }
            stack.push(l);
            let px = composite::composite_layers(&stack, w, h, IRect::from_size(w, h), Background::Transparent);
            let flat: Vec<u8> = px.iter().flatten().copied().collect();
            let img = RgbaImage::from_raw(w, h, flat).ok_or_else(|| IoError::Format("size mismatch".into()))?;
            enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(delay_ms, 1)))?;
        }
    }
    Ok(out)
}

/// File > Export > Layers to Files: one PNG per pixel layer.
pub fn export_layers(state: &DocState, dir: &Path, prefix: &str) -> Result<usize> {
    let (w, h) = (state.width, state.height);
    let mut n = 0;
    for (i, l) in state.layers.iter().enumerate().filter(|(_, l)| !l.is_group()) {
        let mut single = l.clone();
        single.parent = None;
        single.visible = true;
        let px = composite::composite_layers(&[&single], w, h, IRect::from_size(w, h), Background::Transparent);
        let safe: String = l.name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
        let path = dir.join(format!("{prefix}_{i:03}_{safe}.png"));
        std::fs::write(path, encode_rgba(&px, w, h, "png", ExportOptions::default())?)?;
        n += 1;
    }
    Ok(n)
}

pub fn export_bytes(state: &DocState, ext: &str, opts: ExportOptions) -> Result<Vec<u8>> {
    let (w, h) = (state.width, state.height);
    let opaque = matches!(ext, "jpg" | "jpeg" | "bmp");
    let bg = if opaque { Background::Solid(opts.matte) } else { Background::Transparent };
    let px = composite::composite_rect(state, IRect::from_size(w, h), bg);
    encode_rgba(&px, w, h, ext, opts)
}

/// Encodes dense RGBA pixels into the given format.
pub fn encode_rgba(px: &[Rgba8], w: u32, h: u32, ext: &str, opts: ExportOptions) -> Result<Vec<u8>> {
    use image::codecs::{jpeg::JpegEncoder, png::PngEncoder};
    use image::{ExtendedColorType, ImageEncoder, ImageFormat};
    let flat: Vec<u8> = px.iter().flatten().copied().collect();
    let mut out = std::io::Cursor::new(Vec::new());
    match ext {
        "png" => {
            PngEncoder::new_with_quality(
                &mut out,
                image::codecs::png::CompressionType::Fast,
                image::codecs::png::FilterType::Adaptive,
            )
            .write_image(&flat, w, h, ExtendedColorType::Rgba8)?;
        }
        "jpg" | "jpeg" => {
            let rgb: Vec<u8> = px.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
            JpegEncoder::new_with_quality(&mut out, opts.jpeg_quality.clamp(1, 100))
                .write_image(&rgb, w, h, ExtendedColorType::Rgb8)?;
        }
        other => {
            let fmt = ImageFormat::from_extension(other)
                .ok_or_else(|| IoError::Format(format!("cannot export .{other}")))?;
            let img = image::RgbaImage::from_raw(w, h, flat).ok_or_else(|| IoError::Format("size mismatch".into()))?;
            let dynimg = image::DynamicImage::ImageRgba8(img);
            let dynimg = if matches!(fmt, ImageFormat::Bmp) { image::DynamicImage::ImageRgb8(dynimg.to_rgb8()) } else { dynimg };
            dynimg.write_to(&mut out, fmt)?;
        }
    }
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::{BlendMode, Color};

    #[test]
    fn ora_roundtrip() {
        let mut d = Document::new("t", 130, 90, Some(Color::WHITE));
        d.new_layer();
        {
            let l = &mut d.state.layers[1];
            l.pixels.set(100, 70, [10, 20, 30, 200]);
            l.blend = BlendMode::VividLight;
            l.opacity = 0.5;
            l.name = "Ünïcode & <xml>".into();
            l.add_mask();
            l.mask.as_mut().unwrap().set(5, 5, 7);
        }
        d.state.selection = Some(forge_core::selection::rect_mask(130, 90, IRect::new(1, 2, 3, 4)));
        let bytes = write_ora(&d.state).unwrap();
        let s = read_ora(&bytes).unwrap();
        assert_eq!((s.width, s.height), (130, 90));
        assert_eq!(s.layers.len(), 2);
        let l = &s.layers[1];
        assert_eq!(l.name, "Ünïcode & <xml>");
        assert_eq!(l.blend, BlendMode::VividLight);
        assert!((l.opacity - 0.5).abs() < 1e-3);
        assert_eq!(l.pixels.get(100, 70), [10, 20, 30, 200]);
        assert_eq!(l.mask.as_ref().unwrap().get(5, 5), 7);
        assert_eq!(l.mask.as_ref().unwrap().get(6, 5), 255);
        assert_eq!(s.layers[0].pixels.get(0, 0), [255, 255, 255, 255]);
        assert_eq!(s.selection.unwrap().content_bounds(), Some(IRect::new(1, 2, 3, 4)));
    }

    #[test]
    fn psd_roundtrip_with_groups() {
        let mut d = Document::new("t", 70, 40, Some(Color::WHITE));
        d.new_layer();
        d.state.layers[1].pixels.set(65, 35, [10, 20, 30, 255]);
        d.state.layers[1].blend = BlendMode::Multiply;
        d.state.layers[1].name = "Kırmızı".into();
        d.group_layer();
        let bytes = write_psd(&d.state).unwrap();
        let s = psd_import::read_psd(&bytes).unwrap();
        assert_eq!((s.width, s.height), (70, 40));
        assert_eq!(s.layers.len(), 3, "{:?}", s.layers.iter().map(|l| &l.name).collect::<Vec<_>>());
        let px = s.layers.iter().find(|l| l.blend == BlendMode::Multiply).expect("multiply layer");
        assert_eq!(px.pixels.get(65, 35), [10, 20, 30, 255]);
        assert!(px.parent.is_some(), "layer kept its group");
        assert!(s.layers.iter().any(|l| l.is_group()));
    }

    #[test]
    fn ora_groups_and_styles() {
        let mut d = Document::new("t", 30, 30, None);
        d.new_layer();
        d.state.layers[1].clipped = true;
        d.state.layers[1].fill = 0.25;
        d.state.layers[1].style = Some(forge_core::styles::LayerStyle {
            enabled: true,
            drop_shadow: Some(forge_core::styles::Shadow::drop()),
            ..Default::default()
        });
        d.group_layer();
        let mut adj = forge_core::Layer::new_adjustment(99, forge_core::adjust::Adjustment::Invert, 30, 30);
        adj.id = d.state.alloc_id();
        d.state.layers.insert(0, adj);
        let s = read_ora(&write_ora(&d.state).unwrap()).unwrap();
        assert!(s.layers[0].is_adjustment());
        assert_eq!(s.layers[0].adjustment, Some(forge_core::adjust::Adjustment::Invert));
        let s = DocState { layers: s.layers[1..].to_vec(), ..s };
        assert_eq!(s.layers.len(), 3);
        assert!(s.layers[2].is_group());
        assert_eq!(s.layers[1].parent, Some(s.layers[2].id));
        assert!(s.layers[1].clipped);
        assert!((s.layers[1].fill - 0.25).abs() < 1e-3);
        assert_eq!(s.layers[1].style, d.state.layers[2].style);
    }

    #[test]
    fn pdf_gif_export() {
        let mut d = Document::new("t", 16, 8, Some(Color::WHITE));
        d.new_layer();
        let pdf = export_pdf(&d.state, 72.0, ExportOptions::default()).unwrap();
        assert!(pdf.starts_with(b"%PDF") && pdf.ends_with(b"%%EOF\n"));
        let gif = export_gif_animation(&d.state, 100, false).unwrap();
        assert!(gif.starts_with(b"GIF89a"));
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        let svg = String::from_utf8(export_svg(&d.state).unwrap()).unwrap();
        assert!(svg.contains("<svg") && svg.contains("data:image/png;base64,"));
    }

    #[test]
    fn png_and_jpeg_export() {
        let mut d = Document::new("t", 16, 8, None);
        d.state.layers[0].pixels.set(3, 3, [255, 0, 0, 255]);
        let png = export_bytes(&d.state, "png", ExportOptions::default()).unwrap();
        let back = decode_image(&png).unwrap();
        assert_eq!(back.get(3, 3), [255, 0, 0, 255]);
        assert_eq!(back.get(0, 0), [0, 0, 0, 0]);
        let jpg = export_bytes(&d.state, "jpg", ExportOptions::default()).unwrap();
        let back = decode_image(&jpg).unwrap();
        assert!(back.get(0, 0)[0] > 240, "matte is white");
        for ext in ["webp", "bmp", "tiff", "tga", "qoi"] {
            let b = export_bytes(&d.state, ext, ExportOptions::default()).unwrap();
            assert!(!b.is_empty(), "{ext}");
        }
    }
}
