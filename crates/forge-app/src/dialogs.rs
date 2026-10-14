//! Modal dialogs. Dialogs that change pixels show a live preview: they keep
//! the layer's original pixels and re-run the operation (debounced) when a
//! parameter changes. Cancel restores the last committed state.

use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use forge_core::adjust::{self, curve_lut, Adjustment, Channel, Levels};
use forge_core::blend::BlendMode;
use forge_core::buffer::TiledBuffer;
use forge_core::document::Document;
use forge_core::fill;
use forge_core::filters::{self, Filter};
use forge_core::selection;
use forge_core::transform::{self, Affine, Resample};
use forge_core::{Color, IRect, Point, Rgba8};

use crate::canvas::DocTab;
use crate::panels::{from_c32, to_c32};
use crate::text::FontEntry;
use crate::tools::{self, ToolState};

pub enum Outcome {
    Open,
    Close,
    NewDocument(Document),
}

/// Keeps the original layer pixels for previews.
pub struct Preview {
    base: TiledBuffer<Rgba8>,
    base_sel: Option<selection::Mask>,
    layer: usize,
    pending: Option<Instant>,
    pub enabled: bool,
    last_rect: Option<IRect>,
}

impl Preview {
    pub fn new(tab: &DocTab) -> Option<Self> {
        let layer = tab.doc.state.active;
        let l = tab.doc.state.layers.get(layer)?;
        Some(Self {
            base: l.pixels.clone(),
            base_sel: tab.doc.state.selection.clone(),
            layer,
            pending: Some(Instant::now() - Duration::from_secs(1)),
            enabled: true,
            last_rect: None,
        })
    }

    pub fn touch(&mut self) {
        self.pending = Some(Instant::now());
    }

    /// Re-runs `op(target, base)` when parameters settled for a moment.
    fn update(
        &mut self,
        ctx: &egui::Context,
        tab: &mut DocTab,
        op: impl FnOnce(&mut TiledBuffer<Rgba8>, &TiledBuffer<Rgba8>, Option<&selection::Mask>) -> Option<IRect>,
    ) {
        let Some(t) = self.pending else { return };
        if t.elapsed() < Duration::from_millis(90) {
            ctx.request_repaint_after(Duration::from_millis(100));
            return;
        }
        self.pending = None;
        let Some(layer) = tab.doc.state.layers.get_mut(self.layer) else { return };
        layer.pixels = self.base.clone();
        let r = if self.enabled { op(&mut layer.pixels, &self.base, self.base_sel.as_ref()) } else { None };
        if let Some(prev) = self.last_rect {
            tab.doc.mark_dirty(prev);
        }
        if let Some(r) = r {
            tab.doc.mark_dirty(r);
        }
        self.last_rect = r;
    }

    /// Forces a final, synchronous render (used on OK).
    fn finish(
        &mut self,
        tab: &mut DocTab,
        op: impl FnOnce(&mut TiledBuffer<Rgba8>, &TiledBuffer<Rgba8>, Option<&selection::Mask>) -> Option<IRect>,
    ) {
        self.enabled = true;
        self.pending = Some(Instant::now() - Duration::from_secs(1));
        let Some(layer) = tab.doc.state.layers.get_mut(self.layer) else { return };
        layer.pixels = self.base.clone();
        if let Some(r) = op(&mut layer.pixels, &self.base, self.base_sel.as_ref()) {
            tab.doc.mark_dirty(r);
        }
        if let Some(prev) = self.last_rect {
            tab.doc.mark_dirty(prev);
        }
    }
}

fn ok_cancel(ui: &mut egui::Ui) -> (bool, bool) {
    let mut ok = false;
    let mut cancel = false;
    ui.separator();
    ui.horizontal(|ui| {
        ok = ui.button("   OK   ").clicked();
        cancel = ui.button("Cancel").clicked();
    });
    let (enter, esc) = ui.ctx().input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
    (ok || (enter && !ui.ctx().wants_keyboard_input()), cancel || esc)
}

fn window(title: &str) -> egui::Window<'_> {
    egui::Window::new(title).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
}

fn slider(ui: &mut egui::Ui, label: &str, v: &mut f32, range: std::ops::RangeInclusive<f32>) -> bool {
    ui.add(egui::Slider::new(v, range).text(label)).changed()
}

// ----------------------------------------------------------------- new doc

pub struct NewDoc {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub background: usize,
}

impl Default for NewDoc {
    fn default() -> Self {
        Self { name: "Untitled".into(), width: 1920, height: 1080, background: 0 }
    }
}

const PRESETS: [(&str, u32, u32); 8] = [
    ("Full HD 1920×1080", 1920, 1080),
    ("4K UHD 3840×2160", 3840, 2160),
    ("A4 @300 ppi", 2480, 3508),
    ("Letter @300 ppi", 2550, 3300),
    ("Instagram Post 1080×1080", 1080, 1080),
    ("Instagram Story 1080×1920", 1080, 1920),
    ("Icon 512×512", 512, 512),
    ("Web 800×600", 800, 600),
];

impl NewDoc {
    pub fn show(&mut self, ctx: &egui::Context, tools: &ToolState) -> Outcome {
        let mut out = Outcome::Open;
        window("New Document").show(ctx, |ui| {
            egui::Grid::new("newdoc").num_columns(2).show(ui, |ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut self.name);
                ui.end_row();
                ui.label("Preset");
                egui::ComboBox::from_id_salt("preset").selected_text("Choose…").show_ui(ui, |ui| {
                    for (n, w, h) in PRESETS {
                        if ui.selectable_label(false, n).clicked() {
                            self.width = w;
                            self.height = h;
                        }
                    }
                });
                ui.end_row();
                ui.label("Width");
                ui.add(egui::DragValue::new(&mut self.width).range(1..=30000).suffix(" px"));
                ui.end_row();
                ui.label("Height");
                ui.add(egui::DragValue::new(&mut self.height).range(1..=30000).suffix(" px"));
                ui.end_row();
                ui.label("Background");
                egui::ComboBox::from_id_salt("bgc")
                    .selected_text(["White", "Black", "Background Color", "Transparent"][self.background])
                    .show_ui(ui, |ui| {
                        for (i, n) in ["White", "Black", "Background Color", "Transparent"].iter().enumerate() {
                            ui.selectable_value(&mut self.background, i, *n);
                        }
                    });
                ui.end_row();
            });
            let mb = self.width as f64 * self.height as f64 * 4.0 / 1_048_576.0;
            ui.label(egui::RichText::new(format!("{mb:.1} MB per fully painted layer")).weak());
            let (ok, cancel) = ok_cancel(ui);
            if ok {
                let bg = match self.background {
                    0 => Some(Color::WHITE),
                    1 => Some(Color::BLACK),
                    2 => Some(tools.bg),
                    _ => None,
                };
                out = Outcome::NewDocument(Document::new(self.name.clone(), self.width, self.height, bg));
            } else if cancel {
                out = Outcome::Close;
            }
        });
        out
    }
}

// -------------------------------------------------------------- image size

pub struct ImageSize {
    w: u32,
    h: u32,
    orig: (u32, u32),
    keep: bool,
    method: Resample,
}

impl ImageSize {
    pub fn new(tab: &DocTab) -> Self {
        let (w, h) = (tab.doc.width(), tab.doc.height());
        Self { w, h, orig: (w, h), keep: true, method: Resample::Bicubic }
    }

    pub fn show(&mut self, ctx: &egui::Context, tab: &mut DocTab) -> Outcome {
        let mut out = Outcome::Open;
        window("Image Size").show(ctx, |ui| {
            let ratio = self.orig.0 as f32 / self.orig.1 as f32;
            egui::Grid::new("isz").num_columns(3).show(ui, |ui| {
                ui.label("Width");
                if ui.add(egui::DragValue::new(&mut self.w).range(1..=30000).suffix(" px")).changed() && self.keep {
                    self.h = ((self.w as f32 / ratio).round() as u32).max(1);
                }
                ui.label(format!("{:.1}%", self.w as f32 / self.orig.0 as f32 * 100.0));
                ui.end_row();
                ui.label("Height");
                if ui.add(egui::DragValue::new(&mut self.h).range(1..=30000).suffix(" px")).changed() && self.keep {
                    self.w = ((self.h as f32 * ratio).round() as u32).max(1);
                }
                ui.label(format!("{:.1}%", self.h as f32 / self.orig.1 as f32 * 100.0));
                ui.end_row();
            });
            ui.checkbox(&mut self.keep, "Constrain proportions");
            ui.horizontal(|ui| {
                for pct in [25, 50, 200] {
                    if ui.button(format!("{pct}%")).clicked() {
                        self.w = (self.orig.0 * pct / 100).max(1);
                        self.h = (self.orig.1 * pct / 100).max(1);
                    }
                }
            });
            egui::ComboBox::from_label("Resample").selected_text(self.method.name()).show_ui(ui, |ui| {
                for m in Resample::ALL {
                    ui.selectable_value(&mut self.method, m, m.name());
                }
            });
            let (ok, cancel) = ok_cancel(ui);
            if ok {
                transform::resize_document(&mut tab.doc.state, self.w, self.h, self.method);
                tab.doc.mark_all_dirty();
                tab.doc.set_selection(tab.doc.state.selection.clone());
                tab.doc.commit("Image Size");
                tab.view.request_fit();
                out = Outcome::Close;
            } else if cancel {
                out = Outcome::Close;
            }
        });
        out
    }
}

// ------------------------------------------------------------- canvas size

pub struct CanvasSize {
    w: u32,
    h: u32,
    anchor: (usize, usize),
}

impl CanvasSize {
    pub fn new(tab: &DocTab) -> Self {
        Self { w: tab.doc.width(), h: tab.doc.height(), anchor: (1, 1) }
    }

    pub fn show(&mut self, ctx: &egui::Context, tab: &mut DocTab) -> Outcome {
        let mut out = Outcome::Open;
        window("Canvas Size").show(ctx, |ui| {
            ui.label(format!("Current: {} × {} px", tab.doc.width(), tab.doc.height()));
            egui::Grid::new("csz").show(ui, |ui| {
                ui.label("Width");
                ui.add(egui::DragValue::new(&mut self.w).range(1..=30000).suffix(" px"));
                ui.end_row();
                ui.label("Height");
                ui.add(egui::DragValue::new(&mut self.h).range(1..=30000).suffix(" px"));
                ui.end_row();
            });
            ui.label("Anchor");
            egui::Grid::new("anchor").spacing(Vec2::splat(2.0)).show(ui, |ui| {
                for y in 0..3 {
                    for x in 0..3 {
                        let sel = self.anchor == (x, y);
                        if ui.add_sized([26.0, 26.0], egui::SelectableLabel::new(sel, if sel { "●" } else { "" })).clicked() {
                            self.anchor = (x, y);
                        }
                    }
                    ui.end_row();
                }
            });
            let (ok, cancel) = ok_cancel(ui);
            if ok {
                transform::canvas_size(&mut tab.doc.state, self.w, self.h, (self.anchor.0 as f32 / 2.0, self.anchor.1 as f32 / 2.0));
                tab.doc.mark_all_dirty();
                tab.doc.set_selection(tab.doc.state.selection.clone());
                tab.doc.commit("Canvas Size");
                tab.view.request_fit();
                out = Outcome::Close;
            } else if cancel {
                out = Outcome::Close;
            }
        });
        out
    }
}

// -------------------------------------------------------------- adjustments

pub struct AdjustDialog {
    pub adj: Adjustment,
    preview: Preview,
    hist: Option<Box<[[u32; 256]; 4]>>,
    drag_point: Option<usize>,
}

impl AdjustDialog {
    pub fn new(tab: &DocTab, adj: Adjustment) -> Option<Self> {
        let hist = matches!(adj, Adjustment::Levels { .. } | Adjustment::Curves { .. }).then(|| Box::new(tools::layer_histogram(tab)));
        Some(Self { adj, preview: Preview::new(tab)?, hist, drag_point: None })
    }

    pub fn show(&mut self, ctx: &egui::Context, tab: &mut DocTab, tools: &ToolState) -> Outcome {
        let mut out = Outcome::Open;
        let mut changed = false;
        window(self.adj.name()).show(ctx, |ui| {
            ui.set_min_width(300.0);
            changed |= self.controls(ui, tools);
            ui.add_space(4.0);
            if ui.checkbox(&mut self.preview.enabled, "Preview").changed() {
                changed = true;
            }
            let (ok, cancel) = ok_cancel(ui);
            if ok {
                let adj = self.adj.clone();
                self.preview.finish(tab, |t, b, s| adjust::apply(t, b, s, &adj));
                tab.doc.commit(self.adj.name());
                out = Outcome::Close;
            } else if cancel {
                tab.doc.revert_to_current();
                out = Outcome::Close;
            }
        });
        if changed {
            self.preview.touch();
        }
        if matches!(out, Outcome::Open) {
            let adj = self.adj.clone();
            self.preview.update(ctx, tab, |t, b, s| adjust::apply(t, b, s, &adj));
        }
        out
    }

    fn controls(&mut self, ui: &mut egui::Ui, tools: &ToolState) -> bool {
        let mut c = false;
        let hist = self.hist.as_deref();
        match &mut self.adj {
            Adjustment::BrightnessContrast { brightness, contrast } => {
                c |= slider(ui, "Brightness", brightness, -150.0..=150.0);
                c |= slider(ui, "Contrast", contrast, -50.0..=100.0);
            }
            Adjustment::Levels { channel, levels } => {
                c |= channel_combo(ui, channel);
                if let Some(h) = hist {
                    draw_histogram(ui, h, *channel, 256.0, 100.0);
                }
                c |= slider(ui, "Input black", &mut levels.in_black, 0.0..=253.0);
                c |= ui.add(egui::Slider::new(&mut levels.gamma, 0.1..=9.99).logarithmic(true).text("Gamma")).changed();
                c |= slider(ui, "Input white", &mut levels.in_white, 2.0..=255.0);
                if levels.in_white <= levels.in_black + 1.0 {
                    levels.in_white = levels.in_black + 2.0;
                }
                c |= slider(ui, "Output black", &mut levels.out_black, 0.0..=255.0);
                c |= slider(ui, "Output white", &mut levels.out_white, 0.0..=255.0);
                if ui.button("Reset").clicked() {
                    *levels = Levels::default();
                    c = true;
                }
            }
            Adjustment::Curves { channel, points } => {
                c |= channel_combo(ui, channel);
                c |= curve_editor(ui, points, &mut self.drag_point, hist, *channel);
                ui.label(egui::RichText::new("Click to add · drag to move · right-click to delete").weak());
                if ui.button("Reset").clicked() {
                    *points = vec![(0.0, 0.0), (255.0, 255.0)];
                    c = true;
                }
            }
            Adjustment::Exposure { exposure, offset, gamma } => {
                c |= slider(ui, "Exposure", exposure, -5.0..=5.0);
                c |= slider(ui, "Offset", offset, -0.5..=0.5);
                c |= ui.add(egui::Slider::new(gamma, 0.1..=9.99).logarithmic(true).text("Gamma")).changed();
            }
            Adjustment::HueSaturation { hue, saturation, lightness, colorize } => {
                c |= slider(ui, "Hue", hue, if *colorize { 0.0..=360.0 } else { -180.0..=180.0 });
                c |= slider(ui, "Saturation", saturation, if *colorize { 0.0..=100.0 } else { -100.0..=100.0 });
                c |= slider(ui, "Lightness", lightness, -100.0..=100.0);
                if ui.checkbox(colorize, "Colorize").changed() {
                    if *colorize {
                        *hue = hue.rem_euclid(360.0);
                        *saturation = saturation.clamp(0.0, 100.0).max(25.0);
                    }
                    c = true;
                }
            }
            Adjustment::Vibrance { vibrance, saturation } => {
                c |= slider(ui, "Vibrance", vibrance, -100.0..=100.0);
                c |= slider(ui, "Saturation", saturation, -100.0..=100.0);
            }
            Adjustment::ColorBalance { shadows, midtones, highlights } => {
                for (name, tone) in [("Shadows", shadows), ("Midtones", midtones), ("Highlights", highlights)] {
                    ui.label(egui::RichText::new(name).strong());
                    c |= slider(ui, "Cyan ↔ Red", &mut tone[0], -100.0..=100.0);
                    c |= slider(ui, "Magenta ↔ Green", &mut tone[1], -100.0..=100.0);
                    c |= slider(ui, "Yellow ↔ Blue", &mut tone[2], -100.0..=100.0);
                }
            }
            Adjustment::PhotoFilter { color, density } => {
                c |= color_button(ui, "Filter colour", color);
                c |= slider(ui, "Density", density, 0.0..=1.0);
            }
            Adjustment::Threshold { level } => {
                c |= ui.add(egui::Slider::new(level, 1..=255).text("Threshold level")).changed();
            }
            Adjustment::Posterize { levels } => {
                c |= ui.add(egui::Slider::new(levels, 2..=255).text("Levels")).changed();
            }
            Adjustment::GradientMap { from, to } => {
                c |= color_button(ui, "Shadows", from);
                c |= color_button(ui, "Highlights", to);
                if ui.button("Use foreground → background").clicked() {
                    *from = tools.fg;
                    *to = tools.bg;
                    c = true;
                }
            }
            Adjustment::Invert | Adjustment::Desaturate | Adjustment::BlackAndWhite => {
                ui.label("No options.");
            }
        }
        c
    }
}

fn color_button(ui: &mut egui::Ui, label: &str, c: &mut Color) -> bool {
    ui.horizontal(|ui| {
        let mut c32 = to_c32(*c);
        let r = ui.color_edit_button_srgba(&mut c32);
        ui.label(label);
        if r.changed() {
            *c = from_c32(c32);
            true
        } else {
            false
        }
    })
    .inner
}

fn channel_combo(ui: &mut egui::Ui, ch: &mut Channel) -> bool {
    let mut c = false;
    egui::ComboBox::from_label("Channel").selected_text(ch.name()).show_ui(ui, |ui| {
        for v in Channel::ALL {
            c |= ui.selectable_value(ch, v, v.name()).changed();
        }
    });
    c
}

fn draw_histogram(ui: &mut egui::Ui, h: &[[u32; 256]; 4], ch: Channel, w: f32, ht: f32) -> Rect {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, ht), Sense::hover());
    paint_histogram(ui.painter(), rect, h, ch);
    rect
}

fn paint_histogram(p: &egui::Painter, rect: Rect, h: &[[u32; 256]; 4], ch: Channel) {
    p.rect_filled(rect, 0.0, Color32::from_gray(28));
    let idx = match ch {
        Channel::Rgb => 3,
        Channel::Red => 0,
        Channel::Green => 1,
        Channel::Blue => 2,
    };
    let max = h[idx].iter().copied().max().unwrap_or(1).max(1) as f32;
    for i in 0..256 {
        let v = (h[idx][i] as f32 / max).sqrt();
        let x = rect.left() + rect.width() * i as f32 / 256.0;
        p.line_segment([Pos2::new(x, rect.bottom()), Pos2::new(x, rect.bottom() - v * rect.height())], Stroke::new(1.0_f32, Color32::from_gray(110)));
    }
}

fn curve_editor(
    ui: &mut egui::Ui,
    points: &mut Vec<(f32, f32)>,
    drag: &mut Option<usize>,
    hist: Option<&[[u32; 256]; 4]>,
    ch: Channel,
) -> bool {
    let size = 256.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click_and_drag());
    let p = ui.painter_at(rect);
    match hist {
        Some(h) => paint_histogram(&p, rect, h, ch),
        None => {
            p.rect_filled(rect, 0.0, Color32::from_gray(28));
        }
    }
    for i in 1..4 {
        let t = i as f32 / 4.0;
        let g = Stroke::new(1.0_f32, Color32::from_gray(60));
        p.line_segment([Pos2::new(rect.left() + t * size, rect.top()), Pos2::new(rect.left() + t * size, rect.bottom())], g);
        p.line_segment([Pos2::new(rect.left(), rect.top() + t * size), Pos2::new(rect.right(), rect.top() + t * size)], g);
    }
    let to_screen = |(x, y): (f32, f32)| Pos2::new(rect.left() + x / 255.0 * size, rect.bottom() - y / 255.0 * size);
    let to_val = |pos: Pos2| {
        (((pos.x - rect.left()) / size * 255.0).clamp(0.0, 255.0), ((rect.bottom() - pos.y) / size * 255.0).clamp(0.0, 255.0))
    };
    let mut changed = false;
    if let Some(pos) = resp.interact_pointer_pos() {
        let v = to_val(pos);
        if resp.drag_started() || resp.clicked() {
            let near = points.iter().position(|pt| to_screen(*pt).distance(pos) < 8.0);
            *drag = near.or_else(|| {
                points.push(v);
                changed = true;
                Some(points.len() - 1)
            });
        }
        if resp.dragged() {
            if let Some(i) = *drag {
                if i < points.len() {
                    points[i] = v;
                    changed = true;
                }
            }
        }
    }
    if resp.drag_stopped() || resp.clicked() {
        *drag = None;
        points.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    if resp.secondary_clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            if points.len() > 2 {
                if let Some(i) = points.iter().position(|pt| to_screen(*pt).distance(pos) < 8.0) {
                    points.remove(i);
                    changed = true;
                }
            }
        }
    }
    let lut = curve_lut(points);
    let col = match ch {
        Channel::Rgb => Color32::WHITE,
        Channel::Red => Color32::from_rgb(255, 90, 90),
        Channel::Green => Color32::from_rgb(90, 255, 90),
        Channel::Blue => Color32::from_rgb(110, 150, 255),
    };
    let line: Vec<Pos2> = (0..256).map(|i| to_screen((i as f32, lut[i] as f32))).collect();
    p.add(egui::Shape::line(line, Stroke::new(1.5_f32, col)));
    for pt in points.iter() {
        p.circle_filled(to_screen(*pt), 4.0, col);
    }
    changed
}

// ------------------------------------------------------------------ filters

pub struct FilterDialog {
    pub filter: Filter,
    preview: Preview,
}

impl FilterDialog {
    pub fn new(tab: &DocTab, filter: Filter) -> Option<Self> {
        Some(Self { filter, preview: Preview::new(tab)? })
    }

    pub fn show(&mut self, ctx: &egui::Context, tab: &mut DocTab, tools: &ToolState, last: &mut Option<Filter>) -> Outcome {
        let mut out = Outcome::Open;
        let mut changed = false;
        window(self.filter.name()).show(ctx, |ui| {
            ui.set_min_width(300.0);
            changed |= self.controls(ui, tools);
            changed |= ui.checkbox(&mut self.preview.enabled, "Preview").changed();
            let (ok, cancel) = ok_cancel(ui);
            if ok {
                let f = self.filter.clone();
                self.preview.finish(tab, |t, b, s| filters::apply(t, b, s, &f));
                tab.doc.commit(self.filter.name());
                *last = Some(self.filter.clone());
                out = Outcome::Close;
            } else if cancel {
                tab.doc.revert_to_current();
                out = Outcome::Close;
            }
        });
        if changed {
            self.preview.touch();
        }
        if matches!(out, Outcome::Open) {
            let f = self.filter.clone();
            self.preview.update(ctx, tab, |t, b, s| filters::apply(t, b, s, &f));
        }
        out
    }

    fn controls(&mut self, ui: &mut egui::Ui, tools: &ToolState) -> bool {
        let mut c = false;
        let log = |ui: &mut egui::Ui, v: &mut f32, r: std::ops::RangeInclusive<f32>, t: &str| {
            ui.add(egui::Slider::new(v, r).logarithmic(true).text(t)).changed()
        };
        match &mut self.filter {
            Filter::GaussianBlur { radius } => c |= log(ui, radius, 0.1..=250.0, "Radius (px)"),
            Filter::BoxBlur { radius } => c |= ui.add(egui::Slider::new(radius, 1..=500).logarithmic(true).text("Radius (px)")).changed(),
            Filter::MotionBlur { angle, distance } => {
                c |= slider(ui, "Angle (°)", angle, -90.0..=90.0);
                c |= log(ui, distance, 1.0..=500.0, "Distance (px)");
            }
            Filter::UnsharpMask { amount, radius, threshold } => {
                c |= slider(ui, "Amount", amount, 0.0..=5.0);
                c |= log(ui, radius, 0.1..=100.0, "Radius (px)");
                c |= ui.add(egui::Slider::new(threshold, 0..=255).text("Threshold")).changed();
            }
            Filter::HighPass { radius } => c |= log(ui, radius, 0.1..=250.0, "Radius (px)"),
            Filter::Median { radius } => c |= ui.add(egui::Slider::new(radius, 1..=50).text("Radius (px)")).changed(),
            Filter::AddNoise { amount, monochrome, seed } => {
                c |= slider(ui, "Amount", amount, 0.0..=1.0);
                c |= ui.checkbox(monochrome, "Monochromatic").changed();
                if ui.button("New seed").clicked() {
                    *seed = seed.wrapping_mul(747796405).wrapping_add(2891336453);
                    c = true;
                }
            }
            Filter::Pixelate { size } => c |= ui.add(egui::Slider::new(size, 2..=200).text("Cell size (px)")).changed(),
            Filter::Emboss { angle, amount } => {
                c |= slider(ui, "Angle (°)", angle, -180.0..=180.0);
                c |= slider(ui, "Amount", amount, 0.1..=5.0);
            }
            Filter::Twirl { angle } => c |= slider(ui, "Angle (°)", angle, -999.0..=999.0),
            Filter::Wave { amplitude, wavelength } => {
                c |= slider(ui, "Amplitude (px)", amplitude, 0.0..=200.0);
                c |= log(ui, wavelength, 2.0..=1000.0, "Wavelength (px)");
            }
            Filter::Clouds { fg, bg, scale, seed } => {
                c |= log(ui, scale, 4.0..=2000.0, "Scale");
                if ui.button("Use foreground/background colours").clicked() {
                    *fg = tools.fg;
                    *bg = tools.bg;
                    c = true;
                }
                if ui.button("New seed").clicked() {
                    *seed = seed.wrapping_mul(747796405).wrapping_add(2891336453);
                    c = true;
                }
            }
            Filter::Sharpen | Filter::FindEdges => {
                ui.label("No options.");
            }
        }
        c
    }
}

// ---------------------------------------------------------- free transform

pub struct TransformDialog {
    sx: f32,
    sy: f32,
    linked: bool,
    angle: f32,
    tx: f32,
    ty: f32,
    method: Resample,
    preview: Preview,
    center: Point,
    base_sel: Option<selection::Mask>,
}

impl TransformDialog {
    pub fn new(tab: &DocTab) -> Option<Self> {
        let l = tab.doc.state.active_layer()?;
        let bounds = match tab.doc.selection() {
            Some(s) => s.content_bounds(),
            None => l.pixels.content_bounds(),
        }?;
        let center = Point::new((bounds.x0 + bounds.x1) as f32 * 0.5, (bounds.y0 + bounds.y1) as f32 * 0.5);
        Some(Self {
            sx: 100.0,
            sy: 100.0,
            linked: true,
            angle: 0.0,
            tx: 0.0,
            ty: 0.0,
            method: Resample::Bilinear,
            preview: Preview::new(tab)?,
            center,
            base_sel: tab.doc.state.selection.clone(),
        })
    }

    fn matrix(&self) -> Affine {
        Affine::scale_rotate(self.center.x, self.center.y, self.sx / 100.0, self.sy / 100.0, self.angle, self.tx, self.ty)
    }

    pub fn show(&mut self, ctx: &egui::Context, tab: &mut DocTab) -> Outcome {
        let mut out = Outcome::Open;
        let mut changed = false;
        window("Free Transform").show(ctx, |ui| {
            egui::Grid::new("tf").num_columns(2).show(ui, |ui| {
                ui.label("Width");
                if ui.add(egui::DragValue::new(&mut self.sx).range(1.0..=2000.0).suffix("%")).changed() {
                    if self.linked {
                        self.sy = self.sx;
                    }
                    changed = true;
                }
                ui.end_row();
                ui.label("Height");
                if ui.add(egui::DragValue::new(&mut self.sy).range(1.0..=2000.0).suffix("%")).changed() {
                    if self.linked {
                        self.sx = self.sy;
                    }
                    changed = true;
                }
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut self.linked, "Link width & height");
                ui.end_row();
                ui.label("Angle");
                changed |= ui.add(egui::DragValue::new(&mut self.angle).range(-360.0..=360.0).speed(0.5).suffix("°")).changed();
                ui.end_row();
                ui.label("Move X");
                changed |= ui.add(egui::DragValue::new(&mut self.tx).suffix(" px")).changed();
                ui.end_row();
                ui.label("Move Y");
                changed |= ui.add(egui::DragValue::new(&mut self.ty).suffix(" px")).changed();
                ui.end_row();
            });
            egui::ComboBox::from_label("Interpolation").selected_text(self.method.name()).show_ui(ui, |ui| {
                for m in [Resample::Nearest, Resample::Bilinear] {
                    changed |= ui.selectable_value(&mut self.method, m, m.name()).changed();
                }
            });
            ui.horizontal(|ui| {
                for (label, deg) in [("⟲ 90°", -90.0), ("⟳ 90°", 90.0), ("180°", 180.0)] {
                    if ui.button(label).clicked() {
                        self.angle = (self.angle + deg + 180.0).rem_euclid(360.0) - 180.0;
                        changed = true;
                    }
                }
                if ui.button("Flip H").clicked() {
                    self.sx = -self.sx;
                    changed = true;
                }
                if ui.button("Flip V").clicked() {
                    self.sy = -self.sy;
                    changed = true;
                }
            });
            let (ok, cancel) = ok_cancel(ui);
            if ok {
                self.apply(tab);
                tab.doc.commit("Free Transform");
                out = Outcome::Close;
            } else if cancel {
                tab.doc.revert_to_current();
                out = Outcome::Close;
            }
        });
        if changed {
            self.preview.touch();
        }
        if matches!(out, Outcome::Open) && self.preview.pending.is_some_and(|t| t.elapsed() > Duration::from_millis(90)) {
            self.preview.pending = None;
            self.apply(tab);
        } else if self.preview.pending.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        out
    }

    fn apply(&mut self, tab: &mut DocTab) {
        let m = self.matrix();
        let (pixels, sel) = transform::transform_pixels(&self.preview.base, self.base_sel.as_ref(), m, self.method);
        if let Some(l) = tab.doc.state.layers.get_mut(self.preview.layer) {
            l.pixels = pixels;
        }
        if self.base_sel.is_some() {
            tab.doc.set_selection(sel);
        }
        tab.doc.mark_all_dirty();
    }
}

// --------------------------------------------------------------------- text

pub struct TextDialog {
    pub pos: Point,
    pub text: String,
}

impl TextDialog {
    pub fn show(&mut self, ctx: &egui::Context, tab: &mut DocTab, tools: &mut ToolState, fonts: &[FontEntry]) -> Outcome {
        let mut out = Outcome::Open;
        window("Text").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let name = fonts.get(tools.font_index).map(|f| f.name.as_str()).unwrap_or("?");
                egui::ComboBox::from_id_salt("font_dlg").selected_text(name).width(200.0).show_ui(ui, |ui| {
                    for (i, f) in fonts.iter().enumerate() {
                        ui.selectable_value(&mut tools.font_index, i, &f.name);
                    }
                });
                ui.add(egui::DragValue::new(&mut tools.text_size).range(4.0..=1000.0).suffix(" px"));
                color_button(ui, "Colour", &mut tools.fg);
            });
            let te = ui.add(egui::TextEdit::multiline(&mut self.text).desired_rows(4).desired_width(360.0).hint_text("Type your text…"));
            te.request_focus();
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("   OK   ").clicked() {
                    out = Outcome::Close;
                    if !self.text.trim().is_empty() {
                        if let Some(font) = fonts.get(tools.font_index).and_then(crate::text::load_font) {
                            let (w, h) = (tab.doc.width(), tab.doc.height());
                            let px = crate::text::render_text(&font, &self.text, tools.text_size, tools.fg, self.pos, w, h, tools.text_antialias);
                            let name: String = self.text.lines().next().unwrap_or("Text").chars().take(30).collect();
                            let id = tab.doc.state.alloc_id();
                            let layer = forge_core::Layer::with_pixels(id, name, px);
                            tab.doc.add_layer(layer, "Type Layer");
                        } else {
                            tools.status = "Could not load font".into();
                        }
                    }
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    out = Outcome::Close;
                }
            });
        });
        out
    }
}

// --------------------------------------------------------------- fill/stroke

pub struct FillDialog {
    contents: usize,
    color: Color,
    mode: BlendMode,
    opacity: f32,
    preserve: bool,
}

impl FillDialog {
    pub fn new(tools: &ToolState) -> Self {
        Self { contents: 0, color: tools.fg, mode: BlendMode::Normal, opacity: 1.0, preserve: false }
    }

    pub fn show(&mut self, ctx: &egui::Context, tab: &mut DocTab, tools: &ToolState) -> Outcome {
        let mut out = Outcome::Open;
        let names = ["Foreground Color", "Background Color", "Color…", "Black", "50% Gray", "White"];
        window("Fill").show(ctx, |ui| {
            egui::ComboBox::from_label("Contents").selected_text(names[self.contents]).show_ui(ui, |ui| {
                for (i, n) in names.iter().enumerate() {
                    ui.selectable_value(&mut self.contents, i, *n);
                }
            });
            if self.contents == 2 {
                color_button(ui, "Colour", &mut self.color);
            }
            ui.horizontal(|ui| {
                ui.label("Mode");
                tools::blend_combo(ui, "fill_mode", &mut self.mode);
            });
            ui.horizontal(|ui| {
                ui.label("Opacity");
                tools::percent(ui, &mut self.opacity);
            });
            ui.checkbox(&mut self.preserve, "Preserve Transparency");
            let (ok, cancel) = ok_cancel(ui);
            if ok {
                let color = match self.contents {
                    0 => tools.fg,
                    1 => tools.bg,
                    2 => self.color,
                    3 => Color::BLACK,
                    4 => Color::rgb(0.5, 0.5, 0.5),
                    _ => Color::WHITE,
                };
                fill_selection(tab, color, self.mode, self.opacity, self.preserve, "Fill");
                out = Outcome::Close;
            } else if cancel {
                out = Outcome::Close;
            }
        });
        out
    }
}

pub fn fill_selection(tab: &mut DocTab, color: Color, mode: BlendMode, opacity: f32, preserve: bool, label: &str) {
    let sel = tab.doc.state.selection.clone();
    let Some(layer) = tab.doc.state.active_layer_mut() else { return };
    if layer.locked {
        return;
    }
    let lock = preserve || layer.lock_alpha;
    if let Some(r) = fill::fill_mask(&mut layer.pixels, None, sel.as_ref(), color, opacity, mode, lock) {
        tab.doc.mark_dirty(r);
        tab.doc.commit(label);
    }
}

pub struct StrokeDialog {
    width: f32,
    color: Color,
}

impl StrokeDialog {
    pub fn new(tools: &ToolState) -> Self {
        Self { width: 3.0, color: tools.fg }
    }

    pub fn show(&mut self, ctx: &egui::Context, tab: &mut DocTab) -> Outcome {
        let mut out = Outcome::Open;
        window("Stroke").show(ctx, |ui| {
            ui.add(egui::Slider::new(&mut self.width, 1.0..=250.0).logarithmic(true).text("Width (px)"));
            color_button(ui, "Colour", &mut self.color);
            let (ok, cancel) = ok_cancel(ui);
            if ok {
                if let Some(sel) = tab.doc.state.selection.clone() {
                    let edge = fill::stroke_selection_mask(&sel, self.width);
                    if let Some(layer) = tab.doc.state.active_layer_mut() {
                        let lock = layer.lock_alpha;
                        if let Some(r) = fill::fill_mask(&mut layer.pixels, Some(&edge), None, self.color, 1.0, BlendMode::Normal, lock) {
                            tab.doc.mark_dirty(r);
                            tab.doc.commit("Stroke");
                        }
                    }
                }
                out = Outcome::Close;
            } else if cancel {
                out = Outcome::Close;
            }
        });
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SelModifyKind {
    Feather,
    Expand,
    Contract,
    Border,
}

pub struct SelModify {
    pub kind: SelModifyKind,
    pub amount: f32,
}

impl SelModify {
    pub fn show(&mut self, ctx: &egui::Context, tab: &mut DocTab) -> Outcome {
        let mut out = Outcome::Open;
        let title = match self.kind {
            SelModifyKind::Feather => "Feather Selection",
            SelModifyKind::Expand => "Expand Selection",
            SelModifyKind::Contract => "Contract Selection",
            SelModifyKind::Border => "Border Selection",
        };
        window(title).show(ctx, |ui| {
            ui.add(egui::Slider::new(&mut self.amount, 1.0..=250.0).logarithmic(true).text("Pixels"));
            let (ok, cancel) = ok_cancel(ui);
            if ok {
                if let Some(sel) = tab.doc.state.selection.clone() {
                    let n = self.amount.round() as i32;
                    let new = match self.kind {
                        SelModifyKind::Feather => selection::feather(&sel, self.amount),
                        SelModifyKind::Expand => selection::grow(&sel, n),
                        SelModifyKind::Contract => selection::grow(&sel, -n),
                        SelModifyKind::Border => fill::stroke_selection_mask(&sel, self.amount),
                    };
                    tab.doc.set_selection(new.content_bounds().map(|_| new));
                    tab.doc.commit(title);
                }
                out = Outcome::Close;
            } else if cancel {
                out = Outcome::Close;
            }
        });
        out
    }
}

// ---------------------------------------------------------------- simple ones

pub fn about(ctx: &egui::Context) -> Outcome {
    let mut out = Outcome::Open;
    window("About Forge Design").show(ctx, |ui| {
        ui.heading("Forge Design");
        ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
        ui.label("A fast, open-source raster image editor written in Rust.");
        ui.label("Licensed under the Apache License 2.0.");
        ui.hyperlink("https://github.com/burkiuze/ForgeDesign");
        ui.label(format!("Using {} CPU threads for rendering and filters.", rayon_threads()));
        if ui.button("Close").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            out = Outcome::Close;
        }
    });
    out
}

fn rayon_threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
}

pub fn shortcuts(ctx: &egui::Context) -> Outcome {
    let mut out = Outcome::Open;
    let rows = [
        ("Tools", "V Move · M Marquee · L Lasso · W Magic Wand · C Crop · I Eyedropper"),
        ("", "B Brush · N Pencil · S Clone Stamp · E Eraser · G Gradient · K Bucket"),
        ("", "T Type · U Shape · H Hand · Z Zoom · Shift+M toggles marquee shape"),
        ("Brush", "[ / ] size · Shift+[ / ] hardness · 1–0 opacity"),
        ("Colours", "X swap · D default · Alt+click eyedropper while painting"),
        ("View", "Ctrl + / Ctrl − zoom · Ctrl+0 fit · Ctrl+1 100% · Space+drag pan · Alt+wheel zoom"),
        ("Edit", "Ctrl+Z undo · Ctrl+Shift+Z / Ctrl+Y redo · Ctrl+T transform"),
        ("", "Ctrl+X/C/V cut/copy/paste · Ctrl+Shift+C copy merged · Del clear"),
        ("", "Alt+Backspace fill FG · Ctrl+Backspace fill BG"),
        ("Select", "Ctrl+A all · Ctrl+D deselect · Ctrl+Shift+I inverse · Shift/Alt modify"),
        ("Layer", "Ctrl+Shift+N new · Ctrl+J duplicate/via copy · Ctrl+E merge · Ctrl+[ ] arrange"),
        ("Image", "Ctrl+L levels · Ctrl+M curves · Ctrl+U hue/sat · Ctrl+I invert · Ctrl+B balance"),
        ("", "Ctrl+Alt+I image size · Ctrl+Alt+C canvas size · Ctrl+F repeat filter"),
        ("File", "Ctrl+N new · Ctrl+O open · Ctrl+S save · Ctrl+Shift+S save as · Ctrl+Shift+E export"),
    ];
    window("Keyboard Shortcuts").show(ctx, |ui| {
        egui::Grid::new("keys").striped(true).show(ui, |ui| {
            for (a, b) in rows {
                ui.label(egui::RichText::new(a).strong());
                ui.label(b);
                ui.end_row();
            }
        });
        if ui.button("Close").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            out = Outcome::Close;
        }
    });
    out
}

pub fn message(ctx: &egui::Context, title: &str, text: &str) -> Outcome {
    let mut out = Outcome::Open;
    window(title).show(ctx, |ui| {
        ui.label(text);
        if ui.button("OK").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Escape)) {
            out = Outcome::Close;
        }
    });
    out
}
