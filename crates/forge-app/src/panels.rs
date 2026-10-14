//! Side panels: tool palette, colour, histogram, layers and history.

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, TextureOptions, Vec2};
use forge_core::buffer::{Pixel, TiledBuffer};
use forge_core::{Color, LayerId};

use crate::commands::Cmd;

use crate::canvas::DocTab;
use crate::icons;
use crate::tools::{self, Tool, ToolState, GROUPS};

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
    let groups: Vec<usize> = (0..GROUPS.len()).collect();
    for pair in groups.chunks(2) {
        ui.horizontal(|ui| {
            for &gi in pair {
                let group = GROUPS[gi];
                let tool = tools.group_choice[gi];
                let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
                let selected = group.contains(&tools.tool);
                let bg = if selected {
                    crate::app::ACCENT_DIM
                } else if resp.hovered() {
                    Color32::from_gray(70)
                } else {
                    Color32::TRANSPARENT
                };
                ui.painter().rect_filled(rect, 3.0, bg);
                let shown = if selected { tools.tool } else { tool };
                icons::draw(ui.painter(), rect, shown, Color32::from_gray(if selected { 255 } else { 210 }));
                if group.len() > 1 {
                    let c = rect.right_bottom() - Vec2::new(3.0, 3.0);
                    ui.painter().add(egui::Shape::convex_polygon(
                        vec![c, c - Vec2::new(5.0, 0.0), c - Vec2::new(0.0, 5.0)],
                        Color32::from_gray(170),
                        Stroke::NONE,
                    ));
                }
                let key = Tool::group_key(gi).map(|k| format!(" ({})", k.name())).unwrap_or_default();
                let tip = if group.len() > 1 {
                    format!("{}{key}\nRight-click for more tools", shown.name())
                } else {
                    format!("{}{key}", shown.name())
                };
                let popup_id = ui.id().with(("tool_group", gi));
                let resp = resp.on_hover_text(tip);
                if resp.clicked() {
                    tools.select_tool(tool);
                }
                if group.len() > 1 && (resp.secondary_clicked() || resp.long_touched()) {
                    ui.memory_mut(|m| m.toggle_popup(popup_id));
                }
                egui::popup::popup_below_widget(ui, popup_id, &resp, egui::PopupCloseBehavior::CloseOnClick, |ui| {
                    ui.set_min_width(190.0);
                    for &t in group {
                        let (r, row) = ui.allocate_exact_size(Vec2::new(190.0, 26.0), Sense::click());
                        if row.hovered() {
                            ui.painter().rect_filled(r, 2.0, Color32::from_gray(70));
                        }
                        let ir = Rect::from_min_size(r.min, Vec2::splat(26.0));
                        icons::draw(ui.painter(), ir, t, Color32::from_gray(220));
                        ui.painter().text(
                            Pos2::new(ir.max.x + 6.0, r.center().y),
                            egui::Align2::LEFT_CENTER,
                            t.name(),
                            egui::FontId::proportional(13.0),
                            if t == tools.tool { crate::app::ACCENT } else { Color32::from_gray(225) },
                        );
                        if row.clicked() {
                            tools.select_tool(t);
                        }
                    }
                });
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
    pub search: String,
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
    let fp = if mask { layer.mask.as_ref()?.fingerprint() } else { layer.pixels.fingerprint() };
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
    Cmd(Cmd),
    SelectionFromAlpha(usize),
    EditText(usize),
}

/// Rows in display order (top first), honouring collapsed groups and search.
fn visible_rows(tab: &DocTab, search: &str) -> Vec<(usize, usize)> {
    let st = &tab.doc.state;
    let q = search.to_lowercase();
    let mut rows = Vec::new();
    fn walk(st: &forge_core::DocState, parent: Option<LayerId>, depth: usize, q: &str, rows: &mut Vec<(usize, usize)>) {
        for i in (0..st.layers.len()).rev() {
            let l = &st.layers[i];
            let is_child = match parent {
                Some(p) => l.parent == Some(p),
                None => l.parent.is_none_or(|p| !st.layers.iter().any(|o| o.id == p)),
            };
            if !is_child {
                continue;
            }
            if q.is_empty() || l.name.to_lowercase().contains(q) {
                rows.push((i, depth));
            }
            if l.is_group() && (!l.collapsed || !q.is_empty()) {
                walk(st, Some(l.id), depth + 1, q, rows);
            }
        }
    }
    walk(st, None, 0, &q, &mut rows);
    rows
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
        let c1 = ui.toggle_value(&mut la, "▦").on_hover_text("Lock transparent pixels (/)").changed();
        let c2 = ui.toggle_value(&mut lk, "🔒").on_hover_text("Lock all").changed();
        if c1 || c2 {
            l.lock_alpha = la;
            l.locked = lk;
            tab.doc.commit("Lock Change");
        }
        ui.label("Fill");
        let mut fill = tab.doc.state.layers[active].fill;
        let r = tools::percent(ui, &mut fill);
        if r.changed() {
            tab.doc.state.layers[active].fill = fill;
            tab.doc.mark_all_dirty();
        }
        if r.drag_stopped() || (r.changed() && !r.dragged()) {
            tab.doc.commit("Fill Opacity Change");
        }
    });
    ui.add(egui::TextEdit::singleline(&mut state.search).hint_text("🔍 Filter layers by name").desired_width(f32::INFINITY));
    ui.separator();

    let rows = visible_rows(tab, &state.search);
    let row_h = 46.0;
    egui::ScrollArea::vertical().id_salt("layers_scroll").max_height(360.0).auto_shrink([false, true]).show(ui, |ui| {
        for (i, depth) in rows {
            if i >= tab.doc.state.layers.len() {
                break;
            }
            let selected = i == tab.doc.state.active;
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::click_and_drag());
            let bg = if selected { crate::app::ACCENT_DIM } else if resp.hovered() { Color32::from_gray(58) } else { Color32::from_gray(45) };
            ui.painter().rect_filled(rect.shrink2(Vec2::new(0.0, 1.0)), 2.0, bg);
            let label_col = tab.doc.state.layers[i].label;
            if label_col != 0 {
                let c = forge_core::layer::LABEL_COLORS[label_col as usize % 8].1;
                ui.painter().rect_filled(Rect::from_min_size(rect.min, Vec2::new(4.0, row_h)), 0.0, Color32::from_rgb(c[0], c[1], c[2]));
            }

            // Drag & drop: onto a group puts the layer inside it.
            resp.dnd_set_drag_payload(i);
            if let Some(from) = resp.dnd_release_payload::<usize>() {
                let from = *from;
                if from != i {
                    let target = &tab.doc.state.layers[i];
                    let (tid, tparent, tgroup) = (target.id, target.parent, target.is_group());
                    let fid = tab.doc.state.layers[from].id;
                    tab.doc.reorder_layer(from, i);
                    if let Some(ni) = tab.doc.state.layer_index(fid) {
                        tab.doc.set_parent(ni, if tgroup { Some(tid) } else { tparent });
                    }
                }
            }
            if resp.dnd_hover_payload::<usize>().is_some() {
                ui.painter().rect_stroke(rect, 2.0, Stroke::new(2.0_f32, crate::app::ACCENT));
            }

            let mut x = rect.min.x + 6.0 + depth as f32 * 14.0;
            // Visibility eye.
            let eye = Rect::from_min_size(Pos2::new(x, rect.min.y + (row_h - 20.0) / 2.0), Vec2::splat(20.0));
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
                if ctx.input(|inp| inp.modifiers.alt) {
                    // Alt+click: solo this layer.
                    let only = tab.doc.state.layers.iter().enumerate().all(|(j, l)| j == i || !l.visible);
                    for (j, l) in tab.doc.state.layers.iter_mut().enumerate() {
                        l.visible = only || j == i;
                    }
                } else {
                    tab.doc.state.layers[i].visible = !visible;
                }
                tab.doc.mark_all_dirty();
                tab.doc.commit(if visible { "Hide Layer" } else { "Show Layer" });
            }
            x = eye.max.x + 2.0;

            let is_group = tab.doc.state.layers[i].is_group();
            let is_adj = tab.doc.state.layers[i].is_adjustment();
            if tab.doc.state.layers[i].clipped {
                ui.painter().text(Pos2::new(x + 2.0, rect.center().y), egui::Align2::LEFT_CENTER, "↳", egui::FontId::proportional(14.0), Color32::from_gray(200));
                x += 14.0;
            }
            if is_group {
                let collapsed = tab.doc.state.layers[i].collapsed;
                let arrow = Rect::from_min_size(Pos2::new(x, rect.center().y - 8.0), Vec2::splat(16.0));
                ui.painter().text(arrow.center(), egui::Align2::CENTER_CENTER, if collapsed { "▶" } else { "▼" }, egui::FontId::proportional(10.0), Color32::from_gray(200));
                if ui.interact(arrow, ui.id().with(("fold", i)), Sense::click()).clicked() {
                    tab.doc.state.layers[i].collapsed = !collapsed;
                }
                x = arrow.max.x + 2.0;
            }

            // Thumbnails.
            let editing_mask = tab.doc.state.editing_mask && selected;
            let tr = Rect::from_min_size(Pos2::new(x, rect.min.y + 3.0), Vec2::splat(THUMB as f32));
            if is_group || is_adj {
                ui.painter().rect_filled(tr, 3.0, Color32::from_gray(70));
                let glyph = if is_group { "📁" } else { "◑" };
                ui.painter().text(tr.center(), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(20.0), Color32::from_gray(220));
                let tresp = ui.interact(tr, ui.id().with(("thumb", i)), Sense::click());
                if tresp.clicked() {
                    tab.doc.select_layer(i);
                    tab.doc.state.editing_mask = false;
                }
                x = tr.max.x + 4.0;
            } else if let Some(tex) = layer_thumb(&ctx, tab, i, false) {
                ui.painter().image(tex, tr, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                let l = &tab.doc.state.layers[i];
                if l.text.is_some() {
                    ui.painter().text(tr.right_bottom() - Vec2::new(2.0, 2.0), egui::Align2::RIGHT_BOTTOM, "T", egui::FontId::proportional(14.0), Color32::from_rgb(40, 40, 40));
                } else if l.smart.is_some() {
                    ui.painter().text(tr.right_bottom() - Vec2::new(2.0, 2.0), egui::Align2::RIGHT_BOTTOM, "◰", egui::FontId::proportional(14.0), Color32::from_rgb(40, 40, 40));
                }
                if selected && !editing_mask {
                    ui.painter().rect_stroke(tr.expand(1.0), 0.0, Stroke::new(1.0_f32, Color32::WHITE));
                }
                let tresp = ui.interact(tr, ui.id().with(("thumb", i)), Sense::click());
                if tresp.double_clicked() && tab.doc.state.layers[i].text.is_some() {
                    action = Some(LayerAction::EditText(i));
                } else if tresp.clicked() {
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
                    if editing_mask || (is_adj && selected) {
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
                    Pos2::new(name_rect.min.x, rect.center().y - if l.style.is_some() { 6.0 } else { 0.0 }),
                    egui::Align2::LEFT_CENTER,
                    label,
                    egui::FontId::proportional(13.0),
                    Color32::from_gray(if l.visible { 235 } else { 140 }),
                );
                let mut sub = Vec::new();
                if l.blend != forge_core::BlendMode::Normal || l.opacity < 1.0 {
                    sub.push(format!("{} · {:.0}%", l.blend.name(), l.opacity * 100.0));
                }
                if l.style.as_ref().is_some_and(|s| s.is_active()) {
                    sub.push("fx".into());
                }
                if let Some(so) = &l.smart {
                    if !so.filters.is_empty() {
                        sub.push(format!("{} smart filter(s)", so.filters.len()));
                    }
                }
                if !sub.is_empty() {
                    ui.painter().text(
                        Pos2::new(name_rect.min.x, rect.max.y - 9.0),
                        egui::Align2::LEFT_CENTER,
                        sub.join("  "),
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
            resp.context_menu(|ui| {
                tab.doc.select_layer(i);
                let mut c = |ui: &mut egui::Ui, label: &str, cmd: Cmd| {
                    if ui.button(label).clicked() {
                        action = Some(LayerAction::Cmd(cmd));
                        ui.close_menu();
                    }
                };
                c(ui, "Blending Options / Layer Style…", Cmd::LayerStyle);
                c(ui, "Duplicate Layer", Cmd::DuplicateOrViaCopy);
                c(ui, "Delete Layer", Cmd::DeleteLayer);
                c(ui, "Group Layer", Cmd::GroupLayers);
                c(ui, "Convert to Smart Object", Cmd::ConvertSmart);
                c(ui, "Rasterize Layer", Cmd::Rasterize);
                c(ui, "Create / Release Clipping Mask", Cmd::ToggleClip);
                ui.separator();
                c(ui, "Copy Layer Style", Cmd::CopyStyle);
                c(ui, "Paste Layer Style", Cmd::PasteStyle);
                c(ui, "Clear Layer Style", Cmd::ClearStyle);
                ui.separator();
                c(ui, "Merge Down", Cmd::MergeDown);
                c(ui, "Merge Visible", Cmd::MergeVisible);
                c(ui, "Flatten Image", Cmd::Flatten);
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    for (k, (name, col)) in forge_core::layer::LABEL_COLORS.iter().enumerate() {
                        let c = if k == 0 { Color32::from_gray(90) } else { Color32::from_rgb(col[0], col[1], col[2]) };
                        if ui.add(egui::Button::new("  ").fill(c)).on_hover_text(*name).clicked() {
                            tab.doc.state.layers[i].label = k as u8;
                            tab.doc.commit("Layer Color");
                            ui.close_menu();
                        }
                    }
                });
            });
        }
    });

    ui.separator();
    ui.horizontal_wrapped(|ui| {
        let mut b = |ui: &mut egui::Ui, label: &str, tip: &str, c: Cmd| {
            if ui.button(label).on_hover_text(tip).clicked() {
                action = Some(LayerAction::Cmd(c));
            }
        };
        b(ui, "fx", "Layer style", Cmd::LayerStyle);
        b(ui, "Mask", "Add layer mask", Cmd::MaskFromSelectionOrReveal);
        b(ui, "◑", "New adjustment layer (Levels)", Cmd::NewAdjLayer(1));
        b(ui, "📁", "New group (Ctrl+G groups the layer)", Cmd::NewGroup);
        b(ui, "+", "New layer (Shift+Ctrl+N)", Cmd::NewLayer);
        b(ui, "Dup", "Duplicate layer (Ctrl+J)", Cmd::DuplicateOrViaCopy);
        b(ui, "⬆", "Move up (Ctrl+])", Cmd::LayerUp);
        b(ui, "⬇", "Move down (Ctrl+[)", Cmd::LayerDown);
        b(ui, "Merge", "Merge down (Ctrl+E)", Cmd::MergeDown);
        b(ui, "🗑", "Delete layer", Cmd::DeleteLayer);
    });
    action
}

// -------------------------------------------------------------- properties

pub fn properties_panel(ui: &mut egui::Ui, tab: &mut DocTab, fonts: &[crate::text::FontEntry]) {
    let i = tab.doc.state.active;
    let Some(l) = tab.doc.state.layers.get(i) else { return };
    // Adjustment layers: edit the adjustment live.
    if let Some(adj) = l.adjustment.clone().filter(|_| l.is_adjustment()) {
        ui.label(egui::RichText::new(adj.name()).strong());
        let mut a = adj.clone();
        let changed = crate::dialogs::adjustment_controls(ui, &mut a);
        if changed {
            tab.doc.state.layers[i].adjustment = Some(a);
            tab.doc.mark_all_dirty();
        }
        if ui.button("Commit changes").clicked() || (!changed && tab.doc.state.layers[i].adjustment != tab.doc.committed_state().layers.get(i).and_then(|l| l.adjustment.clone()) && ui.input(|inp| inp.pointer.any_released())) {
            tab.doc.commit("Edit Adjustment Layer");
        }
        return;
    }
    if let Some(data) = l.text.clone() {
        ui.label(egui::RichText::new("Type Layer").strong());
        ui.label(format!("{} · {:.0} px", if data.font_name.is_empty() { "Built-in" } else { &data.font_name }, data.size));
        let mut d = data.clone();
        let mut c = false;
        ui.horizontal(|ui| {
            c |= ui.add(egui::DragValue::new(&mut d.size).range(4.0..=1000.0).suffix(" px")).changed();
            let mut col = to_c32(d.color);
            if ui.color_edit_button_srgba(&mut col).changed() {
                d.color = from_c32(col);
                c = true;
            }
            c |= ui.toggle_value(&mut d.bold, "B").changed();
            c |= ui.toggle_value(&mut d.italic, "I").changed();
        });
        c |= ui.add(egui::Slider::new(&mut d.tracking, -200.0..=1000.0).text("Tracking")).changed();
        c |= ui.add(egui::Slider::new(&mut d.leading, 0.5..=3.0).text("Leading")).changed();
        c |= ui.add(egui::Slider::new(&mut d.warp, -100.0..=100.0).text("Warp")).changed();
        if c {
            tab.doc.state.layers[i].text = Some(d);
            crate::commands::rerender_text(tab, i, fonts);
        }
        if ui.input(|inp| inp.pointer.any_released()) && tab.doc.state.layers[i].text != tab.doc.committed_state().layers.get(i).and_then(|l| l.text.clone()) {
            tab.doc.commit("Edit Type");
        }
        ui.label(egui::RichText::new("Double-click the thumbnail or click with the Type tool to edit the text.").weak());
        return;
    }
    if let Some(so) = &l.smart {
        ui.label(egui::RichText::new("Smart Object").strong());
        let filters: Vec<(String, bool)> = so.filters.iter().map(|f| (f.filter.name().to_string(), f.enabled)).collect();
        if filters.is_empty() {
            ui.label(egui::RichText::new("No smart filters. Filters applied now stay editable.").weak());
        }
        let mut toggle = None;
        let mut remove = None;
        for (k, (name, on)) in filters.iter().enumerate() {
            ui.horizontal(|ui| {
                let mut e = *on;
                if ui.checkbox(&mut e, name).changed() {
                    toggle = Some(k);
                }
                if ui.small_button("✖").clicked() {
                    remove = Some(k);
                }
            });
        }
        if toggle.is_some() || remove.is_some() {
            let l = &mut tab.doc.state.layers[i];
            if let Some(so) = &mut l.smart {
                if let Some(k) = toggle {
                    so.filters[k].enabled = !so.filters[k].enabled;
                }
                if let Some(k) = remove {
                    so.filters.remove(k);
                }
                l.pixels = so.render();
            }
            tab.doc.mark_all_dirty();
            tab.doc.commit("Edit Smart Filters");
        }
        ui.separator();
    }
    let l = &tab.doc.state.layers[i];
    ui.label(egui::RichText::new(if l.is_group() { "Group" } else { "Pixel Layer" }).strong());
    if let Some(b) = l.pixels.content_bounds() {
        ui.label(format!("W {}  H {}  X {}  Y {}", b.width(), b.height(), b.x0, b.y0));
    }
    let mut clipped = l.clipped;
    if ui.checkbox(&mut clipped, "Clipping mask (clip to layer below)").changed() {
        tab.doc.state.layers[i].clipped = clipped;
        tab.doc.mark_all_dirty();
        tab.doc.commit(if clipped { "Create Clipping Mask" } else { "Release Clipping Mask" });
    }
    if tab.doc.state.layers[i].mask.is_some() {
        ui.separator();
        ui.label(egui::RichText::new("Layer Mask").strong());
        ui.horizontal(|ui| {
            if ui.button("Invert").clicked() {
                if let Some(m) = tab.doc.state.layers[i].mask.as_mut() {
                    let src = m.clone();
                    let mut inv = TiledBuffer::new_filled(src.width(), src.height(), 255 - src.fill_value());
                    inv.par_map_all(|x, y, _| 255 - src.get(x, y));
                    inv.optimize();
                    *m = inv;
                }
                tab.doc.mark_all_dirty();
                tab.doc.commit("Invert Mask");
            }
            if ui.button("Feather 5 px").clicked() {
                if let Some(m) = tab.doc.state.layers[i].mask.as_mut() {
                    *m = forge_core::selection::feather(m, 5.0);
                }
                tab.doc.mark_all_dirty();
                tab.doc.commit("Feather Mask");
            }
            if ui.button("Density 50%").on_hover_text("Lifts the mask's black towards white").clicked() {
                if let Some(m) = tab.doc.state.layers[i].mask.as_mut() {
                    m.par_map_all(|_, _, v| 128 + v / 2);
                }
                tab.doc.mark_all_dirty();
                tab.doc.commit("Mask Density");
            }
        });
    }
}

// ----------------------------------------------------------------- swatches

pub fn swatches_panel(ui: &mut egui::Ui, tools: &mut ToolState, swatches: &mut Vec<Color>) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(2.0);
        let mut remove = None;
        for (i, c) in swatches.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
            ui.painter().rect_filled(rect, 2.0, to_c32(*c));
            if resp.clicked() {
                tools.fg = *c;
            }
            if resp.secondary_clicked() {
                remove = Some(i);
            }
        }
        if let Some(i) = remove {
            swatches.remove(i);
        }
        if ui.button("+").on_hover_text("Add foreground colour (right-click a swatch to delete)").clicked() {
            swatches.push(tools.fg);
        }
    });
}

// ----------------------------------------------------------------- brushes

#[derive(Clone)]
pub struct BrushPreset {
    pub name: String,
    pub settings: forge_core::brush::BrushSettings,
    pub tip: Option<std::sync::Arc<forge_core::brush::BrushTip>>,
}

pub fn default_presets() -> Vec<BrushPreset> {
    use forge_core::brush::BrushSettings;
    let mut v = vec![
        BrushPreset { name: "Hard Round".into(), settings: BrushSettings { hardness: 1.0, ..Default::default() }, tip: None },
        BrushPreset { name: "Soft Round".into(), settings: BrushSettings { size: 60.0, hardness: 0.0, ..Default::default() }, tip: None },
        BrushPreset { name: "Airbrush".into(), settings: BrushSettings { size: 80.0, hardness: 0.0, flow: 0.08, ..Default::default() }, tip: None },
        BrushPreset { name: "Calligraphy".into(), settings: BrushSettings { size: 30.0, hardness: 0.9, angle: 45.0, roundness: 0.2, ..Default::default() }, tip: None },
        BrushPreset { name: "Spatter".into(), settings: BrushSettings { size: 12.0, hardness: 0.7, spacing: 0.6, scatter: 1.5, ..Default::default() }, tip: None },
    ];
    // Procedural chalk / texture tip.
    let n = 48u32;
    let data: Vec<u8> = (0..n * n)
        .map(|i| {
            let (x, y) = ((i % n) as f32 - n as f32 / 2.0, (i / n) as f32 - n as f32 / 2.0);
            let d = (x * x + y * y).sqrt() / (n as f32 / 2.0);
            let noise = forge_core::blend::pixel_noise((i % n) as i32, (i / n) as i32);
            if d < 1.0 && noise > 0.35 + d * 0.4 {
                255
            } else {
                0
            }
        })
        .collect();
    v.push(BrushPreset {
        name: "Chalk (texture)".into(),
        settings: BrushSettings { size: 40.0, spacing: 0.3, ..Default::default() },
        tip: Some(std::sync::Arc::new(forge_core::brush::BrushTip { width: n, height: n, data })),
    });
    v
}

pub fn brushes_panel(ui: &mut egui::Ui, tools: &mut ToolState, presets: &mut Vec<BrushPreset>) {
    let mut remove = None;
    for (i, p) in presets.iter().enumerate() {
        ui.horizontal(|ui| {
            if ui.selectable_label(false, format!("{}  ({:.0} px)", p.name, p.settings.size)).clicked() {
                if !tools.tool.is_paint() {
                    tools.select_tool(Tool::Brush);
                }
                if let Some(b) = tools.current_brush() {
                    *b = p.settings;
                }
                tools.tip = p.tip.clone();
            }
            if i >= 6 && ui.small_button("✖").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        presets.remove(i);
    }
    if ui.button("Save current brush as preset").clicked() {
        if let Some(b) = tools.current_brush().copied() {
            presets.push(BrushPreset { name: format!("Preset {}", presets.len() + 1), settings: b, tip: tools.tip.clone() });
        }
    }
    ui.label(egui::RichText::new("Edit > Define Brush Preset makes a tip from a selection.").weak());
}

// ----------------------------------------------------------------- actions

pub fn actions_panel(ui: &mut egui::Ui, rec: &mut crate::actions::Recorder) -> Option<usize> {
    let mut play = None;
    let mut remove = None;
    for (i, a) in rec.actions.iter().enumerate() {
        let open = egui::CollapsingHeader::new(format!("{}  ({} steps)", a.name, a.steps.len())).id_salt(("action", i)).show(ui, |ui| {
            for s in &a.steps {
                ui.label(format!("• {}", s.label()));
            }
        });
        if open.header_response.clicked() {
            rec.selected = i;
        }
        open.header_response.context_menu(|ui| {
            if ui.button("Delete action").clicked() {
                remove = Some(i);
                ui.close_menu();
            }
        });
    }
    if let Some(i) = remove {
        rec.actions.remove(i);
        rec.selected = rec.selected.min(rec.actions.len().saturating_sub(1));
        rec.save();
    }
    ui.horizontal(|ui| {
        if rec.recording {
            if ui.button("■ Stop").clicked() {
                rec.stop();
            }
        } else {
            if ui.button("● Record new").clicked() {
                rec.start_new();
            }
            let sel = rec.actions.get(rec.selected).map(|a| a.name.clone()).unwrap_or_default();
            if ui.add_enabled(!rec.actions.is_empty(), egui::Button::new(format!("▶ Play \"{sel}\""))).clicked() {
                play = Some(rec.selected);
            }
        }
    });
    play
}

// --------------------------------------------------------------- navigator

pub fn navigator_panel(ui: &mut egui::Ui, tab: &mut DocTab) {
    let ctx = ui.ctx().clone();
    let (w, h) = (tab.doc.width(), tab.doc.height());
    let key = (u64::MAX - tab.id, false);
    let serial = tab.doc.history().current().serial as usize ^ (w as usize) << 20;
    let tex = match tab.view.thumbs.get(&key) {
        Some((s, t)) if *s == serial => t.id(),
        _ => {
            let maxw = 260.0f32;
            let scale = (maxw / w as f32).min(160.0 / h as f32);
            let (tw, th) = (((w as f32 * scale) as u32).max(1), ((h as f32 * scale) as u32).max(1));
            let merged = forge_core::composite::merged(&tab.doc.state);
            let small = forge_core::transform::resample_rgba(&merged, tw, th, forge_core::transform::Resample::Bilinear);
            let pixels = small
                .to_dense()
                .iter()
                .enumerate()
                .map(|(k, p)| {
                    let (x, y) = (k as u32 % tw, k as u32 / tw);
                    let chk = if ((x / 4) + (y / 4)) % 2 == 0 { 255.0 } else { 205.0 };
                    let a = p[3] as f32 / 255.0;
                    let mix = |c: u8| (c as f32 * a + chk * (1.0 - a)) as u8;
                    Color32::from_rgb(mix(p[0]), mix(p[1]), mix(p[2]))
                })
                .collect();
            let img = egui::ColorImage { size: [tw as usize, th as usize], pixels };
            let t = ctx.load_texture("navigator", img, TextureOptions::LINEAR);
            let id = t.id();
            tab.view.thumbs.insert(key, (serial, t));
            id
        }
    };
    let scale = (260.0 / w as f32).min(160.0 / h as f32);
    let size = Vec2::new(w as f32 * scale, h as f32 * scale);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    ui.painter().image(tex, rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
    // Visible viewport rectangle.
    let cr = tab.view.canvas_rect;
    if cr.is_positive() {
        let a = tab.view.to_image(cr, w, h, cr.min);
        let b = tab.view.to_image(cr, w, h, cr.max);
        let vr = Rect::from_min_max(rect.min + Vec2::new(a.x, a.y) * scale, rect.min + Vec2::new(b.x, b.y) * scale).intersect(rect);
        ui.painter().rect_stroke(vr, 0.0, Stroke::new(1.5_f32, Color32::RED));
    }
    if let Some(p) = resp.interact_pointer_pos() {
        if resp.dragged() || resp.clicked() {
            let ip = (p - rect.min) / scale;
            // Centre the view on the clicked point.
            let z = tab.view.zoom;
            tab.view.pan = Vec2::new((w as f32 * 0.5 - ip.x) * z, (h as f32 * 0.5 - ip.y) * z);
        }
    }
    ui.horizontal(|ui| {
        let mut z = tab.view.zoom * 100.0;
        if ui.add(egui::Slider::new(&mut z, 1.0..=3200.0).logarithmic(true).suffix("%")).changed() {
            let c = tab.view.canvas_rect.center();
            tab.view.zoom_to(z / 100.0, c, w, h);
        }
    });
}

// -------------------------------------------------------------------- info

pub fn info_panel(ui: &mut egui::Ui, tab: &mut DocTab, tools: &ToolState) {
    egui::Grid::new("info").num_columns(2).show(ui, |ui| {
        if let Some(p) = tab.view.hover {
            let (x, y) = (p.x.floor() as i32, p.y.floor() as i32);
            ui.label("X / Y");
            ui.label(format!("{x} / {y}"));
            ui.end_row();
            if tab.doc.state.bounds().contains(x, y) {
                let px = forge_core::composite::composite_rect(&tab.doc.state, forge_core::IRect::new(x, y, x + 1, y + 1), forge_core::composite::Background::Transparent)[0];
                ui.label("RGBA");
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                    ui.painter().rect_filled(r, 2.0, Color32::from_rgba_unmultiplied(px[0], px[1], px[2], px[3]));
                    ui.label(format!("{} {} {} {}", px[0], px[1], px[2], px[3]));
                });
                ui.end_row();
                let c = Color::from_rgba8(px);
                let [hh, ss, vv] = forge_core::color::rgb_to_hsv(c.rgb_array());
                ui.label("HSB");
                ui.label(format!("{:.0}° {:.0}% {:.0}%", hh, ss * 100.0, vv * 100.0));
                ui.end_row();
                let k = 1.0 - c.r.max(c.g).max(c.b);
                let cmyk = if k >= 1.0 { [0.0; 3] } else { [(1.0 - c.r - k) / (1.0 - k), (1.0 - c.g - k) / (1.0 - k), (1.0 - c.b - k) / (1.0 - k)] };
                ui.label("CMYK");
                ui.label(format!("{:.0} {:.0} {:.0} {:.0}", cmyk[0] * 100.0, cmyk[1] * 100.0, cmyk[2] * 100.0, k * 100.0));
                ui.end_row();
                ui.label("Hex");
                ui.label(Color::from_rgba8([px[0], px[1], px[2], 255]).to_hex());
                ui.end_row();
            }
        }
        if let Some(b) = tab.doc.selection().and_then(|s| s.content_bounds()) {
            ui.label("Selection");
            ui.label(format!("{} × {} @ {}, {}", b.width(), b.height(), b.x0, b.y0));
            ui.end_row();
        }
        ui.label("Document");
        ui.label(format!("{} × {} px, {} layers", tab.doc.width(), tab.doc.height(), tab.doc.state.layers.len()));
        ui.end_row();
        ui.label("Tool");
        ui.label(tools.tool.name());
        ui.end_row();
    });
}

// ------------------------------------------------------------------- paths

pub fn paths_panel(ui: &mut egui::Ui, tab: &mut DocTab, tools: &ToolState) {
    let (w, h) = (tab.doc.width(), tab.doc.height());
    let mut remove = None;
    for (i, p) in tab.doc.state.paths.iter().enumerate() {
        ui.horizontal(|ui| {
            let sel = tab.doc.state.active_path == Some(i);
            if ui.selectable_label(sel, format!("〰 {}  ({} sub-paths)", p.name, p.subpaths.len())).clicked() {
                tab.doc.state.active_path = Some(i);
            }
            if ui.small_button("✖").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        tab.doc.state.paths.remove(i);
        tab.doc.state.active_path = None;
        tab.doc.commit("Delete Path");
    }
    if tab.doc.state.paths.is_empty() {
        ui.label(egui::RichText::new("Draw with the Pen tool (P) to create paths.").weak());
    }
    let Some(pi) = tab.doc.state.active_path.filter(|&i| i < tab.doc.state.paths.len()) else { return };
    let path = tab.doc.state.paths[pi].clone();
    ui.horizontal_wrapped(|ui| {
        if ui.button("Make Selection").clicked() {
            let m = path.to_mask(w, h);
            tab.doc.set_selection(m.content_bounds().map(|_| m));
            tab.doc.commit("Make Selection");
        }
        let fill_layer = |tab: &mut DocTab, mask: forge_core::selection::Mask, color: Color, label: &str| {
            let Some(l) = tab.doc.state.active_layer_mut().filter(|l| l.has_pixels() && !l.locked) else { return };
            let lock = l.lock_alpha;
            if let Some(r) = forge_core::fill::fill_mask(&mut l.pixels, Some(&mask), None, color, 1.0, forge_core::BlendMode::Normal, lock) {
                tab.doc.mark_dirty(r);
                tab.doc.commit(label);
            }
        };
        if ui.button("Fill Path (FG)").clicked() {
            fill_layer(tab, path.to_mask(w, h), tools.fg, "Fill Path");
        }
        if ui.button(format!("Stroke Path ({:.0} px)", tools.path_stroke_width)).clicked() {
            fill_layer(tab, path.stroke_mask(w, h, tools.path_stroke_width, None), tools.fg, "Stroke Path");
        }
        if ui.button("Shape Layer").on_hover_text("New layer filled with the path").clicked() {
            let mut l = tab.doc.state.new_layer(format!("{} Shape", path.name));
            forge_core::fill::fill_mask(&mut l.pixels, Some(&path.to_mask(w, h)), None, tools.fg, 1.0, forge_core::BlendMode::Normal, false);
            tab.doc.add_layer(l, "Shape from Path");
        }
        if ui.button("Vector Mask").on_hover_text("Mask the active layer with this path").clicked() {
            let m = path.to_mask(w, h);
            if let Some(l) = tab.doc.state.active_layer_mut() {
                let mut mask = forge_core::buffer::TiledBuffer::new_filled(w, h, 0);
                mask.par_map_all(|x, y, _| m.get(x, y));
                l.mask = Some(mask);
                l.mask_enabled = true;
                tab.doc.mark_all_dirty();
                tab.doc.commit("Vector Mask");
            }
        }
    });
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
