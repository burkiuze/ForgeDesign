//! Spatial filters (Filter menu). They operate on a dense, premultiplied
//! `f32` copy of the affected region and run row-parallel with rayon.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::buffer::TiledBuffer;
use crate::color::{Color, Rgba8};
use crate::geom::IRect;
use crate::selection::{self, coverage, Mask};

pub type Px = [f32; 4];

/// Dense premultiplied RGBA working image.
#[derive(Clone)]
pub struct FImage {
    pub w: usize,
    pub h: usize,
    pub data: Vec<Px>,
}

impl FImage {
    pub fn from_buffer(buf: &TiledBuffer<Rgba8>, rect: IRect) -> Self {
        let raw = buf.region_to_dense(rect);
        let data = raw
            .par_iter()
            .map(|p| {
                let a = p[3] as f32 / 255.0;
                [p[0] as f32 / 255.0 * a, p[1] as f32 / 255.0 * a, p[2] as f32 / 255.0 * a, a]
            })
            .collect();
        Self { w: rect.width() as usize, h: rect.height() as usize, data }
    }

    #[inline]
    pub fn get(&self, x: isize, y: isize) -> Px {
        let x = x.clamp(0, self.w as isize - 1) as usize;
        let y = y.clamp(0, self.h as isize - 1) as usize;
        self.data[y * self.w + x]
    }

    /// Bilinear sample with edge clamping.
    #[inline]
    pub fn sample(&self, x: f32, y: f32) -> Px {
        let x = x - 0.5;
        let y = y - 0.5;
        let x0 = x.floor();
        let y0 = y.floor();
        let fx = x - x0;
        let fy = y - y0;
        let (xi, yi) = (x0 as isize, y0 as isize);
        let a = self.get(xi, yi);
        let b = self.get(xi + 1, yi);
        let c = self.get(xi, yi + 1);
        let d = self.get(xi + 1, yi + 1);
        let mut o = [0.0; 4];
        for i in 0..4 {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bot = c[i] + (d[i] - c[i]) * fx;
            o[i] = top + (bot - top) * fy;
        }
        o
    }
}

#[inline]
fn to_straight(p: Px) -> Rgba8 {
    if p[3] <= 1e-6 {
        return [0, 0, 0, 0];
    }
    let inv = 1.0 / p[3];
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    [q(p[0] * inv), q(p[1] * inv), q(p[2] * inv), q(p[3])]
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Filter {
    GaussianBlur { radius: f32 },
    BoxBlur { radius: u32 },
    MotionBlur { angle: f32, distance: f32 },
    UnsharpMask { amount: f32, radius: f32, threshold: u8 },
    Sharpen,
    HighPass { radius: f32 },
    Median { radius: u32 },
    AddNoise { amount: f32, monochrome: bool, seed: u32 },
    Pixelate { size: u32 },
    Emboss { angle: f32, amount: f32 },
    FindEdges,
    Twirl { angle: f32 },
    Wave { amplitude: f32, wavelength: f32 },
    Clouds { fg: Color, bg: Color, scale: f32, seed: u32 },
    /// Spin (`zoom == false`) or zoom blur around the image centre.
    RadialBlur { amount: f32, zoom: bool },
    /// Bokeh-like disc blur.
    LensBlur { radius: f32 },
    /// Edge-preserving (bilateral) blur.
    SurfaceBlur { radius: u32, threshold: f32 },
    /// Sharp band at `center` (0..1 of height), blurring away from it.
    TiltShift { center: f32, band: f32, blur: f32 },
    SmartSharpen { amount: f32, radius: f32, noise: f32 },
    SharpenEdges,
    ReduceNoise { strength: f32 },
    DustAndScratches { radius: u32, threshold: u8 },
    OilPaint { radius: u32 },
    Ripple { amount: f32, size: f32 },
    /// Positive bulges outwards, negative pinches.
    Spherize { amount: f32 },
    PolarCoordinates { to_polar: bool },
    LensCorrection { distortion: f32, vignette: f32, chromatic: f32 },
    ShadowsHighlights { shadows: f32, highlights: f32, radius: f32 },
    HdrToning { strength: f32, radius: f32, saturation: f32 },
    Offset { dx: i32, dy: i32 },
    Displace { amount: f32, scale: f32, seed: u32 },
}

impl Filter {
    /// How many pixels around the work area the filter reads.
    fn padding(&self) -> i32 {
        match self {
            Filter::GaussianBlur { radius } | Filter::HighPass { radius } => (radius * 3.0).ceil() as i32 + 1,
            Filter::UnsharpMask { radius, .. } => (radius * 3.0).ceil() as i32 + 1,
            Filter::BoxBlur { radius } | Filter::Median { radius } => *radius as i32 + 1,
            Filter::MotionBlur { distance, .. } => distance.ceil() as i32 + 1,
            Filter::Sharpen | Filter::Emboss { .. } | Filter::FindEdges => 2,
            Filter::Wave { amplitude, .. } => amplitude.ceil() as i32 + 2,
            Filter::Pixelate { .. } | Filter::Twirl { .. } | Filter::AddNoise { .. } | Filter::Clouds { .. } => 0,
            Filter::RadialBlur { .. } | Filter::Spherize { .. } | Filter::PolarCoordinates { .. } | Filter::Offset { .. } => 0,
            Filter::LensCorrection { .. } => 0,
            Filter::LensBlur { radius } => radius.ceil() as i32 + 1,
            Filter::SurfaceBlur { radius, .. } | Filter::OilPaint { radius } | Filter::DustAndScratches { radius, .. } => {
                *radius as i32 + 1
            }
            Filter::TiltShift { blur, .. } => (blur * 3.0).ceil() as i32 + 1,
            Filter::SmartSharpen { radius, .. } => (radius * 3.0).ceil() as i32 + 1,
            Filter::SharpenEdges | Filter::ReduceNoise { .. } => 4,
            Filter::Ripple { amount, .. } => amount.ceil() as i32 + 2,
            Filter::ShadowsHighlights { radius, .. } | Filter::HdrToning { radius, .. } => (radius * 3.0).ceil() as i32 + 1,
            Filter::Displace { amount, .. } => amount.ceil() as i32 + 2,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Filter::GaussianBlur { .. } => "Gaussian Blur",
            Filter::BoxBlur { .. } => "Box Blur",
            Filter::MotionBlur { .. } => "Motion Blur",
            Filter::UnsharpMask { .. } => "Unsharp Mask",
            Filter::Sharpen => "Sharpen",
            Filter::HighPass { .. } => "High Pass",
            Filter::Median { .. } => "Median",
            Filter::AddNoise { .. } => "Add Noise",
            Filter::Pixelate { .. } => "Mosaic",
            Filter::Emboss { .. } => "Emboss",
            Filter::FindEdges => "Find Edges",
            Filter::Twirl { .. } => "Twirl",
            Filter::Wave { .. } => "Wave",
            Filter::Clouds { .. } => "Clouds",
            Filter::RadialBlur { .. } => "Radial Blur",
            Filter::LensBlur { .. } => "Lens Blur",
            Filter::SurfaceBlur { .. } => "Surface Blur",
            Filter::TiltShift { .. } => "Tilt-Shift",
            Filter::SmartSharpen { .. } => "Smart Sharpen",
            Filter::SharpenEdges => "Sharpen Edges",
            Filter::ReduceNoise { .. } => "Reduce Noise",
            Filter::DustAndScratches { .. } => "Dust & Scratches",
            Filter::OilPaint { .. } => "Oil Paint",
            Filter::Ripple { .. } => "Ripple",
            Filter::Spherize { .. } => "Spherize",
            Filter::PolarCoordinates { .. } => "Polar Coordinates",
            Filter::LensCorrection { .. } => "Lens Correction",
            Filter::ShadowsHighlights { .. } => "Shadows/Highlights",
            Filter::HdrToning { .. } => "HDR Toning",
            Filter::Offset { .. } => "Offset",
            Filter::Displace { .. } => "Displace",
        }
    }

    /// Runs the filter on `img`. `origin` is the image-space position of
    /// `img`'s top-left pixel (for position-dependent filters).
    pub fn run(&self, img: &FImage, origin: (i32, i32), full: (u32, u32)) -> FImage {
        match *self {
            Filter::GaussianBlur { radius } => {
                let mut o = img.clone();
                gaussian_blur(&mut o.data, o.w, o.h, radius);
                o
            }
            Filter::BoxBlur { radius } => {
                let mut o = img.clone();
                box_blur(&mut o.data, o.w, o.h, radius as usize);
                o
            }
            Filter::MotionBlur { angle, distance } => motion_blur(img, angle, distance),
            Filter::UnsharpMask { amount, radius, threshold } => {
                let mut blurred = img.data.clone();
                gaussian_blur(&mut blurred, img.w, img.h, radius);
                let th = threshold as f32 / 255.0;
                let data = img
                    .data
                    .par_iter()
                    .zip(&blurred)
                    .map(|(o, b)| {
                        let mut p = *o;
                        for i in 0..3 {
                            let d = o[i] - b[i];
                            if d.abs() >= th {
                                p[i] = (o[i] + d * amount).clamp(0.0, o[3]);
                            }
                        }
                        p
                    })
                    .collect();
                FImage { data, ..*img }
            }
            Filter::Sharpen => convolve3(img, [0.0, -0.25, 0.0, -0.25, 2.0, -0.25, 0.0, -0.25, 0.0], 0.0, false),
            Filter::HighPass { radius } => {
                let mut blurred = img.data.clone();
                gaussian_blur(&mut blurred, img.w, img.h, radius);
                let data = img
                    .data
                    .par_iter()
                    .zip(&blurred)
                    .map(|(o, b)| {
                        let a = o[3];
                        let mut p = [0.0, 0.0, 0.0, a];
                        for i in 0..3 {
                            p[i] = ((o[i] - b[i]) + 0.5 * a).clamp(0.0, a);
                        }
                        p
                    })
                    .collect();
                FImage { data, ..*img }
            }
            Filter::Median { radius } => median(img, radius as usize),
            Filter::AddNoise { amount, monochrome, seed } => {
                let (ox, oy) = origin;
                let mut o = img.clone();
                o.data.par_chunks_mut(img.w).enumerate().for_each(|(y, row)| {
                    for (x, p) in row.iter_mut().enumerate() {
                        let (gx, gy) = (ox + x as i32, oy + y as i32);
                        for i in 0..3 {
                            let ch = if monochrome { 0 } else { i as u32 };
                            let n = (noise(gx, gy, seed.wrapping_add(ch * 7919)) - 0.5) * 2.0 * amount;
                            p[i] = (p[i] + n * p[3]).clamp(0.0, p[3]);
                        }
                    }
                });
                o
            }
            Filter::Pixelate { size } => pixelate(img, origin, size.max(1) as i32),
            Filter::Emboss { angle, amount } => {
                let (s, c) = angle.to_radians().sin_cos();
                let k = [-c - s, -s, c - s, -c, 0.0, c, -c + s, s, c + s];
                let k = k.map(|v| v * amount * 0.5);
                convolve3(img, k, 0.5, true)
            }
            Filter::FindEdges => find_edges(img),
            Filter::Twirl { angle } => {
                let (fw, fh) = (full.0 as f32, full.1 as f32);
                let (cx, cy) = (fw * 0.5 - origin.0 as f32, fh * 0.5 - origin.1 as f32);
                let radius = fw.min(fh) * 0.5;
                let ang = angle.to_radians();
                warp(img, |x, y| {
                    let (dx, dy) = (x - cx, y - cy);
                    let r = (dx * dx + dy * dy).sqrt();
                    if r >= radius {
                        return (x, y);
                    }
                    let t = 1.0 - r / radius;
                    let a = ang * t * t;
                    let (s, c) = a.sin_cos();
                    (cx + dx * c - dy * s, cy + dx * s + dy * c)
                })
            }
            Filter::Wave { amplitude, wavelength } => {
                let wl = wavelength.max(1.0);
                let (ox, oy) = (origin.0 as f32, origin.1 as f32);
                warp(img, |x, y| {
                    let tau = std::f32::consts::TAU;
                    (
                        x + amplitude * ((y + oy) * tau / wl).sin(),
                        y + amplitude * ((x + ox) * tau / wl).sin(),
                    )
                })
            }
            Filter::RadialBlur { amount, zoom } => {
                let (cx, cy) = (full.0 as f32 * 0.5 - origin.0 as f32, full.1 as f32 * 0.5 - origin.1 as f32);
                let n = 24;
                let mut out = img.clone();
                out.data.par_chunks_mut(img.w).enumerate().for_each(|(y, row)| {
                    for (x, p) in row.iter_mut().enumerate() {
                        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                        let mut acc = [0.0; 4];
                        for i in 0..n {
                            let t = (i as f32 / (n - 1) as f32 - 0.5) * amount / 100.0;
                            let (sx, sy) = if zoom {
                                (cx + (px - cx) * (1.0 + t * 0.5), cy + (py - cy) * (1.0 + t * 0.5))
                            } else {
                                let (s, c) = (t * 0.5).sin_cos();
                                (cx + (px - cx) * c - (py - cy) * s, cy + (px - cx) * s + (py - cy) * c)
                            };
                            let v = img.sample(sx, sy);
                            for k in 0..4 {
                                acc[k] += v[k];
                            }
                        }
                        *p = acc.map(|v| v / n as f32);
                    }
                });
                out
            }
            Filter::LensBlur { radius } => {
                let n = ((radius * radius * std::f32::consts::PI) as usize).clamp(8, 160);
                let golden = std::f32::consts::PI * (3.0 - 5f32.sqrt());
                let offsets: Vec<(f32, f32)> = (0..n)
                    .map(|i| {
                        let r = radius * ((i as f32 + 0.5) / n as f32).sqrt();
                        let a = i as f32 * golden;
                        (r * a.cos(), r * a.sin())
                    })
                    .collect();
                let mut out = img.clone();
                out.data.par_chunks_mut(img.w).enumerate().for_each(|(y, row)| {
                    for (x, p) in row.iter_mut().enumerate() {
                        let mut acc = [0.0; 4];
                        let mut wsum = 0.0;
                        for &(dx, dy) in &offsets {
                            let v = img.get(x as isize + dx.round() as isize, y as isize + dy.round() as isize);
                            // Bright spots weigh more: bokeh highlights.
                            let w = 1.0 + crate::color::lum([v[0], v[1], v[2]]).powi(4) * 4.0;
                            for k in 0..4 {
                                acc[k] += v[k] * w;
                            }
                            wsum += w;
                        }
                        *p = acc.map(|v| v / wsum);
                    }
                });
                out
            }
            Filter::SurfaceBlur { radius, threshold } => bilateral(img, radius as isize, threshold / 255.0),
            Filter::ReduceNoise { strength } => bilateral(img, 3, (strength * 0.25).max(0.01)),
            Filter::TiltShift { center, band, blur } => {
                let mut blurred = img.data.clone();
                gaussian_blur(&mut blurred, img.w, img.h, blur);
                let fh = full.1 as f32;
                let cy = center * fh;
                let half = band * fh * 0.5;
                let data = blurred
                    .par_iter()
                    .zip(&img.data)
                    .enumerate()
                    .map(|(i, (b, o))| {
                        let gy = origin.1 as f32 + (i / img.w) as f32;
                        let d = ((gy - cy).abs() - half).max(0.0) / (fh * 0.15).max(1.0);
                        let t = d.clamp(0.0, 1.0);
                        let t = t * t * (3.0 - 2.0 * t);
                        [0, 1, 2, 3].map(|k| o[k] + (b[k] - o[k]) * t)
                    })
                    .collect();
                FImage { data, ..*img }
            }
            Filter::SmartSharpen { amount, radius, noise } => {
                Filter::UnsharpMask { amount, radius, threshold: (noise * 2.55) as u8 }.run(img, origin, full)
            }
            Filter::SharpenEdges => {
                let edges = find_edges(img);
                let sharp = Filter::UnsharpMask { amount: 1.2, radius: 1.0, threshold: 0 }.run(img, origin, full);
                let data = img
                    .data
                    .par_iter()
                    .zip(&sharp.data)
                    .zip(&edges.data)
                    .map(|((o, s), e)| {
                        // find_edges gives white = flat; dark = edge.
                        let edge = if e[3] > 0.0 { 1.0 - crate::color::lum([e[0] / e[3], e[1] / e[3], e[2] / e[3]]) } else { 0.0 };
                        let t = (edge * 3.0).clamp(0.0, 1.0);
                        [0, 1, 2, 3].map(|k| o[k] + (s[k] - o[k]) * t)
                    })
                    .collect();
                FImage { data, ..*img }
            }
            Filter::DustAndScratches { radius, threshold } => {
                let med = median(img, radius as usize);
                let th = threshold as f32 / 255.0;
                let data = img
                    .data
                    .par_iter()
                    .zip(&med.data)
                    .map(|(o, m)| {
                        let d = (0..3).map(|k| (o[k] - m[k]).abs()).fold(0.0, f32::max);
                        if d > th {
                            *m
                        } else {
                            *o
                        }
                    })
                    .collect();
                FImage { data, ..*img }
            }
            Filter::OilPaint { radius } => kuwahara(img, radius.max(1) as isize),
            Filter::Ripple { amount, size } => Filter::Wave { amplitude: amount, wavelength: size.max(2.0) }.run(img, origin, full),
            Filter::Spherize { amount } => {
                let (fw, fh) = (full.0 as f32, full.1 as f32);
                let (cx, cy) = (fw * 0.5 - origin.0 as f32, fh * 0.5 - origin.1 as f32);
                let (rx, ry) = (fw * 0.5, fh * 0.5);
                let a = (amount / 100.0).clamp(-1.0, 1.0);
                warp(img, |x, y| {
                    let (nx, ny) = ((x - cx) / rx, (y - cy) / ry);
                    let r = (nx * nx + ny * ny).sqrt();
                    if r >= 1.0 || r == 0.0 {
                        return (x, y);
                    }
                    let nr = if a >= 0.0 { r.powf(1.0 + a) } else { r.powf(1.0 / (1.0 - a)) };
                    let k = nr / r;
                    (cx + nx * k * rx, cy + ny * k * ry)
                })
            }
            Filter::PolarCoordinates { to_polar } => {
                let (fw, fh) = (full.0 as f32, full.1 as f32);
                let (cx, cy) = (fw * 0.5 - origin.0 as f32, fh * 0.5 - origin.1 as f32);
                let rmax = (fw * fw + fh * fh).sqrt() * 0.5;
                let (ox, oy) = (origin.0 as f32, origin.1 as f32);
                warp(img, |x, y| {
                    if to_polar {
                        let (dx, dy) = (x - cx, y - cy);
                        let ang = dx.atan2(-dy).rem_euclid(std::f32::consts::TAU);
                        let r = (dx * dx + dy * dy).sqrt() / rmax;
                        (ang / std::f32::consts::TAU * fw - ox, r * fh - oy)
                    } else {
                        let ang = (x + ox) / fw * std::f32::consts::TAU;
                        let r = (y + oy) / fh * rmax;
                        (cx + r * ang.sin(), cy - r * ang.cos())
                    }
                })
            }
            Filter::LensCorrection { distortion, vignette, chromatic } => {
                let (fw, fh) = (full.0 as f32, full.1 as f32);
                let (cx, cy) = (fw * 0.5 - origin.0 as f32, fh * 0.5 - origin.1 as f32);
                let norm = (cx * cx + cy * cy).sqrt().max(1.0);
                let k = distortion / 100.0;
                let ca = chromatic / 1000.0;
                let mut out = img.clone();
                out.data.par_chunks_mut(img.w).enumerate().for_each(|(y, row)| {
                    for (x, p) in row.iter_mut().enumerate() {
                        let (dx, dy) = ((x as f32 + 0.5 - cx) / norm, (y as f32 + 0.5 - cy) / norm);
                        let r2 = dx * dx + dy * dy;
                        let f = 1.0 + k * r2;
                        let at = |s: f32| img.sample(cx + dx * f * s * norm, cy + dy * f * s * norm);
                        let g = at(1.0);
                        let mut o = g;
                        if ca != 0.0 {
                            o[0] = at(1.0 + ca)[0];
                            o[2] = at(1.0 - ca)[2];
                        }
                        let v = (1.0 + vignette / 100.0 * r2).max(0.0);
                        *p = [o[0] * v, o[1] * v, o[2] * v, o[3]].map(|c| c.max(0.0));
                        p[0] = p[0].min(p[3]);
                        p[1] = p[1].min(p[3]);
                        p[2] = p[2].min(p[3]);
                    }
                });
                out
            }
            Filter::ShadowsHighlights { shadows, highlights, radius } => {
                let lum_img: Vec<[f32; 1]> = img
                    .data
                    .iter()
                    .map(|p| if p[3] > 0.0 { [crate::color::lum([p[0] / p[3], p[1] / p[3], p[2] / p[3]])] } else { [0.0] })
                    .collect();
                let mut lb = lum_img.clone();
                gaussian_blur(&mut lb, img.w, img.h, radius);
                let (s, h) = (shadows / 100.0, highlights / 100.0);
                let data = img
                    .data
                    .par_iter()
                    .zip(&lb)
                    .map(|(p, [l])| {
                        if p[3] <= 0.0 {
                            return *p;
                        }
                        let a = p[3];
                        let ws = s * (1.0 - l).powi(2) * 2.0;
                        let wh = h * l.powi(2) * 1.5;
                        let f = |c: f32| {
                            let c = c / a;
                            let c = c.powf(1.0 / (1.0 + ws));
                            (c.powf(1.0 + wh)).clamp(0.0, 1.0) * a
                        };
                        [f(p[0]), f(p[1]), f(p[2]), a]
                    })
                    .collect();
                FImage { data, ..*img }
            }
            Filter::HdrToning { strength, radius, saturation } => {
                let mut b = img.data.clone();
                gaussian_blur(&mut b, img.w, img.h, radius);
                let data = img
                    .data
                    .par_iter()
                    .zip(&b)
                    .map(|(o, bl)| {
                        let a = o[3];
                        if a <= 0.0 {
                            return *o;
                        }
                        let mut c = [0.0; 3];
                        for k in 0..3 {
                            let detail = o[k] - bl[k];
                            c[k] = ((bl[k] + detail * (1.0 + strength)) / a).clamp(0.0, 1.0);
                        }
                        let [hh, ss, ll] = crate::color::rgb_to_hsl(c);
                        let c = crate::color::hsl_to_rgb([hh, (ss * (1.0 + saturation / 100.0)).clamp(0.0, 1.0), ll]);
                        [c[0] * a, c[1] * a, c[2] * a, a]
                    })
                    .collect();
                FImage { data, ..*img }
            }
            Filter::Offset { dx, dy } => {
                let mut out = img.clone();
                let (w, h) = (img.w as i32, img.h as i32);
                out.data.par_chunks_mut(img.w).enumerate().for_each(|(y, row)| {
                    for (x, p) in row.iter_mut().enumerate() {
                        let sx = (x as i32 - dx).rem_euclid(w.max(1));
                        let sy = (y as i32 - dy).rem_euclid(h.max(1));
                        *p = img.data[sy as usize * img.w + sx as usize];
                    }
                });
                out
            }
            Filter::Displace { amount, scale, seed } => {
                let (ox, oy) = (origin.0 as f32, origin.1 as f32);
                let sc = scale.max(1.0);
                warp(img, |x, y| {
                    let nx = fbm((x + ox) / sc, (y + oy) / sc, seed) - 0.5;
                    let ny = fbm((x + ox) / sc, (y + oy) / sc, seed.wrapping_add(977)) - 0.5;
                    (x + nx * 2.0 * amount, y + ny * 2.0 * amount)
                })
            }
            Filter::Clouds { fg, bg, scale, seed } => {
                let (ox, oy) = origin;
                let scale = scale.max(1.0);
                let mut o = img.clone();
                o.data.par_chunks_mut(img.w).enumerate().for_each(|(y, row)| {
                    for (x, p) in row.iter_mut().enumerate() {
                        let t = fbm((ox + x as i32) as f32 / scale, (oy + y as i32) as f32 / scale, seed);
                        let c = bg.lerp(fg, t);
                        *p = [c.r * c.a, c.g * c.a, c.b * c.a, c.a];
                    }
                });
                o
            }
        }
    }
}

/// Applies `filter` to `base` (within the selection) and writes into `target`.
/// `target` and `base` normally hold the same layer; using a separate base
/// lets dialogs re-run the filter for live previews.
pub fn apply(target: &mut TiledBuffer<Rgba8>, base: &TiledBuffer<Rgba8>, sel: Option<&Mask>, filter: &Filter) -> Option<IRect> {
    let (w, h) = (base.width(), base.height());
    let work = selection::work_rect(sel, w, h);
    if work.is_empty() {
        return None;
    }
    let pad = filter.padding();
    let src_rect = work.expand(pad).clamp_to(w, h);
    let img = FImage::from_buffer(base, src_rect);
    let out = filter.run(&img, (src_rect.x0, src_rect.y0), (w, h));
    write_back(target, base, sel, work, src_rect, &out);
    Some(work)
}

/// Writes `out` (covering `src_rect`) back into `target` for pixels in
/// `work`, mixing with `base` by selection coverage.
pub fn write_back(
    target: &mut TiledBuffer<Rgba8>,
    base: &TiledBuffer<Rgba8>,
    sel: Option<&Mask>,
    work: IRect,
    src_rect: IRect,
    out: &FImage,
) {
    target.par_for_each_in_rect_mut(work, |x, y, p| {
        let s = coverage(sel, x, y);
        let b = base.get(x, y);
        if s <= 0.0 {
            *p = b;
            return;
        }
        let r = out.data[(y - src_rect.y0) as usize * out.w + (x - src_rect.x0) as usize];
        if s >= 1.0 {
            *p = to_straight(r);
            return;
        }
        let ba = b[3] as f32 / 255.0;
        let bp = [b[0] as f32 / 255.0 * ba, b[1] as f32 / 255.0 * ba, b[2] as f32 / 255.0 * ba, ba];
        let mut m = [0.0; 4];
        for i in 0..4 {
            m[i] = bp[i] + (r[i] - bp[i]) * s;
        }
        *p = to_straight(m);
    });
    target.optimize();
}

// ------------------------------------------------------------------ blurs

fn gaussian_kernel(sigma: f32) -> Vec<f32> {
    let r = (sigma * 3.0).ceil().max(1.0) as i32;
    let mut k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= sum);
    k
}

fn convolve_rows<const N: usize>(data: &mut [[f32; N]], w: usize, k: &[f32]) {
    let r = (k.len() / 2) as isize;
    data.par_chunks_mut(w).for_each(|row| {
        let src = row.to_vec();
        for (x, out) in row.iter_mut().enumerate() {
            let mut acc = [0.0f32; N];
            for (ki, kv) in k.iter().enumerate() {
                let sx = (x as isize + ki as isize - r).clamp(0, w as isize - 1) as usize;
                let s = &src[sx];
                for c in 0..N {
                    acc[c] += s[c] * kv;
                }
            }
            *out = acc;
        }
    });
}

fn box_rows<const N: usize>(data: &mut [[f32; N]], w: usize, r: usize) {
    if r == 0 {
        return;
    }
    let norm = 1.0 / (2 * r + 1) as f32;
    data.par_chunks_mut(w).for_each(|row| {
        let src = row.to_vec();
        let at = |i: isize| &src[i.clamp(0, w as isize - 1) as usize];
        let mut acc = [0.0f32; N];
        for i in -(r as isize)..=(r as isize) {
            let s = at(i);
            for c in 0..N {
                acc[c] += s[c];
            }
        }
        for x in 0..w {
            for c in 0..N {
                row[x][c] = acc[c] * norm;
            }
            let add = at(x as isize + r as isize + 1);
            let sub = at(x as isize - r as isize);
            for c in 0..N {
                acc[c] += add[c] - sub[c];
            }
        }
    });
}

fn transpose<const N: usize>(data: &[[f32; N]], w: usize, h: usize) -> Vec<[f32; N]> {
    let mut out = vec![[0.0; N]; w * h];
    out.par_chunks_mut(h).enumerate().for_each(|(x, col)| {
        for (y, v) in col.iter_mut().enumerate() {
            *v = data[y * w + x];
        }
    });
    out
}

/// Runs a row operation horizontally and then vertically (via transposes).
fn separable<const N: usize>(data: &mut Vec<[f32; N]>, w: usize, h: usize, f: impl Fn(&mut [[f32; N]], usize)) {
    if w == 0 || h == 0 {
        return;
    }
    f(data, w);
    let mut t = transpose(data, w, h);
    f(&mut t, h);
    *data = transpose(&t, h, w);
}

/// Gaussian blur. Small radii use an exact kernel; large radii use three
/// successive box blurs (≈ Gaussian, O(1) per pixel regardless of radius).
pub fn gaussian_blur<const N: usize>(data: &mut Vec<[f32; N]>, w: usize, h: usize, sigma: f32) {
    if sigma <= 0.0 {
        return;
    }
    if sigma < 6.0 {
        let k = gaussian_kernel(sigma);
        separable(data, w, h, |d, rw| convolve_rows(d, rw, &k));
    } else {
        // Box sizes for 3-pass approximation (Kovesi).
        let n = 3.0;
        let wi = ((12.0 * sigma * sigma / n) + 1.0).sqrt().floor();
        let wl = if wi as i32 % 2 == 0 { wi - 1.0 } else { wi };
        let wu = wl + 2.0;
        let m = ((12.0 * sigma * sigma - n * wl * wl - 4.0 * n * wl - 3.0 * n) / (-4.0 * wl - 4.0)).round();
        for i in 0..3 {
            let size = if (i as f32) < m { wl } else { wu };
            let r = ((size - 1.0) / 2.0) as usize;
            separable(data, w, h, |d, rw| box_rows(d, rw, r));
        }
    }
}

pub fn gaussian_blur_gray(data: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let mut d: Vec<[f32; 1]> = data.iter().map(|&v| [v]).collect();
    gaussian_blur(&mut d, w, h, sigma);
    d.into_iter().map(|[v]| v).collect()
}

pub fn box_blur<const N: usize>(data: &mut Vec<[f32; N]>, w: usize, h: usize, r: usize) {
    separable(data, w, h, |d, rw| box_rows(d, rw, r));
}

fn motion_blur(img: &FImage, angle: f32, distance: f32) -> FImage {
    let (s, c) = angle.to_radians().sin_cos();
    let n = distance.max(1.0).ceil() as i32;
    let mut out = img.clone();
    out.data.par_chunks_mut(img.w).enumerate().for_each(|(y, row)| {
        for (x, p) in row.iter_mut().enumerate() {
            let mut acc = [0.0; 4];
            for i in -n..=n {
                let t = i as f32 * distance / (2 * n) as f32;
                let s = img.sample(x as f32 + 0.5 + c * t, y as f32 + 0.5 - s * t);
                for k in 0..4 {
                    acc[k] += s[k];
                }
            }
            let norm = 1.0 / (2 * n + 1) as f32;
            *p = acc.map(|v| v * norm);
        }
    });
    out
}

fn convolve3(img: &FImage, k: [f32; 9], bias: f32, gray: bool) -> FImage {
    let mut out = img.clone();
    out.data.par_chunks_mut(img.w).enumerate().for_each(|(y, row)| {
        for (x, p) in row.iter_mut().enumerate() {
            let a = img.data[y * img.w + x][3];
            let mut acc = [0.0f32; 3];
            for ky in 0..3 {
                for kx in 0..3 {
                    let s = img.get(x as isize + kx as isize - 1, y as isize + ky as isize - 1);
                    let inv = if s[3] > 0.0 { 1.0 / s[3] } else { 0.0 };
                    let kv = k[ky * 3 + kx];
                    for c in 0..3 {
                        acc[c] += s[c] * inv * kv;
                    }
                }
            }
            if gray {
                let l = crate::color::lum(acc) + bias;
                acc = [l; 3];
            } else {
                acc = acc.map(|v| v + bias);
            }
            *p = [acc[0].clamp(0.0, 1.0) * a, acc[1].clamp(0.0, 1.0) * a, acc[2].clamp(0.0, 1.0) * a, a];
        }
    });
    out
}

fn find_edges(img: &FImage) -> FImage {
    let mut out = img.clone();
    out.data.par_chunks_mut(img.w).enumerate().for_each(|(y, row)| {
        for (x, p) in row.iter_mut().enumerate() {
            let a = img.data[y * img.w + x][3];
            let g = |dx: isize, dy: isize| {
                let s = img.get(x as isize + dx, y as isize + dy);
                let inv = if s[3] > 0.0 { 1.0 / s[3] } else { 0.0 };
                [s[0] * inv, s[1] * inv, s[2] * inv]
            };
            let mut o = [0.0; 4];
            for c in 0..3 {
                let gx = -g(-1, -1)[c] - 2.0 * g(-1, 0)[c] - g(-1, 1)[c] + g(1, -1)[c] + 2.0 * g(1, 0)[c] + g(1, 1)[c];
                let gy = -g(-1, -1)[c] - 2.0 * g(0, -1)[c] - g(1, -1)[c] + g(-1, 1)[c] + 2.0 * g(0, 1)[c] + g(1, 1)[c];
                o[c] = (1.0 - (gx * gx + gy * gy).sqrt() * 0.5).clamp(0.0, 1.0) * a;
            }
            o[3] = a;
            *p = o;
        }
    });
    out
}

/// Edge-preserving bilateral filter.
fn bilateral(img: &FImage, radius: isize, sigma_c: f32) -> FImage {
    let step = (radius / 6).max(1);
    let inv2 = 1.0 / (2.0 * sigma_c * sigma_c).max(1e-6);
    let sr2 = (radius * radius) as f32;
    let mut out = img.clone();
    out.data.par_chunks_mut(img.w).enumerate().for_each(|(y, row)| {
        for (x, p) in row.iter_mut().enumerate() {
            let c = img.data[y * img.w + x];
            let mut acc = [0.0; 4];
            let mut wsum = 0.0;
            let mut dy = -radius;
            while dy <= radius {
                let mut dx = -radius;
                while dx <= radius {
                    let d2 = (dx * dx + dy * dy) as f32;
                    if d2 <= sr2 {
                        let s = img.get(x as isize + dx, y as isize + dy);
                        let diff = (0..3).map(|k| (s[k] - c[k]).powi(2)).sum::<f32>();
                        let w = (-diff * inv2).exp();
                        for k in 0..4 {
                            acc[k] += s[k] * w;
                        }
                        wsum += w;
                    }
                    dx += step;
                }
                dy += step;
            }
            *p = acc.map(|v| v / wsum.max(1e-6));
        }
    });
    out
}

/// Kuwahara filter (painterly "Oil Paint").
fn kuwahara(img: &FImage, r: isize) -> FImage {
    let mut out = img.clone();
    out.data.par_chunks_mut(img.w).enumerate().for_each(|(y, row)| {
        for (x, p) in row.iter_mut().enumerate() {
            let mut best = (f32::MAX, [0.0; 4]);
            for (qx, qy) in [(-1isize, -1isize), (1, -1), (-1, 1), (1, 1)] {
                let mut sum = [0.0f32; 4];
                let mut sq = 0.0f32;
                let mut n = 0.0f32;
                for dy in 0..=r {
                    for dx in 0..=r {
                        let s = img.get(x as isize + dx * qx, y as isize + dy * qy);
                        for k in 0..4 {
                            sum[k] += s[k];
                        }
                        let l = s[0] + s[1] + s[2];
                        sq += l * l;
                        n += 1.0;
                    }
                }
                let mean = sum.map(|v| v / n);
                let ml = mean[0] + mean[1] + mean[2];
                let var = sq / n - ml * ml;
                if var < best.0 {
                    best = (var, mean);
                }
            }
            *p = best.1;
        }
    });
    out
}

/// Median using a sliding 256-bin histogram per channel (Huang's algorithm).
fn median(img: &FImage, r: usize) -> FImage {
    if r == 0 {
        return img.clone();
    }
    let (w, h) = (img.w, img.h);
    let q: Vec<Rgba8> = img.data.par_iter().map(|p| p.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)).collect();
    let mut out = img.clone();
    let half = ((2 * r + 1) * (2 * r + 1)) / 2;
    out.data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let mut hist = vec![[0u32; 256]; 4];
        let at = |x: isize, y: isize| q[(y.clamp(0, h as isize - 1) as usize) * w + x.clamp(0, w as isize - 1) as usize];
        let ri = r as isize;
        for dy in -ri..=ri {
            for dx in -ri..=ri {
                let p = at(dx, y as isize + dy);
                for c in 0..4 {
                    hist[c][p[c] as usize] += 1;
                }
            }
        }
        for (x, o) in row.iter_mut().enumerate() {
            for c in 0..4 {
                let mut acc = 0usize;
                for (v, &n) in hist[c].iter().enumerate() {
                    acc += n as usize;
                    if acc > half {
                        o[c] = v as f32 / 255.0;
                        break;
                    }
                }
            }
            o[0] = o[0].min(o[3]);
            o[1] = o[1].min(o[3]);
            o[2] = o[2].min(o[3]);
            let xi = x as isize;
            for dy in -ri..=ri {
                let rem = at(xi - ri, y as isize + dy);
                let add = at(xi + ri + 1, y as isize + dy);
                for c in 0..4 {
                    hist[c][rem[c] as usize] -= 1;
                    hist[c][add[c] as usize] += 1;
                }
            }
        }
    });
    out
}

fn pixelate(img: &FImage, origin: (i32, i32), size: i32) -> FImage {
    let mut out = img.clone();
    let (ox, oy) = origin;
    out.data.par_chunks_mut(img.w).enumerate().for_each(|(y, row)| {
        let gy = oy + y as i32;
        let cy0 = (gy.div_euclid(size) * size - oy).max(0) as usize;
        let cy1 = ((gy.div_euclid(size) + 1) * size - oy).min(img.h as i32) as usize;
        let mut x = 0usize;
        while x < img.w {
            let gx = ox + x as i32;
            let cx0 = (gx.div_euclid(size) * size - ox).max(0) as usize;
            let cx1 = ((gx.div_euclid(size) + 1) * size - ox).min(img.w as i32) as usize;
            let mut acc = [0.0; 4];
            for yy in cy0..cy1 {
                for xx in cx0..cx1 {
                    let p = img.data[yy * img.w + xx];
                    for c in 0..4 {
                        acc[c] += p[c];
                    }
                }
            }
            let n = ((cy1 - cy0) * (cx1 - cx0)).max(1) as f32;
            let avg = acc.map(|v| v / n);
            for v in &mut row[x..cx1] {
                *v = avg;
            }
            x = cx1;
        }
    });
    out
}

/// Inverse-mapping warp: `f(dst_x, dst_y) -> (src_x, src_y)` in local pixel-centre coords.
fn warp(img: &FImage, f: impl Fn(f32, f32) -> (f32, f32) + Sync) -> FImage {
    let mut out = img.clone();
    out.data.par_chunks_mut(img.w).enumerate().for_each(|(y, row)| {
        for (x, p) in row.iter_mut().enumerate() {
            let (sx, sy) = f(x as f32 + 0.5, y as f32 + 0.5);
            *p = img.sample(sx, sy);
        }
    });
    out
}

#[inline]
fn noise(x: i32, y: i32, seed: u32) -> f32 {
    crate::blend::pixel_noise(x ^ seed as i32, y.wrapping_add((seed as i32).wrapping_mul(31337)))
}

fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let (xi, yi) = (x0 as i32, y0 as i32);
    let a = noise(xi, yi, seed);
    let b = noise(xi + 1, yi, seed);
    let c = noise(xi, yi + 1, seed);
    let d = noise(xi + 1, yi + 1, seed);
    let top = a + (b - a) * s(fx);
    let bot = c + (d - c) * s(fx);
    top + (bot - top) * s(fy)
}

/// Fractal Brownian motion (Render > Clouds).
fn fbm(x: f32, y: f32, seed: u32) -> f32 {
    let mut amp = 0.5;
    let mut freq = 1.0;
    let mut sum = 0.0;
    let mut norm = 0.0;
    for o in 0..6 {
        sum += value_noise(x * freq, y * freq, seed.wrapping_add(o * 101)) * amp;
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    sum / norm
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(w: u32, h: u32) -> TiledBuffer<Rgba8> {
        let mut b = TiledBuffer::new(w, h);
        b.par_map_all(|x, y, _| if (x + y) % 2 == 0 { [255, 255, 255, 255] } else { [0, 0, 0, 255] });
        b
    }

    #[test]
    fn blur_averages_checker() {
        for radius in [2.0, 10.0] {
            let base = checker(80, 80);
            let mut t = base.clone();
            apply(&mut t, &base, None, &Filter::GaussianBlur { radius });
            let p = t.get(40, 40);
            assert!((p[0] as i32 - 128).abs() < 6, "radius {radius}: {p:?}");
            assert_eq!(p[3], 255);
        }
    }

    #[test]
    fn filters_respect_selection_and_run() {
        let base = checker(70, 70);
        let sel = selection::rect_mask(70, 70, IRect::new(10, 10, 30, 30));
        let all = [
            Filter::BoxBlur { radius: 3 },
            Filter::MotionBlur { angle: 30.0, distance: 8.0 },
            Filter::UnsharpMask { amount: 1.0, radius: 2.0, threshold: 0 },
            Filter::Sharpen,
            Filter::HighPass { radius: 3.0 },
            Filter::Median { radius: 2 },
            Filter::AddNoise { amount: 0.2, monochrome: true, seed: 1 },
            Filter::Pixelate { size: 8 },
            Filter::Emboss { angle: 45.0, amount: 1.0 },
            Filter::FindEdges,
            Filter::Twirl { angle: 90.0 },
            Filter::Wave { amplitude: 4.0, wavelength: 20.0 },
            Filter::Clouds { fg: Color::BLACK, bg: Color::WHITE, scale: 20.0, seed: 3 },
            Filter::RadialBlur { amount: 20.0, zoom: true },
            Filter::RadialBlur { amount: 20.0, zoom: false },
            Filter::LensBlur { radius: 4.0 },
            Filter::SurfaceBlur { radius: 4, threshold: 20.0 },
            Filter::TiltShift { center: 0.5, band: 0.2, blur: 4.0 },
            Filter::SmartSharpen { amount: 1.0, radius: 1.0, noise: 5.0 },
            Filter::SharpenEdges,
            Filter::ReduceNoise { strength: 0.5 },
            Filter::DustAndScratches { radius: 2, threshold: 10 },
            Filter::OilPaint { radius: 3 },
            Filter::Ripple { amount: 3.0, size: 10.0 },
            Filter::Spherize { amount: 50.0 },
            Filter::PolarCoordinates { to_polar: true },
            Filter::PolarCoordinates { to_polar: false },
            Filter::LensCorrection { distortion: 10.0, vignette: -30.0, chromatic: 5.0 },
            Filter::ShadowsHighlights { shadows: 35.0, highlights: 10.0, radius: 10.0 },
            Filter::HdrToning { strength: 0.5, radius: 10.0, saturation: 20.0 },
            Filter::Offset { dx: 5, dy: 3 },
            Filter::Displace { amount: 5.0, scale: 20.0, seed: 2 },
        ];
        for f in all {
            let mut t = base.clone();
            let r = apply(&mut t, &base, Some(&sel), &f);
            assert_eq!(r, Some(IRect::new(10, 10, 30, 30)), "{}", f.name());
            assert_eq!(t.get(50, 50), base.get(50, 50), "{} leaked outside selection", f.name());
        }
    }

    #[test]
    fn median_removes_speck() {
        let mut base = TiledBuffer::solid(20, 20, [0, 0, 0, 255]);
        base.set(10, 10, [255, 255, 255, 255]);
        let mut t = base.clone();
        apply(&mut t, &base, None, &Filter::Median { radius: 1 });
        assert_eq!(t.get(10, 10), [0, 0, 0, 255]);
    }
}
