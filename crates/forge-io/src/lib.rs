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
mod psd_import;

use std::path::Path;

use forge_core::buffer::TiledBuffer;
use forge_core::composite::{self, Background};
use forge_core::document::{DocState, Document};
use forge_core::layer::Layer;
use forge_core::{IRect, Rgba8};

pub use ora::{read_ora, write_ora};

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
pub const EXPORT_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "tga", "gif", "ico", "qoi"];

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
    // Only native files keep their path; others must be "Saved As" .ora.
    if extension(path) == "ora" {
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

/// Saves the document in the native format (OpenRaster).
pub fn save(state: &DocState, path: &Path) -> Result<()> {
    let bytes = write_ora(state)?;
    // Write atomically so a crash never leaves a truncated file behind.
    let tmp = path.with_extension("ora.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Exports the flattened image; the format is picked from the extension.
pub fn export(state: &DocState, path: &Path, opts: ExportOptions) -> Result<()> {
    let bytes = export_bytes(state, &extension(path), opts)?;
    std::fs::write(path, bytes)?;
    Ok(())
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
