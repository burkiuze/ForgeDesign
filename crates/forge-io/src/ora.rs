//! OpenRaster reader/writer (https://www.openraster.org/).

use std::io::{Cursor, Read, Write};

use forge_core::buffer::TiledBuffer;
use forge_core::composite::{self, Background};
use forge_core::document::DocState;
use forge_core::layer::Layer;
use forge_core::{BlendMode, IRect, Rgba8};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::{decode_image, encode_rgba, ExportOptions, IoError, Result};

const FORGE_NS: &str = "https://github.com/burkiuze/ForgeDesign/ns/ora";

fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            c => o.push(c),
        }
    }
    o
}

fn png_rgba(px: &[Rgba8], w: u32, h: u32) -> Result<Vec<u8>> {
    encode_rgba(px, w, h, "png", ExportOptions::default())
}

fn png_gray(px: &[u8], w: u32, h: u32) -> Result<Vec<u8>> {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::{ExtendedColorType, ImageEncoder};
    let mut out = Cursor::new(Vec::new());
    PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Adaptive)
        .write_image(px, w, h, ExtendedColorType::L8)?;
    Ok(out.into_inner())
}

/// Serialises a document as an OpenRaster archive.
pub fn write_ora(state: &DocState) -> Result<Vec<u8>> {
    let (w, h) = (state.width, state.height);
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    // The spec requires an uncompressed "mimetype" entry first.
    zip.start_file("mimetype", stored)?;
    zip.write_all(b"image/openraster")?;

    let mut xml = String::new();
    xml.push_str("<?xml version='1.0' encoding='UTF-8'?>\n");
    xml.push_str(&format!("<image version=\"0.0.5\" w=\"{w}\" h=\"{h}\" xmlns:forge=\"{FORGE_NS}\""));
    if let Some(sel) = &state.selection {
        let data = png_gray(&sel.to_dense(), w, h)?;
        zip.start_file("data/selection.png", stored)?;
        zip.write_all(&data)?;
        xml.push_str(" forge:selection=\"data/selection.png\"");
    }
    xml.push_str(&format!(" forge:active=\"{}\">\n<stack>\n", state.active));

    // OpenRaster lists the top-most layer first.
    for (i, layer) in state.layers.iter().enumerate().rev() {
        let bounds = layer.pixels.content_bounds().unwrap_or(IRect::new(0, 0, 1, 1));
        let px = layer.pixels.region_to_dense(bounds);
        let src = format!("data/layer{i}.png");
        let data = png_rgba(&px, bounds.width() as u32, bounds.height() as u32)?;
        zip.start_file(&src, stored)?; // PNG is already compressed
        zip.write_all(&data)?;
        xml.push_str(&format!(
            "  <layer name=\"{}\" src=\"{src}\" x=\"{}\" y=\"{}\" opacity=\"{:.4}\" visibility=\"{}\" composite-op=\"{}\"",
            escape(&layer.name),
            bounds.x0,
            bounds.y0,
            layer.opacity,
            if layer.visible { "visible" } else { "hidden" },
            layer.blend.ora_name(),
        ));
        if layer.lock_alpha {
            xml.push_str(" forge:lock-alpha=\"true\"");
        }
        if layer.locked {
            xml.push_str(" edit-locked=\"true\"");
        }
        if let Some(mask) = &layer.mask {
            let msrc = format!("data/mask{i}.png");
            let data = png_gray(&mask.to_dense(), w, h)?;
            zip.start_file(&msrc, stored)?;
            zip.write_all(&data)?;
            xml.push_str(&format!(" forge:mask=\"{msrc}\" forge:mask-enabled=\"{}\"", layer.mask_enabled));
        }
        xml.push_str(" />\n");
    }
    xml.push_str("</stack>\n</image>\n");
    zip.start_file("stack.xml", deflated)?;
    zip.write_all(xml.as_bytes())?;

    // Merged image + thumbnail (required by the spec, used by file browsers).
    let merged = composite::composite_rect(state, state.bounds(), Background::Transparent);
    zip.start_file("mergedimage.png", stored)?;
    zip.write_all(&png_rgba(&merged, w, h)?)?;
    let scale = (256.0 / w.max(h) as f32).min(1.0);
    let (tw, th) = (((w as f32 * scale) as u32).max(1), ((h as f32 * scale) as u32).max(1));
    let thumb = forge_core::transform::resample_rgba(&TiledBuffer::from_dense(w, h, &merged), tw, th, forge_core::transform::Resample::Bilinear);
    zip.start_file("Thumbnails/thumbnail.png", stored)?;
    zip.write_all(&png_rgba(&thumb.to_dense(), tw, th)?)?;

    Ok(zip.finish()?.into_inner())
}

fn read_entry(zip: &mut ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<Vec<u8>> {
    let mut f = zip.by_name(name)?;
    let mut buf = Vec::with_capacity(f.size() as usize);
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

fn decode_gray(bytes: &[u8], w: u32, h: u32, fill: u8) -> Result<TiledBuffer<u8>> {
    let img = image::load_from_memory(bytes)?.to_luma8();
    let mut m = TiledBuffer::new_filled(w, h, fill);
    let (iw, ih) = img.dimensions();
    let r = IRect::from_size(iw.min(w), ih.min(h));
    m.par_for_each_in_rect_mut(r, |x, y, p| *p = img.get_pixel(x as u32, y as u32)[0]);
    m.optimize();
    Ok(m)
}

/// Parses an OpenRaster archive.
pub fn read_ora(bytes: &[u8]) -> Result<DocState> {
    let mut zip = ZipArchive::new(Cursor::new(bytes))?;
    let xml = String::from_utf8(read_entry(&mut zip, "stack.xml")?).map_err(|e| IoError::Format(e.to_string()))?;
    let doc = roxmltree::Document::parse(&xml)?;
    let root = doc.root_element();
    let attr_u32 = |n: roxmltree::Node, a: &str| n.attribute(a).and_then(|v| v.trim().parse::<u32>().ok());
    let (w, h) = match (attr_u32(root, "w"), attr_u32(root, "h")) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w, h),
        _ => return Err(IoError::Format("stack.xml lacks image size".into())),
    };
    let mut state = DocState::new(w, h);
    if let Some(src) = root.attribute((FORGE_NS, "selection")) {
        let data = read_entry(&mut zip, src)?;
        let m = decode_gray(&data, w, h, 0)?;
        state.selection = m.content_bounds().map(|_| m);
    }

    // Collect layers top-to-bottom; nested stacks (groups from other apps)
    // are flattened into the list.
    let mut nodes = Vec::new();
    fn collect<'a, 'i>(n: roxmltree::Node<'a, 'i>, out: &mut Vec<roxmltree::Node<'a, 'i>>) {
        for c in n.children().filter(|c| c.is_element()) {
            match c.tag_name().name() {
                "layer" => out.push(c),
                "stack" => collect(c, out),
                _ => {}
            }
        }
    }
    collect(root, &mut nodes);

    for node in nodes.into_iter().rev() {
        let Some(src) = node.attribute("src") else { continue };
        if !src.to_ascii_lowercase().ends_with(".png") {
            continue; // e.g. SVG layers: unsupported
        }
        let data = read_entry(&mut zip, src)?;
        let img = decode_image(&data)?;
        let x = node.attribute("x").and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
        let y = node.attribute("y").and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
        let pixels = forge_core::transform::translate(&img, x, y, w, h);
        let id = state.alloc_id();
        let mut layer = Layer::with_pixels(id, node.attribute("name").unwrap_or("Layer"), pixels);
        layer.opacity = node.attribute("opacity").and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0).clamp(0.0, 1.0);
        layer.visible = node.attribute("visibility") != Some("hidden");
        layer.blend = node.attribute("composite-op").and_then(BlendMode::from_ora_name).unwrap_or_default();
        layer.lock_alpha = node.attribute((FORGE_NS, "lock-alpha")) == Some("true");
        layer.locked = node.attribute("edit-locked") == Some("true");
        if let Some(msrc) = node.attribute((FORGE_NS, "mask")) {
            let data = read_entry(&mut zip, msrc)?;
            layer.mask = Some(decode_gray(&data, w, h, 255)?);
            layer.mask_enabled = node.attribute((FORGE_NS, "mask-enabled")) != Some("false");
        }
        state.layers.push(layer);
    }
    if state.layers.is_empty() {
        let id = state.alloc_id();
        state.layers.push(Layer::new(id, "Background", w, h));
    }
    state.active = root
        .attribute((FORGE_NS, "active"))
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(state.layers.len() - 1)
        .min(state.layers.len() - 1);
    Ok(state)
}
