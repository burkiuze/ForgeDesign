//! Document view: GPU textures, zoom/pan, overlays and canvas input.

use std::collections::HashMap;

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Shape, Stroke as EStroke, TextureHandle, TextureOptions, Vec2};
use forge_core::composite::{self, Background};
use forge_core::document::Document;
use forge_core::selection;
use forge_core::{IRect, LayerId, Point};

use crate::tools::{Interaction, Tool, ToolState};

/// Display textures are split into chunks so any document size works
/// (GPUs cap texture dimensions) and edits only re-upload touched chunks.
const CHUNK: u32 = 1024;

pub struct DocTab {
    /// Unique per session (recovery file names).
    pub id: u64,
    pub doc: Document,
    pub view: View,
    pub pending_crop: Option<IRect>,
    /// For Select > Reselect.
    pub last_selection: Option<forge_core::selection::Mask>,
    /// Saved selections (alpha channels).
    pub channels: Vec<(String, forge_core::selection::Mask)>,
}

static NEXT_TAB: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl DocTab {
    pub fn new(doc: Document) -> Self {
        let id = NEXT_TAB.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + std::process::id() as u64 * 1000;
        Self { id, doc, view: View::default(), pending_crop: None, last_selection: None, channels: Vec::new() }
    }

    pub fn title(&self) -> String {
        format!("{}{}", self.doc.name, if self.doc.is_modified() { " •" } else { "" })
    }
}

pub struct View {
    pub zoom: f32,
    /// Offset of the image centre from the canvas centre, in screen points.
    pub pan: Vec2,
    fit_requested: bool,
    chunks: Vec<Option<TextureHandle>>,
    chunk_dims: (u32, u32),
    outline: Vec<(Point, Point)>,
    pub thumbs: HashMap<(LayerId, bool), (usize, TextureHandle)>,
    /// Last canvas rect on screen (for zoom commands from menus).
    pub canvas_rect: Rect,
    pub hover: Option<Point>,
    space_pan: bool,
    /// Cached histogram of the active layer, keyed by (history serial, layer).
    pub hist: Option<((u64, usize), Box<[[u32; 256]; 4]>)>,
    pub show_rulers: bool,
    pub show_grid: bool,
    pub grid_size: f32,
    pub show_guides: bool,
    pub snap: bool,
    /// (vertical, position in image px)
    pub guides: Vec<(bool, f32)>,
    dragging_guide: Option<usize>,
    pub proof: Proof,
    proof_applied: Proof,
}

/// View > Proof Colors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Proof {
    #[default]
    Off,
    /// Approximate CMYK print gamut (out-of-gamut colours are desaturated).
    Cmyk,
    Grayscale,
    /// Protanopia-like colour-blindness simulation.
    ColorBlind,
}

impl Proof {
    pub const ALL: [Proof; 4] = [Proof::Off, Proof::Cmyk, Proof::Grayscale, Proof::ColorBlind];
    pub fn name(self) -> &'static str {
        match self {
            Proof::Off => "Off",
            Proof::Cmyk => "Working CMYK",
            Proof::Grayscale => "Grayscale",
            Proof::ColorBlind => "Color Blindness (Protanopia)",
        }
    }

    fn apply(self, p: [u8; 4]) -> [u8; 4] {
        let c = [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0];
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        match self {
            Proof::Off => p,
            Proof::Grayscale => {
                let l = forge_core::color::lum(c);
                [q(l), q(l), q(l), p[3]]
            }
            Proof::Cmyk => {
                // Compress saturation of very saturated colours and lift deep
                // blacks a little, roughly like an uncoated print gamut.
                let [h, s, l] = forge_core::color::rgb_to_hsl(c);
                let s2 = if s > 0.75 { 0.75 + (s - 0.75) * 0.35 } else { s };
                let l2 = 0.04 + l * 0.94;
                let o = forge_core::color::hsl_to_rgb([h, s2, l2]);
                [q(o[0]), q(o[1]), q(o[2]), p[3]]
            }
            Proof::ColorBlind => {
                let r = 0.567 * c[0] + 0.433 * c[1];
                let g = 0.558 * c[0] + 0.442 * c[1];
                let b = 0.242 * c[1] + 0.758 * c[2];
                [q(r), q(g), q(b), p[3]]
            }
        }
    }
}

impl Default for View {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: Vec2::ZERO,
            fit_requested: true,
            chunks: Vec::new(),
            chunk_dims: (0, 0),
            outline: Vec::new(),
            thumbs: HashMap::new(),
            canvas_rect: Rect::NOTHING,
            hover: None,
            space_pan: false,
            hist: None,
            show_rulers: true,
            show_grid: false,
            grid_size: 32.0,
            show_guides: true,
            snap: true,
            guides: Vec::new(),
            dragging_guide: None,
            proof: Proof::Off,
            proof_applied: Proof::Off,
        }
    }
}

const ZOOM_STEPS: [f32; 25] = [
    0.01, 0.02, 0.03, 0.05, 0.0625, 0.0833, 0.125, 0.1667, 0.25, 0.3333, 0.5, 0.6667, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0,
    8.0, 12.0, 16.0, 24.0, 32.0, 64.0,
];

impl View {
    pub fn request_fit(&mut self) {
        self.fit_requested = true;
    }

    /// Snaps an image-space point to guides, grid and the canvas edges
    /// when within 6 screen pixels.
    pub fn snap_point(&self, p: Point, w: u32, h: u32) -> Point {
        if !self.snap {
            return p;
        }
        let tol = 6.0 / self.zoom.max(0.01);
        let mut xs: Vec<f32> = vec![0.0, w as f32, w as f32 * 0.5];
        let mut ys: Vec<f32> = vec![0.0, h as f32, h as f32 * 0.5];
        if self.show_guides {
            for &(vertical, pos) in &self.guides {
                if vertical {
                    xs.push(pos);
                } else {
                    ys.push(pos);
                }
            }
        }
        let mut out = p;
        if self.show_grid && self.grid_size > 1.0 {
            xs.push((p.x / self.grid_size).round() * self.grid_size);
            ys.push((p.y / self.grid_size).round() * self.grid_size);
        }
        if let Some(x) = xs.into_iter().filter(|x| (x - p.x).abs() < tol).min_by(|a, b| (a - p.x).abs().total_cmp(&(b - p.x).abs())) {
            out.x = x;
        }
        if let Some(y) = ys.into_iter().filter(|y| (y - p.y).abs() < tol).min_by(|a, b| (a - p.y).abs().total_cmp(&(b - p.y).abs())) {
            out.y = y;
        }
        out
    }

    fn origin(&self, canvas: Rect, w: u32, h: u32) -> Pos2 {
        canvas.center() + self.pan - Vec2::new(w as f32, h as f32) * self.zoom * 0.5
    }

    pub fn to_screen(&self, canvas: Rect, w: u32, h: u32, p: Point) -> Pos2 {
        self.origin(canvas, w, h) + Vec2::new(p.x, p.y) * self.zoom
    }

    pub fn to_image(&self, canvas: Rect, w: u32, h: u32, s: Pos2) -> Point {
        let v = (s - self.origin(canvas, w, h)) / self.zoom;
        Point::new(v.x, v.y)
    }

    pub fn fit(&mut self, w: u32, h: u32) {
        let avail = self.canvas_rect.size() - Vec2::splat(40.0);
        if avail.x <= 0.0 || avail.y <= 0.0 {
            return;
        }
        self.zoom = (avail.x / w as f32).min(avail.y / h as f32).min(1.0);
        self.pan = Vec2::ZERO;
    }

    /// Zooms keeping the image point under `anchor` (screen) fixed.
    pub fn zoom_to(&mut self, new_zoom: f32, anchor: Pos2, w: u32, h: u32) {
        let canvas = self.canvas_rect;
        let img = self.to_image(canvas, w, h, anchor);
        self.zoom = new_zoom.clamp(0.01, 64.0);
        let after = self.to_screen(canvas, w, h, img);
        self.pan += anchor - after;
    }

    pub fn zoom_step(&mut self, dir: i32, anchor: Option<Pos2>, w: u32, h: u32) {
        let z = self.zoom;
        let next = if dir > 0 {
            ZOOM_STEPS.iter().copied().find(|&s| s > z * 1.001).unwrap_or(64.0)
        } else {
            ZOOM_STEPS.iter().rev().copied().find(|&s| s < z * 0.999).unwrap_or(0.01)
        };
        self.zoom_to(next, anchor.unwrap_or(self.canvas_rect.center()), w, h);
    }

    /// Uploads changed regions of the composite to the GPU.
    fn sync_textures(&mut self, ctx: &egui::Context, doc: &mut Document) {
        let (w, h) = (doc.width(), doc.height());
        if self.chunk_dims != (w, h) {
            self.chunks.clear();
            self.chunk_dims = (w, h);
            let n = (w.div_ceil(CHUNK) * h.div_ceil(CHUNK)) as usize;
            self.chunks.resize_with(n, || None);
            doc.mark_all_dirty();
        }
        if self.proof != self.proof_applied {
            self.proof_applied = self.proof;
            doc.mark_all_dirty();
        }
        if doc.take_selection_changed() {
            self.outline = doc.selection().map(selection::outline).unwrap_or_default();
        }
        let Some(dirty) = doc.take_dirty() else { return };
        let dirty = dirty.clamp_to(w, h);
        if dirty.is_empty() {
            return;
        }
        let cx_n = w.div_ceil(CHUNK);
        let opts = TextureOptions {
            magnification: egui::TextureFilter::Nearest,
            minification: egui::TextureFilter::Linear,
            wrap_mode: egui::TextureWrapMode::ClampToEdge,
            mipmap_mode: Some(egui::TextureFilter::Linear),
        };
        // Grow the region to whole chunks for chunks that don't exist yet.
        let mut region = dirty;
        for cy in (dirty.y0 as u32 / CHUNK)..=((dirty.y1 as u32 - 1) / CHUNK) {
            for cx in (dirty.x0 as u32 / CHUNK)..=((dirty.x1 as u32 - 1) / CHUNK) {
                if self.chunks[(cy * cx_n + cx) as usize].is_none() {
                    region = region.union(&chunk_rect(cx, cy, w, h));
                }
            }
        }
        let px = composite::composite_rect(&doc.state, region, Background::Checker);
        let rw = region.width() as usize;
        for cy in (region.y0 as u32 / CHUNK)..=((region.y1 as u32 - 1) / CHUNK) {
            for cx in (region.x0 as u32 / CHUNK)..=((region.x1 as u32 - 1) / CHUNK) {
                let cr = chunk_rect(cx, cy, w, h);
                let slot = &mut self.chunks[(cy * cx_n + cx) as usize];
                let sub = if slot.is_none() { cr } else { cr.intersect(&dirty) };
                if sub.is_empty() {
                    continue;
                }
                let mut pixels = Vec::with_capacity((sub.width() * sub.height()) as usize);
                for y in sub.y0..sub.y1 {
                    let row = (y - region.y0) as usize * rw;
                    for x in sub.x0..sub.x1 {
                        let p = self.proof.apply(px[row + (x - region.x0) as usize]);
                        pixels.push(Color32::from_rgb(p[0], p[1], p[2]));
                    }
                }
                let img = egui::ColorImage { size: [sub.width() as usize, sub.height() as usize], pixels };
                match slot {
                    Some(tex) => tex.set_partial([(sub.x0 - cr.x0) as usize, (sub.y0 - cr.y0) as usize], img, opts),
                    None => *slot = Some(ctx.load_texture(format!("chunk{cx}_{cy}"), img, opts)),
                }
            }
        }
    }
}

fn chunk_rect(cx: u32, cy: u32, w: u32, h: u32) -> IRect {
    IRect::new((cx * CHUNK) as i32, (cy * CHUNK) as i32, ((cx + 1) * CHUNK) as i32, ((cy + 1) * CHUNK) as i32).clamp_to(w, h)
}

const RULER: f32 = 18.0;

/// Draws the canvas and routes pointer input to the active tool.
pub fn canvas_ui(ui: &mut egui::Ui, tab: &mut DocTab, tools: &mut ToolState, interactive: bool) {
    let (full_rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    let ctx = ui.ctx().clone();
    let rect = full_rect;
    tab.view.canvas_rect = rect;
    let (w, h) = (tab.doc.width(), tab.doc.height());
    if tab.view.fit_requested && rect.width() > 50.0 {
        tab.view.fit(w, h);
        tab.view.fit_requested = false;
    }
    tools.update_transform_preview(tab);
    tab.view.sync_textures(&ctx, &mut tab.doc);

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, crate::app::INK);

    // ----------------------------------------------------------- input
    let (mods, pointer, scroll, zoom_delta, space) = ctx.input(|i| {
        (i.modifiers, i.pointer.clone(), i.raw_scroll_delta, i.zoom_delta(), i.key_down(egui::Key::Space))
    });
    let hover_pos = response.hover_pos();
    tab.view.hover = hover_pos.map(|p| tab.view.to_image(rect, w, h, p));

    if response.hovered() {
        if zoom_delta != 1.0 {
            let anchor = hover_pos.unwrap_or(rect.center());
            let z = tab.view.zoom * zoom_delta;
            tab.view.zoom_to(z, anchor, w, h);
        } else if mods.alt && scroll.y != 0.0 {
            tab.view.zoom_step(scroll.y.signum() as i32, hover_pos, w, h);
        } else if scroll != Vec2::ZERO {
            tab.view.pan += scroll;
        }
    }

    // Guides: drag out of the rulers, or move existing ones with the Move tool.
    let in_top_ruler = |p: Pos2| tab.view.show_rulers && p.y < rect.top() + RULER && p.x > rect.left() + RULER;
    let in_left_ruler = |p: Pos2| tab.view.show_rulers && p.x < rect.left() + RULER && p.y > rect.top() + RULER;
    let mut guide_input = false;
    if interactive {
        if let Some(pos) = pointer.interact_pos() {
            if pointer.primary_pressed() && response.hovered() {
                if in_top_ruler(pos) {
                    tab.view.guides.push((false, tab.view.to_image(rect, w, h, pos).y));
                    tab.view.dragging_guide = Some(tab.view.guides.len() - 1);
                    tab.view.show_guides = true;
                } else if in_left_ruler(pos) {
                    tab.view.guides.push((true, tab.view.to_image(rect, w, h, pos).x));
                    tab.view.dragging_guide = Some(tab.view.guides.len() - 1);
                    tab.view.show_guides = true;
                } else if tools.tool == Tool::Move && tab.view.show_guides && tools.transform.is_none() {
                    let ip = tab.view.to_image(rect, w, h, pos);
                    let tol = 4.0 / tab.view.zoom;
                    tab.view.dragging_guide = tab
                        .view
                        .guides
                        .iter()
                        .position(|&(v, g)| if v { (ip.x - g).abs() < tol } else { (ip.y - g).abs() < tol });
                }
            }
            if let Some(gi) = tab.view.dragging_guide {
                guide_input = true;
                let ip = tab.view.to_image(rect, w, h, pos);
                if let Some(g) = tab.view.guides.get_mut(gi) {
                    g.1 = if g.0 { ip.x.round() } else { ip.y.round() };
                }
                if !pointer.primary_down() {
                    // Dropping a guide back on the ruler deletes it.
                    if in_top_ruler(pos) || in_left_ruler(pos) || !rect.contains(pos) {
                        tab.view.guides.remove(gi);
                    }
                    tab.view.dragging_guide = None;
                }
            }
        }
    }

    // Panning: middle mouse, Space+drag, or the Hand tool.
    let panning_tool = tools.tool == Tool::Hand || space;
    if response.dragged_by(egui::PointerButton::Middle)
        || (panning_tool && response.dragged_by(egui::PointerButton::Primary) && !tools.is_busy())
    {
        tab.view.pan += response.drag_delta();
        tab.view.space_pan = true;
    } else if !pointer.any_down() {
        tab.view.space_pan = false;
    }

    if interactive && !guide_input && !panning_tool && !tab.view.space_pan {
        let snapping = tools.tool.snaps() || tools.transform.is_some();
        let snap = move |tab: &DocTab, p: Point| if snapping { tab.view.snap_point(p, w, h) } else { p };
        if let Some(pos) = pointer.interact_pos() {
            let ip = snap(tab, tab.view.to_image(rect, w, h, pos));
            if pointer.primary_pressed() && response.hovered() {
                let double = pointer.button_double_clicked(egui::PointerButton::Primary);
                match tools.tool {
                    Tool::Zoom if tools.transform.is_none() => tab.view.zoom_step(if mods.alt { -1 } else { 1 }, Some(pos), w, h),
                    _ => tools.pointer_down(tab, ip, mods, double),
                }
            } else if tools.is_busy() || (tools.tool == Tool::PolygonLasso && pointer.is_moving()) {
                // Use all intermediate pointer events for smooth strokes.
                let moves: Vec<Pos2> = ctx.input(|i| {
                    i.events
                        .iter()
                        .filter_map(|e| match e {
                            egui::Event::PointerMoved(p) => Some(*p),
                            _ => None,
                        })
                        .collect()
                });
                if pointer.primary_down() || tools.tool == Tool::PolygonLasso {
                    if moves.is_empty() {
                        tools.pointer_move(tab, ip, mods);
                    }
                    for m in moves {
                        let p = snap(tab, tab.view.to_image(rect, w, h, m));
                        tools.pointer_move(tab, p, mods);
                    }
                }
            }
        }
        if tools.is_busy() && !pointer.primary_down() {
            tools.pointer_up(tab);
        }
    }
    // Re-sync after edits made this frame so the result shows immediately.
    tools.update_transform_preview(tab);
    tab.view.sync_textures(&ctx, &mut tab.doc);

    // ---------------------------------------------------------- drawing
    let view = &tab.view;
    let img_rect = Rect::from_min_size(view.origin(rect, w, h), Vec2::new(w as f32, h as f32) * view.zoom);
    painter.rect_filled(img_rect.translate(Vec2::splat(3.0)), 0.0, Color32::from_black_alpha(90));
    let cx_n = w.div_ceil(CHUNK);
    for (i, tex) in view.chunks.iter().enumerate() {
        let Some(tex) = tex else { continue };
        let cr = chunk_rect(i as u32 % cx_n, i as u32 / cx_n, w, h);
        let r = Rect::from_min_max(
            view.to_screen(rect, w, h, Point::new(cr.x0 as f32, cr.y0 as f32)),
            view.to_screen(rect, w, h, Point::new(cr.x1 as f32, cr.y1 as f32)),
        );
        if r.intersects(rect) {
            painter.image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
    }
    painter.rect_stroke(img_rect, 0.0, EStroke::new(1.0_f32, Color32::from_gray(20)));
    let s = |p: Point| view.to_screen(rect, w, h, p);

    // Pixel grid at high zoom, user grid, guides.
    let vis = img_rect.intersect(rect);
    if view.zoom >= 12.0 {
        draw_grid(&painter, view, rect, w, h, vis, 1.0, Color32::from_white_alpha(25));
    }
    if view.show_grid && view.grid_size * view.zoom >= 6.0 {
        draw_grid(&painter, view, rect, w, h, vis, view.grid_size, Color32::from_rgba_unmultiplied(120, 160, 255, 60));
    }
    if view.show_guides {
        let gs = EStroke::new(1.0_f32, Color32::from_rgb(0, 220, 220));
        for &(vertical, pos) in &view.guides {
            if vertical {
                let x = s(Point::new(pos, 0.0)).x;
                painter.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], gs);
            } else {
                let y = s(Point::new(0.0, pos)).y;
                painter.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], gs);
            }
        }
    }

    let time = ctx.input(|i| i.time);

    // Marching ants (offset while patching).
    if !view.outline.is_empty() {
        let shift = match &tools.interaction {
            Interaction::Patch { start, current } => Vec2::new(current.x - start.x, current.y - start.y) * view.zoom,
            _ => Vec2::ZERO,
        };
        let offset = (time * 8.0) as f32 % 8.0;
        let black = EStroke::new(1.0_f32, Color32::BLACK);
        let white = EStroke::new(1.0_f32, Color32::WHITE);
        let too_many = view.outline.len() > 30_000;
        for &(a, b) in &view.outline {
            let (pa, pb) = (s(a) + shift, s(b) + shift);
            if !rect.intersects(Rect::from_two_pos(pa, pb).expand(1.0)) {
                continue;
            }
            painter.line_segment([pa, pb], black);
            if !too_many {
                painter.extend(Shape::dashed_line_with_offset(&[pa, pb], white, &[4.0], &[4.0], (offset + (a.x + a.y) * view.zoom).rem_euclid(8.0)));
            }
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(80));
    }

    // Vector paths.
    let accent = EStroke::new(1.0_f32, crate::app::ACCENT);
    for (pi, path) in tab.doc.state.paths.iter().enumerate() {
        let active = tab.doc.state.active_path == Some(pi);
        let stroke = if active { EStroke::new(1.5_f32, Color32::from_rgb(40, 140, 255)) } else { EStroke::new(1.0_f32, Color32::from_rgba_unmultiplied(40, 140, 255, 90)) };
        for (pts, closed) in path.flatten() {
            let mut sp: Vec<Pos2> = pts.iter().map(|p| s(*p)).collect();
            if closed {
                if let Some(f) = sp.first().copied() {
                    sp.push(f);
                }
            }
            painter.add(Shape::line(sp, stroke));
        }
        if active && matches!(tools.tool, Tool::Pen | Tool::DirectSelect | Tool::PathSelect | Tool::FreeformPen) {
            for sub in &path.subpaths {
                for a in &sub.anchors {
                    let ap = s(a.p);
                    for hnd in [a.h_in, a.h_out] {
                        if hnd != a.p {
                            painter.line_segment([ap, s(hnd)], EStroke::new(1.0_f32, Color32::from_gray(160)));
                            painter.circle_filled(s(hnd), 3.0, Color32::from_rgb(40, 140, 255));
                        }
                    }
                    painter.rect_filled(Rect::from_center_size(ap, Vec2::splat(6.0)), 0.0, Color32::WHITE);
                    painter.rect_stroke(Rect::from_center_size(ap, Vec2::splat(6.0)), 0.0, EStroke::new(1.0_f32, Color32::from_rgb(40, 140, 255)));
                }
            }
        }
    }

    // Interaction previews.
    let dashed = |painter: &egui::Painter, pts: &[Pos2]| {
        painter.add(Shape::line(pts.to_vec(), EStroke::new(1.0_f32, Color32::BLACK)));
        painter.extend(Shape::dashed_line(pts, EStroke::new(1.0_f32, Color32::WHITE), 4.0, 4.0));
    };
    let ellipse = |r: Rect| -> Vec<Pos2> {
        (0..=64)
            .map(|i| {
                let t = i as f32 / 64.0 * std::f32::consts::TAU;
                r.center() + Vec2::new(t.cos() * r.width() * 0.5, t.sin() * r.height() * 0.5)
            })
            .collect()
    };
    match &tools.interaction {
        Interaction::Marquee { start, current, .. } => {
            let r = Rect::from_two_pos(s(*start), s(*current));
            if tools.tool == Tool::EllipseMarquee {
                dashed(&painter, &ellipse(r));
            } else {
                dashed(&painter, &[r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()]);
            }
        }
        Interaction::Lasso { points, .. } => {
            let mut pts: Vec<Pos2> = points.iter().map(|p| s(*p)).collect();
            if let Some(f) = pts.first().copied() {
                pts.push(f);
            }
            dashed(&painter, &pts);
        }
        Interaction::Gradient { start, current } => {
            painter.line_segment([s(*start), s(*current)], EStroke::new(3.0_f32, Color32::BLACK));
            painter.line_segment([s(*start), s(*current)], EStroke::new(1.0_f32, Color32::WHITE));
            painter.circle_filled(s(*start), 3.0, Color32::WHITE);
        }
        Interaction::Shape { start, current } => {
            let params = forge_core::shapes::ShapeParams { line_width: tools.shape_stroke_width, ..tools.shape_params };
            let pts: Vec<Pos2> = forge_core::shapes::outline(tools.shape_kind, *start, *current, &params).into_iter().map(s).collect();
            let mut closed = pts.clone();
            if !tools.shape_kind.is_open() {
                if let Some(f) = closed.first().copied() {
                    closed.push(f);
                }
            }
            painter.add(Shape::line(closed, accent));
        }
        Interaction::Crop { start, current } => draw_crop(&painter, img_rect, Rect::from_two_pos(s(*start), s(*current))),
        Interaction::Ruler { start, current } => draw_ruler(&painter, s(*start), s(*current), *start, *current),
        _ => {}
    }
    if let (Some((a, b)), Interaction::None) = (tools.ruler, &tools.interaction) {
        if tools.tool == Tool::Ruler {
            draw_ruler(&painter, s(a), s(b), a, b);
        }
    }
    if let Some(r) = tab.pending_crop {
        draw_crop(&painter, img_rect, Rect::from_two_pos(s(Point::new(r.x0 as f32, r.y0 as f32)), s(Point::new(r.x1 as f32, r.y1 as f32))));
    }
    if let Some(q) = tools.persp_crop {
        let pts: Vec<Pos2> = q.iter().map(|p| s(*p)).collect();
        let mut closed = pts.clone();
        closed.push(pts[0]);
        painter.add(Shape::line(closed, EStroke::new(1.5_f32, Color32::WHITE)));
        for i in 1..3 {
            let t = i as f32 / 3.0;
            painter.line_segment([pts[0] + (pts[1] - pts[0]) * t, pts[3] + (pts[2] - pts[3]) * t], EStroke::new(1.0_f32, Color32::from_white_alpha(80)));
            painter.line_segment([pts[0] + (pts[3] - pts[0]) * t, pts[1] + (pts[2] - pts[1]) * t], EStroke::new(1.0_f32, Color32::from_white_alpha(80)));
        }
        for p in pts {
            painter.rect_filled(Rect::from_center_size(p, Vec2::splat(8.0)), 0.0, Color32::WHITE);
        }
    }
    if let Some(t) = &tools.transform {
        let pts: Vec<Pos2> = t.quad.iter().map(|p| s(*p)).collect();
        let mut closed = pts.clone();
        closed.push(pts[0]);
        painter.add(Shape::line(closed, EStroke::new(1.0_f32, Color32::from_rgb(40, 140, 255))));
        if t.mode == crate::tools::TransformMode::Warp {
            let g: Vec<Pos2> = t.grid.iter().map(|p| s(*p)).collect();
            for r in 0..4 {
                painter.add(Shape::line((0..4).map(|c| g[r * 4 + c]).collect(), EStroke::new(1.0_f32, Color32::from_gray(200))));
                painter.add(Shape::line((0..4).map(|c| g[c * 4 + r]).collect(), EStroke::new(1.0_f32, Color32::from_gray(200))));
            }
            for p in g {
                painter.circle_filled(p, 4.0, Color32::WHITE);
                painter.circle_stroke(p, 4.0, EStroke::new(1.0_f32, Color32::BLACK));
            }
        } else {
            for i in 0..4 {
                let handles = [pts[i], pts[i] + (pts[(i + 1) % 4] - pts[i]) * 0.5];
                for hp in handles {
                    let r = Rect::from_center_size(hp, Vec2::splat(8.0));
                    painter.rect_filled(r, 0.0, Color32::WHITE);
                    painter.rect_stroke(r, 0.0, EStroke::new(1.0_f32, Color32::BLACK));
                }
            }
            let c = s(t.center());
            painter.circle_stroke(c, 4.0, EStroke::new(1.0_f32, Color32::WHITE));
        }
    }
    if let (Tool::CloneStamp | Tool::Healing, Some(src)) = (tools.tool, tools.clone_source) {
        let p = s(src);
        painter.line_segment([p - Vec2::new(6.0, 0.0), p + Vec2::new(6.0, 0.0)], EStroke::new(1.5_f32, Color32::RED));
        painter.line_segment([p - Vec2::new(0.0, 6.0), p + Vec2::new(0.0, 6.0)], EStroke::new(1.5_f32, Color32::RED));
    }

    // Rulers.
    if view.show_rulers {
        draw_rulers(&painter, view, rect, w, h, hover_pos);
    }

    // Cursor.
    if let Some(pos) = hover_pos {
        let busy_pan = panning_tool || tab.view.space_pan;
        if busy_pan {
            ctx.set_cursor_icon(if pointer.any_down() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
        } else if tools.transform.is_some() {
            ctx.set_cursor_icon(egui::CursorIcon::Move);
        } else if let Some(size) = tools.brush_size() {
            let r = (size * 0.5 * tab.view.zoom).max(1.0);
            painter.circle_stroke(pos, r, EStroke::new(1.0_f32, Color32::BLACK));
            painter.circle_stroke(pos, r + 1.0, EStroke::new(1.0_f32, Color32::from_white_alpha(200)));
            ctx.set_cursor_icon(if r < 4.0 { egui::CursorIcon::Crosshair } else { egui::CursorIcon::None });
        } else {
            ctx.set_cursor_icon(match tools.tool {
                Tool::Move => egui::CursorIcon::Move,
                Tool::Text => egui::CursorIcon::Text,
                Tool::VerticalText => egui::CursorIcon::VerticalText,
                Tool::Zoom => egui::CursorIcon::ZoomIn,
                Tool::PathSelect | Tool::DirectSelect => egui::CursorIcon::Default,
                _ => egui::CursorIcon::Crosshair,
            });
        }
    }
}

fn draw_grid(painter: &egui::Painter, view: &View, rect: Rect, w: u32, h: u32, vis: Rect, step: f32, col: Color32) {
    let a = view.to_image(rect, w, h, vis.min);
    let b = view.to_image(rect, w, h, vis.max);
    let st = EStroke::new(1.0_f32, col);
    let mut x = (a.x.max(0.0) / step).floor() * step;
    while x <= b.x.min(w as f32) {
        let sx = view.to_screen(rect, w, h, Point::new(x, 0.0)).x;
        painter.line_segment([Pos2::new(sx, vis.min.y), Pos2::new(sx, vis.max.y)], st);
        x += step;
    }
    let mut y = (a.y.max(0.0) / step).floor() * step;
    while y <= b.y.min(h as f32) {
        let sy = view.to_screen(rect, w, h, Point::new(0.0, y)).y;
        painter.line_segment([Pos2::new(vis.min.x, sy), Pos2::new(vis.max.x, sy)], st);
        y += step;
    }
}

fn draw_rulers(painter: &egui::Painter, view: &View, rect: Rect, w: u32, h: u32, hover: Option<Pos2>) {
    let bg = Color32::from_rgb(40, 39, 46);
    let fg = Color32::from_gray(150);
    let top = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.min.y + RULER));
    let left = Rect::from_min_max(rect.min, Pos2::new(rect.min.x + RULER, rect.max.y));
    painter.rect_filled(top, 0.0, bg);
    painter.rect_filled(left, 0.0, bg);
    // Pick a tick spacing of >= 50 screen px.
    let steps = [1.0, 2.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0];
    let major = steps.iter().copied().find(|s| s * view.zoom >= 50.0).unwrap_or(10000.0);
    let minor = major / 5.0;
    let font = egui::FontId::monospace(9.0);
    let a = view.to_image(rect, w, h, rect.min);
    let b = view.to_image(rect, w, h, rect.max);
    let mut x = (a.x / minor).floor() * minor;
    while x <= b.x {
        let sx = view.to_screen(rect, w, h, Point::new(x, 0.0)).x;
        if sx > rect.min.x + RULER {
            let is_major = (x / major).round() * major == x;
            let len = if is_major { RULER } else { 5.0 };
            painter.line_segment([Pos2::new(sx, top.max.y - len), Pos2::new(sx, top.max.y)], EStroke::new(1.0_f32, fg));
            if is_major {
                painter.text(Pos2::new(sx + 2.0, top.min.y + 1.0), egui::Align2::LEFT_TOP, format!("{}", x as i64), font.clone(), fg);
            }
        }
        x += minor;
    }
    let mut y = (a.y / minor).floor() * minor;
    while y <= b.y {
        let sy = view.to_screen(rect, w, h, Point::new(0.0, y)).y;
        if sy > rect.min.y + RULER {
            let is_major = (y / major).round() * major == y;
            let len = if is_major { RULER } else { 5.0 };
            painter.line_segment([Pos2::new(left.max.x - len, sy), Pos2::new(left.max.x, sy)], EStroke::new(1.0_f32, fg));
            if is_major {
                painter.text(Pos2::new(left.min.x + 1.0, sy + 2.0), egui::Align2::LEFT_TOP, format!("{}", y as i64), font.clone(), fg);
            }
        }
        y += minor;
    }
    painter.rect_filled(Rect::from_min_size(rect.min, Vec2::splat(RULER)), 0.0, bg);
    if let Some(p) = hover {
        let acc = crate::app::ACCENT;
        painter.line_segment([Pos2::new(p.x, top.min.y), Pos2::new(p.x, top.max.y)], EStroke::new(1.0_f32, acc));
        painter.line_segment([Pos2::new(left.min.x, p.y), Pos2::new(left.max.x, p.y)], EStroke::new(1.0_f32, acc));
    }
}

fn draw_ruler(painter: &egui::Painter, a: Pos2, b: Pos2, ia: Point, ib: Point) {
    painter.line_segment([a, b], EStroke::new(3.0_f32, Color32::BLACK));
    painter.line_segment([a, b], EStroke::new(1.0_f32, Color32::WHITE));
    for p in [a, b] {
        painter.line_segment([p - Vec2::new(5.0, 0.0), p + Vec2::new(5.0, 0.0)], EStroke::new(1.0_f32, Color32::WHITE));
        painter.line_segment([p - Vec2::new(0.0, 5.0), p + Vec2::new(0.0, 5.0)], EStroke::new(1.0_f32, Color32::WHITE));
    }
    let ang = -(ib.y - ia.y).atan2(ib.x - ia.x).to_degrees();
    painter.text(
        b + Vec2::new(8.0, -8.0),
        egui::Align2::LEFT_BOTTOM,
        format!("{:.1} px  {:.1}°", ia.distance(ib), ang),
        egui::FontId::proportional(12.0),
        Color32::WHITE,
    );
}

fn draw_crop(painter: &egui::Painter, img: Rect, r: Rect) {
    let shade = Color32::from_black_alpha(140);
    painter.rect_filled(Rect::from_min_max(img.min, Pos2::new(img.max.x, r.min.y)), 0.0, shade);
    painter.rect_filled(Rect::from_min_max(Pos2::new(img.min.x, r.max.y), img.max), 0.0, shade);
    painter.rect_filled(Rect::from_min_max(Pos2::new(img.min.x, r.min.y), Pos2::new(r.min.x, r.max.y)), 0.0, shade);
    painter.rect_filled(Rect::from_min_max(Pos2::new(r.max.x, r.min.y), Pos2::new(img.max.x, r.max.y)), 0.0, shade);
    painter.rect_stroke(r, 0.0, EStroke::new(1.0_f32, Color32::WHITE));
    // Rule of thirds.
    let third = EStroke::new(1.0_f32, Color32::from_white_alpha(70));
    for i in 1..3 {
        let x = r.min.x + r.width() * i as f32 / 3.0;
        let y = r.min.y + r.height() * i as f32 / 3.0;
        painter.line_segment([Pos2::new(x, r.min.y), Pos2::new(x, r.max.y)], third);
        painter.line_segment([Pos2::new(r.min.x, y), Pos2::new(r.max.x, y)], third);
    }
}
