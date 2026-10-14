//! Photoshop blend modes, implemented after the W3C Compositing and
//! Blending Level 1 specification (which itself documents Photoshop's
//! formulas), plus the Photoshop-only modes (Vivid/Linear/Pin Light,
//! Hard Mix, Darker/Lighter Color, Subtract, Divide, Dissolve).

use serde::{Deserialize, Serialize};

use crate::color::lum;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BlendMode {
    #[default]
    Normal,
    Dissolve,
    Darken,
    Multiply,
    ColorBurn,
    LinearBurn,
    DarkerColor,
    Lighten,
    Screen,
    ColorDodge,
    LinearDodge,
    LighterColor,
    Overlay,
    SoftLight,
    HardLight,
    VividLight,
    LinearLight,
    PinLight,
    HardMix,
    Difference,
    Exclusion,
    Subtract,
    Divide,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl BlendMode {
    /// All modes in Photoshop's menu order.
    pub const ALL: [BlendMode; 27] = [
        BlendMode::Normal,
        BlendMode::Dissolve,
        BlendMode::Darken,
        BlendMode::Multiply,
        BlendMode::ColorBurn,
        BlendMode::LinearBurn,
        BlendMode::DarkerColor,
        BlendMode::Lighten,
        BlendMode::Screen,
        BlendMode::ColorDodge,
        BlendMode::LinearDodge,
        BlendMode::LighterColor,
        BlendMode::Overlay,
        BlendMode::SoftLight,
        BlendMode::HardLight,
        BlendMode::VividLight,
        BlendMode::LinearLight,
        BlendMode::PinLight,
        BlendMode::HardMix,
        BlendMode::Difference,
        BlendMode::Exclusion,
        BlendMode::Subtract,
        BlendMode::Divide,
        BlendMode::Hue,
        BlendMode::Saturation,
        BlendMode::Color,
        BlendMode::Luminosity,
    ];

    pub fn name(self) -> &'static str {
        use BlendMode::*;
        match self {
            Normal => "Normal",
            Dissolve => "Dissolve",
            Darken => "Darken",
            Multiply => "Multiply",
            ColorBurn => "Color Burn",
            LinearBurn => "Linear Burn",
            DarkerColor => "Darker Color",
            Lighten => "Lighten",
            Screen => "Screen",
            ColorDodge => "Color Dodge",
            LinearDodge => "Linear Dodge (Add)",
            LighterColor => "Lighter Color",
            Overlay => "Overlay",
            SoftLight => "Soft Light",
            HardLight => "Hard Light",
            VividLight => "Vivid Light",
            LinearLight => "Linear Light",
            PinLight => "Pin Light",
            HardMix => "Hard Mix",
            Difference => "Difference",
            Exclusion => "Exclusion",
            Subtract => "Subtract",
            Divide => "Divide",
            Hue => "Hue",
            Saturation => "Saturation",
            Color => "Color",
            Luminosity => "Luminosity",
        }
    }

    /// Menu group index; the UI draws separators between groups like Photoshop.
    pub fn group(self) -> u8 {
        use BlendMode::*;
        match self {
            Normal | Dissolve => 0,
            Darken | Multiply | ColorBurn | LinearBurn | DarkerColor => 1,
            Lighten | Screen | ColorDodge | LinearDodge | LighterColor => 2,
            Overlay | SoftLight | HardLight | VividLight | LinearLight | PinLight | HardMix => 3,
            Difference | Exclusion | Subtract | Divide => 4,
            Hue | Saturation | Color | Luminosity => 5,
        }
    }

    /// OpenRaster `composite-op` name. Modes that OpenRaster lacks use a
    /// `forge:` prefix so they round-trip through our own files.
    pub fn ora_name(self) -> &'static str {
        use BlendMode::*;
        match self {
            Normal => "svg:src-over",
            Dissolve => "forge:dissolve",
            Darken => "svg:darken",
            Multiply => "svg:multiply",
            ColorBurn => "svg:color-burn",
            LinearBurn => "forge:linear-burn",
            DarkerColor => "forge:darker-color",
            Lighten => "svg:lighten",
            Screen => "svg:screen",
            ColorDodge => "svg:color-dodge",
            LinearDodge => "svg:plus",
            LighterColor => "forge:lighter-color",
            Overlay => "svg:overlay",
            SoftLight => "svg:soft-light",
            HardLight => "svg:hard-light",
            VividLight => "forge:vivid-light",
            LinearLight => "forge:linear-light",
            PinLight => "forge:pin-light",
            HardMix => "forge:hard-mix",
            Difference => "svg:difference",
            Exclusion => "svg:exclusion",
            Subtract => "forge:subtract",
            Divide => "forge:divide",
            Hue => "svg:hue",
            Saturation => "svg:saturation",
            Color => "svg:color",
            Luminosity => "svg:luminosity",
        }
    }

    pub fn from_ora_name(s: &str) -> Option<BlendMode> {
        BlendMode::ALL.into_iter().find(|m| m.ora_name() == s)
    }

    fn is_separable(self) -> bool {
        !matches!(
            self,
            BlendMode::Hue
                | BlendMode::Saturation
                | BlendMode::Color
                | BlendMode::Luminosity
                | BlendMode::DarkerColor
                | BlendMode::LighterColor
        )
    }
}

#[inline]
fn color_burn(cb: f32, cs: f32) -> f32 {
    if cb >= 1.0 {
        1.0
    } else if cs <= 0.0 {
        0.0
    } else {
        1.0 - ((1.0 - cb) / cs).min(1.0)
    }
}

#[inline]
fn color_dodge(cb: f32, cs: f32) -> f32 {
    if cb <= 0.0 {
        0.0
    } else if cs >= 1.0 {
        1.0
    } else {
        (cb / (1.0 - cs)).min(1.0)
    }
}

#[inline]
fn hard_light(cb: f32, cs: f32) -> f32 {
    if cs <= 0.5 {
        cb * 2.0 * cs
    } else {
        let s = 2.0 * cs - 1.0;
        cb + s - cb * s
    }
}

#[inline]
fn soft_light(cb: f32, cs: f32) -> f32 {
    if cs <= 0.5 {
        cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb)
    } else {
        let d = if cb <= 0.25 {
            ((16.0 * cb - 12.0) * cb + 4.0) * cb
        } else {
            cb.sqrt()
        };
        cb + (2.0 * cs - 1.0) * (d - cb)
    }
}

#[inline]
fn separable(mode: BlendMode, cb: f32, cs: f32) -> f32 {
    use BlendMode::*;
    match mode {
        Normal | Dissolve => cs,
        Darken => cb.min(cs),
        Multiply => cb * cs,
        ColorBurn => color_burn(cb, cs),
        LinearBurn => (cb + cs - 1.0).max(0.0),
        Lighten => cb.max(cs),
        Screen => cb + cs - cb * cs,
        ColorDodge => color_dodge(cb, cs),
        LinearDodge => (cb + cs).min(1.0),
        Overlay => hard_light(cs, cb),
        SoftLight => soft_light(cb, cs),
        HardLight => hard_light(cb, cs),
        VividLight => {
            if cs <= 0.5 {
                color_burn(cb, 2.0 * cs)
            } else {
                color_dodge(cb, 2.0 * (cs - 0.5))
            }
        }
        LinearLight => (cb + 2.0 * cs - 1.0).clamp(0.0, 1.0),
        PinLight => {
            if cs <= 0.5 {
                cb.min(2.0 * cs)
            } else {
                cb.max(2.0 * cs - 1.0)
            }
        }
        HardMix => {
            if cb + cs >= 1.0 {
                1.0
            } else {
                0.0
            }
        }
        Difference => (cb - cs).abs(),
        Exclusion => cb + cs - 2.0 * cb * cs,
        Subtract => (cb - cs).max(0.0),
        Divide => {
            if cs <= 0.0 {
                if cb <= 0.0 {
                    0.0
                } else {
                    1.0
                }
            } else {
                (cb / cs).min(1.0)
            }
        }
        Hue | Saturation | Color | Luminosity | DarkerColor | LighterColor => unreachable!(),
    }
}

fn clip_color(c: [f32; 3]) -> [f32; 3] {
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut out = c;
    if n < 0.0 {
        let d = l - n;
        for v in &mut out {
            *v = l + (*v - l) * l / d;
        }
    }
    if x > 1.0 {
        let d = x - l;
        for v in &mut out {
            *v = l + (*v - l) * (1.0 - l) / d;
        }
    }
    out
}

fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    clip_color([c[0] + d, c[1] + d, c[2] + d])
}

fn sat(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    let mut idx = [0usize, 1, 2];
    idx.sort_by(|&a, &b| c[a].total_cmp(&c[b]));
    let (min, mid, max) = (idx[0], idx[1], idx[2]);
    let mut out = [0.0; 3];
    if c[max] > c[min] {
        out[mid] = (c[mid] - c[min]) * s / (c[max] - c[min]);
        out[max] = s;
    }
    out
}

/// The blend function `B(Cb, Cs)` on straight colours.
#[inline]
pub fn blend_rgb(mode: BlendMode, cb: [f32; 3], cs: [f32; 3]) -> [f32; 3] {
    use BlendMode::*;
    if mode.is_separable() {
        return [
            separable(mode, cb[0], cs[0]),
            separable(mode, cb[1], cs[1]),
            separable(mode, cb[2], cs[2]),
        ];
    }
    match mode {
        Hue => set_lum(set_sat(cs, sat(cb)), lum(cb)),
        Saturation => set_lum(set_sat(cb, sat(cs)), lum(cb)),
        Color => set_lum(cs, lum(cb)),
        Luminosity => set_lum(cb, lum(cs)),
        DarkerColor => {
            if lum(cs) < lum(cb) {
                cs
            } else {
                cb
            }
        }
        LighterColor => {
            if lum(cs) > lum(cb) {
                cs
            } else {
                cb
            }
        }
        _ => unreachable!(),
    }
}

/// Cheap deterministic per-pixel hash for Dissolve, uniform in `0..1`.
#[inline]
pub fn pixel_noise(x: i32, y: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    (h & 0xFF_FFFF) as f32 / 16_777_216.0
}

/// Composites a straight-alpha source over a **premultiplied** destination
/// accumulator using `mode`. `sa` already includes layer opacity and masks.
#[inline]
pub fn composite(dst: &mut [f32; 4], cs: [f32; 3], mut sa: f32, mode: BlendMode, x: i32, y: i32) {
    if mode == BlendMode::Dissolve {
        sa = if pixel_noise(x, y) < sa { 1.0 } else { 0.0 };
    }
    if sa <= 0.0 {
        return;
    }
    let ab = dst[3];
    let inv = 1.0 - sa;
    if ab <= 0.0 || matches!(mode, BlendMode::Normal | BlendMode::Dissolve) {
        dst[0] = cs[0] * sa + dst[0] * inv;
        dst[1] = cs[1] * sa + dst[1] * inv;
        dst[2] = cs[2] * sa + dst[2] * inv;
        dst[3] = sa + ab * inv;
        return;
    }
    let cb = [dst[0] / ab, dst[1] / ab, dst[2] / ab];
    let b = blend_rgb(mode, cb, cs);
    let s_only = sa * (1.0 - ab);
    let both = sa * ab;
    for i in 0..3 {
        dst[i] = s_only * cs[i] + both * b[i] + inv * dst[i];
    }
    dst[3] = sa + ab * inv;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ora_names_roundtrip() {
        for m in BlendMode::ALL {
            assert_eq!(BlendMode::from_ora_name(m.ora_name()), Some(m));
        }
    }

    #[test]
    fn classic_formulas() {
        let cb = [0.5, 0.2, 1.0];
        let cs = [0.5, 0.5, 0.5];
        let approx = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5);
        assert!(approx(blend_rgb(BlendMode::Multiply, cb, cs), [0.25, 0.1, 0.5]));
        assert!(approx(blend_rgb(BlendMode::Screen, cb, cs), [0.75, 0.6, 1.0]));
        assert!(approx(blend_rgb(BlendMode::Difference, cb, cs), [0.0, 0.3, 0.5]));
        // Luminosity keeps the backdrop's hue/sat and takes the source's luma.
        let out = blend_rgb(BlendMode::Luminosity, [1.0, 0.0, 0.0], [0.5, 0.5, 0.5]);
        assert!((lum(out) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn normal_over_opaque() {
        let mut d = [1.0, 1.0, 1.0, 1.0];
        composite(&mut d, [0.0, 0.0, 0.0], 0.5, BlendMode::Normal, 0, 0);
        assert_eq!(d, [0.5, 0.5, 0.5, 1.0]);
        let mut d = [0.0; 4];
        composite(&mut d, [1.0, 0.0, 0.0], 0.25, BlendMode::Multiply, 0, 0);
        assert_eq!(d, [0.25, 0.0, 0.0, 0.25], "on transparent backdrop every mode is Normal");
    }

    #[test]
    fn all_modes_stay_in_range() {
        let vals = [0.0, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0];
        for m in BlendMode::ALL {
            for &a in &vals {
                for &b in &vals {
                    let o = blend_rgb(m, [a, b, a * b], [b, a, 1.0 - a]);
                    for v in o {
                        assert!((-1e-5..=1.0 + 1e-5).contains(&v), "{m:?} {a} {b} -> {o:?}");
                    }
                }
            }
        }
    }
}
