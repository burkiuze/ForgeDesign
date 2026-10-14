//! Tools: state, options, and pointer handling on the canvas.

use eframe::egui::{self, Modifiers};
use forge_core::adjust;
use forge_core::blend::BlendMode;
use forge_core::brush::{BrushSettings, Stroke, StrokeSource, StrokeTarget};
use forge_core::buffer::TiledBuffer;
use forge_core::composite;
use forge_core::fill::{self, GradientKind, ShapeKind};
use forge_core::selection::{self, Mask, SelectionOp};
use forge_core::transform;
use forge_core::{Color, IRect, Layer, Point, Rgba8};

use crate::canvas::DocTab;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    Move,
    Marquee,
    EllipseMarquee,
    Lasso,
    MagicWand,
    Crop,
    Eyedropper,
    Brush,
    Pencil,
    CloneStamp,
    Eraser,
    Gradient,
    Bucket,
    Text,
    Shape,
    Hand,
    Zoom,
}

impl Tool {
    pub const ALL: [Tool; 17] = [
        Tool::Move,
        Tool::Marquee,
        Tool::EllipseMarquee,
        Tool::Lasso,
        Tool::MagicWand,
        Tool::Crop,
        Tool::Eyedropper,
        Tool::Brush,
        Tool::Pencil,
        Tool::CloneStamp,
        Tool::Eraser,
        Tool::Gradient,
        Tool::Bucket,
        Tool::Text,
        Tool::Shape,
        Tool::Hand,
        Tool::Zoom,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Tool::Move => "Move",
            Tool::Marquee => "Rectangular Marquee",
            Tool::EllipseMarquee => "Elliptical Marquee",
            Tool::Lasso => "Lasso",
            Tool::MagicWand => "Magic Wand",
            Tool::Crop => "Crop",
            Tool::Eyedropper => "Eyedropper",
            Tool::Brush => "Brush",
            Tool::Pencil => "Pencil",
            Tool::CloneStamp => "Clone Stamp",
            Tool::Eraser => "Eraser",
            Tool::Gradient => "Gradient",
            Tool::Bucket => "Paint Bucket",
            Tool::Text => "Horizontal Type",
            Tool::Shape => "Shape",
            Tool::Hand => "Hand",
            Tool::Zoom => "Zoom",
        }
    }

    pub fn shortcut(self) -> Option<egui::Key> {
        use egui::Key::*;
        Some(match self {
            Tool::Move => V,
            Tool::Marquee => M,
            Tool::Lasso => L,
            Tool::MagicWand => W,
            Tool::Crop => C,
            Tool::Eyedropper => I,
            Tool::Brush => B,
            Tool::Pencil => N,
            Tool::CloneStamp => S,
            Tool::Eraser => E,
            Tool::Gradient => G,
            Tool::Bucket => K,
            Tool::Text => T,
            Tool::Shape => U,
            Tool::Hand => H,
            Tool::Zoom => Z,
            Tool::EllipseMarquee => return None,
        })
    }

    pub fn is_paint(self) -> bool {
        matches!(self, Tool::Brush | Tool::Pencil | Tool::Eraser | Tool::CloneStamp)
    }

    pub fn is_selection(self) -> bool {
        matches!(self, Tool::Marquee | Tool::EllipseMarquee | Tool::Lasso | Tool::MagicWand)
    }
}

/// In-progress pointer interaction.
pub enum Interaction {
    None,
    Stroke { stroke: Box<Stroke>, label: &'static str },
    Move { base: TiledBuffer<Rgba8>, base_sel: Option<Mask>, start: Point, last: (i32, i32), bounds: IRect },
    Marquee { start: Point, current: Point, op: SelectionOp },
    Lasso { points: Vec<Point>, op: SelectionOp },
    Gradient { start: Point, current: Point },
    Shape { start: Point, current: Point },
    Crop { start: Point, current: Point },
    Eyedropper,
}

pub struct ToolState {
    pub tool: Tool,
    pub fg: Color,
    pub bg: Color,
    pub brush: BrushSettings,
    pub pencil: BrushSettings,
    pub eraser: BrushSettings,
    pub clone: BrushSettings,
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
    pub shape_kind: ShapeKind,
    pub shape_line_width: f32,
    pub shape_new_layer: bool,
    pub text_size: f32,
    pub font_index: usize,
    pub text_antialias: bool,
    pub clone_source: Option<Point>,
    pub clone_offset: Option<(i32, i32)>,
    pub interaction: Interaction,
    /// Set by the Text tool on click; the app opens the text dialog.
    pub text_request: Option<Point>,
    pub status: String,
}

impl Default for ToolState {
    fn default() -> Self {
        Self {
            tool: Tool::Brush,
            fg: Color::BLACK,
            bg: Color::WHITE,
            brush: BrushSettings::default(),
            pencil: BrushSettings::pencil(1.0),
            eraser: BrushSettings { size: 40.0, hardness: 0.6, ..Default::default() },
            clone: BrushSettings { size: 40.0, hardness: 0.5, ..Default::default() },
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
            shape_kind: ShapeKind::Rectangle,
            shape_line_width: 4.0,
            shape_new_layer: true,
            text_size: 48.0,
            font_index: 0,
            text_antialias: true,
            clone_source: None,
            clone_offset: None,
            interaction: Interaction::None,
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

/// Square/circle constraint for marquees and shapes.
fn constrain_square(a: Point, b: Point) -> Point {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let s = dx.abs().max(dy.abs());
    Point::new(a.x + s * dx.signum(), a.y + s * dy.signum())
}

impl ToolState {
    pub fn swap_colors(&mut self) {
        std::mem::swap(&mut self.fg, &mut self.bg);
    }

    pub fn reset_colors(&mut self) {
        self.fg = Color::BLACK;
        self.bg = Color::WHITE;
    }

    pub fn current_brush(&mut self) -> Option<&mut BrushSettings> {
        match self.tool {
            Tool::Brush => Some(&mut self.brush),
            Tool::Pencil => Some(&mut self.pencil),
            Tool::Eraser => Some(&mut self.eraser),
            Tool::CloneStamp => Some(&mut self.clone),
            _ => None,
        }
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
            tab.doc.state.active_layer().map(|l| l.pixels.clone()).unwrap_or_else(|| {
                TiledBuffer::new(tab.doc.width(), tab.doc.height())
            })
        }
    }

    /// Returns `Err(message)` when the active layer cannot be edited.
    fn editable_layer(tab: &DocTab) -> Result<&Layer, String> {
        let l = tab.doc.state.active_layer().ok_or("No layer")?;
        if l.locked {
            return Err(format!("Layer \"{}\" is locked", l.name));
        }
        if !l.visible {
            return Err(format!("Layer \"{}\" is hidden", l.name));
        }
        Ok(l)
    }

    pub fn pointer_down(&mut self, tab: &mut DocTab, p: Point, mods: Modifiers) {
        self.status.clear();
        // Alt+click with a painting tool picks a colour (Photoshop behaviour).
        if mods.alt && matches!(self.tool, Tool::Brush | Tool::Pencil | Tool::Bucket | Tool::Gradient | Tool::Shape) {
            if let Some(c) = Self::sample_color(tab, p) {
                self.fg = c;
            }
            self.interaction = Interaction::Eyedropper;
            return;
        }
        match self.tool {
            Tool::Brush | Tool::Pencil | Tool::Eraser | Tool::CloneStamp => {
                if self.tool == Tool::CloneStamp && mods.alt {
                    self.clone_source = Some(p);
                    self.clone_offset = None;
                    self.status = "Clone source set".into();
                    return;
                }
                let layer = match Self::editable_layer(tab) {
                    Ok(l) => l,
                    Err(e) => {
                        self.status = e;
                        return;
                    }
                };
                let editing_mask = tab.doc.state.editing_mask && layer.mask.is_some();
                let (settings, source, label) = match self.tool {
                    Tool::Brush => (self.brush, StrokeSource::Color(self.fg), "Brush Tool"),
                    Tool::Pencil => (self.pencil, StrokeSource::Color(self.fg), "Pencil"),
                    Tool::Eraser => (self.eraser, StrokeSource::Erase, "Eraser"),
                    _ => {
                        let Some(src) = self.clone_source else {
                            self.status = "Alt+click to set the clone source first".into();
                            return;
                        };
                        let off = *self.clone_offset.get_or_insert(((src.x - p.x).round() as i32, (src.y - p.y).round() as i32));
                        let source = Self::sample_source(tab, self.sample_merged);
                        (self.clone, StrokeSource::Clone { source, dx: off.0, dy: off.1 }, "Clone Stamp")
                    }
                };
                let sel = tab.doc.state.selection.clone();
                let mut stroke = if editing_mask {
                    Stroke::on_mask(settings, source, layer.mask.clone().unwrap(), sel)
                } else {
                    Stroke::on_pixels(settings, source, layer.pixels.clone(), sel, layer.lock_alpha)
                };
                let r = Self::stroke_into(&mut stroke, tab, p, editing_mask);
                if let Some(r) = r {
                    tab.doc.mark_dirty(r);
                }
                self.interaction = Interaction::Stroke { stroke: Box::new(stroke), label };
            }
            Tool::Move => {
                let layer = match Self::editable_layer(tab) {
                    Ok(l) => l,
                    Err(e) => {
                        self.status = e;
                        return;
                    }
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
            Tool::Lasso => {
                self.interaction = Interaction::Lasso { points: vec![p], op: op_from_mods(self.sel_op, mods) };
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
            Tool::Bucket => {
                if let Err(e) = Self::editable_layer(tab) {
                    self.status = e;
                    return;
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
            Tool::Gradient => self.interaction = Interaction::Gradient { start: p, current: p },
            Tool::Shape => self.interaction = Interaction::Shape { start: p, current: p },
            Tool::Crop => {
                tab.pending_crop = None;
                self.interaction = Interaction::Crop { start: p, current: p };
            }
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
            Tool::Text => {
                if Self::editable_layer(tab).is_ok() || !tab.doc.state.layers.is_empty() {
                    self.text_request = Some(p);
                }
            }
            Tool::Hand | Tool::Zoom => {}
        }
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
        match &mut self.interaction {
            Interaction::Stroke { stroke, .. } => {
                let mask = tab.doc.state.editing_mask
                    && tab.doc.state.active_layer().is_some_and(|l| l.mask.is_some());
                if let Some(r) = Self::stroke_into(stroke, tab, p, mask) {
                    tab.doc.mark_dirty(r);
                }
            }
            Interaction::Move { base, base_sel, start, last, bounds } => {
                let d = ((p.x - start.x).round() as i32, (p.y - start.y).round() as i32);
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
            Interaction::Marquee { start, current, .. } => {
                *current = if mods.shift && !matches!(self.sel_op, SelectionOp::Add) && start.distance(p) > 0.0 {
                    constrain_square(*start, p)
                } else {
                    p
                };
            }
            Interaction::Lasso { points, .. } => {
                if points.last().is_none_or(|l| l.distance(p) >= 1.0) {
                    points.push(p);
                }
            }
            Interaction::Gradient { start, current } => {
                *current = if mods.shift { constrain45(*start, p) } else { p };
            }
            Interaction::Shape { start, current } => {
                *current = if mods.shift {
                    if self.shape_kind == ShapeKind::Line {
                        constrain45(*start, p)
                    } else {
                        constrain_square(*start, p)
                    }
                } else {
                    p
                };
            }
            Interaction::Crop { current, .. } => *current = p,
            Interaction::Eyedropper => {
                if let Some(c) = Self::sample_color(tab, p) {
                    if mods.alt && self.tool == Tool::Eyedropper {
                        self.bg = c;
                    } else {
                        self.fg = c;
                    }
                }
            }
            Interaction::None => {}
        }
    }

    pub fn pointer_up(&mut self, tab: &mut DocTab) {
        let (w, h) = (tab.doc.width(), tab.doc.height());
        match std::mem::replace(&mut self.interaction, Interaction::None) {
            Interaction::Stroke { stroke, label } => {
                if stroke.touched().is_some() {
                    if let Some(l) = tab.doc.state.active_layer_mut() {
                        l.pixels.optimize();
                    }
                    tab.doc.commit(label);
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
                        tab.doc.commit(if self.tool == Tool::EllipseMarquee { "Elliptical Marquee" } else { "Rectangular Marquee" });
                    }
                    None if op == SelectionOp::Replace && tab.doc.selection().is_some() => {
                        tab.doc.set_selection(None);
                        tab.doc.commit("Deselect");
                    }
                    None => {}
                }
            }
            Interaction::Lasso { points, op } => {
                if points.len() >= 3 {
                    let m = selection::polygon_mask(w, h, &points);
                    let m = if self.feather > 0.0 { selection::feather(&m, self.feather) } else { m };
                    let sel = selection::combine(tab.doc.selection(), m, op);
                    tab.doc.set_selection(sel);
                    tab.doc.commit("Lasso");
                }
            }
            Interaction::Gradient { start, current } => {
                if start.distance(current) < 1.0 {
                    return;
                }
                if let Err(e) = Self::editable_layer(tab) {
                    self.status = e;
                    return;
                }
                let (mut c0, mut c1) = (self.fg, if self.gradient_to_transparent { self.fg.with_alpha(0.0) } else { self.bg });
                if self.gradient_reverse {
                    std::mem::swap(&mut c0, &mut c1);
                }
                let sel = tab.doc.state.selection.clone();
                let (kind, op, mode) = (self.gradient_kind, self.fill_opacity, self.fill_mode);
                let layer = tab.doc.state.active_layer_mut().unwrap();
                let lock = layer.lock_alpha;
                if let Some(r) = fill::gradient(&mut layer.pixels, sel.as_ref(), kind, start, current, c0, c1, op, mode, lock) {
                    tab.doc.mark_dirty(r);
                    tab.doc.commit("Gradient");
                }
            }
            Interaction::Shape { start, current } => {
                if start.distance(current) < 1.0 {
                    return;
                }
                let shape = fill::shape_mask(w, h, self.shape_kind, start, current, self.shape_line_width);
                if self.shape_new_layer {
                    let name = format!("{} {}", self.shape_kind.name(), tab.doc.state.layers.len());
                    let mut layer = tab.doc.state.new_layer(name);
                    fill::fill_mask(&mut layer.pixels, Some(&shape), None, self.fg, 1.0, BlendMode::Normal, false);
                    tab.doc.add_layer(layer, self.shape_kind.name());
                } else {
                    if let Err(e) = Self::editable_layer(tab) {
                        self.status = e;
                        return;
                    }
                    let sel = tab.doc.state.selection.clone();
                    let (fg, op, mode) = (self.fg, self.fill_opacity, self.fill_mode);
                    let layer = tab.doc.state.active_layer_mut().unwrap();
                    let lock = layer.lock_alpha;
                    if let Some(r) = fill::fill_mask(&mut layer.pixels, Some(&shape), sel.as_ref(), fg, op, mode, lock) {
                        tab.doc.mark_dirty(r);
                        tab.doc.commit(self.shape_kind.name());
                    }
                }
            }
            Interaction::Crop { start, current } => {
                let r = IRect::from_points(start, current).clamp_to(w, h);
                tab.pending_crop = (!r.is_empty() && r.width() > 1 && r.height() > 1).then_some(r);
                if tab.pending_crop.is_some() {
                    self.status = "Press Enter to crop, Esc to cancel".into();
                }
            }
            Interaction::Eyedropper | Interaction::None => {}
        }
    }

    /// Cancels the current interaction (Esc), restoring the committed state.
    pub fn cancel(&mut self, tab: &mut DocTab) {
        let was_editing = matches!(self.interaction, Interaction::Stroke { .. } | Interaction::Move { .. });
        self.interaction = Interaction::None;
        if was_editing {
            tab.doc.revert_to_current();
        }
        tab.pending_crop = None;
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

    /// Tool options bar (below the menu).
    pub fn options_ui(&mut self, ui: &mut egui::Ui, fonts: &[crate::text::FontEntry], tab: Option<&mut DocTab>) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(self.tool.name()).strong());
            ui.separator();
            match self.tool {
                Tool::Brush | Tool::Pencil | Tool::Eraser | Tool::CloneStamp => {
                    let tool = self.tool;
                    let b = self.current_brush().unwrap();
                    ui.label("Size");
                    ui.add(egui::DragValue::new(&mut b.size).range(1.0..=2000.0).speed(0.5).suffix(" px"));
                    if tool != Tool::Pencil {
                        ui.label("Hardness");
                        percent(ui, &mut b.hardness);
                    }
                    ui.label("Opacity");
                    percent(ui, &mut b.opacity);
                    if tool != Tool::Pencil {
                        ui.label("Flow");
                        percent(ui, &mut b.flow);
                    }
                    ui.label("Spacing");
                    ui.add(egui::DragValue::new(&mut b.spacing).range(0.01..=2.0).speed(0.01).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
                    if tool == Tool::Eraser {
                        ui.checkbox(&mut b.antialias, "Smooth");
                    }
                    if tool == Tool::CloneStamp {
                        ui.checkbox(&mut self.sample_merged, "Sample All Layers");
                        if ui.button("Reset Alignment").clicked() {
                            self.clone_offset = None;
                        }
                    }
                }
                Tool::Marquee | Tool::EllipseMarquee | Tool::Lasso | Tool::MagicWand => {
                    for op in SelectionOp::ALL {
                        ui.selectable_value(&mut self.sel_op, op, op.name());
                    }
                    ui.separator();
                    ui.label("Feather");
                    ui.add(egui::DragValue::new(&mut self.feather).range(0.0..=250.0).speed(0.2).suffix(" px"));
                    if self.tool == Tool::MagicWand {
                        ui.label("Tolerance");
                        ui.add(egui::DragValue::new(&mut self.wand_tolerance).range(0..=255));
                        ui.checkbox(&mut self.wand_contiguous, "Contiguous");
                        ui.checkbox(&mut self.sample_merged, "Sample All Layers");
                    }
                }
                Tool::Bucket => {
                    blend_combo(ui, "bucket_mode", &mut self.fill_mode);
                    ui.label("Opacity");
                    percent(ui, &mut self.fill_opacity);
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
                    ui.checkbox(&mut self.gradient_to_transparent, "FG → Transparent");
                }
                Tool::Shape => {
                    for k in ShapeKind::ALL {
                        ui.selectable_value(&mut self.shape_kind, k, k.name());
                    }
                    if self.shape_kind == ShapeKind::Line {
                        ui.label("Weight");
                        ui.add(egui::DragValue::new(&mut self.shape_line_width).range(1.0..=500.0).suffix(" px"));
                    }
                    ui.checkbox(&mut self.shape_new_layer, "New Layer");
                }
                Tool::Text => {
                    let name = fonts.get(self.font_index).map(|f| f.name.as_str()).unwrap_or("?");
                    egui::ComboBox::from_id_salt("font").selected_text(name).width(220.0).show_ui(ui, |ui| {
                        for (i, f) in fonts.iter().enumerate() {
                            ui.selectable_value(&mut self.font_index, i, &f.name);
                        }
                    });
                    ui.add(egui::DragValue::new(&mut self.text_size).range(4.0..=1000.0).suffix(" pt"));
                    ui.checkbox(&mut self.text_antialias, "Anti-alias");
                    ui.label("Click on the canvas to place text");
                }
                Tool::Crop => {
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
                Tool::Move => {
                    ui.label("Drag to move the layer or selection. Arrow keys nudge (Shift = 10 px).");
                }
                Tool::Eyedropper => {
                    ui.label("Click: foreground colour · Alt+click: background colour");
                }
                Tool::Hand => {
                    ui.label("Drag to scroll. Space+drag works with any tool.");
                }
                Tool::Zoom => {
                    ui.label("Click to zoom in · Alt+click to zoom out");
                }
            }
        });
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
