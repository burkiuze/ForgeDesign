//! Side panels: tool palette, colour, histogram, layers and history.

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, TextureOptions, Vec2};
use forge_core::buffer::{Pixel, TiledBuffer};
use forge_core::{Color, LayerId};

use crate::canvas::DocTab;
use crate::icons;
use crate::tools::{self, Tool, ToolState};

pub fn to_c32(c: Color) -> Color32 {
    let [r, g, b, a] = c.to_rgba8();
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

pub fn from_c32(c: Color32) -> Color {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    Color::from_rgba8([r, g, b, a])
}

// ---------------------------------------------------------------- tool bar

pub fn toolbar(ui: &mut egui::Ui, tools: &mut ToolState, editing_bg: &mut bool) {
    ui.spacing_mut().item_spacing = Vec2::splat(2.0);
    let size = Vec2::splat(30.0);
    for pair in Tool::ALL.chunks(2) {
        ui.horizontal(|ui| {
            for &tool in pair {
                let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
                let selected = tools.tool == tool;
                let bg = if selected {
                    crate::app::ACCENT_DIM
                } else if resp.hovered() {
                    Color32::from_gray(70)
                } else {
                    Color32::TRANSPARENT
                };
                ui.painter().rect_filled(rect, 3.0, bg);
                icons::draw(ui.painter(), rect, tool, Color32::from_gray(if selected { 255 } else { 210 }));
                let tip = match tool.shortcut() {
                    Some(k) => format!("{} ({})", tool.name(), k.name()),
                    None => format!("{} (Shift+M)", tool.name()),
                };
                if resp.on_hover_text(tip).clicked() && !tools.is_busy() {
                    tools.tool = tool;
                }
            }
        });
    }
    ui.add_space(10.0);

    // Foreground / background swatches, Photoshop style.
    let (rect, _) = ui.allocate_exact_size(Vec2::new(62.0, 56.0), Sense::hover());
    let fg_r = Rect::from_min_size(rect.min + Vec2::new(4.0, 4.0), Vec2::splat(30.0));
    let bg_r = Rect::from_min_size(rect.min + Vec2::new(22.0, 22.0), Vec2::splat(30.0));
    let p = ui.painter();
    p.rect_filled(bg_r, 0.0, to_c32(tools.bg));
    p.rect_stroke(bg_r, 0.0, Stroke::new(1.0_f32, if *editing_bg { crate::app::ACCENT } else { Color32::from_gray(160) }));
    p.rect_filled(fg_r, 0.0, to_c32(tools.fg));
    p.rect_stroke(fg_r, 0.0, Stroke::new(1.0_f32, if !*editing_bg { crate::app::ACCENT } else { Color32::from_gray(160) }));
    let bg_resp = ui.interact(bg_r, ui.id().with("bg"), Sense::click()).on_hover_text("Background colour");
    let fg_resp = ui.interact(fg_r, ui.id().with("fg"), Sense::click()).on_hover_text("Foreground colour");
    if fg_resp.clicked() {
        *editing_bg = false;
    } else if bg_resp.clicked() {
        *editing_bg = true;
    }
    ui.horizontal(|ui| {
        if ui.small_button("X").on_hover_text("Swap colours (X)").clicked() {
            tools.swap_colors();
        }
        if ui.small_button("D").on_hover_text("Default colours (D)").clicked() {
            tools.reset_colors();
        }
    });
}

// ------------------------------------------------------------------ colour

const SWATCHES: [u32; 24] = [
    0x000000, 0x404040, 0x808080, 0xc0c0c0, 0xffffff, 0x7f0000, 0xff0000, 0xff7f00, 0xffff00, 0x7fff00, 0x00ff00, 0x00ff7f,
    0x00ffff, 0x007fff, 0x0000ff, 0x7f00ff, 0xff00ff, 0xff007f, 0x8b4513, 0xf5deb3, 0x2f4f4f, 0x556b2f, 0x4682b4, 0xffc0cb,
];

pub fn color_panel(ui: &mut egui::Ui, tools: &mut ToolState, editing_bg: bool, hex_buf: &mut String) {
    let target = if editing_bg { &mut tools.bg } else { &mut tools.fg };
    let mut c = to_c32(*target);
    ui.label(if editing_bg { "Background" } else { "Foreground" });
    if egui::color_picker::color_picker_color32(ui, &mut c, egui::color_picker::Alpha::Opaque) {
        *target = from_c32(c);
        *hex_buf = target.to_hex();
    }
    ui.horizontal(|ui| {
        ui.label("#");
        let r = ui.add(egui::TextEdit::singleline(hex_buf).desired_width(80.0));
        if r.lost_focus() {
            if let Some(nc) = Color::from_hex(hex_buf) {
                *target = nc;
            }
        }
        if !r.has_focus() {
            *hex_buf = target.to_hex();
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(2.0);
        for hex in SWATCHES {
            let col = Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
            let (rect, resp) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::click());
            ui.painter().rect_filled(rect, 2.0, col);
            if resp.hovered() {
                ui.painter().rect_stroke(rect, 2.0, Stroke::new(1.0_f32, Color32::WHITE));
            }
            if resp.clicked() {
                *target = from_c32(col);
            }
        }
    });
}

// --------------------------------------------------------------- histogram

pub fn histogram_panel(ui: &mut egui::Ui, tab: &mut DocTab) {
    let key = (tab.doc.history().current().serial, tab.doc.state.active);
    if tab.view.hist.as_ref().is_none_or(|(k, _)| *k != key) {
        tab.view.hist = Some((key, Box::new(tools::layer_histogram(tab))));
    }
    let hist = &tab.view.hist.as_ref().unwrap().1;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 80.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 2.0, Color32::from_gray(25));
    let colors = [
        Color32::from_rgba_unmultiplied(255, 60, 60, 110),
        Color32::from_rgba_unmultiplied(60, 255, 60, 110),
        Color32::from_rgba_unmultiplied(80, 120, 255, 110),
        Color32::from_rgba_unmultiplied(220, 220, 220, 160),
    ];
    let max = hist.iter().flat_map(|c| c.iter()).copied().max().unwrap_or(1).max(1) as f32;
    for (ch, col) in colors.iter().enumerate() {
        for i in 0..256 {
            let v = (hist[ch][i] as f32 / max).sqrt();
            let x = rect.left() + rect.width() * i as f32 / 256.0;
            let wbar = rect.width() / 256.0 + 0.5;
            let top = rect.bottom() - v * rect.height();
            p.rect_filled(Rect::from_min_max(Pos2::new(x, top), Pos2::new(x + wbar, rect.bottom())), 0.0, *col);
        }
    }
}

// ------------------------------------------------------------------ layers

#[derive(Default)]
pub struct LayersUi {
    pub renaming: Option<(LayerId, String)>,
}

fn fingerprint<P: Pixel>(b: &TiledBuffer<P>) -> usize {
    let mut h = (b.width() as usize) << 32 ^ b.height() as usize;
    for ty in 0..b.tiles_y() {
        for tx in 0..b.tiles_x() {
            let p = b.tile_arc(tx, ty).map_or(0, |a| std::sync::Arc::as_ptr(a) as usize);
            h = h.rotate_left(5) ^ p ^ (tx as usize * 31 + ty as usize);
        }
    }
    h
}

const THUMB: usize = 40;

fn thumb_image<P: Pixel>(b: &TiledBuffer<P>, to_rgb: impl Fn(P) -> [u8; 4]) -> egui::ColorImage {
    let (w, h) = (b.width() as f32, b.height() as f32);
    let scale = THUMB as f32 / w.max(h);
    let (tw, th) = (((w * scale) as usize).max(1), ((h * scale) as usize).max(1));
    let mut pixels = Vec::with_capacity(tw * th);
    for y in 0..th {
        for x in 0..tw {
            let p = to_rgb(b.get((x as f32 / scale) as i32, (y as f32 / scale) as i32));
            let chk = if ((x / 4) + (y / 4)) % 2 == 0 { 255.0 } else { 205.0 };
            let a = p[3] as f32 / 255.0;
            let mix = |c: u8| (c as f32 * a + chk * (1.0 - a)) as u8;
            pixels.push(Color32::from_rgb(mix(p[0]), mix(p[1]), mix(p[2])));
        }
    }
    egui::ColorImage { size: [tw, th], pixels }
}

fn layer_thumb(ctx: &egui::Context, tab: &mut DocTab, idx: usize, mask: bool) -> Option<egui::TextureId> {
    let layer = &tab.doc.state.layers[idx];
    let key = (layer.id, mask);
    let fp = if mask { fingerprint(layer.mask.as_ref()?) } else { fingerprint(&layer.pixels) };
    if let Some((f, tex)) = tab.view.thumbs.get(&key) {
        if *f == fp {
            return Some(tex.id());
        }
    }
    let img = if mask {
        thumb_image(layer.mask.as_ref()?, |v: u8| [v, v, v, 255])
    } else {
        thumb_image(&layer.pixels, |p| p)
    };
    let tex = ctx.load_texture(format!("thumb{}_{mask}", layer.id), img, TextureOptions::LINEAR);
    let id = tex.id();
    tab.view.thumbs.insert(key, (fp, tex));
    Some(id)
}

/// Actions the layers panel asks the app to perform.
pub enum LayerAction {
    New,
    Duplicate,
    Delete,
    AddMask,
    MergeDown,
    Up,
    Down,
    SelectionFromAlpha(usize),
}

pub fn layers_panel(ui: &mut egui::Ui, tab: &mut DocTab, state: &mut LayersUi) -> Option<LayerAction> {
    let ctx = ui.ctx().clone();
    let mut action = None;
    let n = tab.doc.state.layers.len();
    if n == 0 {
        return None;
    }

    // Active layer properties.
    let active = tab.doc.state.active;
    ui.horizontal(|ui| {
        let mut mode = tab.doc.state.layers[active].blend;
        if tools::blend_combo(ui, "layer_blend", &mut mode) {
            tab.doc.state.layers[active].blend = mode;
            tab.doc.mark_all_dirty();
            tab.doc.commit("Blending Change");
        }
        ui.label("Opacity");
        let mut op = tab.doc.state.layers[active].opacity;
        let r = tools::percent(ui, &mut op);
        if r.changed() {
            tab.doc.state.layers[active].opacity = op;
            tab.doc.mark_all_dirty();
        }
        if r.drag_stopped() || (r.changed() && !r.dragged()) {
            tab.doc.commit("Opacity Change");
        }
    });
    ui.horizontal(|ui| {
        ui.label("Lock:");
        let l = &mut tab.doc.state.layers[active];
        let mut la = l.lock_alpha;
        let mut lk = l.locked;
        let c1 = ui.toggle_value(&mut la, "▦ Transparency").on_hover_text("Lock transparent pixels (/)").changed();
        let c2 = ui.toggle_value(&mut lk, "🔒 All").on_hover_text("Lock all").changed();
        if c1 || c2 {
            l.lock_alpha = la;
            l.locked = lk;
            tab.doc.commit("Lock Change");
        }
    });
    ui.separator();

    let row_h = 46.0;
    egui::ScrollArea::vertical().max_height(ui.available_height() - 34.0).auto_shrink([false, true]).show(ui, |ui| {
        for i in (0..n).rev() {
            let selected = i == tab.doc.state.active;
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::click_and_drag());
            let bg = if selected { crate::app::ACCENT_DIM } else if resp.hovered() { Color32::from_gray(58) } else { Color32::from_gray(45) };
            ui.painter().rect_filled(rect.shrink2(Vec2::new(0.0, 1.0)), 2.0, bg);

            // Drag & drop reordering.
            resp.dnd_set_drag_payload(i);
            if let Some(from) = resp.dnd_release_payload::<usize>() {
                tab.doc.reorder_layer(*from, i);
            }
            if resp.dnd_hover_payload::<usize>().is_some() {
                ui.painter().line_segment([rect.left_top(), rect.right_top()], Stroke::new(2.0_f32, crate::app::ACCENT));
            }

            // Visibility eye.
            let eye = Rect::from_min_size(rect.min + Vec2::new(4.0, (row_h - 20.0) / 2.0), Vec2::splat(20.0));
            let eye_resp = ui.interact(eye, ui.id().with(("eye", i)), Sense::click());
            let visible = tab.doc.state.layers[i].visible;
            ui.painter().text(
                eye.center(),
                egui::Align2::CENTER_CENTER,
                if visible { "👁" } else { "–" },
                egui::FontId::proportional(14.0),
                Color32::from_gray(if visible { 230 } else { 120 }),
            );
            if eye_resp.clicked() {
                tab.doc.state.layers[i].visible = !visible;
                tab.doc.mark_all_dirty();
                tab.doc.commit(if visible { "Hide Layer" } else { "Show Layer" });
            }

            // Thumbnails.
            let mut x = eye.max.x + 6.0;
            let editing_mask = tab.doc.state.editing_mask && selected;
            if let Some(tex) = layer_thumb(&ctx, tab, i, false) {
                let tr = Rect::from_min_size(Pos2::new(x, rect.min.y + 3.0), Vec2::splat(THUMB as f32));
                ui.painter().image(tex, tr, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                if selected && !editing_mask {
                    ui.painter().rect_stroke(tr.expand(1.0), 0.0, Stroke::new(1.0_f32, Color32::WHITE));
                }
                let tresp = ui.interact(tr, ui.id().with(("thumb", i)), Sense::click());
                if tresp.clicked() {
                    if ctx.input(|inp| inp.modifiers.command) {
                        action = Some(LayerAction::SelectionFromAlpha(i));
                    } else {
                        tab.doc.select_layer(i);
                        tab.doc.state.editing_mask = false;
                    }
                }
                x = tr.max.x + 4.0;
            }
            if tab.doc.state.layers[i].mask.is_some() {
                if let Some(tex) = layer_thumb(&ctx, tab, i, true) {
                    let tr = Rect::from_min_size(Pos2::new(x, rect.min.y + 3.0), Vec2::splat(THUMB as f32));
                    ui.painter().image(tex, tr, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                    let enabled = tab.doc.state.layers[i].mask_enabled;
                    if !enabled {
                        ui.painter().line_segment([tr.left_top(), tr.right_bottom()], Stroke::new(2.0_f32, Color32::RED));
                    }
                    if editing_mask {
                        ui.painter().rect_stroke(tr.expand(1.0), 0.0, Stroke::new(1.0_f32, Color32::WHITE));
                    }
                    let mresp = ui.interact(tr, ui.id().with(("mask", i)), Sense::click()).on_hover_text("Click: edit mask · Shift+click: disable/enable");
                    if mresp.clicked() {
                        if ctx.input(|inp| inp.modifiers.shift) {
                            tab.doc.state.layers[i].mask_enabled = !enabled;
                            tab.doc.mark_all_dirty();
                            tab.doc.commit(if enabled { "Disable Layer Mask" } else { "Enable Layer Mask" });
                        } else {
                            tab.doc.select_layer(i);
                            tab.doc.state.editing_mask = true;
                        }
                    }
                    x = tr.max.x + 4.0;
                }
            }

            // Name (double-click to rename).
            let name_rect = Rect::from_min_max(Pos2::new(x + 4.0, rect.min.y), rect.max - Vec2::new(4.0, 0.0));
            let layer_id = tab.doc.state.layers[i].id;
            let renaming_this = state.renaming.as_ref().is_some_and(|(id, _)| *id == layer_id);
            if renaming_this {
                let (_, buf) = state.renaming.as_mut().unwrap();
                let te = ui.put(name_rect.shrink2(Vec2::new(0.0, 12.0)), egui::TextEdit::singleline(buf));
                te.request_focus();
                if te.lost_focus() {
                    let (_, name) = state.renaming.take().unwrap();
                    if !name.trim().is_empty() && name != tab.doc.state.layers[i].name {
                        tab.doc.state.layers[i].name = name;
                        tab.doc.commit("Rename Layer");
                    }
                }
            } else {
                let l = &tab.doc.state.layers[i];
                let mut label = l.name.clone();
                if l.locked {
                    label.push_str("  🔒");
                } else if l.lock_alpha {
                    label.push_str("  ▦");
                }
                ui.painter().text(
                    Pos2::new(name_rect.min.x, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    label,
                    egui::FontId::proportional(13.0),
                    Color32::from_gray(if l.visible { 235 } else { 140 }),
                );
                if l.blend != forge_core::BlendMode::Normal || l.opacity < 1.0 {
                    ui.painter().text(
                        Pos2::new(name_rect.min.x, rect.max.y - 9.0),
                        egui::Align2::LEFT_CENTER,
                        format!("{} · {:.0}%", l.blend.name(), l.opacity * 100.0),
                        egui::FontId::proportional(10.0),
                        Color32::from_gray(150),
                    );
                }
            }
            if resp.double_clicked() {
                state.renaming = Some((layer_id, tab.doc.state.layers[i].name.clone()));
            } else if resp.clicked() {
                tab.doc.select_layer(i);
            }
        }
    });

    ui.separator();
    ui.horizontal(|ui| {
        let mut b = |ui: &mut egui::Ui, label: &str, tip: &str, a: LayerAction| {
            if ui.button(label).on_hover_text(tip).clicked() {
                action = Some(a);
            }
        };
        b(ui, "+", "New layer (Ctrl+Shift+N)", LayerAction::New);
        b(ui, "Dup", "Duplicate layer (Ctrl+J)", LayerAction::Duplicate);
        b(ui, "Mask", "Add layer mask", LayerAction::AddMask);
        b(ui, "⬆", "Move up (Ctrl+])", LayerAction::Up);
        b(ui, "⬇", "Move down (Ctrl+[)", LayerAction::Down);
        b(ui, "Merge", "Merge down (Ctrl+E)", LayerAction::MergeDown);
        b(ui, "🗑", "Delete layer", LayerAction::Delete);
    });
    action
}

// ----------------------------------------------------------------- history

pub fn history_panel(ui: &mut egui::Ui, tab: &mut DocTab) {
    let cursor = tab.doc.history().cursor();
    let labels: Vec<String> = tab.doc.history().entries().iter().map(|e| e.label.clone()).collect();
    let mut jump = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        for (i, label) in labels.iter().enumerate() {
            let text = if i > cursor { egui::RichText::new(label).color(Color32::from_gray(120)) } else { egui::RichText::new(label) };
            if ui.selectable_label(i == cursor, text).clicked() {
                jump = Some(i);
            }
        }
    });
    if let Some(i) = jump {
        tab.doc.jump_history(i);
    }
}
