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
        Tool::RowMarquee => {
            p.extend(Shape::dashed_line(&[at(-1.0, 0.0), at(1.0, 0.0)], st, 3.0, 2.5));
            p.line_segment([at(-1.0, -0.4), at(-1.0, 0.4)], thin);
            p.line_segment([at(1.0, -0.4), at(1.0, 0.4)], thin);
        }
        Tool::ColumnMarquee => {
            p.extend(Shape::dashed_line(&[at(0.0, -1.0), at(0.0, 1.0)], st, 3.0, 2.5));
            p.line_segment([at(-0.4, -1.0), at(0.4, -1.0)], thin);
            p.line_segment([at(-0.4, 1.0), at(0.4, 1.0)], thin);
        }
        Tool::PolygonLasso => {
            poly(vec![at(-0.9, -0.2), at(-0.2, -0.9), at(0.9, -0.5), at(0.6, 0.4), at(-0.4, 0.3)], true);
            poly(vec![at(-0.4, 0.3), at(-0.5, 0.95)], false);
        }
        Tool::MagneticLasso => {
            let pts: Vec<Pos2> = (0..=30).map(|i| { let t = i as f32 / 30.0 * std::f32::consts::TAU; at(t.cos() * 0.9, t.sin() * 0.5 - 0.35) }).collect();
            poly(pts, false);
            poly(vec![at(-0.3, 0.3), at(-0.3, 0.9), at(0.3, 0.9), at(0.3, 0.3)], false);
            p.line_segment([at(-0.3, 0.75), at(0.3, 0.75)], Stroke::new(2.0_f32, Color32::from_rgb(220, 70, 70)));
        }
        Tool::QuickSelect => {
            p.line_segment([at(0.9, -0.9), at(0.0, 0.0)], Stroke::new(2.0_f32, col));
            p.circle_filled(at(-0.25, 0.25), s * 0.25, col);
            let pts: Vec<Pos2> = (0..=24).map(|i| { let t = i as f32 / 24.0 * std::f32::consts::TAU; at(-0.3 + t.cos() * 0.6, 0.3 + t.sin() * 0.6) }).collect();
            p.extend(Shape::dashed_line(&pts, thin, 2.0, 2.0));
        }
        Tool::PerspectiveCrop => {
            poly(vec![at(-0.6, -0.8), at(0.6, -0.5), at(0.8, 0.8), at(-0.8, 0.6)], true);
            p.line_segment([at(-0.6, -0.8), at(-0.6, -1.0)], st);
            p.line_segment([at(0.8, 0.8), at(1.0, 0.8)], st);
        }
        Tool::Ruler => {
            poly(vec![at(-1.0, 0.4), at(0.4, -1.0), at(1.0, -0.4), at(-0.4, 1.0)], true);
            for k in 0..4 {
                let t = -0.55 + k as f32 * 0.35;
                p.line_segment([at(t, -t), at(t + 0.18, -t + 0.18)], thin);
            }
        }
        Tool::SpotHealing | Tool::Healing => {
            poly(vec![at(-0.95, 0.35), at(0.35, -0.95), at(0.95, -0.35), at(-0.35, 0.95)], true);
            p.rect_stroke(Rect::from_center_size(c, Vec2::splat(s * 0.7)), 0.0, thin);
            if tool == Tool::SpotHealing {
                for (dx, dy) in [(-0.1, -0.1), (0.15, 0.1), (-0.05, 0.2)] {
                    p.circle_filled(at(dx, dy), 1.0, col);
                }
            }
        }
        Tool::Patch => {
            p.rect_stroke(Rect::from_min_max(at(-0.9, -0.6), at(0.6, 0.8)), 2.0, st);
            p.extend(Shape::dashed_line(&[at(-0.6, -0.9), at(0.9, -0.9), at(0.9, 0.5)], thin, 2.0, 2.0));
        }
        Tool::RedEye => {
            let pts: Vec<Pos2> = (0..=24).map(|i| { let t = i as f32 / 24.0 * std::f32::consts::TAU; at(t.cos() * 0.95, t.sin() * 0.5) }).collect();
            poly(pts, false);
            p.circle_filled(c, s * 0.3, Color32::from_rgb(220, 60, 60));
            p.line_segment([at(0.55, -0.95), at(0.55, -0.55)], thin);
            p.line_segment([at(0.35, -0.75), at(0.75, -0.75)], thin);
        }
        Tool::ColorReplace => {
            p.line_segment([at(0.9, -0.9), at(-0.1, 0.1)], Stroke::new(2.0_f32, col));
            p.circle_filled(at(-0.4, 0.45), s * 0.3, col);
            p.circle_stroke(at(0.55, 0.55), s * 0.3, thin);
        }
        Tool::Mixer => {
            p.line_segment([at(0.9, -0.9), at(-0.1, 0.1)], Stroke::new(2.0_f32, col));
            p.circle_filled(at(-0.4, 0.45), s * 0.32, col);
            p.circle_filled(at(0.6, 0.6), s * 0.18, Color32::from_rgb(90, 160, 240));
        }
        Tool::PatternStamp => {
            poly(vec![at(-0.9, 0.9), at(0.9, 0.9), at(0.9, 0.5), at(-0.9, 0.5)], true);
            poly(vec![at(-0.25, 0.5), at(-0.2, -0.2), at(0.2, -0.2), at(0.25, 0.5)], false);
            p.rect_filled(Rect::from_center_size(at(-0.2, -0.75), Vec2::splat(s * 0.35)), 0.0, col);
            p.rect_filled(Rect::from_center_size(at(0.2, -0.45), Vec2::splat(s * 0.35)), 0.0, col);
        }
        Tool::BackgroundEraser | Tool::MagicEraser => {
            poly(vec![at(-0.95, 0.4), at(0.2, -0.75), at(0.95, 0.0), at(-0.2, 0.95)], true);
            if tool == Tool::MagicEraser {
                for (dx, dy) in [(0.6, -0.8), (0.9, -0.6), (0.4, -0.95)] {
                    p.circle_filled(at(dx, dy), 1.3, col);
                }
            } else {
                p.extend(Shape::dashed_line(&[at(-0.9, -0.9), at(-0.2, -0.9)], thin, 2.0, 2.0));
            }
        }
        Tool::Blur => {
            let pts = vec![at(0.0, -1.0), at(0.55, 0.15), at(0.45, 0.6), at(0.0, 0.85), at(-0.45, 0.6), at(-0.55, 0.15)];
            p.add(Shape::convex_polygon(pts, col, Stroke::NONE));
        }
        Tool::Sharpen => poly(vec![at(0.0, -1.0), at(0.7, 0.9), at(-0.7, 0.9)], true),
        Tool::Smudge => {
            p.line_segment([at(-0.8, 0.8), at(0.4, -0.4)], Stroke::new(3.5_f32, col));
            p.circle_filled(at(0.5, -0.5), s * 0.28, col);
        }
        Tool::Dodge => {
            p.line_segment([at(0.0, 0.0), at(0.8, 0.9)], Stroke::new(2.0_f32, col));
            p.circle_stroke(at(-0.3, -0.35), s * 0.5, st);
        }
        Tool::Burn => {
            poly(vec![at(-0.6, 0.9), at(-0.6, -0.2), at(-0.2, -0.7), at(0.3, -0.7), at(0.6, -0.3), at(0.2, 0.0)], false);
            p.line_segment([at(0.2, 0.0), at(0.6, 0.9)], st);
        }
        Tool::Sponge => {
            p.rect_stroke(Rect::from_min_max(at(-0.9, -0.6), at(0.9, 0.6)), 4.0, st);
            for (dx, dy) in [(-0.4, -0.2), (0.3, 0.2), (0.0, -0.3), (-0.2, 0.3), (0.5, -0.2)] {
                p.circle_filled(at(dx, dy), 1.4, col);
            }
        }
        Tool::Liquify => {
            for k in 0..3 {
                let yo = -0.6 + k as f32 * 0.6;
                let pts: Vec<Pos2> = (0..=16).map(|i| { let t = i as f32 / 16.0; at(-0.9 + 1.8 * t, yo + (t * std::f32::consts::TAU).sin() * 0.2) }).collect();
                poly(pts, false);
            }
        }
        Tool::Pen | Tool::FreeformPen => {
            poly(vec![at(0.0, -1.0), at(0.6, 0.1), at(0.25, 0.9), at(-0.25, 0.9), at(-0.6, 0.1)], true);
            p.line_segment([at(0.0, -0.9), at(0.0, 0.2)], thin);
            p.circle_filled(at(0.0, 0.25), 1.6, col);
            if tool == Tool::FreeformPen {
                let pts: Vec<Pos2> = (0..=10).map(|i| { let t = i as f32 / 10.0; at(-1.0 + t * 0.8, 0.95 - (t * 6.0).sin() * 0.1) }).collect();
                poly(pts, false);
            }
        }
        Tool::PathSelect | Tool::DirectSelect => {
            let pts = vec![at(-0.6, -1.0), at(0.6, 0.3), at(0.05, 0.3), at(0.35, 0.95), at(0.1, 1.0), at(-0.2, 0.4), at(-0.6, 0.8)];
            if tool == Tool::PathSelect {
                p.add(Shape::convex_polygon(pts.clone(), col, Stroke::NONE));
            }
            poly(pts, true);
        }
        Tool::VerticalText => {
            poly(vec![at(-0.6, -0.8), at(-0.85, -0.8), at(-0.85, 0.8), at(-0.6, 0.8)], false);
            p.line_segment([at(-0.85, 0.0), at(0.85, 0.0)], Stroke::new(2.0_f32, col));
            p.line_segment([at(0.85, -0.3), at(0.85, 0.3)], st);
        }
    }
}
