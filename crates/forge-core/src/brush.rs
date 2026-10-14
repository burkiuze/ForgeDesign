//! Brush engine: Brush, Pencil, Eraser, Clone/Pattern Stamp, Healing,
//! Dodge/Burn/Sponge, Blur/Sharpen/Smudge, Color Replacement, Background
//! Eraser, Red Eye and Mixer brushes.
//!
//! Strokes work like Photoshop's: every dab accumulates into a floating-
//! point *coverage* buffer (controlled by Flow), and the layer is re-rendered
//! from the stroke's starting snapshot using `coverage × Opacity`. Overlapping
//! dabs within one stroke therefore never exceed the stroke opacity.
//! Smudge/Mixer are the exception: they push paint around dab by dab.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::buffer::TiledBuffer;
use crate::color::{hsl_to_rgb, lum, rgb_to_hsl, Color, Rgba8};
use crate::geom::{union_opt, IRect, Point};
use crate::selection::{coverage, Mask};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrushSettings {
    /// Diameter in pixels.
    pub size: f32,
    /// 0 = very soft edge, 1 = hard edge.
    pub hardness: f32,
    /// Maximum alpha a single stroke can reach.
    pub opacity: f32,
    /// How much each dab adds.
    pub flow: f32,
    /// Dab spacing as a fraction of the diameter.
    pub spacing: f32,
    /// `false` gives the aliased "pencil" look.
    pub antialias: bool,
    /// Tip rotation in degrees.
    pub angle: f32,
    /// Tip roundness, 1 = circle, 0.1 = thin ellipse.
    pub roundness: f32,
    /// Random dab position jitter as a fraction of the size.
    pub scatter: f32,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self {
            size: 20.0,
            hardness: 0.8,
            opacity: 1.0,
            flow: 1.0,
            spacing: 0.15,
            antialias: true,
            angle: 0.0,
            roundness: 1.0,
            scatter: 0.0,
        }
    }
}

impl BrushSettings {
    pub fn pencil(size: f32) -> Self {
        Self { size, hardness: 1.0, spacing: 0.1, antialias: false, ..Default::default() }
    }
}

/// Custom brush tip ("Define Brush Preset"): grayscale coverage, 255 = paint.
#[derive(Clone, Debug, PartialEq)]
pub struct BrushTip {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl BrushTip {
    /// Bilinear sample at normalised tip coordinates (`-1..1`).
    fn sample(&self, u: f32, v: f32) -> f32 {
        let x = (u * 0.5 + 0.5) * self.width as f32 - 0.5;
        let y = (v * 0.5 + 0.5) * self.height as f32 - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let at = |xi: i32, yi: i32| -> f32 {
            if xi < 0 || yi < 0 || xi >= self.width as i32 || yi >= self.height as i32 {
                0.0
            } else {
                self.data[(yi as u32 * self.width + xi as u32) as usize] as f32 / 255.0
            }
        };
        let (xi, yi) = (x0 as i32, y0 as i32);
        let top = at(xi, yi) + (at(xi + 1, yi) - at(xi, yi)) * fx;
        let bot = at(xi, yi + 1) + (at(xi + 1, yi + 1) - at(xi, yi + 1)) * fx;
        top + (bot - top) * fy
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToneRange {
    Shadows,
    #[default]
    Midtones,
    Highlights,
}

impl ToneRange {
    pub const ALL: [ToneRange; 3] = [ToneRange::Shadows, ToneRange::Midtones, ToneRange::Highlights];
    pub fn name(self) -> &'static str {
        match self {
            ToneRange::Shadows => "Shadows",
            ToneRange::Midtones => "Midtones",
            ToneRange::Highlights => "Highlights",
        }
    }
    fn weight(self, l: f32) -> f32 {
        match self {
            ToneRange::Shadows => (1.0 - l) * (1.0 - l),
            ToneRange::Midtones => 4.0 * l * (1.0 - l),
            ToneRange::Highlights => l * l,
        }
    }
}

/// Mirror axes for symmetry painting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Symmetry {
    #[default]
    None,
    Vertical,
    Horizontal,
    Both,
}

impl Symmetry {
    pub const ALL: [Symmetry; 4] = [Symmetry::None, Symmetry::Vertical, Symmetry::Horizontal, Symmetry::Both];
    pub fn name(self) -> &'static str {
        match self {
            Symmetry::None => "Off",
            Symmetry::Vertical => "Vertical",
            Symmetry::Horizontal => "Horizontal",
            Symmetry::Both => "Both",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LiquifyMode {
    #[default]
    Push,
    TwirlClockwise,
    TwirlCounter,
    Pucker,
    Bloat,
    Reconstruct,
}

impl LiquifyMode {
    pub const ALL: [LiquifyMode; 6] = [
        LiquifyMode::Push,
        LiquifyMode::TwirlClockwise,
        LiquifyMode::TwirlCounter,
        LiquifyMode::Pucker,
        LiquifyMode::Bloat,
        LiquifyMode::Reconstruct,
    ];
    pub fn name(self) -> &'static str {
        match self {
            LiquifyMode::Push => "Forward Warp",
            LiquifyMode::TwirlClockwise => "Twirl CW",
            LiquifyMode::TwirlCounter => "Twirl CCW",
            LiquifyMode::Pucker => "Pucker",
            LiquifyMode::Bloat => "Bloat",
            LiquifyMode::Reconstruct => "Reconstruct",
        }
    }
}

/// What a stroke does.
#[derive(Clone, Debug)]
pub enum StrokeSource {
    Color(Color),
    Erase,
    /// Clone Stamp: copies from `source` displaced by `(dx, dy)`.
    Clone { source: TiledBuffer<Rgba8>, dx: i32, dy: i32 },
    /// Healing Brush: clone texture, keep the destination's colour/lighting.
    Heal { source: TiledBuffer<Rgba8>, dx: i32, dy: i32 },
    /// Pattern Stamp: paints a repeating pattern.
    Pattern(Arc<TiledBuffer<Rgba8>>),
    Dodge { range: ToneRange, exposure: f32 },
    Burn { range: ToneRange, exposure: f32 },
    /// Sponge: `amount > 0` saturates, `< 0` desaturates.
    Sponge { amount: f32 },
    Blur { strength: f32 },
    Sharpen { strength: f32 },
    /// Color Replacement: recolours keeping luminosity (Color blend mode).
    ColorReplace(Color),
    /// Background Eraser: erases pixels similar to `sample`.
    BackgroundErase { sample: Rgba8, tolerance: u8 },
    RedEye,
    /// Smudge (`paint = None`) / Mixer Brush (`paint = Some(colour, wetness)`).
    Smudge { strength: f32, paint: Option<(Color, f32)> },
    /// Liquify: geometric displacement of the pixels under the brush.
    Liquify { mode: LiquifyMode, strength: f32 },
}

impl StrokeSource {
    fn is_smudge(&self) -> bool {
        matches!(self, StrokeSource::Smudge { .. })
    }

    fn is_liquify(&self) -> bool {
        matches!(self, StrokeSource::Liquify { .. })
    }
}

/// The buffer a stroke paints into.
pub enum StrokeTarget<'a> {
    Pixels(&'a mut TiledBuffer<Rgba8>),
    Mask(&'a mut TiledBuffer<u8>),
}

enum Base {
    Pixels(TiledBuffer<Rgba8>),
    Mask(TiledBuffer<u8>),
}

pub struct Stroke {
    pub settings: BrushSettings,
    source: StrokeSource,
    base: Base,
    lock_alpha: bool,
    selection: Option<Mask>,
    cov: TiledBuffer<f32>,
    tip: Option<Arc<BrushTip>>,
    symmetry: Symmetry,
    /// Canvas centre used as the symmetry axis.
    center: Point,
    last: Option<Point>,
    last_dab: Option<Point>,
    /// Distance travelled since the last dab.
    carry: f32,
    dab_count: u32,
    touched: Option<IRect>,
    /// Liquify displacement field (output pixel samples base at p - d).
    disp: Option<TiledBuffer<[f32; 2]>>,
}

impl Stroke {
    /// Starts a stroke on layer pixels.
    pub fn on_pixels(
        settings: BrushSettings,
        source: StrokeSource,
        base: TiledBuffer<Rgba8>,
        selection: Option<Mask>,
        lock_alpha: bool,
    ) -> Self {
        let (w, h) = (base.width(), base.height());
        Self::new(settings, source, Base::Pixels(base), selection, lock_alpha, w, h)
    }

    /// Starts a stroke on a layer mask (colours are painted as their luminance).
    pub fn on_mask(settings: BrushSettings, source: StrokeSource, base: TiledBuffer<u8>, selection: Option<Mask>) -> Self {
        let (w, h) = (base.width(), base.height());
        Self::new(settings, source, Base::Mask(base), selection, false, w, h)
    }

    fn new(
        settings: BrushSettings,
        source: StrokeSource,
        base: Base,
        selection: Option<Mask>,
        lock_alpha: bool,
        w: u32,
        h: u32,
    ) -> Self {
        Self {
            settings,
            source,
            base,
            lock_alpha,
            selection,
            cov: TiledBuffer::new(w, h),
            tip: None,
            symmetry: Symmetry::None,
            center: Point::new(w as f32 * 0.5, h as f32 * 0.5),
            last: None,
            last_dab: None,
            carry: 0.0,
            dab_count: 0,
            touched: None,
            disp: None,
        }
    }

    pub fn with_tip(mut self, tip: Option<Arc<BrushTip>>) -> Self {
        self.tip = tip;
        self
    }

    pub fn with_symmetry(mut self, symmetry: Symmetry) -> Self {
        self.symmetry = symmetry;
        self
    }

    /// Bounding box of everything the stroke modified so far.
    pub fn touched(&self) -> Option<IRect> {
        self.touched
    }

    /// Accumulated coverage (used by Spot Healing to know what to repair).
    pub fn coverage_mask(&self) -> Mask {
        let mut m = Mask::new(self.cov.width(), self.cov.height());
        if let Some(r) = self.touched {
            let cov = &self.cov;
            m.par_for_each_in_rect_mut(r, |x, y, v| *v = (cov.get(x, y).min(1.0) * 255.0) as u8);
            m.optimize();
        }
        m
    }

    fn mirrors(&self, c: Point) -> Vec<Point> {
        let mx = 2.0 * self.center.x - c.x;
        let my = 2.0 * self.center.y - c.y;
        match self.symmetry {
            Symmetry::None => vec![c],
            Symmetry::Vertical => vec![c, Point::new(mx, c.y)],
            Symmetry::Horizontal => vec![c, Point::new(c.x, my)],
            Symmetry::Both => vec![c, Point::new(mx, c.y), Point::new(c.x, my), Point::new(mx, my)],
        }
    }

    /// Extends the stroke to `p`, placing dabs along the way, and re-renders
    /// the affected pixels into `target`. Returns the modified rectangle.
    pub fn stroke_to(&mut self, p: Point, mut target: StrokeTarget<'_>) -> Option<IRect> {
        let step = (self.settings.spacing * self.settings.size).max(0.5);
        let mut centers = Vec::new();
        match self.last {
            None => {
                centers.push(p);
                self.carry = 0.0;
            }
            Some(last) => {
                let dist = last.distance(p);
                let mut t = step - self.carry;
                while t <= dist {
                    centers.push(last.lerp(p, t / dist));
                    t += step;
                }
                self.carry = dist - (t - step);
            }
        }
        self.last = Some(p);
        let mut dirty = None;
        for c in centers {
            let c = self.jitter(c);
            let prev = self.last_dab.replace(c);
            for (i, m) in self.mirrors(c).into_iter().enumerate() {
                if self.source.is_liquify() {
                    let pm = prev.map(|pp| self.mirrors(pp)[i]);
                    dirty = union_opt(dirty, self.liquify_dab(m, pm));
                } else if self.source.is_smudge() {
                    // Mirror the previous dab position the same way.
                    let pm = prev.map(|pp| self.mirrors(pp)[i]);
                    dirty = union_opt(dirty, self.smudge_dab(m, pm, &mut target));
                } else {
                    dirty = union_opt(dirty, self.dab(m));
                }
            }
        }
        if let Some(r) = dirty {
            if self.source.is_liquify() {
                self.liquify_render(r, &mut target);
            } else if !self.source.is_smudge() {
                self.render(r, target);
            }
            self.touched = union_opt(self.touched, Some(r));
        }
        dirty
    }

    fn jitter(&mut self, c: Point) -> Point {
        self.dab_count = self.dab_count.wrapping_add(1);
        if self.settings.scatter <= 0.0 {
            return c;
        }
        let n1 = crate::blend::pixel_noise(self.dab_count as i32, 17) - 0.5;
        let n2 = crate::blend::pixel_noise(self.dab_count as i32, 91) - 0.5;
        let s = self.settings.scatter * self.settings.size * 2.0;
        Point::new(c.x + n1 * s, c.y + n2 * s)
    }

    /// Tip value (0..1) for a pixel offset `(dx, dy)` from the dab centre.
    #[inline]
    fn tip_value(&self, dx: f32, dy: f32, r: f32, cos: f32, sin: f32) -> f32 {
        let s = &self.settings;
        // Rotate into tip space and apply roundness.
        let u = dx * cos + dy * sin;
        let v = (-dx * sin + dy * cos) / s.roundness.clamp(0.05, 1.0);
        if let Some(tip) = &self.tip {
            let aspect = tip.height as f32 / tip.width.max(1) as f32;
            return tip.sample(u / r, v / (r * aspect.max(0.01)));
        }
        let d = (u * u + v * v).sqrt();
        if !s.antialias {
            return if r <= 0.75 { ((u.abs() < 0.5) && (v.abs() < 0.5)) as u8 as f32 } else { (d <= r) as u8 as f32 };
        }
        let edge = (r - d + 0.5).clamp(0.0, 1.0);
        let hard = s.hardness.clamp(0.0, 1.0);
        if hard >= 1.0 {
            edge
        } else {
            let t = ((d / r - hard) / (1.0 - hard)).clamp(0.0, 1.0);
            (1.0 - t * t * (3.0 - 2.0 * t)) * edge
        }
    }

    fn dab_rect(&self, c: Point) -> IRect {
        let r = (self.settings.size * 0.5).max(0.5);
        let extent = if self.tip.is_some() { r * 1.5 } else { r };
        IRect::around(c, extent).clamp_to(self.cov.width(), self.cov.height())
    }

    fn dab(&mut self, c: Point) -> Option<IRect> {
        let rect = self.dab_rect(c);
        if rect.is_empty() {
            return None;
        }
        let r = (self.settings.size * 0.5).max(0.5);
        let flow = self.settings.flow.clamp(0.0, 1.0);
        let (sin, cos) = self.settings.angle.to_radians().sin_cos();
        let pencil_snap = !self.settings.antialias && r <= 0.75;
        let mut vals = Vec::with_capacity((rect.width() * rect.height()) as usize);
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let (dx, dy) = if pencil_snap {
                    (x as f32 - c.x.floor(), y as f32 - c.y.floor())
                } else {
                    (x as f32 + 0.5 - c.x, y as f32 + 0.5 - c.y)
                };
                vals.push(self.tip_value(dx, dy, r, cos, sin));
            }
        }
        let w = rect.width();
        self.cov.for_each_in_rect_mut(rect, |x, y, v| {
            let a = vals[((y - rect.y0) * w + (x - rect.x0)) as usize];
            if a > 0.0 {
                *v += (1.0 - *v) * a * flow;
            }
        });
        Some(rect)
    }

    /// Smudge / mixer: drags colour from the previous dab position.
    fn smudge_dab(&mut self, c: Point, prev: Option<Point>, target: &mut StrokeTarget<'_>) -> Option<IRect> {
        let StrokeTarget::Pixels(dst) = target else { return None };
        let StrokeSource::Smudge { strength, paint } = self.source.clone() else { return None };
        let prev = prev?;
        let rect = self.dab_rect(c);
        if rect.is_empty() {
            return None;
        }
        let (ox, oy) = ((c.x - prev.x).round() as i32, (c.y - prev.y).round() as i32);
        let r = (self.settings.size * 0.5).max(0.5);
        let (sin, cos) = self.settings.angle.to_radians().sin_cos();
        let snapshot = dst.region_to_dense(rect.translate(-ox, -oy));
        let w = rect.width();
        let sel = self.selection.clone();
        let mut weights = Vec::with_capacity(snapshot.len());
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                weights.push(self.tip_value(x as f32 + 0.5 - c.x, y as f32 + 0.5 - c.y, r, cos, sin));
            }
        }
        let opacity = self.settings.opacity;
        dst.for_each_in_rect_mut(rect, |x, y, px| {
            let i = ((y - rect.y0) * w + (x - rect.x0)) as usize;
            let a = weights[i] * strength * opacity * coverage(sel.as_ref(), x, y);
            if a <= 0.0 {
                return;
            }
            let mut src = snapshot[i];
            if let Some((col, wet)) = paint {
                let c8 = col.to_rgba8();
                for k in 0..3 {
                    src[k] = (src[k] as f32 * wet + c8[k] as f32 * (1.0 - wet)) as u8;
                }
                src[3] = src[3].max((255.0 * (1.0 - wet)) as u8);
            }
            for k in 0..4 {
                px[k] = (px[k] as f32 + (src[k] as f32 - px[k] as f32) * a + 0.5) as u8;
            }
        });
        Some(rect)
    }

    fn liquify_dab(&mut self, c: Point, prev: Option<Point>) -> Option<IRect> {
        let StrokeSource::Liquify { mode, strength } = self.source else { return None };
        let rect = self.dab_rect(c);
        if rect.is_empty() {
            return None;
        }
        let (w, h) = (self.cov.width(), self.cov.height());
        let disp = self.disp.get_or_insert_with(|| TiledBuffer::new(w, h));
        let r = (self.settings.size * 0.5).max(1.0);
        let hard = self.settings.hardness.clamp(0.0, 0.99);
        let (mx, my) = prev.map_or((0.0, 0.0), |p| (c.x - p.x, c.y - p.y));
        let k = strength.clamp(0.0, 1.0);
        disp.for_each_in_rect_mut(rect, |x, y, d| {
            let (dx, dy) = (x as f32 + 0.5 - c.x, y as f32 + 0.5 - c.y);
            let dist = (dx * dx + dy * dy).sqrt() / r;
            if dist >= 1.0 {
                return;
            }
            let t = ((dist - hard) / (1.0 - hard)).clamp(0.0, 1.0);
            let fall = (1.0 - t * t * (3.0 - 2.0 * t)) * k;
            match mode {
                LiquifyMode::Push => {
                    d[0] += mx * fall;
                    d[1] += my * fall;
                }
                LiquifyMode::TwirlClockwise | LiquifyMode::TwirlCounter => {
                    let s = if mode == LiquifyMode::TwirlClockwise { 0.08 } else { -0.08 } * fall;
                    d[0] += -dy * s;
                    d[1] += dx * s;
                }
                LiquifyMode::Pucker => {
                    d[0] += dx * 0.05 * fall;
                    d[1] += dy * 0.05 * fall;
                }
                LiquifyMode::Bloat => {
                    d[0] -= dx * 0.05 * fall;
                    d[1] -= dy * 0.05 * fall;
                }
                LiquifyMode::Reconstruct => {
                    d[0] *= 1.0 - fall * 0.3;
                    d[1] *= 1.0 - fall * 0.3;
                }
            }
        });
        Some(rect)
    }

    fn liquify_render(&self, rect: IRect, target: &mut StrokeTarget<'_>) {
        let (StrokeTarget::Pixels(dst), Base::Pixels(base), Some(disp)) = (target, &self.base, &self.disp) else { return };
        let sel = self.selection.as_ref();
        dst.par_for_each_in_rect_mut(rect, |x, y, px| {
            let d = disp.get(x, y);
            let b = base.get(x, y);
            if d == [0.0, 0.0] {
                *px = b;
                return;
            }
            let s = sample_bilinear(base, x as f32 + 0.5 - d[0], y as f32 + 0.5 - d[1]);
            let a = coverage(sel, x, y);
            *px = if a >= 1.0 {
                s
            } else {
                [0, 1, 2, 3].map(|k| (b[k] as f32 + (s[k] as f32 - b[k] as f32) * a + 0.5) as u8)
            };
        });
    }

    fn render(&self, rect: IRect, target: StrokeTarget<'_>) {
        let opacity = self.settings.opacity.clamp(0.0, 1.0);
        let sel = self.selection.as_ref();
        let cov = &self.cov;
        match (target, &self.base) {
            (StrokeTarget::Pixels(dst), Base::Pixels(base)) => {
                let lock = self.lock_alpha;
                let source = &self.source;
                dst.par_for_each_in_rect_mut(rect, |x, y, px| {
                    let c = cov.get(x, y);
                    let b = base.get(x, y);
                    if c <= 0.0 {
                        *px = b;
                        return;
                    }
                    let a = c * opacity * coverage(sel, x, y);
                    *px = apply_source(source, base, b, a, lock, x, y);
                });
            }
            (StrokeTarget::Mask(dst), Base::Mask(base)) => {
                let value = match &self.source {
                    StrokeSource::Color(c) => lum(c.rgb_array()) * 255.0,
                    _ => 255.0,
                };
                dst.par_for_each_in_rect_mut(rect, |x, y, m| {
                    let b = base.get(x, y) as f32;
                    let a = cov.get(x, y) * opacity * coverage(sel, x, y);
                    *m = (b + (value - b) * a + 0.5).clamp(0.0, 255.0) as u8;
                });
            }
            _ => debug_assert!(false, "stroke target does not match stroke base"),
        }
    }
}

#[inline]
fn f(p: Rgba8) -> [f32; 3] {
    [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]
}

#[inline]
fn mix_rgb(b: Rgba8, t: [f32; 3], a: f32) -> Rgba8 {
    let bb = f(b);
    [
        q(bb[0] + (t[0] - bb[0]) * a),
        q(bb[1] + (t[1] - bb[1]) * a),
        q(bb[2] + (t[2] - bb[2]) * a),
        b[3],
    ]
}

/// Mean colour of a 7×7 neighbourhood (sparse 4×4 sampling).
fn local_mean(buf: &TiledBuffer<Rgba8>, x: i32, y: i32) -> [f32; 3] {
    let mut acc = [0.0; 3];
    let mut n = 0.0;
    for dy in [-3, -1, 1, 3] {
        for dx in [-3, -1, 1, 3] {
            let p = buf.get(x + dx, y + dy);
            if p[3] > 0 {
                let c = f(p);
                for k in 0..3 {
                    acc[k] += c[k];
                }
                n += 1.0;
            }
        }
    }
    if n > 0.0 {
        acc.map(|v| v / n)
    } else {
        acc
    }
}

fn apply_source(source: &StrokeSource, base: &TiledBuffer<Rgba8>, b: Rgba8, a: f32, lock: bool, x: i32, y: i32) -> Rgba8 {
    match source {
        StrokeSource::Color(col) => paint_over(b, col.rgb_array(), a * col.a, lock),
        StrokeSource::Erase => {
            if lock {
                b
            } else {
                let mut o = b;
                o[3] = (b[3] as f32 * (1.0 - a) + 0.5) as u8;
                o
            }
        }
        StrokeSource::Clone { source, dx, dy } => {
            let s = source.get(x + dx, y + dy);
            paint_over(b, f(s), a * s[3] as f32 / 255.0, lock)
        }
        StrokeSource::Heal { source, dx, dy } => {
            let s = source.get(x + dx, y + dy);
            let ms = local_mean(source, x + dx, y + dy);
            let md = local_mean(base, x, y);
            let sc = f(s);
            let t = [
                (sc[0] - ms[0] + md[0]).clamp(0.0, 1.0),
                (sc[1] - ms[1] + md[1]).clamp(0.0, 1.0),
                (sc[2] - ms[2] + md[2]).clamp(0.0, 1.0),
            ];
            paint_over(b, t, a * s[3] as f32 / 255.0, lock)
        }
        StrokeSource::Pattern(pat) => {
            let (pw, ph) = (pat.width() as i32, pat.height() as i32);
            let s = pat.get(x.rem_euclid(pw.max(1)), y.rem_euclid(ph.max(1)));
            paint_over(b, f(s), a * s[3] as f32 / 255.0, lock)
        }
        StrokeSource::Dodge { range, exposure } | StrokeSource::Burn { range, exposure } => {
            if b[3] == 0 {
                return b;
            }
            let dodge = matches!(source, StrokeSource::Dodge { .. });
            let c = f(b);
            let w = range.weight(lum(c)) * exposure;
            let t = c.map(|v| if dodge { 1.0 - (1.0 - v) * (1.0 - w * 0.6) } else { v * (1.0 - w * 0.6) });
            mix_rgb(b, t, a)
        }
        StrokeSource::Sponge { amount } => {
            let [h, s, l] = rgb_to_hsl(f(b));
            let s2 = (s + amount * if *amount > 0.0 { 1.0 - s } else { s }).clamp(0.0, 1.0);
            mix_rgb(b, hsl_to_rgb([h, s2, l]), a)
        }
        StrokeSource::Blur { strength } | StrokeSource::Sharpen { strength } => {
            let mut acc = [0.0f32; 3];
            for dy in -2..=2 {
                for dx in -2..=2 {
                    let c = f(base.get(x + dx, y + dy));
                    for k in 0..3 {
                        acc[k] += c[k] / 25.0;
                    }
                }
            }
            let c = f(b);
            let t = if matches!(source, StrokeSource::Blur { .. }) {
                acc
            } else {
                [
                    (c[0] + (c[0] - acc[0]) * 1.5).clamp(0.0, 1.0),
                    (c[1] + (c[1] - acc[1]) * 1.5).clamp(0.0, 1.0),
                    (c[2] + (c[2] - acc[2]) * 1.5).clamp(0.0, 1.0),
                ]
            };
            mix_rgb(b, t, a * strength)
        }
        StrokeSource::ColorReplace(col) => {
            let t = crate::blend::blend_rgb(crate::BlendMode::Color, f(b), col.rgb_array());
            mix_rgb(b, t, a)
        }
        StrokeSource::BackgroundErase { sample, tolerance } => {
            let tol = *tolerance as i32;
            let close = (0..3).all(|k| (b[k] as i32 - sample[k] as i32).abs() <= tol);
            if close && !lock {
                let mut o = b;
                o[3] = (b[3] as f32 * (1.0 - a) + 0.5) as u8;
                o
            } else {
                b
            }
        }
        StrokeSource::RedEye => {
            let c = f(b);
            let gb = (c[1] + c[2]) * 0.5;
            if c[0] > gb * 1.3 {
                let t = [gb * 0.8, c[1] * 0.8, c[2] * 0.8];
                mix_rgb(b, t, a)
            } else {
                b
            }
        }
        StrokeSource::Smudge { .. } | StrokeSource::Liquify { .. } => b,
    }
}

/// Premultiplied bilinear sample of a straight-alpha buffer.
pub fn sample_bilinear(buf: &TiledBuffer<Rgba8>, x: f32, y: f32) -> Rgba8 {
    let (x, y) = (x - 0.5, y - 0.5);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (xi, yi) = (x0 as i32, y0 as i32);
    let mut acc = [0.0f32; 4];
    for (dx, dy, w) in [(0, 0, (1.0 - fx) * (1.0 - fy)), (1, 0, fx * (1.0 - fy)), (0, 1, (1.0 - fx) * fy), (1, 1, fx * fy)] {
        let p = buf.get(xi + dx, yi + dy);
        let a = p[3] as f32 * w;
        acc[0] += p[0] as f32 * a;
        acc[1] += p[1] as f32 * a;
        acc[2] += p[2] as f32 * a;
        acc[3] += a;
    }
    if acc[3] <= 0.0 {
        return [0, 0, 0, 0];
    }
    [
        (acc[0] / acc[3] + 0.5) as u8,
        (acc[1] / acc[3] + 0.5) as u8,
        (acc[2] / acc[3] + 0.5) as u8,
        (acc[3] + 0.5).min(255.0) as u8,
    ]
}

/// Source-over of a straight colour with alpha `a` onto a straight pixel.
#[inline]
pub fn paint_over(b: Rgba8, c: [f32; 3], a: f32, lock_alpha: bool) -> Rgba8 {
    if a <= 0.0 {
        return b;
    }
    let ba = b[3] as f32 / 255.0;
    if lock_alpha {
        let mix = |i: usize| b[i] as f32 / 255.0 * (1.0 - a) + c[i] * a;
        return [q(mix(0)), q(mix(1)), q(mix(2)), b[3]];
    }
    let oa = a + ba * (1.0 - a);
    if oa <= 0.0 {
        return [0, 0, 0, 0];
    }
    let ch = |i: usize| (c[i] * a + b[i] as f32 / 255.0 * ba * (1.0 - a)) / oa;
    [q(ch(0)), q(ch(1)), q(ch(2)), q(oa)]
}

#[inline]
fn q(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Builds a brush tip from an image region (luminance inverted, like
/// Photoshop: dark pixels paint) — "Edit > Define Brush Preset".
pub fn tip_from_buffer(buf: &TiledBuffer<Rgba8>, rect: IRect) -> Option<BrushTip> {
    let rect = rect.clamp_to(buf.width(), buf.height());
    if rect.is_empty() {
        return None;
    }
    let px = buf.region_to_dense(rect);
    let data = px
        .iter()
        .map(|p| {
            let a = p[3] as f32 / 255.0;
            ((1.0 - lum(f(*p))) * a * 255.0).round() as u8
        })
        .collect();
    Some(BrushTip { width: rect.width() as u32, height: rect.height() as u32, data })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stroke_paints_and_respects_opacity() {
        let base = TiledBuffer::new(100, 100);
        let mut target = base.clone();
        let settings = BrushSettings { size: 10.0, hardness: 1.0, opacity: 0.5, ..Default::default() };
        let mut s = Stroke::on_pixels(settings, StrokeSource::Color(Color::rgb(1.0, 0.0, 0.0)), base, None, false);
        s.stroke_to(Point::new(20.0, 50.0), StrokeTarget::Pixels(&mut target));
        // Re-crossing the same spot must not exceed the stroke opacity.
        for i in 0..10 {
            let x = 20.0 + (i % 2) as f32 * 40.0;
            s.stroke_to(Point::new(x, 50.0), StrokeTarget::Pixels(&mut target));
        }
        let p = target.get(40, 50);
        assert_eq!(p[0], 255);
        assert_eq!(p[3], 128);
        assert_eq!(target.get(40, 20), [0, 0, 0, 0]);
        assert!(s.touched().unwrap().contains(55, 50));
    }

    #[test]
    fn eraser_and_selection() {
        let base = TiledBuffer::solid(50, 50, [10, 20, 30, 255]);
        let mut target = base.clone();
        let sel = crate::selection::rect_mask(50, 50, IRect::new(0, 0, 25, 50));
        let settings = BrushSettings { size: 50.0, hardness: 1.0, ..Default::default() };
        let mut s = Stroke::on_pixels(settings, StrokeSource::Erase, base, Some(sel), false);
        s.stroke_to(Point::new(25.0, 25.0), StrokeTarget::Pixels(&mut target));
        assert_eq!(target.get(20, 25)[3], 0);
        assert_eq!(target.get(30, 25)[3], 255, "outside the selection is protected");
    }

    #[test]
    fn symmetry_and_retouch() {
        let base = TiledBuffer::solid(100, 100, [128, 128, 128, 255]);
        let settings = BrushSettings { size: 10.0, hardness: 1.0, ..Default::default() };
        let mut t = base.clone();
        let mut s = Stroke::on_pixels(settings, StrokeSource::Color(Color::BLACK), base.clone(), None, false)
            .with_symmetry(Symmetry::Vertical);
        s.stroke_to(Point::new(20.0, 50.0), StrokeTarget::Pixels(&mut t));
        assert_eq!(t.get(20, 50)[0], 0);
        assert_eq!(t.get(79, 50)[0], 0, "mirrored dab");

        for (src, brighter) in [
            (StrokeSource::Dodge { range: ToneRange::Midtones, exposure: 1.0 }, true),
            (StrokeSource::Burn { range: ToneRange::Midtones, exposure: 1.0 }, false),
        ] {
            let mut t = base.clone();
            let mut s = Stroke::on_pixels(settings, src, base.clone(), None, false);
            s.stroke_to(Point::new(50.0, 50.0), StrokeTarget::Pixels(&mut t));
            assert_eq!(t.get(50, 50)[0] > 128, brighter);
        }
    }

    #[test]
    fn liquify_push() {
        let mut base = TiledBuffer::solid(60, 20, [255, 255, 255, 255]);
        for y in 0..20 {
            for x in 0..30 {
                base.set(x, y, [0, 0, 0, 255]);
            }
        }
        let mut t = base.clone();
        let settings = BrushSettings { size: 16.0, hardness: 0.5, spacing: 0.1, ..Default::default() };
        let src = StrokeSource::Liquify { mode: LiquifyMode::Push, strength: 1.0 };
        let mut s = Stroke::on_pixels(settings, src, base, None, false);
        for x in (26..36).step_by(1) {
            s.stroke_to(Point::new(x as f32, 10.0), StrokeTarget::Pixels(&mut t));
        }
        assert!(t.get(32, 10)[0] < 128, "black pushed right: {:?}", t.get(32, 10));
    }

    #[test]
    fn smudge_moves_paint() {
        let mut base = TiledBuffer::solid(60, 20, [255, 255, 255, 255]);
        for y in 0..20 {
            for x in 0..20 {
                base.set(x, y, [0, 0, 0, 255]);
            }
        }
        let mut t = base.clone();
        let settings = BrushSettings { size: 8.0, hardness: 1.0, spacing: 0.1, ..Default::default() };
        let mut s = Stroke::on_pixels(settings, StrokeSource::Smudge { strength: 0.9, paint: None }, base, None, false);
        for x in (15..40).step_by(2) {
            s.stroke_to(Point::new(x as f32, 10.0), StrokeTarget::Pixels(&mut t));
        }
        assert!(t.get(28, 10)[0] < 200, "dark paint dragged right: {:?}", t.get(28, 10));
    }
}
