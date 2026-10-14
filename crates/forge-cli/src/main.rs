//! `forge-cli`: headless batch processing with the Forge Design engine.
//!
//! ```text
//! forge-cli input.psd --resize 1920x1080 --sharpen --auto-contrast -o out.jpg
//! ```

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use forge_core::adjust::{self, Adjustment};
use forge_core::document::Document;
use forge_core::filters::{self, Filter};
use forge_core::transform::{self, Orientation, Resample};
use forge_core::IRect;

const HELP: &str = "\
forge-cli — Forge Design batch processor

USAGE:
    forge-cli <INPUT> [OPERATIONS...] -o <OUTPUT>

Operations run in order on the flattened image:
    --resize WxH            Resize (Lanczos-3); use 0 for one side to keep aspect
    --crop X,Y,W,H          Crop
    --rotate 90|180|270     Rotate clockwise
    --flip h|v              Flip horizontally / vertically
    --blur R                Gaussian blur (radius px)
    --sharpen               Unsharp mask (amount 0.8, radius 1.5)
    --median R              Median (noise reduction)
    --noise A               Add noise (0..1)
    --pixelate N            Mosaic with N px cells
    --invert                Invert colours
    --grayscale             Black & white
    --brightness B          -150..150
    --contrast C            -50..100
    --hue H                 Hue shift in degrees
    --saturation S          -100..100
    --posterize N           Posterize to N levels
    --threshold T           Threshold 0..255
    --auto-contrast         Stretch levels per channel
    --info                  Print document info
    -o, --output FILE       Output file (.ora keeps layers; others are flattened)
    -q, --quality Q         JPEG quality (default 92)
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn parse<T: std::str::FromStr>(v: Option<&String>, what: &str) -> Result<T, String> {
    v.ok_or_else(|| format!("{what} expects a value"))?
        .parse::<T>()
        .map_err(|_| format!("invalid value for {what}"))
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{HELP}");
        return Ok(());
    }
    let input = PathBuf::from(&args[0]);
    let t0 = Instant::now();
    let mut doc = forge_io::open(&input).map_err(|e| format!("{}: {e}", input.display()))?;
    let mut output: Option<PathBuf> = None;
    let mut opts = forge_io::ExportOptions::default();

    let mut i = 1;
    while i < args.len() {
        let a = args[i].as_str();
        let v = args.get(i + 1);
        let mut consumed = 1;
        match a {
            "-o" | "--output" => {
                output = Some(PathBuf::from(v.ok_or("-o expects a file")?));
                consumed = 2;
            }
            "-q" | "--quality" => {
                opts.jpeg_quality = parse(v, a)?;
                consumed = 2;
            }
            "--info" => {
                let s = &doc.state;
                println!("{}: {}×{} px, {} layer(s)", doc.name, s.width, s.height, s.layers.len());
                for l in s.layers.iter().rev() {
                    println!(
                        "  - {:<24} {:>3}%  {:<18} {}",
                        l.name,
                        (l.opacity * 100.0).round(),
                        l.blend.name(),
                        if l.visible { "" } else { "(hidden)" }
                    );
                }
            }
            "--resize" => {
                let v: String = parse(v, a)?;
                let (w, h) = v.split_once('x').ok_or("--resize expects WxH")?;
                let (mut w, mut h): (u32, u32) =
                    (w.parse().map_err(|_| "bad width")?, h.parse().map_err(|_| "bad height")?);
                let (ow, oh) = (doc.width() as f32, doc.height() as f32);
                if w == 0 && h == 0 {
                    return Err("--resize needs at least one side".into());
                }
                if w == 0 {
                    w = (ow * h as f32 / oh).round() as u32;
                }
                if h == 0 {
                    h = (oh * w as f32 / ow).round() as u32;
                }
                transform::resize_document(&mut doc.state, w, h, Resample::Lanczos3);
                consumed = 2;
            }
            "--crop" => {
                let v: String = parse(v, a)?;
                let n: Vec<i32> = v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
                if n.len() != 4 {
                    return Err("--crop expects X,Y,W,H".into());
                }
                transform::crop(&mut doc.state, IRect::new(n[0], n[1], n[0] + n[2], n[1] + n[3]));
                consumed = 2;
            }
            "--rotate" => {
                let o = match parse::<u32>(v, a)? {
                    90 => Orientation::Rotate90Cw,
                    180 => Orientation::Rotate180,
                    270 => Orientation::Rotate90Ccw,
                    _ => return Err("--rotate expects 90, 180 or 270".into()),
                };
                transform::reorient_document(&mut doc.state, o);
                consumed = 2;
            }
            "--flip" => {
                let o = match v.map(|s| s.as_str()) {
                    Some("h") => Orientation::FlipHorizontal,
                    Some("v") => Orientation::FlipVertical,
                    _ => return Err("--flip expects h or v".into()),
                };
                transform::reorient_document(&mut doc.state, o);
                consumed = 2;
            }
            _ => {
                let (op, takes_value) = parse_pixel_op(a, v)?;
                consumed = if takes_value { 2 } else { 1 };
                flatten_if_needed(&mut doc);
                let layer = &mut doc.state.layers[0];
                let base = layer.pixels.clone();
                match op {
                    PixelOp::Filter(f) => {
                        filters::apply(&mut layer.pixels, &base, None, &f);
                    }
                    PixelOp::Adjust(adj) => {
                        adjust::apply(&mut layer.pixels, &base, None, &adj);
                    }
                    PixelOp::AutoContrast => {
                        for adj in adjust::auto_levels(&base, base.bounds()) {
                            let b = layer.pixels.clone();
                            adjust::apply(&mut layer.pixels, &b, None, &adj);
                        }
                    }
                }
            }
        }
        i += consumed;
    }

    let Some(out) = output else {
        if args.iter().any(|a| a == "--info") {
            return Ok(());
        }
        return Err("no output file given (-o)".into());
    };
    let ext = out.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext == "ora" {
        forge_io::save(&doc.state, &out)
    } else {
        forge_io::export(&doc.state, &out, opts)
    }
    .map_err(|e| format!("{}: {e}", out.display()))?;
    eprintln!("wrote {} ({}×{}) in {:.0?}", out.display(), doc.width(), doc.height(), t0.elapsed());
    Ok(())
}

enum PixelOp {
    Filter(Filter),
    Adjust(Adjustment),
    AutoContrast,
}

fn parse_pixel_op(a: &str, v: Option<&String>) -> Result<(PixelOp, bool), String> {
    Ok(match a {
        "--blur" => (PixelOp::Filter(Filter::GaussianBlur { radius: parse(v, a)? }), true),
        "--sharpen" => (PixelOp::Filter(Filter::UnsharpMask { amount: 0.8, radius: 1.5, threshold: 0 }), false),
        "--median" => (PixelOp::Filter(Filter::Median { radius: parse(v, a)? }), true),
        "--noise" => (PixelOp::Filter(Filter::AddNoise { amount: parse(v, a)?, monochrome: false, seed: 1 }), true),
        "--pixelate" => (PixelOp::Filter(Filter::Pixelate { size: parse(v, a)? }), true),
        "--invert" => (PixelOp::Adjust(Adjustment::Invert), false),
        "--grayscale" => (PixelOp::Adjust(Adjustment::BlackAndWhite), false),
        "--brightness" => {
            (PixelOp::Adjust(Adjustment::BrightnessContrast { brightness: parse(v, a)?, contrast: 0.0 }), true)
        }
        "--contrast" => (PixelOp::Adjust(Adjustment::BrightnessContrast { brightness: 0.0, contrast: parse(v, a)? }), true),
        "--hue" => (
            PixelOp::Adjust(Adjustment::HueSaturation { hue: parse(v, a)?, saturation: 0.0, lightness: 0.0, colorize: false }),
            true,
        ),
        "--saturation" => (
            PixelOp::Adjust(Adjustment::HueSaturation { hue: 0.0, saturation: parse(v, a)?, lightness: 0.0, colorize: false }),
            true,
        ),
        "--posterize" => (PixelOp::Adjust(Adjustment::Posterize { levels: parse(v, a)? }), true),
        "--threshold" => (PixelOp::Adjust(Adjustment::Threshold { level: parse(v, a)? }), true),
        "--auto-contrast" => (PixelOp::AutoContrast, false),
        other => return Err(format!("unknown option {other} (see --help)")),
    })
}

/// Pixel operations work on one flattened layer, like a batch "Save for Web".
fn flatten_if_needed(doc: &mut Document) {
    if doc.state.layers.len() > 1 || doc.state.layers[0].mask.is_some() || doc.state.layers[0].opacity < 1.0 {
        let pixels = forge_core::composite::merged(&doc.state);
        let id = doc.state.alloc_id();
        doc.state.layers = vec![forge_core::Layer::with_pixels(id, "Background", pixels)];
        doc.state.active = 0;
    }
}
