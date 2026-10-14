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
    pub doc: Document,
    pub view: View,
    pub pending_crop: Option<IRect>,
}

impl DocTab {
    pub fn new(doc: Document) -> Self {
        Self { doc, view: View::default(), pending_crop: None }
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
                        let p = px[row + (x - region.x0) as usize];
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

/// Draws the canvas and routes pointer input to the active tool.
pub fn canvas_ui(ui: &mut egui::Ui, tab: &mut DocTab, tools: &mut ToolState, interactive: bool) {
    let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    let ctx = ui.ctx().clone();
    tab.view.canvas_rect = rect;
    let (w, h) = (tab.doc.width(), tab.doc.height());
    if tab.view.fit_requested && rect.width() > 50.0 {
        tab.view.fit(w, h);
        tab.view.fit_requested = false;
    }
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

    if interactive && !panning_tool && !tab.view.space_pan {
        if let Some(pos) = pointer.interact_pos() {
            let ip = tab.view.to_image(rect, w, h, pos);
            if pointer.primary_pressed() && response.hovered() {
                match tools.tool {
                    Tool::Zoom => tab.view.zoom_step(if mods.alt { -1 } else { 1 }, Some(pos), w, h),
                    _ => tools.pointer_down(tab, ip, mods),
                }
            } else if tools.is_busy() && pointer.primary_down() {
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
                if moves.is_empty() {
                    tools.pointer_move(tab, ip, mods);
                }
                for m in moves {
                    let p = tab.view.to_image(rect, w, h, m);
                    tools.pointer_move(tab, p, mods);
                }
            }
        }
        if tools.is_busy() && !pointer.primary_down() {
            tools.pointer_up(tab);
        }
    }
    // Re-sync after edits made this frame so the result shows immediately.
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

    // Pixel grid at high zoom.
    if view.zoom >= 12.0 {
        let vis = img_rect.intersect(rect);
        let a = view.to_image(rect, w, h, vis.min);
        let b = view.to_image(rect, w, h, vis.max);
        let grid = EStroke::new(1.0_f32, Color32::from_white_alpha(25));
        for x in a.x.floor().max(0.0) as i32..=b.x.ceil().min(w as f32) as i32 {
            let sx = view.to_screen(rect, w, h, Point::new(x as f32, 0.0)).x;
            painter.line_segment([Pos2::new(sx, vis.min.y), Pos2::new(sx, vis.max.y)], grid);
        }
        for y in a.y.floor().max(0.0) as i32..=b.y.ceil().min(h as f32) as i32 {
            let sy = view.to_screen(rect, w, h, Point::new(0.0, y as f32)).y;
            painter.line_segment([Pos2::new(vis.min.x, sy), Pos2::new(vis.max.x, sy)], grid);
        }
    }

    let s = |p: Point| view.to_screen(rect, w, h, p);
    let time = ctx.input(|i| i.time);

    // Marching ants.
    if !view.outline.is_empty() {
        let offset = (time * 8.0) as f32 % 8.0;
        let black = EStroke::new(1.0_f32, Color32::BLACK);
        let white = EStroke::new(1.0_f32, Color32::WHITE);
        let too_many = view.outline.len() > 30_000;
        for &(a, b) in &view.outline {
            let (pa, pb) = (s(a), s(b));
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

    // Interaction previews.
    let accent = EStroke::new(1.0_f32, crate::app::ACCENT);
    let dashed = |painter: &egui::Painter, pts: &[Pos2]| {
        painter.add(Shape::line(pts.to_vec(), EStroke::new(1.0_f32, Color32::BLACK)));
        painter.extend(Shape::dashed_line(pts, EStroke::new(1.0_f32, Color32::WHITE), 4.0, 4.0));
    };
    match &tools.interaction {
        Interaction::Marquee { start, current, .. } => {
            let r = Rect::from_two_pos(s(*start), s(*current));
            if tools.tool == Tool::EllipseMarquee {
                let pts: Vec<Pos2> = (0..=64)
                    .map(|i| {
                        let t = i as f32 / 64.0 * std::f32::consts::TAU;
                        r.center() + Vec2::new(t.cos() * r.width() * 0.5, t.sin() * r.height() * 0.5)
                    })
                    .collect();
                dashed(&painter, &pts);
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
            let r = Rect::from_two_pos(s(*start), s(*current));
            match tools.shape_kind {
                forge_core::fill::ShapeKind::Rectangle => {
                    painter.rect_stroke(r, 0.0, accent);
                }
                forge_core::fill::ShapeKind::Ellipse => {
                    let pts: Vec<Pos2> = (0..=64)
                        .map(|i| {
                            let t = i as f32 / 64.0 * std::f32::consts::TAU;
                            r.center() + Vec2::new(t.cos() * r.width() * 0.5, t.sin() * r.height() * 0.5)
                        })
                        .collect();
                    painter.add(Shape::line(pts, accent));
                }
                forge_core::fill::ShapeKind::Line => {
                    painter.line_segment([s(*start), s(*current)], EStroke::new((tools.shape_line_width * view.zoom).max(1.0), Color32::from_rgba_unmultiplied(198, 244, 50, 140)));
                }
            }
        }
        Interaction::Crop { start, current } => draw_crop(&painter, img_rect, Rect::from_two_pos(s(*start), s(*current))),
        _ => {}
    }
    if let Some(r) = tab.pending_crop {
        draw_crop(&painter, img_rect, Rect::from_two_pos(s(Point::new(r.x0 as f32, r.y0 as f32)), s(Point::new(r.x1 as f32, r.y1 as f32))));
    }
    if let (Tool::CloneStamp, Some(src)) = (tools.tool, tools.clone_source) {
        let p = s(src);
        painter.line_segment([p - Vec2::new(6.0, 0.0), p + Vec2::new(6.0, 0.0)], EStroke::new(1.5_f32, Color32::RED));
        painter.line_segment([p - Vec2::new(0.0, 6.0), p + Vec2::new(0.0, 6.0)], EStroke::new(1.5_f32, Color32::RED));
    }

    // Cursor.
    if let Some(pos) = hover_pos {
        let busy_pan = panning_tool || tab.view.space_pan;
        if busy_pan {
            ctx.set_cursor_icon(if pointer.any_down() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
        } else if tools.tool.is_paint() {
            let size = tools.current_brush().map(|b| b.size).unwrap_or(1.0);
            let r = (size * 0.5 * tab.view.zoom).max(1.0);
            painter.circle_stroke(pos, r, EStroke::new(1.0_f32, Color32::BLACK));
            painter.circle_stroke(pos, r + 1.0, EStroke::new(1.0_f32, Color32::from_white_alpha(200)));
            ctx.set_cursor_icon(if r < 4.0 { egui::CursorIcon::Crosshair } else { egui::CursorIcon::None });
        } else {
            ctx.set_cursor_icon(match tools.tool {
                Tool::Move => egui::CursorIcon::Move,
                Tool::Text => egui::CursorIcon::Text,
                Tool::Zoom => egui::CursorIcon::ZoomIn,
                _ => egui::CursorIcon::Crosshair,
            });
        }
    }
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
