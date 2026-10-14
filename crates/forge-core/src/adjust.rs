//! Colour adjustments (Image > Adjustments). Per-channel adjustments are
//! compiled to 256-entry lookup tables, so applying them is one table
//! lookup per channel per pixel.

use serde::{Deserialize, Serialize};

use crate::buffer::TiledBuffer;
use crate::color::{hsl_to_rgb, lum, rgb_to_hsl, Color, Rgba8};
use crate::geom::IRect;
use crate::selection::{self, coverage, Mask};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Channel {
    #[default]
    Rgb,
    Red,
    Green,
    Blue,
}

impl Channel {
    pub const ALL: [Channel; 4] = [Channel::Rgb, Channel::Red, Channel::Green, Channel::Blue];
    pub fn name(self) -> &'static str {
        match self {
            Channel::Rgb => "RGB",
            Channel::Red => "Red",
            Channel::Green => "Green",
            Channel::Blue => "Blue",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Levels {
    pub in_black: f32,
    pub in_white: f32,
    pub gamma: f32,
    pub out_black: f32,
    pub out_white: f32,
}

impl Default for Levels {
    fn default() -> Self {
        Self { in_black: 0.0, in_white: 255.0, gamma: 1.0, out_black: 0.0, out_white: 255.0 }
    }
}

impl Levels {
    pub fn map(&self, v: f32) -> f32 {
        let t = ((v - self.in_black) / (self.in_white - self.in_black).max(1.0)).clamp(0.0, 1.0);
        let t = t.powf(1.0 / self.gamma.max(0.01));
        self.out_black + t * (self.out_white - self.out_black)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Adjustment {
    BrightnessContrast { brightness: f32, contrast: f32 },
    Levels { channel: Channel, levels: Levels },
    /// Control points in `0..=255` space, applied to `channel`.
    Curves { channel: Channel, points: Vec<(f32, f32)> },
    Exposure { exposure: f32, offset: f32, gamma: f32 },
    HueSaturation { hue: f32, saturation: f32, lightness: f32, colorize: bool },
    Vibrance { vibrance: f32, saturation: f32 },
    ColorBalance { shadows: [f32; 3], midtones: [f32; 3], highlights: [f32; 3] },
    PhotoFilter { color: Color, density: f32 },
    BlackAndWhite,
    Invert,
    Desaturate,
    Threshold { level: u8 },
    Posterize { levels: u8 },
    GradientMap { from: Color, to: Color },
}

impl Adjustment {
    pub fn name(&self) -> &'static str {
        match self {
            Adjustment::BrightnessContrast { .. } => "Brightness/Contrast",
            Adjustment::Levels { .. } => "Levels",
            Adjustment::Curves { .. } => "Curves",
            Adjustment::Exposure { .. } => "Exposure",
            Adjustment::HueSaturation { .. } => "Hue/Saturation",
            Adjustment::Vibrance { .. } => "Vibrance",
            Adjustment::ColorBalance { .. } => "Color Balance",
            Adjustment::PhotoFilter { .. } => "Photo Filter",
            Adjustment::BlackAndWhite => "Black & White",
            Adjustment::Invert => "Invert",
            Adjustment::Desaturate => "Desaturate",
            Adjustment::Threshold { .. } => "Threshold",
            Adjustment::Posterize { .. } => "Posterize",
            Adjustment::GradientMap { .. } => "Gradient Map",
        }
    }

    /// Per-channel lookup tables, when the adjustment is separable.
    fn luts(&self) -> Option<[[u8; 256]; 3]> {
        let build = |f: &dyn Fn(f32) -> f32| -> [u8; 256] {
            std::array::from_fn(|i| f(i as f32).round().clamp(0.0, 255.0) as u8)
        };
        let identity = build(&|v| v);
        let per_channel = |channel: Channel, table: [u8; 256]| match channel {
            Channel::Rgb => [table; 3],
            Channel::Red => [table, identity, identity],
            Channel::Green => [identity, table, identity],
            Channel::Blue => [identity, identity, table],
        };
        Some(match self {
            Adjustment::BrightnessContrast { brightness, contrast } => {
                let b = brightness / 150.0;
                let c = (contrast / 100.0).clamp(-1.0, 0.99);
                let factor = if c >= 0.0 { 1.0 / (1.0 - c) } else { 1.0 + c };
                [build(&|v| {
                    let t = v / 255.0;
                    let t = if b >= 0.0 { t + (1.0 - t) * b * t.sqrt() } else { t * (1.0 + b) };
                    ((t - 0.5) * factor + 0.5) * 255.0
                }); 3]
            }
            Adjustment::Levels { channel, levels } => per_channel(*channel, build(&|v| levels.map(v))),
            Adjustment::Curves { channel, points } => per_channel(*channel, curve_lut(points)),
            Adjustment::Exposure { exposure, offset, gamma } => {
                let m = 2f32.powf(*exposure);
                [build(&|v| {
                    let t = (v / 255.0 * m + offset).max(0.0);
                    t.powf(1.0 / gamma.max(0.01)) * 255.0
                }); 3]
            }
            Adjustment::Invert => [build(&|v| 255.0 - v); 3],
            Adjustment::Posterize { levels } => {
                let n = (*levels).max(2) as f32 - 1.0;
                [build(&|v| (v / 255.0 * n).round() / n * 255.0); 3]
            }
            _ => return None,
        })
    }

    /// Returns a per-pixel function implementing the adjustment.
    pub fn compile(&self) -> Box<dyn Fn(Rgba8) -> Rgba8 + Send + Sync> {
        if let Some(l) = self.luts() {
            return Box::new(move |p| [l[0][p[0] as usize], l[1][p[1] as usize], l[2][p[2] as usize], p[3]]);
        }
        let adj = self.clone();
        Box::new(move |p| {
            let c = [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0];
            let o = adj.apply_rgb(c);
            let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            [q(o[0]), q(o[1]), q(o[2]), p[3]]
        })
    }

    fn apply_rgb(&self, c: [f32; 3]) -> [f32; 3] {
        match self {
            Adjustment::HueSaturation { hue, saturation, lightness, colorize } => {
                let [h, s, l] = rgb_to_hsl(c);
                let (h, s, l) = if *colorize {
                    (hue.rem_euclid(360.0), (saturation / 100.0).clamp(0.0, 1.0), lum(c))
                } else {
                    let sat = saturation / 100.0;
                    let s = if sat >= 0.0 { s + (1.0 - s) * sat * s.sqrt() } else { s * (1.0 + sat) };
                    (h + hue, s.clamp(0.0, 1.0), l)
                };
                let l2 = lightness / 100.0;
                let l = if l2 >= 0.0 { l + (1.0 - l) * l2 } else { l * (1.0 + l2) };
                hsl_to_rgb([h, s, l])
            }
            Adjustment::Vibrance { vibrance, saturation } => {
                let [h, s, l] = rgb_to_hsl(c);
                let v = vibrance / 100.0;
                // Muted colours get the strongest boost; saturated ones barely move.
                let s = (s * (1.0 + v * (1.0 - s) * 2.0)).clamp(0.0, 1.0);
                let sat = saturation / 100.0;
                let s = if sat >= 0.0 { s + (1.0 - s) * sat * s } else { s * (1.0 + sat) };
                hsl_to_rgb([h, s.clamp(0.0, 1.0), l])
            }
            Adjustment::ColorBalance { shadows, midtones, highlights } => {
                let l = lum(c);
                let ws = ((0.33 - l) / 0.33).clamp(0.0, 1.0);
                let wh = ((l - 0.66) / 0.34).clamp(0.0, 1.0);
                let wm = 1.0 - ws - wh;
                let mut o = c;
                for i in 0..3 {
                    o[i] += (shadows[i] * ws + midtones[i] * wm + highlights[i] * wh) / 100.0 * 0.5;
                }
                // Preserve luminosity like Photoshop's default.
                let d = l - lum(o);
                o.map(|v| (v + d).clamp(0.0, 1.0))
            }
            Adjustment::PhotoFilter { color, density } => {
                let l = lum(c);
                let mut o = [0.0; 3];
                let fc = color.rgb_array();
                for i in 0..3 {
                    o[i] = c[i] + (c[i] * fc[i] * 2.0 - c[i]) * density;
                }
                let d = l - lum(o);
                o.map(|v| (v + d).clamp(0.0, 1.0))
            }
            Adjustment::BlackAndWhite => {
                // Photoshop's default B&W mix favours reds/yellows slightly.
                let v = (c[0] * 0.4 + c[1] * 0.4 + c[2] * 0.2).clamp(0.0, 1.0);
                [v; 3]
            }
            Adjustment::Desaturate => {
                let max = c[0].max(c[1]).max(c[2]);
                let min = c[0].min(c[1]).min(c[2]);
                [(max + min) * 0.5; 3]
            }
            Adjustment::Threshold { level } => {
                let v = if lum(c) * 255.0 >= *level as f32 { 1.0 } else { 0.0 };
                [v; 3]
            }
            Adjustment::GradientMap { from, to } => {
                let t = lum(c);
                from.lerp(*to, t).rgb_array()
            }
            _ => c,
        }
    }
}

/// Applies an adjustment to `base` within the selection, writing into `target`.
pub fn apply(target: &mut TiledBuffer<Rgba8>, base: &TiledBuffer<Rgba8>, sel: Option<&Mask>, adj: &Adjustment) -> Option<IRect> {
    let rect = selection::work_rect(sel, base.width(), base.height());
    if rect.is_empty() {
        return None;
    }
    let f = adj.compile();
    target.par_for_each_in_rect_mut(rect, |x, y, p| {
        let b = base.get(x, y);
        if b[3] == 0 {
            *p = b;
            return;
        }
        let s = coverage(sel, x, y);
        let o = f(b);
        *p = if s >= 1.0 {
            o
        } else {
            let mix = |i: usize| (b[i] as f32 + (o[i] as f32 - b[i] as f32) * s + 0.5) as u8;
            [mix(0), mix(1), mix(2), b[3]]
        };
    });
    Some(rect)
}

/// Monotone cubic (Fritsch–Carlson) interpolation through curve points,
/// sampled into a lookup table. Points are `(input, output)` in `0..=255`.
pub fn curve_lut(points: &[(f32, f32)]) -> [u8; 256] {
    let mut pts: Vec<(f32, f32)> = points.to_vec();
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    pts.dedup_by(|a, b| (a.0 - b.0).abs() < 0.5);
    if pts.is_empty() {
        return std::array::from_fn(|i| i as u8);
    }
    if pts.len() == 1 {
        let v = pts[0].1.round().clamp(0.0, 255.0) as u8;
        return [v; 256];
    }
    let n = pts.len();
    let d: Vec<f32> = (0..n - 1).map(|i| (pts[i + 1].1 - pts[i].1) / (pts[i + 1].0 - pts[i].0)).collect();
    let mut m = vec![0.0f32; n];
    m[0] = d[0];
    m[n - 1] = d[n - 2];
    for i in 1..n - 1 {
        m[i] = if d[i - 1] * d[i] <= 0.0 { 0.0 } else { (d[i - 1] + d[i]) * 0.5 };
    }
    for i in 0..n - 1 {
        if d[i] == 0.0 {
            m[i] = 0.0;
            m[i + 1] = 0.0;
        } else {
            let a = m[i] / d[i];
            let b = m[i + 1] / d[i];
            let s = a * a + b * b;
            if s > 9.0 {
                let t = 3.0 / s.sqrt();
                m[i] = t * a * d[i];
                m[i + 1] = t * b * d[i];
            }
        }
    }
    std::array::from_fn(|i| {
        let x = i as f32;
        let v = if x <= pts[0].0 {
            pts[0].1
        } else if x >= pts[n - 1].0 {
            pts[n - 1].1
        } else {
            let k = pts.windows(2).position(|w| x >= w[0].0 && x <= w[1].0).unwrap_or(0);
            let (x0, y0) = pts[k];
            let (x1, y1) = pts[k + 1];
            let h = x1 - x0;
            let t = (x - x0) / h;
            let (t2, t3) = (t * t, t * t * t);
            (2.0 * t3 - 3.0 * t2 + 1.0) * y0
                + (t3 - 2.0 * t2 + t) * h * m[k]
                + (-2.0 * t3 + 3.0 * t2) * y1
                + (t3 - t2) * h * m[k + 1]
        };
        v.round().clamp(0.0, 255.0) as u8
    })
}

/// Histogram of R, G, B and luminosity for pixels in `rect` (alpha-weighted
/// pixels with zero alpha are ignored).
pub fn histogram(buf: &TiledBuffer<Rgba8>, rect: IRect) -> [[u32; 256]; 4] {
    use rayon::prelude::*;
    let rect = rect.clamp_to(buf.width(), buf.height());
    (rect.y0..rect.y1)
        .into_par_iter()
        .fold(
            || [[0u32; 256]; 4],
            |mut h, y| {
                for x in rect.x0..rect.x1 {
                    let p = buf.get(x, y);
                    if p[3] == 0 {
                        continue;
                    }
                    h[0][p[0] as usize] += 1;
                    h[1][p[1] as usize] += 1;
                    h[2][p[2] as usize] += 1;
                    let l = (0.3 * p[0] as f32 + 0.59 * p[1] as f32 + 0.11 * p[2] as f32) as usize;
                    h[3][l.min(255)] += 1;
                }
                h
            },
        )
        .reduce(
            || [[0u32; 256]; 4],
            |mut a, b| {
                for c in 0..4 {
                    for i in 0..256 {
                        a[c][i] += b[c][i];
                    }
                }
                a
            },
        )
}

/// "Auto Contrast": stretches each channel, clipping 0.1% at both ends.
pub fn auto_levels(buf: &TiledBuffer<Rgba8>, rect: IRect) -> Vec<Adjustment> {
    let h = histogram(buf, rect);
    let mut out = Vec::new();
    for (ci, channel) in [Channel::Red, Channel::Green, Channel::Blue].into_iter().enumerate() {
        let total: u32 = h[ci].iter().sum();
        if total == 0 {
            continue;
        }
        let clip = (total as f32 * 0.001) as u32;
        let mut acc = 0;
        let lo = h[ci].iter().position(|&n| {
            acc += n;
            acc > clip
        });
        acc = 0;
        let hi = h[ci].iter().rposition(|&n| {
            acc += n;
            acc > clip
        });
        if let (Some(lo), Some(hi)) = (lo, hi) {
            if hi > lo {
                out.push(Adjustment::Levels {
                    channel,
                    levels: Levels { in_black: lo as f32, in_white: hi as f32, ..Default::default() },
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_identity_and_shape() {
        let id = curve_lut(&[(0.0, 0.0), (255.0, 255.0)]);
        assert!(id.iter().enumerate().all(|(i, &v)| v as usize == i));
        let s = curve_lut(&[(0.0, 0.0), (64.0, 40.0), (192.0, 220.0), (255.0, 255.0)]);
        assert!(s.windows(2).all(|w| w[1] >= w[0]), "monotone");
        assert!(s[64] == 40 && s[192] == 220);
    }

    #[test]
    fn invert_and_levels() {
        let f = Adjustment::Invert.compile();
        assert_eq!(f([0, 128, 255, 77]), [255, 127, 0, 77]);
        let lv = Adjustment::Levels {
            channel: Channel::Rgb,
            levels: Levels { in_black: 50.0, in_white: 200.0, ..Default::default() },
        }
        .compile();
        assert_eq!(lv([50, 200, 125, 255]), [0, 255, 128, 255]);
    }

    #[test]
    fn hue_sat_neutral_is_identity() {
        let f = Adjustment::HueSaturation { hue: 0.0, saturation: 0.0, lightness: 0.0, colorize: false }.compile();
        for p in [[10, 200, 30, 255], [255, 0, 0, 255], [128, 128, 128, 255]] {
            let o = f(p);
            for i in 0..3 {
                assert!((o[i] as i32 - p[i] as i32).abs() <= 1, "{p:?} -> {o:?}");
            }
        }
    }

    #[test]
    fn auto_levels_stretches() {
        let mut b = TiledBuffer::new(10, 10);
        b.par_map_all(|x, _, _| {
            let v = 100 + x as u8 * 5;
            [v, v, v, 255]
        });
        let adj = auto_levels(&b, b.bounds());
        assert_eq!(adj.len(), 3);
        let mut t = b.clone();
        for a in &adj {
            let base = t.clone();
            apply(&mut t, &base, None, a);
        }
        assert_eq!(t.get(0, 0)[0], 0);
        assert_eq!(t.get(9, 0)[0], 255);
    }
}
