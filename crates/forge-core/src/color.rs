//! Colour types and colour-space conversions.

use serde::{Deserialize, Serialize};

/// Storage pixel: straight (non-premultiplied) 8-bit RGBA.
pub type Rgba8 = [u8; 4];

pub const TRANSPARENT: Rgba8 = [0, 0, 0, 0];

/// Floating point straight-alpha colour, components in `0.0..=1.0`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Default for Color {
    fn default() -> Self {
        Self::BLACK
    }
}

impl Color {
    pub const BLACK: Color = Color::rgb(0.0, 0.0, 0.0);
    pub const WHITE: Color = Color::rgb(1.0, 1.0, 1.0);
    pub const TRANSPARENT: Color = Color::rgba(0.0, 0.0, 0.0, 0.0);

    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub fn from_rgba8(p: Rgba8) -> Self {
        Self::rgba(
            p[0] as f32 / 255.0,
            p[1] as f32 / 255.0,
            p[2] as f32 / 255.0,
            p[3] as f32 / 255.0,
        )
    }

    pub fn to_rgba8(self) -> Rgba8 {
        [to_u8(self.r), to_u8(self.g), to_u8(self.b), to_u8(self.a)]
    }

    pub fn rgb_array(self) -> [f32; 3] {
        [self.r, self.g, self.b]
    }

    /// Parses `#rgb`, `#rrggbb` or `#rrggbbaa` (leading `#` optional).
    pub fn from_hex(s: &str) -> Option<Color> {
        let s = s.trim().trim_start_matches('#');
        let byte = |i: usize| u8::from_str_radix(s.get(i..i + 2)?, 16).ok();
        match s.len() {
            3 => {
                let mut c = [0u8; 4];
                for (i, ch) in s.chars().enumerate() {
                    let v = ch.to_digit(16)? as u8;
                    c[i] = v * 17;
                }
                c[3] = 255;
                Some(Color::from_rgba8(c))
            }
            6 => Some(Color::from_rgba8([byte(0)?, byte(2)?, byte(4)?, 255])),
            8 => Some(Color::from_rgba8([byte(0)?, byte(2)?, byte(4)?, byte(6)?])),
            _ => None,
        }
    }

    pub fn to_hex(self) -> String {
        let [r, g, b, a] = self.to_rgba8();
        if a == 255 {
            format!("#{r:02x}{g:02x}{b:02x}")
        } else {
            format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
        }
    }

    pub fn with_alpha(self, a: f32) -> Color {
        Color { a, ..self }
    }

    pub fn lerp(self, o: Color, t: f32) -> Color {
        Color::rgba(
            self.r + (o.r - self.r) * t,
            self.g + (o.g - self.g) * t,
            self.b + (o.b - self.b) * t,
            self.a + (o.a - self.a) * t,
        )
    }
}

#[inline]
pub fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Rec. 601 luma, as used by Photoshop's non-separable blend modes.
#[inline]
pub fn lum(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

/// RGB → HSL, hue in degrees `0..360`, s/l in `0..1`.
pub fn rgb_to_hsl(c: [f32; 3]) -> [f32; 3] {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let l = (max + min) * 0.5;
    let d = max - min;
    if d <= f32::EPSILON {
        return [0.0, 0.0, l];
    }
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == c[0] {
        (c[1] - c[2]) / d + if c[1] < c[2] { 6.0 } else { 0.0 }
    } else if max == c[1] {
        (c[2] - c[0]) / d + 2.0
    } else {
        (c[0] - c[1]) / d + 4.0
    };
    [h * 60.0, s, l]
}

pub fn hsl_to_rgb(hsl: [f32; 3]) -> [f32; 3] {
    let [h, s, l] = hsl;
    if s <= 0.0 {
        return [l, l, l];
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let h = h.rem_euclid(360.0) / 360.0;
    let f = |t: f32| {
        let t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    [f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0)]
}

/// RGB → HSV, hue in degrees.
pub fn rgb_to_hsv(c: [f32; 3]) -> [f32; 3] {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let d = max - min;
    let s = if max > 0.0 { d / max } else { 0.0 };
    let h = if d <= f32::EPSILON {
        0.0
    } else if max == c[0] {
        60.0 * ((c[1] - c[2]) / d).rem_euclid(6.0)
    } else if max == c[1] {
        60.0 * ((c[2] - c[0]) / d + 2.0)
    } else {
        60.0 * ((c[0] - c[1]) / d + 4.0)
    };
    [h, s, max]
}

pub fn hsv_to_rgb(hsv: [f32; 3]) -> [f32; 3] {
    let [h, s, v] = hsv;
    let c = v * s;
    let hp = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r + m, g + m, b + m]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn hex_roundtrip() {
        let c = Color::from_hex("#ff8000").unwrap();
        assert_eq!(c.to_rgba8(), [255, 128, 0, 255]);
        assert_eq!(c.to_hex(), "#ff8000");
        assert_eq!(Color::from_hex("fff").unwrap().to_rgba8(), [255; 4]);
        assert!(Color::from_hex("xyz").is_none());
    }

    #[test]
    fn hsl_hsv_roundtrip() {
        for c in [[0.2, 0.4, 0.9], [1.0, 0.0, 0.0], [0.5, 0.5, 0.5], [0.1, 0.9, 0.3]] {
            assert!(close(hsl_to_rgb(rgb_to_hsl(c)), c), "hsl {c:?}");
            assert!(close(hsv_to_rgb(rgb_to_hsv(c)), c), "hsv {c:?}");
        }
    }
}
