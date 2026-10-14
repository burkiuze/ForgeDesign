//! Tools: state, options bar, and pointer handling on the canvas.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{self, Modifiers};
use forge_core::adjust;
use forge_core::blend::BlendMode;
use forge_core::brush::{BrushSettings, BrushTip, LiquifyMode, Stroke, StrokeSource, StrokeTarget, Symmetry, ToneRange};
use forge_core::buffer::TiledBuffer;
use forge_core::composite;
use forge_core::fill::{self, GradientKind};
use forge_core::layer::{TextAlign, TextData};
use forge_core::paths::{Anchor, SubPath, VectorPath};
use forge_core::retouch;
use forge_core::selection::{self, Mask, SelectionOp};
use forge_core::shapes::{self, ShapeKind, ShapeParams};
use forge_core::transform;
use forge_core::{Color, IRect, Layer, Point, Rgba8};

use crate::canvas::DocTab;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    Move,
    Marquee,
    EllipseMarquee,
    RowMarquee,
    ColumnMarquee,
    Lasso,
    PolygonLasso,
    MagneticLasso,
    QuickSelect,
    MagicWand,
    Crop,
    PerspectiveCrop,
    Eyedropper,
    Ruler,
    SpotHealing,
    Healing,
    Patch,
    RedEye,
    Brush,
    Pencil,
    ColorReplace,
    Mixer,
    CloneStamp,
    PatternStamp,
    Eraser,
    BackgroundEraser,
    MagicEraser,
    Gradient,
    Bucket,
    Blur,
    Sharpen,
    Smudge,
    Dodge,
    Burn,
    Sponge,
    Liquify,
    Pen,
    FreeformPen,
    PathSelect,
    DirectSelect,
    Text,
    VerticalText,
    Shape,
    Hand,
    Zoom,
}

/// Toolbar slots; each slot is a flyout group like in Photoshop.
pub const GROUPS: &[&[Tool]] = &[
    &[Tool::Move],
    &[Tool::Marquee, Tool::EllipseMarquee, Tool::RowMarquee, Tool::ColumnMarquee],
    &[Tool::Lasso, Tool::PolygonLasso, Tool::MagneticLasso],
    &[Tool::QuickSelect, Tool::MagicWand],
    &[Tool::Crop, Tool::PerspectiveCrop],
    &[Tool::Eyedropper, Tool::Ruler],
    &[Tool::SpotHealing, Tool::Healing, Tool::Patch, Tool::RedEye],
    &[Tool::Brush, Tool::Pencil, Tool::ColorReplace, Tool::Mixer],
    &[Tool::CloneStamp, Tool::PatternStamp],
    &[Tool::Eraser, Tool::BackgroundEraser, Tool::MagicEraser],
    &[Tool::Gradient, Tool::Bucket],
    &[Tool::Blur, Tool::Sharpen, Tool::Smudge],
    &[Tool::Dodge, Tool::Burn, Tool::Sponge],
    &[Tool::Liquify],
    &[Tool::Pen, Tool::FreeformPen],
    &[Tool::Text, Tool::VerticalText],
    &[Tool::PathSelect, Tool::DirectSelect],
    &[Tool::Shape],
    &[Tool::Hand],
    &[Tool::Zoom],
];

impl Tool {
    pub fn name(self) -> &'static str {
        match self {
            Tool::Move => "Move",
            Tool::Marquee => "Rectangular Marquee",
            Tool::EllipseMarquee => "Elliptical Marquee",
            Tool::RowMarquee => "Single Row Marquee",
            Tool::ColumnMarquee => "Single Column Marquee",
            Tool::Lasso => "Lasso",
            Tool::PolygonLasso => "Polygonal Lasso",
            Tool::MagneticLasso => "Magnetic Lasso",
            Tool::QuickSelect => "Quick Selection",
            Tool::MagicWand => "Magic Wand",
            Tool::Crop => "Crop",
            Tool::PerspectiveCrop => "Perspective Crop",
            Tool::Eyedropper => "Eyedropper",
            Tool::Ruler => "Ruler",
            Tool::SpotHealing => "Spot Healing Brush",
            Tool::Healing => "Healing Brush",
            Tool::Patch => "Patch",
            Tool::RedEye => "Red Eye",
            Tool::Brush => "Brush",
            Tool::Pencil => "Pencil",
            Tool::ColorReplace => "Color Replacement",
            Tool::Mixer => "Mixer Brush",
            Tool::CloneStamp => "Clone Stamp",
            Tool::PatternStamp => "Pattern Stamp",
            Tool::Eraser => "Eraser",
            Tool::BackgroundEraser => "Background Eraser",
            Tool::MagicEraser => "Magic Eraser",
            Tool::Gradient => "Gradient",
            Tool::Bucket => "Paint Bucket",
            Tool::Blur => "Blur",
            Tool::Sharpen => "Sharpen",
            Tool::Smudge => "Smudge",
            Tool::Dodge => "Dodge",
            Tool::Burn => "Burn",
            Tool::Sponge => "Sponge",
            Tool::Liquify => "Liquify",
            Tool::Pen => "Pen",
            Tool::FreeformPen => "Freeform Pen",
            Tool::PathSelect => "Path Selection",
            Tool::DirectSelect => "Direct Selection",
            Tool::Text => "Horizontal Type",
            Tool::VerticalText => "Vertical Type",
            Tool::Shape => "Shape",
            Tool::Hand => "Hand",
            Tool::Zoom => "Zoom",
        }
    }

    /// Group shortcut key (Shift+key cycles within the group).
    pub fn group_key(group: usize) -> Option<egui::Key> {
        use egui::Key::*;
        Some(match group {
            0 => V,
            1 => M,
            2 => L,
            3 => W,
            4 => C,
            5 => I,
            6 => J,
            7 => B,
            8 => S,
            9 => E,
            10 => G,
            11 => R,
            12 => O,
            14 => P,
            15 => T,
            16 => A,
            17 => U,
            18 => H,
            19 => Z,
            _ => return None,
        })
    }

    pub fn group_of(self) -> usize {
        GROUPS.iter().position(|g| g.contains(&self)).unwrap_or(0)
    }

    /// Tools that paint with a round brush (show a brush cursor).
    pub fn is_paint(self) -> bool {
        matches!(
            self,
            Tool::Brush
                | Tool::Pencil
                | Tool::Eraser
                | Tool::CloneStamp
                | Tool::PatternStamp
                | Tool::SpotHealing
                | Tool::Healing
                | Tool::RedEye
                | Tool::ColorReplace
                | Tool::Mixer
                | Tool::BackgroundEraser
                | Tool::Blur
                | Tool::Sharpen
                | Tool::Smudge
                | Tool::Dodge
                | Tool::Burn
                | Tool::Sponge
                | Tool::Liquify
                | Tool::QuickSelect
        )
    }

    /// Tools whose pointer positions snap to guides/grid.
    pub fn snaps(self) -> bool {
        matches!(
            self,
            Tool::Marquee | Tool::EllipseMarquee | Tool::Crop | Tool::Shape | Tool::Gradient | Tool::Pen | Tool::PolygonLasso | Tool::Ruler
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransformMode {
    Free,
    Skew,
    Distort,
    Perspective,
    Warp,
}

impl TransformMode {
    pub fn name(self) -> &'static str {
        match self {
            TransformMode::Free => "Free Transform",
            TransformMode::Skew => "Skew",
            TransformMode::Distort => "Distort",
            TransformMode::Perspective => "Perspective",
            TransformMode::Warp => "Warp",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Handle {
    Corner(usize),
    Edge(usize),
    Rotate,
    Move,
    WarpPoint(usize),
}

/// On-canvas Free Transform / Warp session (Ctrl+T).
pub struct TransformSession {
    pub mode: TransformMode,
    pub layer: usize,
    base: TiledBuffer<Rgba8>,
    base_sel: Option<Mask>,
    pub quad: [Point; 4],
    pub grid: [Point; 16],
    drag: Option<(Handle, Point, [Point; 4], [Point; 16])>,
    dirty: bool,
    smart: bool,
}

impl TransformSession {
    pub fn center(&self) -> Point {
        let mut c = Point::new(0.0, 0.0);
        for p in self.quad {
            c.x += p.x * 0.25;
            c.y += p.y * 0.25;
        }
        c
    }
}

/// In-progress pointer interaction.
pub enum Interaction {
    None,
    Stroke { stroke: Box<Stroke>, label: &'static str },
    SpotHeal { stroke: Box<Stroke>, base: TiledBuffer<Rgba8> },
    Move { base: TiledBuffer<Rgba8>, base_sel: Option<Mask>, start: Point, last: (i32, i32), bounds: IRect },
    Marquee { start: Point, current: Point, op: SelectionOp },
    Lasso { points: Vec<Point>, op: SelectionOp, magnetic: Option<TiledBuffer<Rgba8>> },
    QuickSelect { src: TiledBuffer<Rgba8>, acc: Mask, subtract: bool, last: Option<Point> },
    Patch { start: Point, current: Point },
    Gradient { start: Point, current: Point },
    Shape { start: Point, current: Point },
    Crop { start: Point, current: Point },
    Ruler { start: Point, current: Point },
    PenDrag { anchor: usize },
    Freeform { points: Vec<Point> },
    PathDrag { kind: PathHandle, last: Point },
    Eyedropper,
    Transform,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathHandle {
    Whole,
    Anchor(usize, usize),
    In(usize, usize),
    Out(usize, usize),
}

pub struct ToolState {
    pub tool: Tool,
    /// The tool shown in each toolbar slot.
    pub group_choice: Vec<Tool>,
    pub fg: Color,
    pub bg: Color,
    pub brushes: HashMap<Tool, BrushSettings>,
    pub tip: Option<Arc<BrushTip>>,
    pub symmetry: Symmetry,
    pub sel_op: SelectionOp,
    pub feather: f32,
    pub wand_tolerance: u8,
    pub wand_contiguous: bool,
    pub sample_merged: bool,
    pub bucket_tolerance: u8,
    pub bucket_contiguous: bool,
    pub fill_opacity: f32,
    pub fill_mode: BlendMode,
    pub gradient_kind: GradientKind,
    pub gradient_reverse: bool,
    pub gradient_to_transparent: bool,
    /// Custom gradient stops (position, colour); empty = FG → BG.
    pub gradient_stops: Vec<(f32, Color)>,
    pub shape_kind: ShapeKind,
    pub shape_params: ShapeParams,
    pub shape_fill: bool,
    pub shape_stroke: bool,
    pub shape_stroke_width: f32,
    pub shape_stroke_color: Color,
    pub shape_dashed: bool,
    pub shape_new_layer: bool,
    pub text: TextData,
    pub font_index: usize,
    pub tone_range: ToneRange,
    pub exposure: f32,
    pub sponge_saturate: bool,
    pub strength: f32,
    pub finger_painting: bool,
    pub wetness: f32,
    pub liquify_mode: LiquifyMode,
    pub bg_tolerance: u8,
    pub pattern: Arc<TiledBuffer<Rgba8>>,
    pub crop_ratio: Option<(f32, f32)>,
    pub clone_source: Option<Point>,
    pub clone_offset: Option<(i32, i32)>,
    pub interaction: Interaction,
    pub transform: Option<TransformSession>,
    pub persp_crop: Option<[Point; 4]>,
    pub persp_drag: Option<usize>,
    pub ruler: Option<(Point, Point)>,
    /// Pen tool: whether clicks extend the last sub-path of the active path.
    pub pen_building: bool,
    pub path_stroke_width: f32,
    /// Set by the Text tool on click; the app opens the text dialog.
    pub text_request: Option<(Point, Option<usize>)>,
    pub status: String,
}

fn default_pattern() -> TiledBuffer<Rgba8> {
    let mut p = TiledBuffer::new(16, 16);
    p.par_map_all(|x, y, _| if (x / 8 + y / 8) % 2 == 0 { [60, 60, 60, 255] } else { [200, 200, 200, 255] });
    p
}

impl Default for ToolState {
    fn default() -> Self {
        let mut brushes = HashMap::new();
        let soft = BrushSettings { size: 40.0, hardness: 0.5, ..Default::default() };
        for t in [
            Tool::Brush,
            Tool::ColorReplace,
            Tool::Mixer,
            Tool::BackgroundEraser,
            Tool::QuickSelect,
            Tool::RedEye,
        ] {
            brushes.insert(t, BrushSettings::default());
        }
        brushes.insert(Tool::Pencil, BrushSettings::pencil(1.0));
        for t in [Tool::Eraser, Tool::CloneStamp, Tool::PatternStamp, Tool::Healing, Tool::SpotHealing, Tool::Blur, Tool::Sharpen, Tool::Smudge, Tool::Dodge, Tool::Burn, Tool::Sponge] {
            brushes.insert(t, soft);
        }
        brushes.insert(Tool::Liquify, BrushSettings { size: 80.0, hardness: 0.3, spacing: 0.08, ..Default::default() });
        Self {
            tool: Tool::Brush,
            group_choice: GROUPS.iter().map(|g| g[0]).collect(),
            fg: Color::BLACK,
            bg: Color::WHITE,
            brushes,
            tip: None,
            symmetry: Symmetry::None,
            sel_op: SelectionOp::Replace,
            feather: 0.0,
            wand_tolerance: 32,
            wand_contiguous: true,
            sample_merged: false,
            bucket_tolerance: 32,
            bucket_contiguous: true,
            fill_opacity: 1.0,
            fill_mode: BlendMode::Normal,
            gradient_kind: GradientKind::Linear,
            gradient_reverse: false,
            gradient_to_transparent: false,
            gradient_stops: Vec::new(),
            shape_kind: ShapeKind::Rectangle,
            shape_params: ShapeParams::default(),
            shape_fill: true,
            shape_stroke: false,
            shape_stroke_width: 3.0,
            shape_stroke_color: Color::BLACK,
            shape_dashed: false,
            shape_new_layer: true,
            text: TextData {
                text: String::new(),
                font_path: None,
                font_name: String::new(),
                size: 48.0,
                color: Color::BLACK,
                bold: false,
                italic: false,
                underline: false,
                tracking: 0.0,
                leading: 1.2,
                align: TextAlign::Left,
                vertical: false,
                box_width: None,
                warp: 0.0,
                x: 0.0,
                y: 0.0,
                antialias: true,
            },
            font_index: 0,
            tone_range: ToneRange::Midtones,
            exposure: 0.5,
            sponge_saturate: false,
            strength: 0.5,
            finger_painting: false,
            wetness: 0.5,
            liquify_mode: LiquifyMode::Push,
            bg_tolerance: 40,
            pattern: Arc::new(default_pattern()),
            crop_ratio: None,
            clone_source: None,
            clone_offset: None,
            interaction: Interaction::None,
            transform: None,
            persp_crop: None,
            persp_drag: None,
            ruler: None,
            pen_building: false,
            path_stroke_width: 3.0,
            text_request: None,
            status: String::new(),
        }
    }
}

fn op_from_mods(base: SelectionOp, m: Modifiers) -> SelectionOp {
    match (m.shift, m.alt) {
        (true, true) => SelectionOp::Intersect,
        (true, false) => SelectionOp::Add,
        (false, true) => SelectionOp::Subtract,
        _ => base,
    }
}

/// Snaps the vector `a → b` to multiples of 45° (Shift while dragging).
fn constrain45(a: Point, b: Point) -> Point {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = (dx * dx + dy * dy).sqrt();
    let ang = (dy.atan2(dx) / std::f32::consts::FRAC_PI_4).round() * std::f32::consts::FRAC_PI_4;
    Point::new(a.x + len * ang.cos(), a.y + len * ang.sin())
}

/// Aspect-ratio constraint for marquees, crop and shapes.
fn constrain_ratio(a: Point, b: Point, rw: f32, rh: f32) -> Point {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let s = (dx.abs() / rw).max(dy.abs() / rh);
    Point::new(a.x + s * rw * dx.signum(), a.y + s * rh * dy.signum())
}

impl ToolState {
    pub fn select_tool(&mut self, t: Tool) {
        if self.is_busy() {
            return;
        }
        self.tool = t;
        let g = t.group_of();
        self.group_choice[g] = t;
        self.pen_building = false;
    }

    pub fn swap_colors(&mut self) {
        std::mem::swap(&mut self.fg, &mut self.bg);
    }

    pub fn reset_colors(&mut self) {
        self.fg = Color::BLACK;
        self.bg = Color::WHITE;
    }

    pub fn current_brush(&mut self) -> Option<&mut BrushSettings> {
        if self.tool.is_paint() && self.tool != Tool::QuickSelect {
            return self.brushes.get_mut(&self.tool);
        }
        if self.tool == Tool::QuickSelect {
            return self.brushes.get_mut(&Tool::QuickSelect);
        }
        None
    }

    pub fn brush_size(&self) -> Option<f32> {
        self.tool.is_paint().then(|| self.brushes.get(&self.tool).map_or(20.0, |b| b.size))
    }

    pub fn is_busy(&self) -> bool {
        !matches!(self.interaction, Interaction::None)
    }

    fn sample_color(tab: &DocTab, p: Point) -> Option<Color> {
        let (x, y) = (p.x.floor() as i32, p.y.floor() as i32);
        if !tab.doc.state.bounds().contains(x, y) {
            return None;
        }
        let px = composite::composite_rect(&tab.doc.state, IRect::new(x, y, x + 1, y + 1), composite::Background::Transparent);
        let c = px[0];
        (c[3] > 0).then(|| Color::from_rgba8([c[0], c[1], c[2], 255]))
    }

    fn sample_source(tab: &DocTab, merged: bool) -> TiledBuffer<Rgba8> {
        if merged {
            composite::merged(&tab.doc.state)
        } else {
            tab.doc
                .state
                .active_layer()
                .map(|l| l.pixels.clone())
                .unwrap_or_else(|| TiledBuffer::new(tab.doc.width(), tab.doc.height()))
        }
    }

    /// Returns `Err(message)` when the active layer cannot be edited.
    fn editable_layer(tab: &DocTab) -> Result<&Layer, String> {
        let l = tab.doc.state.active_layer().ok_or("No layer")?;
        if l.locked {
            return Err(format!("Layer \"{}\" is locked", l.name));
        }
        if !l.has_pixels() && !(tab.doc.state.editing_mask && l.mask.is_some()) {
            return Err(format!("\"{}\" is not a pixel layer", l.name));
        }
        if l.smart.is_some() && !tab.doc.state.editing_mask {
            return Err("Smart Objects must be rasterized before painting (Layer > Rasterize)".into());
        }
        if !l.visible {
            return Err(format!("Layer \"{}\" is hidden", l.name));
        }
        Ok(l)
    }

    fn brush_source(&self, tab: &DocTab, p: Point) -> Option<(BrushSettings, StrokeSource, &'static str)> {
        let b = *self.brushes.get(&self.tool)?;
        let s = self.strength;
        Some(match self.tool {
            Tool::Brush => (b, StrokeSource::Color(self.fg), "Brush Tool"),
            Tool::Pencil => (b, StrokeSource::Color(self.fg), "Pencil"),
            Tool::Eraser => (b, StrokeSource::Erase, "Eraser"),
            Tool::ColorReplace => (b, StrokeSource::ColorReplace(self.fg), "Color Replacement"),
            Tool::Mixer => (
                b,
                StrokeSource::Smudge { strength: 0.8, paint: Some((self.fg, self.wetness)) },
                "Mixer Brush",
            ),
            Tool::BackgroundEraser => {
                let l = tab.doc.state.active_layer()?;
                let sample = l.pixels.get(p.x.floor() as i32, p.y.floor() as i32);
                (b, StrokeSource::BackgroundErase { sample, tolerance: self.bg_tolerance }, "Background Eraser")
            }
            Tool::Blur => (b, StrokeSource::Blur { strength: s }, "Blur Tool"),
            Tool::Sharpen => (b, StrokeSource::Sharpen { strength: s }, "Sharpen Tool"),
            Tool::Smudge => (
                b,
                StrokeSource::Smudge { strength: s, paint: self.finger_painting.then_some((self.fg, 0.5)) },
                "Smudge Tool",
            ),
            Tool::Dodge => (b, StrokeSource::Dodge { range: self.tone_range, exposure: self.exposure }, "Dodge"),
            Tool::Burn => (b, StrokeSource::Burn { range: self.tone_range, exposure: self.exposure }, "Burn"),
            Tool::Sponge => (
                b,
                StrokeSource::Sponge { amount: if self.sponge_saturate { s } else { -s } },
                "Sponge",
            ),
            Tool::Liquify => (b, StrokeSource::Liquify { mode: self.liquify_mode, strength: s }, "Liquify"),
            Tool::PatternStamp => (b, StrokeSource::Pattern(self.pattern.clone()), "Pattern Stamp"),
            Tool::RedEye => (b, StrokeSource::RedEye, "Red Eye"),
            _ => return None,
        })
    }

    pub fn pointer_down(&mut self, tab: &mut DocTab, p: Point, mods: Modifiers, double: bool) {
        self.status.clear();
        if self.transform.is_some() {
            self.transform_down(tab, p);
            return;
        }
        // Alt+click with a painting tool picks a colour (Photoshop behaviour).
        if mods.alt
            && matches!(self.tool, Tool::Brush | Tool::Pencil | Tool::Bucket | Tool::Gradient | Tool::Shape | Tool::ColorReplace | Tool::Mixer)
        {
            if let Some(c) = Self::sample_color(tab, p) {
                self.fg = c;
            }
            self.interaction = Interaction::Eyedropper;
            return;
        }
        let (w, h) = (tab.doc.width(), tab.doc.height());
        match self.tool {
            Tool::CloneStamp | Tool::Healing => {
                if mods.alt {
                    self.clone_source = Some(p);
                    self.clone_offset = None;
                    self.status = "Source set".into();
                    return;
                }
                let layer = match Self::editable_layer(tab) {
                    Ok(l) => l.clone(),
                    Err(e) => return self.status = e,
                };
                let Some(src) = self.clone_source else {
                    self.status = "Alt+click to set the source first".into();
                    return;
                };
                let off = *self.clone_offset.get_or_insert(((src.x - p.x).round() as i32, (src.y - p.y).round() as i32));
                let source = Self::sample_source(tab, self.sample_merged);
                let b = self.brushes[&self.tool];
                let (src_kind, label) = if self.tool == Tool::Healing {
                    (StrokeSource::Heal { source, dx: off.0, dy: off.1 }, "Healing Brush")
                } else {
                    (StrokeSource::Clone { source, dx: off.0, dy: off.1 }, "Clone Stamp")
                };
                let stroke = self.make_stroke(tab, &layer, b, src_kind);
                self.begin_stroke(tab, stroke, label, p);
            }
            Tool::SpotHealing => {
                let layer = match Self::editable_layer(tab) {
                    Ok(l) => l.clone(),
                    Err(e) => return self.status = e,
                };
                let b = self.brushes[&self.tool];
                let base = layer.pixels.clone();
                let mut stroke = Box::new(Stroke::on_pixels(b, StrokeSource::Color(Color::rgba(0.1, 0.1, 0.1, 0.5)), base.clone(), None, false));
                if let Some(r) = Self::stroke_into(&mut stroke, tab, p, false) {
                    tab.doc.mark_dirty(r);
                }
                self.interaction = Interaction::SpotHeal { stroke, base };
            }
            t if self.brushes.contains_key(&t) && t != Tool::QuickSelect => {
                let layer = match Self::editable_layer(tab) {
                    Ok(l) => l.clone(),
                    Err(e) => return self.status = e,
                };
                let Some((b, src, label)) = self.brush_source(tab, p) else { return };
                let stroke = self.make_stroke(tab, &layer, b, src);
                self.begin_stroke(tab, stroke, label, p);
            }
            Tool::Move => {
                let layer = match Self::editable_layer(tab) {
                    Ok(l) => l,
                    Err(e) => return self.status = e,
                };
                let base = layer.pixels.clone();
                let base_sel = tab.doc.state.selection.clone();
                let bounds = match &base_sel {
                    Some(s) => s.content_bounds().unwrap_or_default(),
                    None => base.content_bounds().unwrap_or_default(),
                };
                self.interaction = Interaction::Move { base, base_sel, start: p, last: (0, 0), bounds };
            }
            Tool::Marquee | Tool::EllipseMarquee => {
                self.interaction = Interaction::Marquee { start: p, current: p, op: op_from_mods(self.sel_op, mods) };
            }
            Tool::RowMarquee | Tool::ColumnMarquee => {
                let m = if self.tool == Tool::RowMarquee {
                    selection::row_mask(w, h, p.y.floor() as i32)
                } else {
                    selection::column_mask(w, h, p.x.floor() as i32)
                };
                let sel = selection::combine(tab.doc.selection(), m, op_from_mods(self.sel_op, mods));
                tab.doc.set_selection(sel);
                tab.doc.commit(self.tool.name());
            }
            Tool::Lasso | Tool::MagneticLasso => {
                let magnetic = (self.tool == Tool::MagneticLasso).then(|| composite::merged(&tab.doc.state));
                self.interaction = Interaction::Lasso { points: vec![p], op: op_from_mods(self.sel_op, mods), magnetic };
            }
            Tool::PolygonLasso => {
                // Click-by-click polygon; handled across clicks.
                if let Interaction::Lasso { points, .. } = &mut self.interaction {
                    points.push(p);
                } else {
                    self.interaction = Interaction::Lasso { points: vec![p, p], op: op_from_mods(self.sel_op, mods), magnetic: None };
                }
                if double {
                    self.finish_lasso(tab);
                }
            }
            Tool::QuickSelect => {
                let src = composite::merged(&tab.doc.state);
                let subtract = mods.alt;
                let acc = match (tab.doc.selection(), mods.shift || subtract || self.sel_op == SelectionOp::Add) {
                    (Some(s), true) => s.clone(),
                    _ => Mask::new(w, h),
                };
                self.interaction = Interaction::QuickSelect { src, acc, subtract, last: None };
                self.pointer_move(tab, p, mods);
            }
            Tool::MagicWand => {
                let src = Self::sample_source(tab, self.sample_merged);
                let m = fill::flood_mask(&src, p.x.floor() as i32, p.y.floor() as i32, self.wand_tolerance, self.wand_contiguous);
                let m = if self.feather > 0.0 { selection::feather(&m, self.feather) } else { m };
                let op = op_from_mods(self.sel_op, mods);
                let sel = selection::combine(tab.doc.selection(), m, op);
                tab.doc.set_selection(sel);
                tab.doc.commit("Magic Wand");
            }
            Tool::MagicEraser => {
                if let Err(e) = Self::editable_layer(tab) {
                    return self.status = e;
                }
                let src = Self::sample_source(tab, self.sample_merged);
                let mut region = fill::flood_mask(&src, p.x.floor() as i32, p.y.floor() as i32, self.bucket_tolerance, self.bucket_contiguous);
                if let Some(sel) = tab.doc.selection() {
                    region.par_map_all(|x, y, v| ((v as u32 * sel.get(x, y) as u32) / 255) as u8);
                }
                let layer = tab.doc.state.active_layer_mut().unwrap();
                if let Some(r) = fill::clear(&mut layer.pixels, Some(&region)) {
                    tab.doc.mark_dirty(r);
                    tab.doc.commit("Magic Eraser");
                }
            }
            Tool::Bucket => {
                if let Err(e) = Self::editable_layer(tab) {
                    return self.status = e;
                }
                let src = Self::sample_source(tab, self.sample_merged);
                let region =
                    fill::flood_mask(&src, p.x.floor() as i32, p.y.floor() as i32, self.bucket_tolerance, self.bucket_contiguous);
                let sel = tab.doc.state.selection.clone();
                let (fg, op, mode) = (self.fg, self.fill_opacity, self.fill_mode);
                let layer = tab.doc.state.active_layer_mut().unwrap();
                let lock = layer.lock_alpha;
                if let Some(r) = fill::fill_mask(&mut layer.pixels, Some(&region), sel.as_ref(), fg, op, mode, lock) {
                    tab.doc.mark_dirty(r);
                    tab.doc.commit("Paint Bucket");
                }
            }
            Tool::Patch => {
                if tab.doc.selection().is_none() {
                    self.status = "Make a selection around the area to patch first".into();
                    return;
                }
                if let Err(e) = Self::editable_layer(tab) {
                    return self.status = e;
                }
                self.interaction = Interaction::Patch { start: p, current: p };
            }
            Tool::Gradient => self.interaction = Interaction::Gradient { start: p, current: p },
            Tool::Shape => self.interaction = Interaction::Shape { start: p, current: p },
            Tool::Crop => {
                tab.pending_crop = None;
                self.interaction = Interaction::Crop { start: p, current: p };
            }
            Tool::PerspectiveCrop => {
                if let Some(q) = self.persp_crop {
                    let i = (0..4).min_by(|&a, &b| q[a].distance(p).total_cmp(&q[b].distance(p))).unwrap();
                    if q[i].distance(p) < 20.0 / tab.view.zoom.max(0.01) {
                        self.persp_drag = Some(i);
                        self.interaction = Interaction::Eyedropper; // generic "busy" marker
                        return;
                    }
                }
                self.persp_crop = None;
                self.interaction = Interaction::Crop { start: p, current: p };
            }
            Tool::Ruler => self.interaction = Interaction::Ruler { start: p, current: p },
            Tool::Eyedropper => {
                if let Some(c) = Self::sample_color(tab, p) {
                    if mods.alt {
                        self.bg = c;
                    } else {
                        self.fg = c;
                    }
                }
                self.interaction = Interaction::Eyedropper;
            }
            Tool::Text | Tool::VerticalText => {
                // Clicking an existing type layer re-edits it.
                let hit = tab.doc.state.layers.iter().enumerate().rev().find(|(_, l)| {
                    l.text.is_some() && l.visible && l.pixels.get(p.x as i32, p.y as i32)[3] > 0
                });
                self.text_request = Some((p, hit.map(|(i, _)| i)));
            }
            Tool::Pen => self.pen_down(tab, p),
            Tool::FreeformPen => self.interaction = Interaction::Freeform { points: vec![p] },
            Tool::PathSelect | Tool::DirectSelect => {
                let kind = if self.tool == Tool::PathSelect { Some(PathHandle::Whole) } else { self.hit_path_handle(tab, p) };
                if let Some(kind) = kind {
                    if tab.doc.state.active_path.is_none() && !tab.doc.state.paths.is_empty() {
                        tab.doc.state.active_path = Some(tab.doc.state.paths.len() - 1);
                    }
                    if tab.doc.state.active_path.is_some() {
                        self.interaction = Interaction::PathDrag { kind, last: p };
                    }
                }
            }
            Tool::Hand | Tool::Zoom => {}
            _ => {}
        }
    }

    fn make_stroke(&self, tab: &DocTab, layer: &Layer, b: BrushSettings, src: StrokeSource) -> Box<Stroke> {
        let sel = tab.doc.state.selection.clone();
        let editing_mask = tab.doc.state.editing_mask && layer.mask.is_some();
        let tip = if matches!(self.tool, Tool::Brush | Tool::Eraser | Tool::Mixer) { self.tip.clone() } else { None };
        let s = if editing_mask {
            Stroke::on_mask(b, src, layer.mask.clone().unwrap(), sel)
        } else {
            Stroke::on_pixels(b, src, layer.pixels.clone(), sel, layer.lock_alpha)
        };
        Box::new(s.with_tip(tip).with_symmetry(self.symmetry))
    }

    fn begin_stroke(&mut self, tab: &mut DocTab, mut stroke: Box<Stroke>, label: &'static str, p: Point) {
        let mask = tab.doc.state.editing_mask && tab.doc.state.active_layer().is_some_and(|l| l.mask.is_some());
        if let Some(r) = Self::stroke_into(&mut stroke, tab, p, mask) {
            tab.doc.mark_dirty(r);
        }
        self.interaction = Interaction::Stroke { stroke, label };
    }

    fn stroke_into(stroke: &mut Stroke, tab: &mut DocTab, p: Point, mask: bool) -> Option<IRect> {
        let layer = tab.doc.state.active_layer_mut()?;
        if mask {
            stroke.stroke_to(p, StrokeTarget::Mask(layer.mask.as_mut()?))
        } else {
            stroke.stroke_to(p, StrokeTarget::Pixels(&mut layer.pixels))
        }
    }

    pub fn pointer_move(&mut self, tab: &mut DocTab, p: Point, mods: Modifiers) {
        if self.transform.is_some() {
            self.transform_move(tab, p, mods);
            return;
        }
        if let Some(i) = self.persp_drag {
            if let Some(q) = &mut self.persp_crop {
                q[i] = p;
            }
            return;
        }
        let ratio = self.crop_ratio;
        match &mut self.interaction {
            Interaction::Stroke { stroke, .. } => {
                let mask = tab.doc.state.editing_mask && tab.doc.state.active_layer().is_some_and(|l| l.mask.is_some());
                if let Some(r) = Self::stroke_into(stroke, tab, p, mask) {
                    tab.doc.mark_dirty(r);
                }
            }
            Interaction::SpotHeal { stroke, .. } => {
                if let Some(r) = Self::stroke_into(stroke, tab, p, false) {
                    tab.doc.mark_dirty(r);
                }
            }
            Interaction::Move { base, base_sel, start, last, bounds } => {
                let d = ((p.x - start.x).round() as i32, (p.y - start.y).round() as i32);
                let d = if mods.shift { if d.0.abs() > d.1.abs() { (d.0, 0) } else { (0, d.1) } } else { d };
                if d != *last {
                    let (pixels, sel) = transform::move_pixels(base, base_sel.as_ref(), d.0, d.1);
                    let dirty = bounds.translate(last.0, last.1).union(&bounds.translate(d.0, d.1)).union(bounds);
                    *last = d;
                    if let Some(l) = tab.doc.state.active_layer_mut() {
                        l.pixels = pixels;
                    }
                    if base_sel.is_some() {
                        tab.doc.set_selection(sel);
                    }
                    tab.doc.mark_dirty(dirty);
                }
            }
            Interaction::Marquee { start, current, op } => {
                let constrain = mods.shift && *op != SelectionOp::Add;
                *current = if constrain { constrain_ratio(*start, p, 1.0, 1.0) } else { p };
            }
            Interaction::Lasso { points, magnetic, .. } => {
                if self.tool == Tool::PolygonLasso {
                    let n = points.len();
                    let q = if mods.shift && n >= 2 { constrain45(points[n - 2], p) } else { p };
                    if let Some(last) = points.last_mut() {
                        *last = q;
                    }
                } else {
                    let p = match magnetic {
                        Some(src) => selection::snap_to_edge(src, p, 6),
                        None => p,
                    };
                    if points.last().is_none_or(|l| l.distance(p) >= 1.0) {
                        points.push(p);
                    }
                }
            }
            Interaction::QuickSelect { src, acc, subtract, last } => {
                if last.is_some_and(|l| l.distance(p) < 3.0) {
                    return;
                }
                *last = Some(p);
                let r = self.brushes.get(&Tool::QuickSelect).map_or(30.0, |b| b.size) * 0.5;
                let window = IRect::around(p, r * 3.0);
                let region = fill::flood_mask_window(src, p.x.floor() as i32, p.y.floor() as i32, 40, window);
                // Keep only the part near the brush, softly.
                let bounds = region.content_bounds().unwrap_or_default();
                let sub = *subtract;
                acc.par_for_each_in_rect_mut(bounds, |x, y, v| {
                    let m = region.get(x, y);
                    if m > 0 {
                        *v = if sub { 0 } else { (*v).max(m) };
                    }
                });
                let sel = acc.content_bounds().map(|_| acc.clone());
                tab.doc.set_selection(sel);
            }
            Interaction::Patch { current, .. } => *current = p,
            Interaction::Gradient { start, current } => {
                *current = if mods.shift { constrain45(*start, p) } else { p };
            }
            Interaction::Shape { start, current } => {
                *current = if mods.shift {
                    if self.shape_kind.is_open() || self.shape_kind == ShapeKind::Arrow {
                        constrain45(*start, p)
                    } else {
                        constrain_ratio(*start, p, 1.0, 1.0)
                    }
                } else {
                    p
                };
            }
            Interaction::Crop { start, current } => {
                *current = match ratio {
                    Some((rw, rh)) if self.tool == Tool::Crop => constrain_ratio(*start, p, rw, rh),
                    _ => p,
                };
            }
            Interaction::Ruler { start, current } => {
                *current = if mods.shift { constrain45(*start, p) } else { p };
            }
            Interaction::Eyedropper => {
                if self.persp_drag.is_none() {
                    if let Some(c) = Self::sample_color(tab, p) {
                        if mods.alt && self.tool == Tool::Eyedropper {
                            self.bg = c;
                        } else {
                            self.fg = c;
                        }
                    }
                }
            }
            Interaction::PenDrag { anchor } => {
                let i = *anchor;
                if let Some(sp) = Self::building_subpath(tab) {
                    if let Some(a) = sp.anchors.get_mut(i) {
                        *a = Anchor::smooth(a.p, p);
                    }
                }
            }
            Interaction::Freeform { points } => {
                if points.last().is_none_or(|l| l.distance(p) >= 4.0) {
                    points.push(p);
                }
            }
            Interaction::PathDrag { kind, last } => {
                let (dx, dy) = (p.x - last.x, p.y - last.y);
                *last = p;
                let Some(pi) = tab.doc.state.active_path else { return };
                let Some(path) = tab.doc.state.paths.get_mut(pi) else { return };
                match *kind {
                    PathHandle::Whole => path.translate(dx, dy),
                    PathHandle::Anchor(s, a) => {
                        if let Some(an) = path.subpaths.get_mut(s).and_then(|sp| sp.anchors.get_mut(a)) {
                            for q in [&mut an.p, &mut an.h_in, &mut an.h_out] {
                                q.x += dx;
                                q.y += dy;
                            }
                        }
                    }
                    PathHandle::In(s, a) | PathHandle::Out(s, a) => {
                        if let Some(an) = path.subpaths.get_mut(s).and_then(|sp| sp.anchors.get_mut(a)) {
                            let is_in = matches!(kind, PathHandle::In(..));
                            let h = if is_in { &mut an.h_in } else { &mut an.h_out };
                            h.x += dx;
                            h.y += dy;
                            if !mods.alt {
                                // Keep the anchor smooth: mirror the other handle.
                                let (hx, hy) = (if is_in { an.h_in } else { an.h_out }).into_tuple();
                                let o = Point::new(2.0 * an.p.x - hx, 2.0 * an.p.y - hy);
                                if is_in {
                                    an.h_out = o;
                                } else {
                                    an.h_in = o;
                                }
                            }
                        }
                    }
                }
            }
            Interaction::Transform | Interaction::None => {}
        }
    }

    pub fn pointer_up(&mut self, tab: &mut DocTab) {
        if self.transform.is_some() {
            if let Some(t) = &mut self.transform {
                t.drag = None;
            }
            self.interaction = Interaction::None;
            return;
        }
        if self.persp_drag.take().is_some() {
            self.interaction = Interaction::None;
            return;
        }
        let (w, h) = (tab.doc.width(), tab.doc.height());
        if self.tool == Tool::PolygonLasso && matches!(self.interaction, Interaction::Lasso { .. }) {
            return; // polygon continues until double-click / Enter
        }
        match std::mem::replace(&mut self.interaction, Interaction::None) {
            Interaction::Stroke { stroke, label } => {
                if stroke.touched().is_some() {
                    if let Some(l) = tab.doc.state.active_layer_mut() {
                        l.pixels.optimize();
                    }
                    tab.doc.commit(label);
                }
            }
            Interaction::SpotHeal { stroke, base } => {
                let cov = stroke.coverage_mask();
                let cov = selection::grow(&cov, 2);
                if let Some(l) = tab.doc.state.active_layer_mut() {
                    l.pixels = base.clone();
                    if let Some(r) = retouch::content_aware_fill(&mut l.pixels, &base, &cov) {
                        tab.doc.mark_dirty(r.expand(4));
                        tab.doc.commit("Spot Healing Brush");
                    }
                }
                if let Some(r) = stroke.touched() {
                    tab.doc.mark_dirty(r);
                }
            }
            Interaction::Move { last, .. } => {
                if last != (0, 0) {
                    tab.doc.commit("Move");
                }
            }
            Interaction::Marquee { start, current, op } => {
                let r = IRect::from_points(start, current);
                let shape = if r.width() < 1 || r.height() < 1 {
                    None
                } else if self.tool == Tool::EllipseMarquee {
                    Some(selection::ellipse_mask(w, h, r))
                } else {
                    Some(selection::rect_mask(w, h, r))
                };
                match shape {
                    Some(m) => {
                        let m = if self.feather > 0.0 { selection::feather(&m, self.feather) } else { m };
                        let sel = selection::combine(tab.doc.selection(), m, op);
                        tab.doc.set_selection(sel);
                        tab.doc.commit(self.tool.name());
                    }
                    None if op == SelectionOp::Replace && tab.doc.selection().is_some() => {
                        tab.doc.set_selection(None);
                        tab.doc.commit("Deselect");
                    }
                    None => {}
                }
            }
            lasso @ Interaction::Lasso { .. } => {
                self.interaction = lasso;
                self.finish_lasso(tab);
            }
            Interaction::QuickSelect { .. } => {
                tab.doc.commit("Quick Selection");
            }
            Interaction::Patch { start, current } => {
                let (dx, dy) = ((current.x - start.x).round() as i32, (current.y - start.y).round() as i32);
                if dx == 0 && dy == 0 {
                    return;
                }
                let Some(sel) = tab.doc.state.selection.clone() else { return };
                if let Some(l) = tab.doc.state.active_layer_mut() {
                    let base = l.pixels.clone();
                    if let Some(r) = retouch::patch(&mut l.pixels, &base, &sel, dx, dy) {
                        tab.doc.mark_dirty(r);
                        tab.doc.commit("Patch");
                    }
                }
            }
            Interaction::Gradient { start, current } => {
                if start.distance(current) < 1.0 {
                    return;
                }
                if let Err(e) = Self::editable_layer(tab) {
                    return self.status = e;
                }
                if tab.doc.state.editing_mask && tab.doc.state.active_layer().is_some_and(|l| l.mask.is_some()) {
                    self.draw_mask_gradient(tab, start, current);
                } else {
                    self.draw_gradient(tab, start, current);
                }
            }
            Interaction::Shape { start, current } => {
                if start.distance(current) < 1.0 {
                    return;
                }
                self.draw_shape(tab, start, current);
            }
            Interaction::Crop { start, current } => {
                let r = IRect::from_points(start, current).clamp_to(w, h);
                if self.tool == Tool::PerspectiveCrop {
                    if r.width() > 4 && r.height() > 4 {
                        let (x0, y0, x1, y1) = (r.x0 as f32, r.y0 as f32, r.x1 as f32, r.y1 as f32);
                        self.persp_crop = Some([Point::new(x0, y0), Point::new(x1, y0), Point::new(x1, y1), Point::new(x0, y1)]);
                        self.status = "Drag the corners onto the plane, then press Enter".into();
                    }
                    return;
                }
                tab.pending_crop = (!r.is_empty() && r.width() > 1 && r.height() > 1).then_some(r);
                if tab.pending_crop.is_some() {
                    self.status = "Press Enter to crop, Esc to cancel".into();
                }
            }
            Interaction::Ruler { start, current } => {
                self.ruler = (start.distance(current) > 1.0).then_some((start, current));
            }
            Interaction::PenDrag { .. } => {}
            Interaction::Freeform { points } => {
                if points.len() >= 2 {
                    let sp = SubPath { anchors: points.into_iter().map(Anchor::corner).collect(), closed: false };
                    Self::push_subpath(tab, sp);
                    tab.doc.commit("Freeform Pen");
                }
            }
            Interaction::PathDrag { .. } => tab.doc.commit("Edit Path"),
            Interaction::Eyedropper | Interaction::Transform | Interaction::None => {}
        }
    }

    fn finish_lasso(&mut self, tab: &mut DocTab) {
        let (w, h) = (tab.doc.width(), tab.doc.height());
        if let Interaction::Lasso { mut points, op, .. } = std::mem::replace(&mut self.interaction, Interaction::None) {
            if self.tool == Tool::PolygonLasso {
                points.pop(); // rubber-band point
                points.dedup_by(|a, b| a.distance(*b) < 0.5);
            }
            if points.len() >= 3 {
                let m = selection::polygon_mask(w, h, &points);
                let m = if self.feather > 0.0 { selection::feather(&m, self.feather) } else { m };
                let sel = selection::combine(tab.doc.selection(), m, op);
                tab.doc.set_selection(sel);
                tab.doc.commit(self.tool.name());
            }
        }
    }

    fn gradient_color(&self, t: f32) -> Color {
        if self.gradient_stops.len() >= 2 {
            let stops = &self.gradient_stops;
            if t <= stops[0].0 {
                return stops[0].1;
            }
            for w in stops.windows(2) {
                if t <= w[1].0 {
                    let k = (t - w[0].0) / (w[1].0 - w[0].0).max(1e-6);
                    return w[0].1.lerp(w[1].1, k);
                }
            }
            return stops[stops.len() - 1].1;
        }
        let c1 = if self.gradient_to_transparent { self.fg.with_alpha(0.0) } else { self.bg };
        self.fg.lerp(c1, t)
    }

    fn draw_gradient(&mut self, tab: &mut DocTab, start: Point, current: Point) {
        let sel = tab.doc.state.selection.clone();
        let (kind, op, mode, reverse) = (self.gradient_kind, self.fill_opacity, self.fill_mode, self.gradient_reverse);
        // Bake the gradient (multi-stop aware) through a 256-entry palette.
        let palette: Vec<Color> = (0..256).map(|i| self.gradient_color(i as f32 / 255.0)).collect();
        let layer = tab.doc.state.active_layer_mut().unwrap();
        let lock = layer.lock_alpha;
        let rect = selection::work_rect(sel.as_ref(), layer.width(), layer.height());
        layer.pixels.par_for_each_in_rect_mut(rect, |x, y, p| {
            let mut t = kind.t_at(start, current, Point::new(x as f32 + 0.5, y as f32 + 0.5));
            t = (t + (forge_core::blend::pixel_noise(x, y) - 0.5) / 255.0).clamp(0.0, 1.0);
            if reverse {
                t = 1.0 - t;
            }
            let c = palette[(t * 255.0) as usize];
            let s = selection::coverage(sel.as_ref(), x, y);
            *p = fill::blend_pixel(*p, c.rgb_array(), c.a * op * s, mode, lock, x, y);
        });
        tab.doc.mark_dirty(rect);
        tab.doc.commit("Gradient");
    }

    /// Gradient masks: fades the layer mask from FG luminance to BG luminance.
    fn draw_mask_gradient(&mut self, tab: &mut DocTab, start: Point, current: Point) {
        let sel = tab.doc.state.selection.clone();
        let kind = self.gradient_kind;
        let (mut a, mut b) = (forge_core::color::lum(self.fg.rgb_array()), forge_core::color::lum(self.bg.rgb_array()));
        if self.gradient_reverse {
            std::mem::swap(&mut a, &mut b);
        }
        let Some(mask) = tab.doc.state.active_layer_mut().and_then(|l| l.mask.as_mut()) else { return };
        let rect = selection::work_rect(sel.as_ref(), mask.width(), mask.height());
        mask.par_for_each_in_rect_mut(rect, |x, y, v| {
            let t = kind.t_at(start, current, Point::new(x as f32 + 0.5, y as f32 + 0.5));
            let target = (a + (b - a) * t) * 255.0;
            let s = selection::coverage(sel.as_ref(), x, y);
            *v = (*v as f32 + (target - *v as f32) * s).round().clamp(0.0, 255.0) as u8;
        });
        tab.doc.mark_dirty(rect);
        tab.doc.commit("Gradient Mask");
    }

    fn draw_shape(&mut self, tab: &mut DocTab, start: Point, current: Point) {
        let (w, h) = (tab.doc.width(), tab.doc.height());
        let kind = self.shape_kind;
        let params = ShapeParams { line_width: self.shape_stroke_width.max(1.0), ..self.shape_params };
        let fill_mask = (self.shape_fill || kind.is_open()).then(|| shapes::shape_mask(w, h, kind, start, current, &params));
        let dash = self.shape_dashed.then_some((self.shape_stroke_width * 3.0, self.shape_stroke_width * 2.0));
        let stroke_mask = (self.shape_stroke && !kind.is_open()).then(|| shapes::shape_stroke(w, h, kind, start, current, &params, self.shape_stroke_width, dash));
        let paint = |px: &mut TiledBuffer<Rgba8>, sel: Option<&Mask>, lock: bool, s: &Self| -> Option<IRect> {
            let mut r = None;
            if let Some(m) = &fill_mask {
                r = forge_core::geom::union_opt(r, fill::fill_mask(px, Some(m), sel, s.fg, 1.0, BlendMode::Normal, lock));
            }
            if let Some(m) = &stroke_mask {
                r = forge_core::geom::union_opt(r, fill::fill_mask(px, Some(m), sel, s.shape_stroke_color, 1.0, BlendMode::Normal, lock));
            }
            r
        };
        if self.shape_new_layer {
            let name = format!("{} {}", kind.name(), tab.doc.state.layers.len());
            let mut layer = tab.doc.state.new_layer(name);
            paint(&mut layer.pixels, None, false, self);
            tab.doc.add_layer(layer, kind.name());
        } else {
            if let Err(e) = Self::editable_layer(tab) {
                return self.status = e;
            }
            let sel = tab.doc.state.selection.clone();
            let layer = tab.doc.state.active_layer_mut().unwrap();
            let lock = layer.lock_alpha;
            let mut px = std::mem::replace(&mut layer.pixels, TiledBuffer::new(1, 1));
            let r = paint(&mut px, sel.as_ref(), lock, self);
            tab.doc.state.active_layer_mut().unwrap().pixels = px;
            if let Some(r) = r {
                tab.doc.mark_dirty(r);
                tab.doc.commit(kind.name());
            }
        }
    }

    // ----------------------------------------------------------------- pen

    fn building_subpath(tab: &mut DocTab) -> Option<&mut SubPath> {
        let pi = tab.doc.state.active_path?;
        tab.doc.state.paths.get_mut(pi)?.subpaths.last_mut()
    }

    fn push_subpath(tab: &mut DocTab, sp: SubPath) {
        let pi = match tab.doc.state.active_path.filter(|&i| i < tab.doc.state.paths.len()) {
            Some(i) => i,
            None => {
                let n = tab.doc.state.paths.len() + 1;
                tab.doc.state.paths.push(VectorPath { name: format!("Path {n}"), subpaths: Vec::new() });
                tab.doc.state.paths.len() - 1
            }
        };
        tab.doc.state.paths[pi].subpaths.push(sp);
        tab.doc.state.active_path = Some(pi);
    }

    fn pen_down(&mut self, tab: &mut DocTab, p: Point) {
        let close_dist = 8.0 / tab.view.zoom.max(0.01);
        if self.pen_building {
            if let Some(sp) = Self::building_subpath(tab) {
                if sp.anchors.len() >= 2 && sp.anchors[0].p.distance(p) < close_dist {
                    sp.closed = true;
                    self.pen_building = false;
                    tab.doc.commit("Close Path");
                    return;
                }
                sp.anchors.push(Anchor::corner(p));
                let i = sp.anchors.len() - 1;
                self.interaction = Interaction::PenDrag { anchor: i };
                return;
            }
        }
        Self::push_subpath(tab, SubPath { anchors: vec![Anchor::corner(p)], closed: false });
        self.pen_building = true;
        self.interaction = Interaction::PenDrag { anchor: 0 };
    }

    /// Enter / Esc: finish the path being drawn.
    pub fn finish_pen(&mut self, tab: &mut DocTab) {
        if self.pen_building {
            self.pen_building = false;
            tab.doc.commit("Pen");
        }
    }

    fn hit_path_handle(&self, tab: &DocTab, p: Point) -> Option<PathHandle> {
        let path = tab.doc.state.active_path.and_then(|i| tab.doc.state.paths.get(i)).or(tab.doc.state.paths.last())?;
        let r = 8.0 / tab.view.zoom.max(0.01);
        for (si, sp) in path.subpaths.iter().enumerate() {
            for (ai, a) in sp.anchors.iter().enumerate() {
                if a.p.distance(p) < r {
                    return Some(PathHandle::Anchor(si, ai));
                }
                if a.h_in != a.p && a.h_in.distance(p) < r {
                    return Some(PathHandle::In(si, ai));
                }
                if a.h_out != a.p && a.h_out.distance(p) < r {
                    return Some(PathHandle::Out(si, ai));
                }
            }
        }
        None
    }

    // ------------------------------------------------------------ transform

    /// Starts on-canvas transform of the active layer / selection.
    pub fn begin_transform(&mut self, tab: &mut DocTab, mode: TransformMode) -> Result<(), String> {
        if self.transform.is_some() {
            if let Some(t) = &mut self.transform {
                t.mode = mode;
            }
            return Ok(());
        }
        let li = tab.doc.state.active;
        let l = tab.doc.state.layers.get(li).ok_or("No layer")?;
        if l.locked || !l.has_pixels() {
            return Err("This layer cannot be transformed".into());
        }
        let (src_quad, base, smart) = match &l.smart {
            Some(so) => {
                let b = so.source.content_bounds().ok_or("Empty layer")?;
                let src = [
                    Point::new(b.x0 as f32, b.y0 as f32),
                    Point::new(b.x1 as f32, b.y0 as f32),
                    Point::new(b.x1 as f32, b.y1 as f32),
                    Point::new(b.x0 as f32, b.y1 as f32),
                ];
                (src, so.source.clone(), true)
            }
            None => {
                let b = transform::transform_source_bounds(&l.pixels, tab.doc.selection()).ok_or("Nothing to transform")?;
                let src = [
                    Point::new(b.x0 as f32, b.y0 as f32),
                    Point::new(b.x1 as f32, b.y0 as f32),
                    Point::new(b.x1 as f32, b.y1 as f32),
                    Point::new(b.x0 as f32, b.y1 as f32),
                ];
                (src, l.pixels.clone(), false)
            }
        };
        let quad = match &l.smart {
            Some(so) => so.quad.map(|(x, y)| Point::new(x, y)),
            None => src_quad,
        };
        let b = IRect::new(src_quad[0].x as i32, src_quad[0].y as i32, src_quad[2].x as i32, src_quad[2].y as i32);
        let grid = transform::warp_identity_grid(b).map(|(x, y)| Point::new(x, y));
        self.transform = Some(TransformSession {
            mode,
            layer: li,
            base,
            base_sel: if smart { None } else { tab.doc.state.selection.clone() },
            quad,
            grid,
            drag: None,
            dirty: false,
            smart,
        });
        self.interaction = Interaction::None;
        self.status = "Drag handles · Enter to apply · Esc to cancel".into();
        Ok(())
    }

    fn transform_down(&mut self, tab: &mut DocTab, p: Point) {
        let zoom = tab.view.zoom.max(0.01);
        let Some(t) = &mut self.transform else { return };
        let r = 10.0 / zoom;
        let handle = if t.mode == TransformMode::Warp {
            (0..16).find(|&i| t.grid[i].distance(p) < r).map(Handle::WarpPoint).unwrap_or(Handle::Move)
        } else if let Some(i) = (0..4).find(|&i| t.quad[i].distance(p) < r) {
            Handle::Corner(i)
        } else if let Some(i) = (0..4).find(|&i| t.quad[i].lerp(t.quad[(i + 1) % 4], 0.5).distance(p) < r) {
            Handle::Edge(i)
        } else if point_in_quad(&t.quad, p) {
            Handle::Move
        } else {
            Handle::Rotate
        };
        t.drag = Some((handle, p, t.quad, t.grid));
        self.interaction = Interaction::Transform;
    }

    fn transform_move(&mut self, tab: &mut DocTab, p: Point, mods: Modifiers) {
        let Some(t) = &mut self.transform else { return };
        let Some((handle, start, q0, g0)) = t.drag else { return };
        let (dx, dy) = (p.x - start.x, p.y - start.y);
        let mode = if mods.command && t.mode == TransformMode::Free { TransformMode::Distort } else { t.mode };
        match handle {
            Handle::Move => {
                t.quad = q0.map(|q| Point::new(q.x + dx, q.y + dy));
                t.grid = g0.map(|q| Point::new(q.x + dx, q.y + dy));
            }
            Handle::WarpPoint(i) => {
                t.grid[i] = Point::new(g0[i].x + dx, g0[i].y + dy);
            }
            Handle::Rotate => {
                let c = {
                    let mut c = Point::new(0.0, 0.0);
                    for q in q0 {
                        c.x += q.x * 0.25;
                        c.y += q.y * 0.25;
                    }
                    c
                };
                let mut ang = (p.y - c.y).atan2(p.x - c.x) - (start.y - c.y).atan2(start.x - c.x);
                if mods.shift {
                    let step = 15f32.to_radians();
                    ang = (ang / step).round() * step;
                }
                let (s, co) = ang.sin_cos();
                t.quad = q0.map(|q| Point::new(c.x + (q.x - c.x) * co - (q.y - c.y) * s, c.y + (q.x - c.x) * s + (q.y - c.y) * co));
            }
            Handle::Corner(i) => match mode {
                TransformMode::Distort | TransformMode::Warp => t.quad[i] = Point::new(q0[i].x + dx, q0[i].y + dy),
                TransformMode::Perspective => {
                    // Move this corner and mirror the neighbour on the same edge.
                    t.quad[i] = Point::new(q0[i].x + dx, q0[i].y + dy);
                    let j = if i % 2 == 0 { i + 1 } else { i - 1 };
                    let horizontal = (q0[i].y - q0[j].y).abs() < (q0[i].x - q0[j].x).abs();
                    t.quad[j] = if horizontal { Point::new(q0[j].x - dx, q0[j].y + dy) } else { Point::new(q0[j].x + dx, q0[j].y - dy) };
                }
                TransformMode::Skew => t.quad[i] = Point::new(q0[i].x + dx, q0[i].y + dy),
                TransformMode::Free => {
                    // Scale relative to the opposite corner (affine, keeps parallels).
                    let o = q0[(i + 2) % 4];
                    let (ux, uy) = (q0[(i + 1) % 4].x - o.x, q0[(i + 1) % 4].y - o.y);
                    let (vx, vy) = (q0[(i + 3) % 4].x - o.x, q0[(i + 3) % 4].y - o.y);
                    let target = Point::new(q0[i].x + dx, q0[i].y + dy);
                    let (tx, ty) = (target.x - o.x, target.y - o.y);
                    let det = ux * vy - uy * vx;
                    if det.abs() > 1e-6 {
                        // Express the target in the (u, v) edge basis; the dragged
                        // corner itself is o + u + v, so these are the scale factors.
                        let mut sa = (tx * vy - ty * vx) / det;
                        let mut sb = (ux * ty - uy * tx) / det;
                        if mods.shift {
                            let s = (sa + sb) * 0.5;
                            sa = s;
                            sb = s;
                        }
                        let scale = |pt: Point| {
                            let (px, py) = (pt.x - o.x, pt.y - o.y);
                            let a = (px * vy - py * vx) / det * sa;
                            let b = (ux * py - uy * px) / det * sb;
                            Point::new(o.x + a * ux + b * vx, o.y + a * uy + b * vy)
                        };
                        t.quad = q0.map(scale);
                    }
                }
            },
            Handle::Edge(i) => {
                let (a, b) = (i, (i + 1) % 4);
                if mode == TransformMode::Skew || mods.command {
                    // Slide the edge along itself.
                    let (ex, ey) = (q0[b].x - q0[a].x, q0[b].y - q0[a].y);
                    let len = (ex * ex + ey * ey).sqrt().max(1e-6);
                    let along = (dx * ex + dy * ey) / len;
                    t.quad[a] = Point::new(q0[a].x + ex / len * along, q0[a].y + ey / len * along);
                    t.quad[b] = Point::new(q0[b].x + ex / len * along, q0[b].y + ey / len * along);
                } else {
                    // Push the edge outward along its normal.
                    let (ex, ey) = (q0[b].x - q0[a].x, q0[b].y - q0[a].y);
                    let len = (ex * ex + ey * ey).sqrt().max(1e-6);
                    let (nx, ny) = (ey / len, -ex / len);
                    let d = dx * nx + dy * ny;
                    t.quad[a] = Point::new(q0[a].x + nx * d, q0[a].y + ny * d);
                    t.quad[b] = Point::new(q0[b].x + nx * d, q0[b].y + ny * d);
                }
            }
        }
        t.dirty = true;
        let _ = tab;
    }

    /// Re-renders the transform preview if something changed.
    pub fn update_transform_preview(&mut self, tab: &mut DocTab) {
        let Some(t) = &mut self.transform else { return };
        if !t.dirty {
            return;
        }
        t.dirty = false;
        let (pixels, sel) = Self::render_transform(t);
        if let Some(l) = tab.doc.state.layers.get_mut(t.layer) {
            l.pixels = pixels;
        }
        if t.base_sel.is_some() {
            tab.doc.set_selection(sel);
        }
        tab.doc.mark_all_dirty();
    }

    fn render_transform(t: &TransformSession) -> (TiledBuffer<Rgba8>, Option<Mask>) {
        if t.mode == TransformMode::Warp {
            let grid = t.grid.map(|p| (p.x, p.y));
            transform::warp_pixels(&t.base, t.base_sel.as_ref(), &grid)
        } else {
            let quad = t.quad.map(|p| (p.x, p.y));
            transform::transform_pixels_quad(&t.base, t.base_sel.as_ref(), quad, transform::Resample::Bilinear)
        }
    }

    pub fn commit_transform(&mut self, tab: &mut DocTab) {
        let Some(t) = self.transform.take() else { return };
        if t.smart && t.mode != TransformMode::Warp {
            if let Some(l) = tab.doc.state.layers.get_mut(t.layer) {
                if let Some(so) = &mut l.smart {
                    so.quad = t.quad.map(|p| (p.x, p.y));
                    l.pixels = so.render();
                }
            }
        } else {
            let (pixels, sel) = Self::render_transform(&t);
            if let Some(l) = tab.doc.state.layers.get_mut(t.layer) {
                l.pixels = pixels;
                l.smart = None;
            }
            if t.base_sel.is_some() {
                tab.doc.set_selection(sel);
            }
        }
        tab.doc.mark_all_dirty();
        tab.doc.commit(t.mode.name());
        self.status.clear();
    }

    /// Cancels the current interaction (Esc), restoring the committed state.
    pub fn cancel(&mut self, tab: &mut DocTab) {
        if self.transform.take().is_some() {
            tab.doc.revert_to_current();
            self.interaction = Interaction::None;
            self.status.clear();
            return;
        }
        if self.pen_building {
            self.finish_pen(tab);
            return;
        }
        let was_editing = matches!(
            self.interaction,
            Interaction::Stroke { .. } | Interaction::Move { .. } | Interaction::SpotHeal { .. } | Interaction::QuickSelect { .. }
        );
        self.interaction = Interaction::None;
        if was_editing {
            tab.doc.revert_to_current();
        }
        tab.pending_crop = None;
        self.persp_crop = None;
        self.ruler = None;
    }

    /// Enter key.
    pub fn confirm(&mut self, tab: &mut DocTab) {
        if self.transform.is_some() {
            return self.commit_transform(tab);
        }
        if self.tool == Tool::PolygonLasso && matches!(self.interaction, Interaction::Lasso { .. }) {
            return self.finish_lasso(tab);
        }
        if self.pen_building {
            return self.finish_pen(tab);
        }
        if let Some(q) = self.persp_crop.take() {
            transform::perspective_crop(&mut tab.doc.state, q.map(|p| (p.x, p.y)));
            tab.doc.mark_all_dirty();
            tab.doc.set_selection(None);
            tab.doc.commit("Perspective Crop");
            tab.view.request_fit();
            return;
        }
        self.apply_crop(tab);
    }

    pub fn apply_crop(&mut self, tab: &mut DocTab) {
        if let Some(r) = tab.pending_crop.take() {
            transform::crop(&mut tab.doc.state, r);
            tab.doc.mark_all_dirty();
            tab.doc.set_selection(tab.doc.state.selection.clone());
            tab.doc.commit("Crop");
            tab.view.request_fit();
        }
    }

    /// Ruler: rotate the active layer so the measured line becomes level.
    pub fn straighten(&mut self, tab: &mut DocTab) {
        let Some((a, b)) = self.ruler else { return };
        let mut ang = (b.y - a.y).atan2(b.x - a.x).to_degrees();
        if ang > 45.0 {
            ang -= 90.0;
        } else if ang < -45.0 {
            ang += 90.0;
        }
        if let Some(l) = tab.doc.state.active_layer_mut() {
            l.pixels = transform::rotate_layer(&l.pixels, -ang);
            tab.doc.mark_all_dirty();
            tab.doc.commit("Straighten Layer");
        }
        self.ruler = None;
    }

    /// Arrow-key nudging with the Move tool.
    pub fn nudge(&mut self, tab: &mut DocTab, dx: i32, dy: i32) {
        let Ok(layer) = Self::editable_layer(tab) else { return };
        let sel = tab.doc.state.selection.clone();
        let (pixels, nsel) = transform::move_pixels(&layer.pixels, sel.as_ref(), dx, dy);
        if let Some(l) = tab.doc.state.active_layer_mut() {
            l.pixels = pixels;
        }
        if sel.is_some() {
            tab.doc.set_selection(nsel);
        }
        tab.doc.mark_all_dirty();
        tab.doc.commit("Nudge");
    }

    // ------------------------------------------------------------ options bar

    /// Tool options bar (below the menu).
    pub fn options_ui(&mut self, ui: &mut egui::Ui, fonts: &[crate::text::FontEntry], tab: Option<&mut DocTab>) {
        ui.horizontal_wrapped(|ui| {
            if let Some(t) = &mut self.transform {
                ui.label(egui::RichText::new("Transform").strong());
                ui.separator();
                for m in [TransformMode::Free, TransformMode::Skew, TransformMode::Distort, TransformMode::Perspective, TransformMode::Warp] {
                    if ui.selectable_label(t.mode == m, m.name()).clicked() {
                        t.mode = m;
                        t.dirty = true;
                    }
                }
                ui.separator();
                if let Some(tab) = tab {
                    if ui.button("✔ Commit (Enter)").clicked() {
                        self.commit_transform(tab);
                    }
                    if ui.button("✖ Cancel (Esc)").clicked() {
                        self.cancel(tab);
                    }
                }
                return;
            }
            ui.label(egui::RichText::new(self.tool.name()).strong());
            ui.separator();
            let tool = self.tool;
            if tool.is_paint() {
                self.brush_options(ui, tool);
            }
            match tool {
                Tool::CloneStamp | Tool::Healing => {
                    ui.checkbox(&mut self.sample_merged, "Sample All Layers");
                    if ui.button("Reset Alignment").clicked() {
                        self.clone_offset = None;
                    }
                    ui.label("Alt+click sets the source");
                }
                Tool::Mixer => {
                    ui.label("Wet");
                    percent(ui, &mut self.wetness);
                }
                Tool::BackgroundEraser => {
                    ui.label("Tolerance");
                    ui.add(egui::DragValue::new(&mut self.bg_tolerance).range(0..=255));
                }
                Tool::Blur | Tool::Sharpen | Tool::Sponge => {
                    ui.label("Strength");
                    percent(ui, &mut self.strength);
                    if tool == Tool::Sponge {
                        ui.selectable_value(&mut self.sponge_saturate, false, "Desaturate");
                        ui.selectable_value(&mut self.sponge_saturate, true, "Saturate");
                    }
                }
                Tool::Smudge => {
                    ui.label("Strength");
                    percent(ui, &mut self.strength);
                    ui.checkbox(&mut self.finger_painting, "Finger Painting");
                }
                Tool::Dodge | Tool::Burn => {
                    egui::ComboBox::from_id_salt("range").selected_text(self.tone_range.name()).show_ui(ui, |ui| {
                        for r in ToneRange::ALL {
                            ui.selectable_value(&mut self.tone_range, r, r.name());
                        }
                    });
                    ui.label("Exposure");
                    percent(ui, &mut self.exposure);
                }
                Tool::Liquify => {
                    for m in LiquifyMode::ALL {
                        ui.selectable_value(&mut self.liquify_mode, m, m.name());
                    }
                    ui.label("Pressure");
                    percent(ui, &mut self.strength);
                }
                Tool::Marquee | Tool::EllipseMarquee | Tool::RowMarquee | Tool::ColumnMarquee | Tool::Lasso | Tool::PolygonLasso | Tool::MagneticLasso | Tool::MagicWand | Tool::QuickSelect => {
                    for op in SelectionOp::ALL {
                        ui.selectable_value(&mut self.sel_op, op, op.name());
                    }
                    ui.separator();
                    if tool != Tool::QuickSelect {
                        ui.label("Feather");
                        ui.add(egui::DragValue::new(&mut self.feather).range(0.0..=250.0).speed(0.2).suffix(" px"));
                    }
                    if tool == Tool::MagicWand {
                        ui.label("Tolerance");
                        ui.add(egui::DragValue::new(&mut self.wand_tolerance).range(0..=255));
                        ui.checkbox(&mut self.wand_contiguous, "Contiguous");
                        ui.checkbox(&mut self.sample_merged, "Sample All Layers");
                    }
                    if tool == Tool::PolygonLasso {
                        ui.label("Click to add points · double-click or Enter to close");
                    }
                }
                Tool::Bucket | Tool::MagicEraser => {
                    if tool == Tool::Bucket {
                        blend_combo(ui, "bucket_mode", &mut self.fill_mode);
                        ui.label("Opacity");
                        percent(ui, &mut self.fill_opacity);
                    }
                    ui.label("Tolerance");
                    ui.add(egui::DragValue::new(&mut self.bucket_tolerance).range(0..=255));
                    ui.checkbox(&mut self.bucket_contiguous, "Contiguous");
                    ui.checkbox(&mut self.sample_merged, "All Layers");
                }
                Tool::Gradient => {
                    for k in GradientKind::ALL {
                        ui.selectable_value(&mut self.gradient_kind, k, k.name());
                    }
                    ui.separator();
                    blend_combo(ui, "grad_mode", &mut self.fill_mode);
                    ui.label("Opacity");
                    percent(ui, &mut self.fill_opacity);
                    ui.checkbox(&mut self.gradient_reverse, "Reverse");
                    if self.gradient_stops.is_empty() {
                        ui.checkbox(&mut self.gradient_to_transparent, "FG → Transparent");
                    }
                    self.gradient_editor(ui);
                }
                Tool::Shape => {
                    egui::ComboBox::from_id_salt("shape_kind").selected_text(self.shape_kind.name()).show_ui(ui, |ui| {
                        for k in ShapeKind::ALL {
                            ui.selectable_value(&mut self.shape_kind, k, k.name());
                        }
                    });
                    match self.shape_kind {
                        ShapeKind::RoundedRectangle => {
                            ui.label("Radius");
                            ui.add(egui::DragValue::new(&mut self.shape_params.corner_radius).range(0.0..=1000.0).suffix(" px"));
                        }
                        ShapeKind::Polygon | ShapeKind::Star => {
                            ui.label("Sides");
                            ui.add(egui::DragValue::new(&mut self.shape_params.sides).range(3..=100));
                            if self.shape_kind == ShapeKind::Star {
                                ui.label("Indent");
                                percent(ui, &mut self.shape_params.star_inner);
                            }
                        }
                        _ => {}
                    }
                    ui.separator();
                    if !self.shape_kind.is_open() {
                        ui.checkbox(&mut self.shape_fill, "Fill (FG)");
                        ui.checkbox(&mut self.shape_stroke, "Stroke");
                    }
                    let mut c = crate::panels::to_c32(self.shape_stroke_color);
                    if ui.color_edit_button_srgba(&mut c).changed() {
                        self.shape_stroke_color = crate::panels::from_c32(c);
                    }
                    ui.add(egui::DragValue::new(&mut self.shape_stroke_width).range(1.0..=500.0).suffix(" px"));
                    ui.checkbox(&mut self.shape_dashed, "Dashed");
                    ui.checkbox(&mut self.shape_new_layer, "New Layer");
                }
                Tool::Text | Tool::VerticalText => {
                    let name = fonts.get(self.font_index).map(|f| f.name.as_str()).unwrap_or("?");
                    egui::ComboBox::from_id_salt("font").selected_text(name).width(200.0).show_ui(ui, |ui| {
                        for (i, f) in fonts.iter().enumerate() {
                            ui.selectable_value(&mut self.font_index, i, &f.name);
                        }
                    });
                    ui.add(egui::DragValue::new(&mut self.text.size).range(4.0..=1000.0).suffix(" px"));
                    ui.toggle_value(&mut self.text.bold, egui::RichText::new("B").strong());
                    ui.toggle_value(&mut self.text.italic, egui::RichText::new("I").italics());
                    ui.toggle_value(&mut self.text.underline, egui::RichText::new("U").underline());
                    for (a, l) in [(TextAlign::Left, "⬅"), (TextAlign::Center, "↔"), (TextAlign::Right, "➡")] {
                        ui.selectable_value(&mut self.text.align, a, l);
                    }
                    ui.label("Click to add text, click a type layer to edit it");
                }
                Tool::Crop => {
                    let ratios: [(&str, Option<(f32, f32)>); 7] = [
                        ("Ratio", None),
                        ("1:1", Some((1.0, 1.0))),
                        ("4:3", Some((4.0, 3.0))),
                        ("3:2", Some((3.0, 2.0))),
                        ("16:9", Some((16.0, 9.0))),
                        ("9:16", Some((9.0, 16.0))),
                        ("4:5", Some((4.0, 5.0))),
                    ];
                    let current = ratios.iter().find(|r| r.1 == self.crop_ratio).map_or("Custom", |r| r.0);
                    egui::ComboBox::from_id_salt("crop_ratio").selected_text(current).show_ui(ui, |ui| {
                        for (n, r) in ratios {
                            ui.selectable_value(&mut self.crop_ratio, r, n);
                        }
                    });
                    if let Some(tab) = tab {
                        if let Some(r) = tab.pending_crop {
                            ui.label(format!("{} × {} px", r.width(), r.height()));
                            if ui.button("✔ Apply").clicked() {
                                self.apply_crop(tab);
                            }
                            if ui.button("✖ Cancel").clicked() {
                                tab.pending_crop = None;
                            }
                        } else {
                            ui.label("Drag to choose the crop area");
                        }
                    }
                }
                Tool::PerspectiveCrop => {
                    ui.label("Drag a box, move its corners onto the plane, then press Enter");
                }
                Tool::Ruler => {
                    if let Some((a, b)) = self.ruler {
                        let ang = -(b.y - a.y).atan2(b.x - a.x).to_degrees();
                        ui.label(format!("L: {:.1} px   A: {:.1}°   W: {:.0}  H: {:.0}", a.distance(b), ang, (b.x - a.x).abs(), (b.y - a.y).abs()));
                        if let Some(tab) = tab {
                            if ui.button("Straighten Layer").clicked() {
                                self.straighten(tab);
                            }
                        }
                        if ui.button("Clear").clicked() {
                            self.ruler = None;
                        }
                    } else {
                        ui.label("Drag to measure distance and angle");
                    }
                }
                Tool::Pen | Tool::FreeformPen | Tool::PathSelect | Tool::DirectSelect => {
                    ui.label("Click to add anchors, drag for curves, click the first anchor to close. Use the Paths panel to fill, stroke or select.");
                }
                Tool::Move => {
                    ui.label("Drag to move the layer or selection. Arrow keys nudge (Shift = 10 px).");
                }
                Tool::Eyedropper => {
                    ui.label("Click: foreground colour · Alt+click: background colour");
                }
                Tool::Patch => {
                    ui.label("Select the area to repair, then drag it onto a clean source area");
                }
                Tool::SpotHealing => {
                    ui.label("Paint over spots and blemishes");
                }
                Tool::Hand => {
                    ui.label("Drag to scroll. Space+drag works with any tool.");
                }
                Tool::Zoom => {
                    ui.label("Click to zoom in · Alt+click to zoom out");
                }
                _ => {}
            }
        });
    }

    fn brush_options(&mut self, ui: &mut egui::Ui, tool: Tool) {
        let has_tip = self.tip.is_some() && matches!(tool, Tool::Brush | Tool::Eraser | Tool::Mixer);
        let Some(b) = self.brushes.get_mut(&tool) else { return };
        ui.label("Size");
        ui.add(egui::DragValue::new(&mut b.size).range(1.0..=2000.0).speed(0.5).suffix(" px"));
        if tool == Tool::QuickSelect {
            return;
        }
        if tool != Tool::Pencil && !has_tip {
            ui.label("Hardness");
            percent(ui, &mut b.hardness);
        }
        if !matches!(tool, Tool::Liquify | Tool::Smudge | Tool::Blur | Tool::Sharpen) {
            ui.label("Opacity");
            percent(ui, &mut b.opacity);
        }
        if matches!(tool, Tool::Brush | Tool::Eraser | Tool::CloneStamp | Tool::PatternStamp | Tool::Healing) {
            ui.label("Flow");
            percent(ui, &mut b.flow);
        }
        ui.menu_button("Brush…", |ui| {
            ui.add(egui::Slider::new(&mut b.spacing, 0.01..=2.0).text("Spacing"));
            ui.add(egui::Slider::new(&mut b.angle, -180.0..=180.0).text("Angle °"));
            ui.add(egui::Slider::new(&mut b.roundness, 0.05..=1.0).text("Roundness"));
            ui.add(egui::Slider::new(&mut b.scatter, 0.0..=2.0).text("Scatter"));
            ui.checkbox(&mut b.antialias, "Smooth edges");
        });
        if matches!(tool, Tool::Brush | Tool::Pencil | Tool::Eraser | Tool::Mixer) {
            egui::ComboBox::from_id_salt("symmetry").selected_text(format!("Symmetry: {}", self.symmetry.name())).show_ui(ui, |ui| {
                for s in Symmetry::ALL {
                    ui.selectable_value(&mut self.symmetry, s, s.name());
                }
            });
        }
        if has_tip && ui.button("Round tip").on_hover_text("Stop using the custom brush tip").clicked() {
            self.tip = None;
        }
    }

    fn gradient_editor(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("Gradient…", |ui| {
            ui.set_min_width(260.0);
            if self.gradient_stops.is_empty() && ui.button("Edit custom gradient").clicked() {
                self.gradient_stops = vec![(0.0, self.fg), (1.0, self.bg)];
            }
            let presets: [(&str, Vec<(f32, Color)>); 4] = [
                ("Spectrum", (0..7).map(|i| (i as f32 / 6.0, Color::from_hex(["#ff0000", "#ffff00", "#00ff00", "#00ffff", "#0000ff", "#ff00ff", "#ff0000"][i]).unwrap())).collect()),
                ("Sunset", vec![(0.0, Color::from_hex("#2b1055").unwrap()), (0.5, Color::from_hex("#d53369").unwrap()), (1.0, Color::from_hex("#ffcc70").unwrap())]),
                ("Chrome", vec![(0.0, Color::from_hex("#333333").unwrap()), (0.45, Color::WHITE), (0.55, Color::from_hex("#555555").unwrap()), (1.0, Color::from_hex("#dddddd").unwrap())]),
                ("Forge", vec![(0.0, Color::from_hex("#18171c").unwrap()), (1.0, Color::from_hex("#c6f432").unwrap())]),
            ];
            ui.horizontal_wrapped(|ui| {
                for (n, stops) in presets {
                    if ui.button(n).clicked() {
                        self.gradient_stops = stops;
                    }
                }
            });
            let mut remove = None;
            let n = self.gradient_stops.len();
            for (i, (pos, col)) in self.gradient_stops.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    let mut c = crate::panels::to_c32(*col);
                    if egui::color_picker::color_edit_button_srgba(ui, &mut c, egui::color_picker::Alpha::OnlyBlend).changed() {
                        *col = crate::panels::from_c32(c);
                    }
                    ui.add(egui::Slider::new(pos, 0.0..=1.0).text("Location"));
                    if n > 2 && ui.small_button("✖").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                self.gradient_stops.remove(i);
            }
            self.gradient_stops.sort_by(|a, b| a.0.total_cmp(&b.0));
            ui.horizontal(|ui| {
                if !self.gradient_stops.is_empty() && ui.button("+ Stop").clicked() {
                    self.gradient_stops.push((0.5, self.fg));
                    self.gradient_stops.sort_by(|a, b| a.0.total_cmp(&b.0));
                }
                if !self.gradient_stops.is_empty() && ui.button("Use FG → BG").clicked() {
                    self.gradient_stops.clear();
                }
            });
        });
    }
}

fn point_in_quad(q: &[Point; 4], p: Point) -> bool {
    let mut inside = false;
    for i in 0..4 {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
    }
    inside
}

trait IntoTuple {
    fn into_tuple(self) -> (f32, f32);
}

impl IntoTuple for Point {
    fn into_tuple(self) -> (f32, f32) {
        (self.x, self.y)
    }
}

pub fn percent(ui: &mut egui::Ui, v: &mut f32) -> egui::Response {
    let mut p = *v * 100.0;
    let r = ui.add(egui::DragValue::new(&mut p).range(0.0..=100.0).speed(0.5).max_decimals(0).suffix("%"));
    if r.changed() {
        *v = p / 100.0;
    }
    r
}

pub fn blend_combo(ui: &mut egui::Ui, id: &str, mode: &mut BlendMode) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id).selected_text(mode.name()).width(140.0).show_ui(ui, |ui| {
        let mut group = 0;
        for m in BlendMode::ALL {
            if m.group() != group {
                ui.separator();
                group = m.group();
            }
            changed |= ui.selectable_value(mode, m, m.name()).changed();
        }
    });
    changed
}

/// Used by the histogram panel and Levels dialog.
pub fn layer_histogram(tab: &DocTab) -> [[u32; 256]; 4] {
    match tab.doc.state.active_layer() {
        Some(l) => adjust::histogram(&l.pixels, tab.doc.state.bounds()),
        None => [[0; 256]; 4],
    }
}
