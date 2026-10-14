//! Vector tool icons drawn with the egui painter (no image assets needed).

use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2};

use crate::tools::Tool;

pub fn draw(p: &Painter, r: Rect, tool: Tool, col: Color32) {
    let c = r.center();
    let s = r.width() * 0.5 * 0.62;
    let at = |x: f32, y: f32| Pos2::new(c.x + x * s, c.y + y * s);
    let st = Stroke::new(1.6_f32, col);
    let thin = Stroke::new(1.0_f32, col);
    let poly = |pts: Vec<Pos2>, closed: bool| {
        if closed {
            p.add(Shape::closed_line(pts, st));
        } else {
            p.add(Shape::line(pts, st));
        }
    };
    match tool {
        Tool::Move => {
            for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                p.line_segment([c, at(dx, dy)], st);
                let tip = at(dx, dy);
                let side = Vec2::new(-dy, dx) * s * 0.3;
                let back = Vec2::new(-dx, -dy) * s * 0.3;
                poly(vec![tip + back + side, tip, tip + back - side], false);
            }
        }
        Tool::Marquee => {
            let rr = Rect::from_center_size(c, Vec2::splat(s * 1.7));
            p.extend(Shape::dashed_line(
                &[rr.left_top(), rr.right_top(), rr.right_bottom(), rr.left_bottom(), rr.left_top()],
                st,
                3.0,
                2.5,
            ));
        }
        Tool::EllipseMarquee => {
            let pts: Vec<Pos2> =
                (0..=40).map(|i| { let t = i as f32 / 40.0 * std::f32::consts::TAU; at(t.cos(), t.sin() * 0.75) }).collect();
            p.extend(Shape::dashed_line(&pts, st, 3.0, 2.5));
        }
        Tool::Lasso => {
            let pts: Vec<Pos2> = (0..=40)
                .map(|i| {
                    let t = i as f32 / 40.0 * std::f32::consts::TAU;
                    at(t.cos() * 0.9, t.sin() * 0.55 - 0.25)
                })
                .collect();
            poly(pts, false);
            poly(vec![at(-0.3, 0.25), at(-0.45, 0.7), at(-0.2, 1.0)], false);
        }
        Tool::MagicWand => {
            p.line_segment([at(-0.9, 0.9), at(0.2, -0.2)], Stroke::new(2.2_f32, col));
            for (dx, dy) in [(0.6, -0.6), (0.2, -0.95), (0.95, -0.2), (0.85, -0.95)] {
                p.circle_filled(at(dx, dy), 1.3, col);
            }
        }
        Tool::Crop => {
            poly(vec![at(-0.5, -1.0), at(-0.5, 0.5), at(1.0, 0.5)], false);
            poly(vec![at(-1.0, -0.5), at(0.5, -0.5), at(0.5, 1.0)], false);
        }
        Tool::Eyedropper => {
            p.line_segment([at(-0.9, 0.9), at(0.3, -0.3)], Stroke::new(2.0_f32, col));
            p.circle_filled(at(0.55, -0.55), s * 0.3, col);
        }
        Tool::Brush => {
            p.line_segment([at(0.9, -0.9), at(-0.1, 0.1)], Stroke::new(2.0_f32, col));
            p.circle_filled(at(-0.4, 0.45), s * 0.32, col);
            poly(vec![at(-0.65, 0.65), at(-0.95, 0.95)], false);
        }
        Tool::Pencil => {
            poly(vec![at(0.6, -0.95), at(0.95, -0.6), at(-0.5, 0.85), at(-0.95, 0.95), at(-0.85, 0.5)], true);
            p.line_segment([at(0.35, -0.7), at(0.7, -0.35)], thin);
        }
        Tool::CloneStamp => {
            poly(vec![at(-0.9, 0.9), at(0.9, 0.9), at(0.9, 0.5), at(-0.9, 0.5)], true);
            poly(vec![at(-0.25, 0.5), at(-0.2, -0.2), at(0.2, -0.2), at(0.25, 0.5)], false);
            p.circle_stroke(at(0.0, -0.55), s * 0.35, st);
        }
        Tool::Eraser => {
            poly(vec![at(-0.95, 0.4), at(0.2, -0.75), at(0.95, 0.0), at(-0.2, 1.15 - 0.2)], true);
            p.line_segment([at(-0.55, 0.0), at(0.2, 0.75)], thin);
        }
        Tool::Gradient => {
            let rr = Rect::from_center_size(c, Vec2::splat(s * 1.8));
            let n = 6;
            for i in 0..n {
                let t = i as f32 / n as f32;
                let x0 = rr.left() + rr.width() * t;
                let a = (255.0 * (1.0 - t)) as u8;
                p.rect_filled(
                    Rect::from_min_max(Pos2::new(x0, rr.top()), Pos2::new(x0 + rr.width() / n as f32 + 0.5, rr.bottom())),
                    0.0,
                    Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), a),
                );
            }
            p.rect_stroke(rr, 0.0, thin);
        }
        Tool::Bucket => {
            poly(vec![at(-0.7, -0.2), at(0.1, -1.0), at(0.9, -0.2), at(0.1, 0.6)], true);
            p.circle_filled(at(0.8, 0.6), s * 0.22, col);
            p.line_segment([at(-0.7, -0.2), at(-0.9, 0.5)], st);
        }
        Tool::Text => {
            poly(vec![at(-0.8, -0.6), at(-0.8, -0.85), at(0.8, -0.85), at(0.8, -0.6)], false);
            p.line_segment([at(0.0, -0.85), at(0.0, 0.85)], Stroke::new(2.0_f32, col));
            p.line_segment([at(-0.3, 0.85), at(0.3, 0.85)], st);
        }
        Tool::Shape => {
            p.rect_stroke(Rect::from_min_max(at(-0.9, -0.9), at(0.3, 0.3)), 0.0, st);
            p.circle_filled(at(0.45, 0.45), s * 0.5, col);
        }
        Tool::Hand => {
            for (i, x) in [-0.55f32, -0.2, 0.15, 0.5].iter().enumerate() {
                let top = if i == 0 || i == 3 { -0.5 } else { -0.85 };
                p.line_segment([at(*x, top), at(*x, 0.2)], Stroke::new(2.0_f32, col));
            }
            poly(vec![at(-0.7, 0.1), at(-0.6, 0.8), at(0.6, 0.8), at(0.8, -0.1)], false);
        }
        Tool::Zoom => {
            p.circle_stroke(at(-0.2, -0.2), s * 0.6, st);
            p.line_segment([at(0.25, 0.25), at(0.95, 0.95)], Stroke::new(2.4_f32, col));
        }
    }
}
