//! Brush engine (Brush, Pencil, Eraser, Clone Stamp).
//!
//! Strokes work like Photoshop's: every dab accumulates into a floating-
//! point *coverage* buffer (controlled by Flow), and the layer is re-rendered
//! from the stroke's starting snapshot using `coverage × Opacity`. Overlapping
//! dabs within one stroke therefore never exceed the stroke opacity.

use serde::{Deserialize, Serialize};

use crate::buffer::TiledBuffer;
use crate::color::{lum, Color, Rgba8};
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
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self { size: 20.0, hardness: 0.8, opacity: 1.0, flow: 1.0, spacing: 0.15, antialias: true }
    }
}

impl BrushSettings {
    pub fn pencil(size: f32) -> Self {
        Self { size, hardness: 1.0, spacing: 0.1, antialias: false, ..Default::default() }
    }
}

/// What a stroke deposits.
#[derive(Clone, Debug)]
pub enum StrokeSource {
    Color(Color),
    Erase,
    /// Clone Stamp: copies from `source` displaced by `(dx, dy)`.
    Clone { source: TiledBuffer<Rgba8>, dx: i32, dy: i32 },
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
    last: Option<Point>,
    /// Distance travelled since the last dab.
    carry: f32,
    touched: Option<IRect>,
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
            last: None,
            carry: 0.0,
            touched: None,
        }
    }

    /// Bounding box of everything the stroke modified so far.
    pub fn touched(&self) -> Option<IRect> {
        self.touched
    }

    /// Extends the stroke to `p`, placing dabs along the way, and re-renders
    /// the affected pixels into `target`. Returns the modified rectangle.
    pub fn stroke_to(&mut self, p: Point, target: StrokeTarget<'_>) -> Option<IRect> {
        let step = (self.settings.spacing * self.settings.size).max(0.5);
        let mut dirty = None;
        match self.last {
            None => {
                dirty = self.dab(p);
                self.carry = 0.0;
            }
            Some(last) => {
                let dist = last.distance(p);
                let mut t = step - self.carry;
                while t <= dist {
                    dirty = union_opt(dirty, self.dab(last.lerp(p, t / dist)));
                    t += step;
                }
                self.carry = dist - (t - step);
            }
        }
        self.last = Some(p);
        if let Some(r) = dirty {
            self.render(r, target);
            self.touched = union_opt(self.touched, Some(r));
        }
        dirty
    }

    fn dab(&mut self, c: Point) -> Option<IRect> {
        let s = self.settings;
        let r = (s.size * 0.5).max(0.5);
        let rect = IRect::around(c, r).clamp_to(self.cov.width(), self.cov.height());
        if rect.is_empty() {
            return None;
        }
        let flow = s.flow.clamp(0.0, 1.0);
        let hard = s.hardness.clamp(0.0, 1.0);
        self.cov.for_each_in_rect_mut(rect, |x, y, v| {
            let dx = x as f32 + 0.5 - c.x;
            let dy = y as f32 + 0.5 - c.y;
            let d = (dx * dx + dy * dy).sqrt();
            let a = if !s.antialias {
                if r <= 0.75 {
                    // single pixel pencil
                    ((x as f32 - c.x.floor()).abs() < 0.5 && (y as f32 - c.y.floor()).abs() < 0.5) as u8 as f32
                } else {
                    (d <= r) as u8 as f32
                }
            } else {
                let edge = (r - d + 0.5).clamp(0.0, 1.0);
                if hard >= 1.0 {
                    edge
                } else {
                    let t = ((d / r - hard) / (1.0 - hard)).clamp(0.0, 1.0);
                    let soft = 1.0 - t * t * (3.0 - 2.0 * t);
                    soft * edge
                }
            };
            if a > 0.0 {
                *v += (1.0 - *v) * a * flow;
            }
        });
        Some(rect)
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
                    *px = match source {
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
                            let sc = [s[0] as f32 / 255.0, s[1] as f32 / 255.0, s[2] as f32 / 255.0];
                            paint_over(b, sc, a * s[3] as f32 / 255.0, lock)
                        }
                    };
                });
            }
            (StrokeTarget::Mask(dst), Base::Mask(base)) => {
                let value = match &self.source {
                    StrokeSource::Color(c) => lum(c.rgb_array()) * 255.0,
                    StrokeSource::Erase => 255.0,
                    StrokeSource::Clone { .. } => 255.0,
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
}
