//! Small icons drawn directly with egui. No icon font or SVG runtime.
use eframe::egui::{self, Color32, Rect, Response, Sense, Stroke, Vec2};

#[derive(Clone, Copy, Debug)]
pub enum Icon {
    Home,
    Clock,
    Sparkles,
    ArrowLeft,
    ArrowRight,
    Library,
    Search,
    Eye,
    EyeOff,
    Refresh,
    Plus,
    Headphones,
    Play,
    Pause,
    Previous,
    Next,
    Volume(u8),
    Settings,
    Interface,
    Storage,
    Info,
    Shuffle,
    Loop,
    LoopOne,
    Close,
    Account,
    Heart,
    HeartFilled,
    QueueAdd,
    Queue,
    Lyrics,
    Expand,
    Collapse,
    PlaylistAdd,
    Radio,
    Album,
    CopyLink,
    Edit,
    Trash,
    RemoveFromLibrary,
    Pin,
}

impl Icon {
    pub fn volume(volume: f32) -> Self {
        let waves = if !volume.is_finite() || volume <= 0.0 {
            0
        } else if volume <= 0.5 {
            1
        } else {
            2
        };
        Self::Volume(waves)
    }
}

/// Symmetric cubic lobes, a smooth cleft and a broad rounded bottom tip.
fn heart_path(rect: Rect) -> Vec<egui::Pos2> {
    let point = |x, y| rect.min + egui::vec2(x, y) * rect.size() / 24.0;
    let mut start = point(12.0, 6.8);
    let mut contour = Vec::with_capacity(84);
    for (first, second, end) in [
        ((13.1, 6.8), (13.0, 3.5), (16.5, 3.5)),
        ((19.3, 3.5), (21.0, 5.4), (21.0, 8.1)),
        ((21.0, 11.9), (16.1, 16.4), (12.9, 19.4)),
        ((12.3, 20.0), (11.7, 20.0), (11.1, 19.4)),
        ((7.9, 16.4), (3.0, 11.9), (3.0, 8.1)),
        ((3.0, 5.4), (4.7, 3.5), (7.5, 3.5)),
        ((11.0, 3.5), (10.9, 6.8), (12.0, 6.8)),
    ] {
        let first = point(first.0, first.1);
        let second = point(second.0, second.1);
        let end = point(end.0, end.1);
        for sample in 0..12 {
            let t = sample as f32 / 12.0;
            let u = 1.0 - t;
            contour.push(
                (start.to_vec2() * u.powi(3)
                    + first.to_vec2() * (3.0 * u * u * t)
                    + second.to_vec2() * (3.0 * u * t * t)
                    + end.to_vec2() * t.powi(3))
                .to_pos2(),
            );
        }
        start = end;
    }
    contour
}

/// True circular fillets soften the entire contour, including acute corners.
fn rounded_path(points: &[egui::Pos2], radius: f32, closed: bool) -> Vec<egui::Pos2> {
    if points.len() < 3 || radius <= f32::EPSILON {
        return points.to_vec();
    }
    let mut result = Vec::with_capacity(points.len() * 13);
    if !closed {
        result.push(points[0]);
    }
    let first = usize::from(!closed);
    let end = if closed {
        points.len()
    } else {
        points.len() - 1
    };
    for index in first..end {
        let current = points[index];
        let before = points[(index + points.len() - 1) % points.len()] - current;
        let after = points[(index + 1) % points.len()] - current;
        let incoming = before.normalized();
        let outgoing = after.normalized();
        let bisector = incoming + outgoing;
        if bisector.length_sq() < 0.0001 {
            result.push(current);
            continue;
        }
        let half_angle = incoming.dot(outgoing).clamp(-1.0, 1.0).acos() * 0.5;
        let trim = (radius / half_angle.tan())
            .min(before.length() * 0.45)
            .min(after.length() * 0.45);
        let actual_radius = trim * half_angle.tan();
        let center = current + bisector.normalized() * (actual_radius / half_angle.sin());
        let start = current + incoming * trim - center;
        let finish = current + outgoing * trim - center;
        let start_angle = start.y.atan2(start.x);
        let sweep = (start.x * finish.y - start.y * finish.x).atan2(start.dot(finish));
        for sample in 0..=12 {
            let angle = start_angle + sweep * sample as f32 / 12.0;
            result.push(center + Vec2::angled(angle) * actual_radius);
        }
    }
    if !closed {
        result.push(points[points.len() - 1]);
    }
    result
}

/// One continuous mesh shares its round caps with the stroke. Overdrawing a
/// circle at each endpoint creates darker, wider-looking bulbs at small sizes.
fn capped_stroke_mesh(
    points: &[egui::Pos2],
    stroke: Stroke,
    pixels_per_point: f32,
) -> egui::epaint::Mesh {
    let mut mesh = egui::epaint::Mesh::default();
    let radius = stroke.width * 0.5;
    let feather = 0.5 / pixels_per_point;
    let inner = (radius - feather).max(0.0);
    let outer = radius + feather;
    let mut normals = Vec::with_capacity(points.len());
    for index in 0..points.len() {
        let before = if index == 0 {
            points[1] - points[0]
        } else {
            points[index] - points[index - 1]
        }
        .normalized();
        let after = if index + 1 == points.len() {
            before
        } else {
            (points[index + 1] - points[index]).normalized()
        };
        let before_normal = egui::vec2(-before.y, before.x);
        let after_normal = egui::vec2(-after.y, after.x);
        let normal =
            (before_normal + after_normal) / (1.0 + before_normal.dot(after_normal)).max(0.01);
        normals.push(normal);
        for (offset, color) in [
            (outer, Color32::TRANSPARENT),
            (inner, stroke.color),
            (-inner, stroke.color),
            (-outer, Color32::TRANSPARENT),
        ] {
            mesh.colored_vertex(points[index] + normal * offset, color);
        }
        if index > 0 {
            let previous = ((index - 1) * 4) as u32;
            let current = (index * 4) as u32;
            for side in 0..3 {
                mesh.add_triangle(previous + side, current + side, previous + side + 1);
                mesh.add_triangle(previous + side + 1, current + side, current + side + 1);
            }
        }
    }
    for (point, normal) in [
        (points[0], normals[0]),
        (points[points.len() - 1], -normals[normals.len() - 1]),
    ] {
        let center = mesh.vertices.len() as u32;
        mesh.colored_vertex(point, stroke.color);
        let start_angle = normal.y.atan2(normal.x);
        for sample in 0..=12 {
            let angle = start_angle + std::f32::consts::PI * sample as f32 / 12.0;
            let direction = Vec2::angled(angle);
            mesh.colored_vertex(point + direction * inner, stroke.color);
            mesh.colored_vertex(point + direction * outer, Color32::TRANSPARENT);
            if sample > 0 {
                let current = center + 1 + sample * 2;
                let previous = current - 2;
                mesh.add_triangle(center, previous, current);
                mesh.add_triangle(previous, previous + 1, current);
                mesh.add_triangle(current, previous + 1, current + 1);
            }
        }
    }
    mesh
}

fn rounded_stroke(
    painter: &egui::Painter,
    mut points: Vec<egui::Pos2>,
    stroke: Stroke,
    radius: f32,
) {
    if points.len() < 2 {
        return;
    }
    let closed = points.first() == points.last();
    if closed {
        points.pop();
    }
    let curve = rounded_path(&points, radius, closed);
    if closed {
        painter.add(egui::Shape::closed_line(curve, stroke));
    } else {
        painter.add(egui::Shape::mesh(capped_stroke_mesh(
            &curve,
            stroke,
            painter.ctx().pixels_per_point(),
        )));
    }
}

/// Fill a simple concave outline with ear-clipped triangles and one AA fringe.
/// The speaker's recessed neck is concave, so a convex triangle fan is unsafe.
fn filled_concave_mesh(
    points: &[egui::Pos2],
    color: Color32,
    pixels_per_point: f32,
) -> egui::epaint::Mesh {
    let cross = |a: Vec2, b: Vec2| a.x * b.y - a.y * b.x;
    let signed_area: f32 = points
        .iter()
        .enumerate()
        .map(|(index, point)| {
            let next = points[(index + 1) % points.len()];
            point.x * next.y - next.x * point.y
        })
        .sum();
    let winding = signed_area.signum();
    let feather = 0.5 / pixels_per_point;
    let mut mesh = egui::epaint::Mesh::default();
    let mut interior = Vec::with_capacity(points.len());
    for (index, point) in points.iter().copied().enumerate() {
        let incoming = (point - points[(index + points.len() - 1) % points.len()]).normalized();
        let outgoing = (points[(index + 1) % points.len()] - point).normalized();
        let before = egui::vec2(incoming.y, -incoming.x) * winding;
        let after = egui::vec2(outgoing.y, -outgoing.x) * winding;
        let normal = (before + after) / (1.0 + before.dot(after)).max(0.01);
        let inner = point - normal * feather;
        interior.push(inner);
        mesh.colored_vertex(inner, color);
        mesh.colored_vertex(point + normal * feather, Color32::TRANSPARENT);
        if index > 0 {
            let previous = ((index - 1) * 2) as u32;
            let current = (index * 2) as u32;
            mesh.add_triangle(previous, current, previous + 1);
            mesh.add_triangle(previous + 1, current, current + 1);
        }
    }
    let last = ((points.len() - 1) * 2) as u32;
    mesh.add_triangle(last, 0, last + 1);
    mesh.add_triangle(last + 1, 0, 1);
    let mut remaining: Vec<_> = (0..points.len()).collect();
    while remaining.len() > 2 {
        let ear = (0..remaining.len()).find(|index| {
            let a = interior[remaining[(index + remaining.len() - 1) % remaining.len()]];
            let b = interior[remaining[*index]];
            let c = interior[remaining[(index + 1) % remaining.len()]];
            if cross(b - a, c - b) * winding <= 0.00001 {
                return false;
            }
            !remaining.iter().any(|candidate| {
                let point = interior[*candidate];
                if point == a || point == b || point == c {
                    return false;
                }
                cross(b - a, point - a) * winding >= -0.00001
                    && cross(c - b, point - b) * winding >= -0.00001
                    && cross(a - c, point - c) * winding >= -0.00001
            })
        });
        let Some(index) = ear else { break };
        mesh.add_triangle(
            (remaining[(index + remaining.len() - 1) % remaining.len()] * 2) as u32,
            (remaining[index] * 2) as u32,
            (remaining[(index + 1) % remaining.len()] * 2) as u32,
        );
        remaining.remove(index);
    }
    mesh
}

pub fn paint(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let point =
        |x: f32, y: f32| rect.min + Vec2::new(x / 24.0 * rect.width(), y / 24.0 * rect.height());
    let scale = rect.width().min(rect.height()) / 24.0;
    let stroke = Stroke::new(1.6, color);
    let line = |coordinates: &[(f32, f32)]| {
        rounded_stroke(
            painter,
            coordinates.iter().map(|(x, y)| point(*x, *y)).collect(),
            stroke,
            2.7 * scale,
        );
    };
    let polygon = |coordinates: &[(f32, f32)], radius: f32| {
        let points: Vec<_> = coordinates.iter().map(|(x, y)| point(*x, *y)).collect();
        painter.add(egui::Shape::convex_polygon(
            rounded_path(&points, radius * scale, true),
            color,
            Stroke::NONE,
        ));
    };
    let arc = |center: (f32, f32), radius: f32, start: f32, end: f32| {
        let points = (0..=20)
            .map(|index| {
                let angle = (start + (end - start) * index as f32 / 20.0).to_radians();
                point(
                    center.0 + radius * angle.cos(),
                    center.1 + radius * angle.sin(),
                )
            })
            .collect();
        rounded_stroke(painter, points, stroke, 0.0);
    };
    match icon {
        Icon::Home => {
            // A single closed outline avoids overlapping roof/body ends.
            line(&[
                (3.0, 10.5),
                (12.0, 3.0),
                (21.0, 10.5),
                (19.0, 10.5),
                (19.0, 21.0),
                (14.5, 21.0),
                (14.5, 16.0),
                (12.0, 13.5),
                (9.5, 16.0),
                (9.5, 21.0),
                (5.0, 21.0),
                (5.0, 10.5),
                (3.0, 10.5),
            ]);
        }
        Icon::Clock => {
            painter.circle_stroke(point(12.0, 12.0), 9.0 * scale, stroke);
            line(&[(12.0, 6.0), (12.0, 12.0), (16.5, 14.5)]);
        }
        Icon::Sparkles => {
            line(&[
                (10.0, 4.5),
                (12.0, 10.5),
                (17.5, 13.0),
                (12.0, 15.5),
                (10.0, 21.5),
                (8.0, 15.5),
                (2.5, 13.0),
                (8.0, 10.5),
                (10.0, 4.5),
            ]);
            line(&[
                (19.0, 1.8),
                (20.0, 4.2),
                (22.2, 5.0),
                (20.0, 5.8),
                (19.0, 8.2),
                (18.0, 5.8),
                (15.8, 5.0),
                (18.0, 4.2),
                (19.0, 1.8),
            ]);
        }
        Icon::ArrowLeft => {
            line(&[(20.0, 12.0), (4.0, 12.0)]);
            line(&[(10.0, 6.0), (4.0, 12.0), (10.0, 18.0)]);
        }
        Icon::ArrowRight => {
            line(&[(4.0, 12.0), (20.0, 12.0)]);
            line(&[(14.0, 6.0), (20.0, 12.0), (14.0, 18.0)]);
        }
        Icon::Settings => {
            let gear_stroke = Stroke::new(1.8, color);
            let mut outline = Vec::with_capacity(25);
            for tooth in 0..6 {
                let center_angle = tooth as f32 * 60.0 - 90.0;
                for (offset, radius) in [(-19.0, 7.5), (-14.0, 9.3), (14.0, 9.3), (19.0, 7.5)] {
                    let angle = (center_angle + offset).to_radians();
                    outline.push(point(
                        12.0 + radius * angle.cos(),
                        12.0 + radius * angle.sin(),
                    ));
                }
            }
            outline.push(outline[0]);
            rounded_stroke(painter, outline, gear_stroke, 1.5 * scale);
            painter.circle_stroke(point(12.0, 12.0), 3.0 * scale, gear_stroke);
        }
        Icon::Interface => {
            painter.rect_stroke(
                Rect::from_min_max(point(3.0, 4.0), point(21.0, 17.0)),
                2.5 * scale,
                stroke,
                egui::StrokeKind::Middle,
            );
            line(&[(12.0, 17.0), (12.0, 21.0)]);
            line(&[(8.0, 21.0), (16.0, 21.0)]);
        }
        Icon::Storage => {
            let top = (0..=32)
                .map(|i| {
                    let angle = i as f32 / 32.0 * std::f32::consts::TAU;
                    point(12.0 + 8.0 * angle.cos(), 5.5 + 3.0 * angle.sin())
                })
                .collect();
            painter.add(egui::Shape::closed_line(top, stroke));
            line(&[(4.0, 5.5), (4.0, 18.0)]);
            line(&[(20.0, 5.5), (20.0, 18.0)]);
            let bottom = (0..=16)
                .map(|i| {
                    let angle = i as f32 / 16.0 * std::f32::consts::PI;
                    point(12.0 + 8.0 * angle.cos(), 18.0 + 3.0 * angle.sin())
                })
                .collect();
            rounded_stroke(painter, bottom, stroke, 0.8 * scale);
        }
        Icon::Info => {
            painter.circle_stroke(point(12.0, 12.0), 9.0 * scale, stroke);
            painter.circle_filled(point(12.0, 7.5), 1.0 * scale, color);
            line(&[(12.0, 11.5), (12.0, 17.0)]);
        }
        Icon::Play => polygon(&[(6.5, 3.0), (21.0, 12.0), (6.5, 21.0)], 2.8),
        Icon::Pause => {
            // Both bars use the same integer pixel dimensions and positions.
            // Independent floating edges can make one look wider at fractional DPI.
            let pixels_per_point = painter.ctx().pixels_per_point();
            let width = (5.5 * scale * pixels_per_point).round().max(2.0);
            let gap = (2.0 * scale * pixels_per_point).round().max(1.0);
            let height = (18.0 * scale * pixels_per_point).round().max(4.0);
            let left = (rect.center().x * pixels_per_point - (width * 2.0 + gap) * 0.5).round();
            let top = (rect.center().y * pixels_per_point - height * 0.5).round();
            for left in [left, left + width + gap] {
                painter.rect_filled(
                    Rect::from_min_size(
                        egui::pos2(left, top) / pixels_per_point,
                        egui::vec2(width, height) / pixels_per_point,
                    ),
                    width * 0.5 / pixels_per_point,
                    color,
                );
            }
        }
        Icon::Previous => {
            painter.rect_filled(
                Rect::from_min_max(point(4.0, 4.0), point(7.5, 20.0)),
                1.75 * scale,
                color,
            );
            polygon(&[(20.5, 3.5), (6.5, 12.0), (20.5, 20.5)], 2.5);
        }
        Icon::Next => {
            painter.rect_filled(
                Rect::from_min_max(point(16.5, 4.0), point(20.0, 20.0)),
                1.75 * scale,
                color,
            );
            polygon(&[(3.5, 3.5), (17.5, 12.0), (3.5, 20.5)], 2.5);
        }
        Icon::Volume(waves) => {
            let body = rounded_path(
                &[
                    point(3.0, 8.5),
                    point(6.5, 8.5),
                    point(12.0, 3.0),
                    point(12.0, 21.0),
                    point(6.5, 15.5),
                    point(3.0, 15.5),
                ],
                2.7 * scale,
                true,
            );
            painter.add(egui::Shape::mesh(filled_concave_mesh(
                &body,
                color,
                painter.ctx().pixels_per_point(),
            )));
            for (radius_x, radius_y) in [(5.0, 7.0), (8.0, 10.0)]
                .into_iter()
                .take(usize::from(waves.min(2)))
            {
                let points = (0..=20)
                    .map(|index| {
                        let angle = (-45.0 + 90.0 * index as f32 / 20.0).to_radians();
                        point(13.0 + radius_x * angle.cos(), 12.0 + radius_y * angle.sin())
                    })
                    .collect();
                rounded_stroke(painter, points, stroke, 0.0);
            }
        }
        Icon::Library => {
            let pixels_per_point = painter.ctx().pixels_per_point();
            let width = (stroke.width * pixels_per_point).round().max(1.0);
            let top = (point(4.0, 4.0).y * pixels_per_point).round();
            let bottom = (point(4.0, 20.0).y * pixels_per_point).round();
            for x in [4.0, 9.0] {
                let left = (point(x, 4.0).x * pixels_per_point - width * 0.5).round();
                painter.rect_filled(
                    Rect::from_min_size(
                        egui::pos2(left, top) / pixels_per_point,
                        egui::vec2(width, bottom - top) / pixels_per_point,
                    ),
                    width * 0.5 / pixels_per_point,
                    color,
                );
            }
            line(&[
                (14.0, 5.0),
                (18.0, 4.0),
                (22.0, 19.0),
                (18.0, 20.0),
                (14.0, 5.0),
            ]);
        }
        Icon::Search => {
            painter.circle_stroke(point(10.0, 10.0), 6.5 * scale, stroke);
            let handle_start = 10.0 + (6.5 + stroke.width / scale) / 2.0_f32.sqrt();
            line(&[(handle_start, handle_start), (21.0, 21.0)]);
        }
        Icon::Eye | Icon::EyeOff => {
            let mut contour = Vec::with_capacity(48);
            for (start, first, second, end) in [
                ((3.0, 12.0), (7.0, 4.0), (17.0, 4.0), (21.0, 12.0)),
                ((21.0, 12.0), (17.0, 20.0), (7.0, 20.0), (3.0, 12.0)),
            ] {
                for sample in 0..24 {
                    let t = sample as f32 / 24.0;
                    let u = 1.0 - t;
                    contour.push(
                        (point(start.0, start.1).to_vec2() * u.powi(3)
                            + point(first.0, first.1).to_vec2() * (3.0 * u * u * t)
                            + point(second.0, second.1).to_vec2() * (3.0 * u * t * t)
                            + point(end.0, end.1).to_vec2() * t.powi(3))
                        .to_pos2(),
                    );
                }
            }
            painter.add(egui::Shape::closed_line(
                rounded_path(&contour, scale, true),
                stroke,
            ));
            painter.circle_stroke(point(12.0, 12.0), 2.6 * scale, stroke);
            if matches!(icon, Icon::EyeOff) {
                line(&[(4.0, 4.0), (20.0, 20.0)]);
            }
        }
        Icon::Refresh => {
            arc((12.0, 12.0), 8.5, 185.0, 340.0);
            arc((12.0, 12.0), 8.5, 5.0, 160.0);
            line(&[(20.0, 3.0), (20.0, 9.0), (14.0, 9.0)]);
            line(&[(4.0, 21.0), (4.0, 15.0), (10.0, 15.0)]);
        }
        Icon::Plus => {
            line(&[(5.0, 12.0), (19.0, 12.0)]);
            line(&[(12.0, 5.0), (12.0, 19.0)]);
        }
        Icon::Headphones => {
            arc((12.0, 12.0), 9.0, 180.0, 360.0);
            for (left, right) in [(3.0, 7.0), (17.0, 21.0)] {
                painter.rect_stroke(
                    Rect::from_min_max(point(left, 11.0), point(right, 21.0)),
                    2.0 * scale,
                    stroke,
                    egui::StrokeKind::Inside,
                );
            }
        }
        Icon::Shuffle => {
            line(&[(3.0, 6.0), (6.0, 6.0), (18.0, 18.0), (21.0, 18.0)]);
            line(&[(3.0, 18.0), (6.0, 18.0), (10.0, 14.0)]);
            line(&[(14.0, 10.0), (18.0, 6.0), (21.0, 6.0)]);
            line(&[(18.0, 3.0), (21.0, 6.0), (18.0, 9.0)]);
            line(&[(18.0, 15.0), (21.0, 18.0), (18.0, 21.0)]);
        }
        Icon::Loop | Icon::LoopOne => {
            line(&[(4.0, 11.0), (4.0, 8.0), (6.0, 6.0), (20.0, 6.0)]);
            line(&[(17.0, 3.0), (20.0, 6.0), (17.0, 9.0)]);
            line(&[(20.0, 13.0), (20.0, 16.0), (18.0, 18.0), (4.0, 18.0)]);
            line(&[(7.0, 15.0), (4.0, 18.0), (7.0, 21.0)]);
            if matches!(icon, Icon::LoopOne) {
                line(&[(10.5, 10.0), (12.0, 9.0), (12.0, 15.0)]);
                line(&[(10.0, 15.0), (14.0, 15.0)]);
            }
        }
        Icon::Close => {
            line(&[(6.0, 6.0), (18.0, 18.0)]);
            line(&[(18.0, 6.0), (6.0, 18.0)]);
        }
        Icon::RemoveFromLibrary => {
            painter.circle_stroke(point(12.0, 12.0), 9.0 * scale, stroke);
            line(&[(9.0, 9.0), (15.0, 15.0)]);
            line(&[(15.0, 9.0), (9.0, 15.0)]);
        }
        Icon::Account => {
            painter.circle_stroke(point(12.0, 8.0), 3.5 * scale, stroke);
            let shoulders = [
                (4.0, 21.0),
                (4.5, 18.5),
                (7.0, 16.0),
                (12.0, 15.0),
                (17.0, 16.0),
                (19.5, 18.5),
                (20.0, 21.0),
            ];
            line(&shoulders);
        }
        Icon::Lyrics => {
            // Balance the heavier capsule against the handle so the microphone
            // sits on the same optical center as adjacent player controls.
            let mic_point = |x: f32, y: f32| {
                point(
                    11.25 + (x + y) * std::f32::consts::FRAC_1_SQRT_2,
                    12.75 + (x - y) * std::f32::consts::FRAC_1_SQRT_2,
                )
            };
            rounded_stroke(
                painter,
                [
                    (-1.6, 1.2),
                    (-1.25, -8.3),
                    (0.0, -9.0),
                    (1.25, -8.3),
                    (1.6, 1.2),
                ]
                .into_iter()
                .map(|(x, y)| mic_point(x, y))
                .collect(),
                stroke,
                1.0 * scale,
            );
            let mut head = Vec::with_capacity(34);
            for (center, start) in [(6.0, 0.0), (3.0, std::f32::consts::PI)] {
                for index in 0..=16 {
                    let angle = start + std::f32::consts::PI * index as f32 / 16.0;
                    head.push(mic_point(3.0 * angle.cos(), center + 3.0 * angle.sin()));
                }
            }
            painter.add(egui::Shape::closed_line(head, stroke));
            rounded_stroke(
                painter,
                vec![mic_point(-2.8, 2.0), mic_point(2.8, 2.0)],
                stroke,
                0.0,
            );
        }
        Icon::Expand => {
            line(&[(3.0, 9.0), (3.0, 3.0), (9.0, 3.0)]);
            line(&[(15.0, 3.0), (21.0, 3.0), (21.0, 9.0)]);
            line(&[(21.0, 15.0), (21.0, 21.0), (15.0, 21.0)]);
            line(&[(9.0, 21.0), (3.0, 21.0), (3.0, 15.0)]);
        }
        Icon::Collapse => {
            line(&[(3.0, 8.0), (8.0, 8.0), (8.0, 3.0)]);
            line(&[(16.0, 3.0), (16.0, 8.0), (21.0, 8.0)]);
            line(&[(21.0, 16.0), (16.0, 16.0), (16.0, 21.0)]);
            line(&[(8.0, 21.0), (8.0, 16.0), (3.0, 16.0)]);
        }
        Icon::Queue => {
            line(&[(3.0, 5.0), (21.0, 5.0)]);
            line(&[(3.0, 11.0), (21.0, 11.0)]);
            line(&[(3.0, 17.0), (11.0, 17.0)]);
            polygon(&[(16.0, 15.0), (21.0, 18.0), (16.0, 21.0)], 1.3);
        }
        Icon::QueueAdd => {
            line(&[(3.0, 6.0), (16.0, 6.0)]);
            line(&[(3.0, 11.0), (13.0, 11.0)]);
            line(&[(3.0, 16.0), (10.0, 16.0)]);
            line(&[(17.0, 12.0), (17.0, 21.0)]);
            line(&[(12.5, 16.5), (21.5, 16.5)]);
        }
        Icon::PlaylistAdd => {
            let pixels = painter.ctx().pixels_per_point();
            let width = (stroke.width * pixels).round().max(1.0);
            let top = (point(4.0, 4.0).y * pixels).round();
            let bottom = (point(4.0, 20.0).y * pixels).round();
            for x in [4.0, 8.0] {
                let left = (point(x, 4.0).x * pixels - width * 0.5).round();
                painter.rect_filled(
                    Rect::from_min_size(
                        egui::pos2(left, top) / pixels,
                        egui::vec2(width, bottom - top) / pixels,
                    ),
                    width * 0.5 / pixels,
                    color,
                );
            }
            line(&[
                (12.0, 5.0),
                (15.5, 4.0),
                (18.0, 13.5),
                (14.5, 14.5),
                (12.0, 5.0),
            ]);
            line(&[(18.0, 16.0), (18.0, 22.0)]);
            line(&[(15.0, 19.0), (21.0, 19.0)]);
        }
        Icon::Radio => {
            painter.circle_filled(point(12.0, 12.0), 2.3 * scale, color);
            for radius in [6.0, 10.0] {
                arc((12.0, 12.0), radius, -45.0, 45.0);
                arc((12.0, 12.0), radius, 135.0, 225.0);
            }
        }
        Icon::Album => {
            painter.circle_stroke(point(12.0, 12.0), 9.0 * scale, stroke);
            painter.circle_stroke(point(12.0, 12.0), 3.0 * scale, stroke);
            painter.circle_filled(point(12.0, 12.0), 0.9 * scale, color);
        }
        Icon::Pin => {
            line(&[(8.0, 3.5), (16.0, 3.5)]);
            line(&[
                (9.0, 3.5),
                (9.0, 10.0),
                (5.5, 14.0),
                (18.5, 14.0),
                (15.0, 10.0),
                (15.0, 3.5),
            ]);
            line(&[(12.0, 14.0), (12.0, 21.0)]);
        }
        Icon::Edit => {
            line(&[
                (4.0, 16.5),
                (4.0, 20.0),
                (7.5, 20.0),
                (20.0, 7.5),
                (16.5, 4.0),
                (4.0, 16.5),
            ]);
            line(&[(14.5, 6.0), (18.0, 9.5)]);
        }
        Icon::Trash => {
            line(&[(4.0, 6.0), (20.0, 6.0)]);
            line(&[(8.5, 6.0), (8.5, 3.5), (15.5, 3.5), (15.5, 6.0)]);
            line(&[(6.0, 6.0), (7.0, 20.0), (17.0, 20.0), (18.0, 6.0)]);
            line(&[(10.0, 10.0), (10.5, 16.5)]);
            line(&[(14.0, 10.0), (13.5, 16.5)]);
        }
        Icon::CopyLink => {
            line(&[(9.0, 15.0), (15.0, 9.0)]);
            line(&[
                (7.0, 13.0),
                (4.0, 16.0),
                (4.0, 19.0),
                (7.0, 21.0),
                (10.0, 20.0),
                (13.0, 17.0),
            ]);
            line(&[
                (11.0, 7.0),
                (14.0, 4.0),
                (17.0, 3.0),
                (20.0, 5.0),
                (20.0, 8.0),
                (17.0, 11.0),
            ]);
        }
        Icon::Heart | Icon::HeartFilled => {
            let contour = heart_path(rect);
            if matches!(icon, Icon::HeartFilled) {
                painter.add(egui::Shape::mesh(filled_concave_mesh(
                    &contour,
                    color,
                    painter.ctx().pixels_per_point(),
                )));
            } else {
                painter.add(egui::Shape::closed_line(contour, stroke));
            }
        }
    }
}

pub fn compact_button(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    enabled: bool,
    primary: bool,
) -> Response {
    ui.add_enabled_ui(enabled, |ui| {
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(38.0), Sense::click());
        let visuals = ui.style().interact(&response);
        let fill = if primary {
            crate::theme::PRIMARY
        } else {
            visuals.bg_fill
        };
        if !matches!(icon, Icon::Previous | Icon::Next)
            || response.hovered()
            || response.has_focus()
            || response.is_pointer_button_down_on()
        {
            ui.painter().rect_filled(rect, 19.0, fill);
        }
        let inset = if primary && matches!(icon, Icon::Play) {
            5.0
        } else if primary && matches!(icon, Icon::Pause) {
            7.0
        } else {
            9.0
        };
        paint(
            ui.painter(),
            rect.shrink(inset),
            icon,
            visuals.fg_stroke.color,
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
        });
        response.on_hover_text(label)
    })
    .inner
}

/// Playback modes use a colored glyph and dot rather than an idle button frame.
pub fn toggle_button(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    enabled: bool,
    selected: bool,
) -> Response {
    toggle_button_sized(ui, icon, label, enabled, selected, 38.0)
}

pub fn toggle_button_sized(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    enabled: bool,
    selected: bool,
    size: f32,
) -> Response {
    ui.add_enabled_ui(enabled, |ui| {
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
        let visuals = ui.style().interact(&response);
        if response.hovered() || response.has_focus() || response.is_pointer_button_down_on() {
            ui.painter().rect_filled(rect, size * 0.5, visuals.bg_fill);
        }
        let color = if selected {
            crate::theme::ACCENT
        } else {
            visuals.fg_stroke.color
        };
        paint(
            ui.painter(),
            Rect::from_center_size(rect.center(), Vec2::splat(20.0)),
            icon,
            color,
        );
        if selected {
            ui.painter().circle_filled(
                egui::pos2(rect.center().x, rect.bottom() - 4.0),
                1.5,
                color,
            );
        }
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), selected, label)
        });
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(label)
    })
    .inner
}

pub fn panel_button(ui: &mut egui::Ui, rect: Rect, icon: Icon, label: &str) -> Response {
    let response = ui
        .put(rect, egui::Button::new("").frame(false))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(label);
    paint(
        ui.painter(),
        rect.shrink(6.0),
        icon,
        if response.hovered() {
            Color32::WHITE
        } else {
            crate::theme::MUTED
        },
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    response
}

/// Compact library action: the caller owns the saved state and server request.
pub fn heart_button(
    ui: &mut egui::Ui,
    label: &str,
    enabled: bool,
    liked: bool,
    pending: bool,
) -> Response {
    ui.add_enabled_ui(enabled && !pending, |ui| {
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::click());
        let base = if liked {
            crate::theme::ACCENT
        } else {
            crate::theme::MUTED
        };
        let color = if response.hovered() || response.has_focus() {
            base.lerp_to_gamma(Color32::WHITE, 0.18)
        } else {
            base
        };
        if pending {
            egui::Spinner::new()
                .color(color)
                .paint_at(ui, rect.shrink(2.0));
        } else {
            paint(
                ui.painter(),
                rect.shrink(2.0),
                if liked {
                    Icon::HeartFilled
                } else {
                    Icon::Heart
                },
                color,
            );
        }
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), liked, label)
        });
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(label)
    })
    .inner
}

pub fn show(ui: &mut egui::Ui, icon: Icon, color: Color32, size: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint(ui.painter(), rect, icon, color);
    response
}

pub fn button(ui: &mut egui::Ui, icon: Icon, label: &str, enabled: bool) -> Response {
    ui.add_enabled_ui(enabled, |ui| {
        let font = egui::TextStyle::Button.resolve(ui.style());
        let text_color = ui.visuals().text_color();
        let galley = ui.painter().layout_no_wrap(label.into(), font, text_color);
        let size = Vec2::new(galley.size().x + 54.0, 36.0);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let visuals = ui.style().interact(&response);
        ui.painter().rect(
            rect,
            8.0,
            visuals.bg_fill,
            visuals.bg_stroke,
            egui::StrokeKind::Inside,
        );
        let icon_rect = Rect::from_min_size(rect.min + Vec2::new(12.0, 10.0), Vec2::splat(16.0));
        paint(ui.painter(), icon_rect, icon, visuals.fg_stroke.color);
        ui.painter().galley(
            rect.min + Vec2::new(36.0, (36.0 - galley.size().y) * 0.5),
            galley,
            text_color,
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
        });
        response
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(icon: Icon, size: f32) -> (egui::Context, egui::FullOutput, Rect) {
        render_at(icon, size, 1.0)
    }

    fn render_at(
        icon: Icon,
        size: f32,
        pixels_per_point: f32,
    ) -> (egui::Context, egui::FullOutput, Rect) {
        let context = egui::Context::default();
        context.set_pixels_per_point(pixels_per_point);
        context.tessellation_options_mut(|options| options.prerasterized_discs = false);
        let rect = Rect::from_min_size(egui::pos2(12.0, 12.0), Vec2::splat(size));
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(80.0, 80.0),
                )),
                ..Default::default()
            },
            |ui| paint(ui.painter(), rect, icon, Color32::WHITE),
        );
        output.textures_delta.clear();
        (context, output, rect)
    }

    #[test]
    fn every_icon_tessellates_within_its_padded_bounds_at_small_and_large_sizes() {
        let icons = [
            Icon::Home,
            Icon::Clock,
            Icon::Sparkles,
            Icon::ArrowLeft,
            Icon::ArrowRight,
            Icon::Library,
            Icon::Search,
            Icon::Eye,
            Icon::EyeOff,
            Icon::Refresh,
            Icon::Plus,
            Icon::Headphones,
            Icon::Play,
            Icon::Pause,
            Icon::Previous,
            Icon::Next,
            Icon::Volume(2),
            Icon::Settings,
            Icon::Interface,
            Icon::Storage,
            Icon::Info,
            Icon::Shuffle,
            Icon::Loop,
            Icon::LoopOne,
            Icon::Close,
            Icon::Account,
            Icon::Heart,
            Icon::HeartFilled,
            Icon::QueueAdd,
            Icon::Queue,
            Icon::Lyrics,
            Icon::Expand,
            Icon::Collapse,
            Icon::PlaylistAdd,
            Icon::Radio,
            Icon::Album,
            Icon::CopyLink,
            Icon::Edit,
            Icon::Trash,
            Icon::RemoveFromLibrary,
            Icon::Pin,
        ];
        for size in [12.0, 19.0, 20.0, 24.0, 32.0] {
            for icon in icons {
                let (context, output, rect) = render(icon, size);
                let primitives = context.tessellate(output.shapes, output.pixels_per_point);
                assert!(!primitives.is_empty(), "Missing {icon:?}");
                for primitive in primitives {
                    if let egui::epaint::Primitive::Mesh(mesh) = primitive.primitive {
                        assert!(!mesh.indices.is_empty());
                        assert!(
                            mesh.vertices.iter().all(|vertex| vertex.pos.is_finite()
                                && rect.expand(2.0).contains(vertex.pos)),
                            "Out of bounds {icon:?} at {size}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn pause_bars_are_thicker_than_their_gap_and_have_rounded_ends() {
        let (_, output, _) = render(Icon::Pause, 24.0);
        let mut bars: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect) if rect.fill == Color32::WHITE => Some(rect),
                _ => None,
            })
            .collect();
        bars.sort_by(|left, right| left.rect.left().total_cmp(&right.rect.left()));
        assert_eq!(bars.len(), 2);
        let gap = bars[1].rect.left() - bars[0].rect.right();
        assert!(bars[0].rect.width() > gap && bars[1].rect.width() > gap);
        assert_eq!(gap, 2.0);
        assert!(
            bars.iter()
                .all(|bar| bar.rect.width() == 6.0 && bar.rect.height() == 18.0)
        );
        assert!(
            bars.iter()
                .all(|bar| bar.corner_radius.nw >= 3 && bar.corner_radius.se >= 3)
        );
    }

    #[test]
    fn filled_transport_glyphs_have_soft_turns_instead_of_three_sharp_corners() {
        for icon in [Icon::Play, Icon::Previous, Icon::Next] {
            let (_, output, _) = render(icon, 24.0);
            let path = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Path(path)
                        if path.closed
                            && path.fill == Color32::WHITE
                            && path.stroke.width == 0.0 =>
                    {
                        Some(path)
                    }
                    _ => None,
                })
                .unwrap();
            assert!(path.points.len() > 3);
            let mut area = 0.0;
            for (index, current) in path.points.iter().enumerate() {
                let previous = path.points[(index + path.points.len() - 1) % path.points.len()];
                let next = path.points[(index + 1) % path.points.len()];
                let incoming = *current - previous;
                let outgoing = next - *current;
                if incoming.length() > 1e-4 && outgoing.length() > 1e-4 {
                    assert!(
                        incoming.normalized().dot(outgoing.normalized()) > 0.3,
                        "Sharp turn in {icon:?}"
                    );
                }
                area += current.x * next.y - next.x * current.y;
            }
            assert!(
                area.abs() * 0.5 > 50.0,
                "Solid transport glyph became too small"
            );
        }
    }

    fn alpha_at(primitives: &[egui::ClippedPrimitive], point: egui::Pos2) -> f32 {
        let mut alpha = 0.0_f32;
        let cross = |a: Vec2, b: Vec2| a.x * b.y - a.y * b.x;
        for primitive in primitives {
            if !primitive.clip_rect.contains(point) {
                continue;
            }
            let egui::epaint::Primitive::Mesh(mesh) = &primitive.primitive else {
                continue;
            };
            for triangle in mesh.indices.as_chunks::<3>().0 {
                let a = mesh.vertices[triangle[0] as usize];
                let b = mesh.vertices[triangle[1] as usize];
                let c = mesh.vertices[triangle[2] as usize];
                let area = cross(b.pos - a.pos, c.pos - a.pos);
                if area.abs() < 1e-6 {
                    continue;
                }
                let weights = [
                    cross(b.pos - point, c.pos - point) / area,
                    cross(c.pos - point, a.pos - point) / area,
                    cross(a.pos - point, b.pos - point) / area,
                ];
                if weights.iter().all(|weight| *weight >= -1e-5) {
                    alpha = alpha.max(
                        weights[0] * f32::from(a.color.a())
                            + weights[1] * f32::from(b.color.a())
                            + weights[2] * f32::from(c.color.a()),
                    );
                }
            }
        }
        alpha
    }

    fn sharp_triangle_alpha(points: Vec<egui::Pos2>, point: egui::Pos2) -> f32 {
        let context = egui::Context::default();
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::splat(80.0))),
                ..Default::default()
            },
            |ui| {
                ui.painter().add(egui::Shape::convex_polygon(
                    points.clone(),
                    Color32::WHITE,
                    Stroke::NONE,
                ));
            },
        );
        output.textures_delta.clear();
        let primitives = context.tessellate(output.shapes, output.pixels_per_point);
        alpha_at(&primitives, point)
    }

    #[test]
    fn open_strokes_have_visible_round_caps_beyond_their_centerline_endpoints() {
        let (context, output, rect) = render(Icon::ArrowRight, 24.0);
        let primitives = context.tessellate(output.shapes, output.pixels_per_point);
        assert!(alpha_at(&primitives, rect.min + egui::vec2(3.5, 12.0)) > 80.0);
        assert!(alpha_at(&primitives, rect.min + egui::vec2(2.5, 10.5)) < 10.0);
    }

    #[test]
    fn play_tip_has_a_broad_round_silhouette_at_the_actual_player_size() {
        let (context, output, rect) = render(Icon::Play, 24.0);
        let primitives = context.tessellate(output.shapes, output.pixels_per_point);
        let point = |x: f32, y: f32| rect.min + egui::vec2(x, y);

        // The rounded nose stays solid through a broad middle and drops off
        // before the old sharp point, rather than merely adding path vertices.
        assert!(alpha_at(&primitives, point(17.0, 12.0)) > 230.0);
        assert!(alpha_at(&primitives, point(17.0, 10.8)) > 170.0);
        assert!(alpha_at(&primitives, point(19.6, 12.0)) < 20.0);
        assert!(alpha_at(&primitives, point(20.0, 12.0)) < 5.0);
        assert!(
            sharp_triangle_alpha(
                vec![point(6.5, 3.0), point(21.0, 12.0), point(6.5, 21.0)],
                point(19.6, 12.0)
            ) > 230.0
        );
    }

    #[test]
    fn skip_triangle_tips_are_visibly_round_at_twenty_pixels() {
        for (icon, triangle, tip) in [
            (
                Icon::Previous,
                [(20.5, 3.5), (6.5, 12.0), (20.5, 20.5)],
                (8.0, 12.0),
            ),
            (
                Icon::Next,
                [(3.5, 3.5), (17.5, 12.0), (3.5, 20.5)],
                (16.0, 12.0),
            ),
        ] {
            let (context, output, rect) = render(icon, 20.0);
            let triangle_shapes = output
                .shapes
                .into_iter()
                .filter(|shape| matches!(shape.shape, egui::Shape::Path(_)))
                .collect();
            let primitives = context.tessellate(triangle_shapes, output.pixels_per_point);
            let point = |(x, y)| rect.min + egui::vec2(x, y) * (20.0 / 24.0);
            assert!(alpha_at(&primitives, point(tip)) < 20.0);
            assert!(alpha_at(&primitives, point((12.0, 12.0))) > 230.0);
            assert!(
                sharp_triangle_alpha(triangle.into_iter().map(point).collect(), point(tip)) > 230.0
            );
        }
    }

    #[test]
    fn pause_bars_have_identical_pixel_coverage_even_at_fractional_display_scales() {
        for pixels_per_point in [1.0, 1.25, 1.5, 1.75, 2.0] {
            let (context, output, _) = render_at(Icon::Pause, 24.0, pixels_per_point);
            assert_eq!(output.pixels_per_point, pixels_per_point);
            let mut bars: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Rect(rect) if rect.fill == Color32::WHITE => Some(rect.rect),
                    _ => None,
                })
                .collect();
            bars.sort_by(|left, right| left.left().total_cmp(&right.left()));
            assert_eq!(bars.len(), 2);
            assert!((bars[0].width() - bars[1].width()).abs() < 0.0001);
            for bar in &bars {
                for coordinate in [bar.left(), bar.top(), bar.right(), bar.bottom()] {
                    let pixel_coordinate = coordinate * pixels_per_point;
                    assert!((pixel_coordinate - pixel_coordinate.round()).abs() < 0.0001);
                }
            }
            let primitives = context.tessellate(output.shapes, output.pixels_per_point);
            for y in 0..(bars[0].height() * pixels_per_point) as usize {
                for x in 0..(bars[0].width() * pixels_per_point) as usize {
                    let offset = egui::vec2(x as f32 + 0.5, y as f32 + 0.5) / pixels_per_point;
                    let left = alpha_at(&primitives, bars[0].min + offset);
                    let right = alpha_at(&primitives, bars[1].min + offset);
                    assert!(
                        (left - right).abs() < 0.01,
                        "Unequal bars at scale {pixels_per_point}: {left} / {right}"
                    );
                }
            }
        }
    }

    #[test]
    fn search_handle_starts_outside_the_lens_outer_edge_including_its_cap() {
        for size in [19.0, 20.0, 24.0] {
            let (context, output, _) = render(Icon::Search, size);
            let lens = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Circle(circle) if circle.stroke.width > 0.0 => Some(*circle),
                    _ => None,
                })
                .unwrap();
            let handle_shapes = output
                .shapes
                .into_iter()
                .filter(|shape| matches!(shape.shape, egui::Shape::Mesh(_)))
                .collect();
            let primitives = context.tessellate(handle_shapes, output.pixels_per_point);
            assert!(!primitives.is_empty());
            let inside_lens = lens.center + Vec2::splat((lens.radius - 0.2) / 2.0_f32.sqrt());
            assert!(
                alpha_at(&primitives, inside_lens) < 5.0,
                "Handle enters the lens at {size}px"
            );
        }
    }

    #[test]
    fn home_is_one_rounded_contour_and_library_stems_are_plain_pixel_aligned_capsules() {
        let (_, output, rect) = render(Icon::Home, 24.0);
        let paths: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Path(path) if path.stroke.width > 0.0 => Some(path),
                _ => None,
            })
            .collect();
        assert_eq!(paths.len(), 1);
        assert!(paths[0].closed);
        let highest_point = paths[0]
            .points
            .iter()
            .map(|point| point.y)
            .fold(f32::INFINITY, f32::min);
        assert!(
            highest_point > rect.top() + 3.6,
            "Roof apex still has a sharp point"
        );
        for size in [19.0, 20.0, 24.0, 32.0] {
            let (_, output, _) = render(Icon::Library, size);
            let stems: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Rect(rect) if rect.fill == Color32::WHITE => Some(rect),
                    _ => None,
                })
                .collect();
            assert_eq!(stems.len(), 2);
            assert_eq!(stems[0].rect.size(), stems[1].rect.size());
            for stem in stems {
                assert!(stem.corner_radius.nw >= 1);
                assert_eq!(stem.rect.left().fract(), 0.0);
                assert_eq!(stem.rect.top().fract(), 0.0);
            }
            assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Circle(circle) if circle.fill == Color32::WHITE)));
        }
    }

    #[test]
    fn settings_is_a_continuous_toothed_gear_outline_with_one_center_hole() {
        for size in [19.0, 20.0, 24.0] {
            let (_, output, rect) = render(Icon::Settings, size);
            let paths: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Path(path) => Some(path),
                    _ => None,
                })
                .collect();
            let circles: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Circle(circle) => Some(circle),
                    _ => None,
                })
                .collect();
            assert_eq!(paths.len(), 1);
            assert!(paths[0].closed);
            assert_eq!(circles.len(), 1);
            assert!((circles[0].radius - size * 3.0 / 24.0).abs() < 0.001);
            let radii: Vec<_> = paths[0]
                .points
                .iter()
                .map(|point| point.distance(rect.center()) * 24.0 / size)
                .collect();
            assert!(radii.iter().any(|radius| *radius > 9.0));
            assert!(radii.iter().any(|radius| *radius < 7.8));
            let teeth = radii
                .iter()
                .zip(radii.iter().cycle().skip(1))
                .filter(|(a, b)| **a <= 8.8 && **b > 8.8)
                .count();
            assert_eq!(teeth, 6, "Gear should keep just six broad, rounded teeth");
            assert!(
                !output
                    .shapes
                    .iter()
                    .any(|shape| matches!(&shape.shape, egui::Shape::Mesh(_))),
                "Gear must have no detached spoke strokes"
            );
        }
    }

    #[test]
    fn volume_speaker_is_filled_and_sound_waves_remain_separate_thin_curves() {
        for size in [19.0, 20.0, 24.0] {
            let (context, output, rect) = render(Icon::Volume(2), size);
            let body = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Mesh(mesh) => Some(mesh),
                    _ => None,
                })
                .unwrap();
            let contour_points = body.vertices.len() / 2;
            assert_eq!(
                body.indices.len(),
                contour_points * 9 - 6,
                "Speaker contour must be completely triangulated at {size}px"
            );
            let primitives = context.tessellate(output.shapes, output.pixels_per_point);
            let point = |x, y| rect.min + egui::vec2(x, y) * (size / 24.0);
            assert!(alpha_at(&primitives, point(6.0, 12.0)) > 250.0);
            assert!(alpha_at(&primitives, point(10.0, 12.0)) > 250.0);
            assert!(alpha_at(&primitives, point(18.0, 12.0)) > 230.0);
            assert!(alpha_at(&primitives, point(19.5, 12.0)) < 130.0);
            assert!(alpha_at(&primitives, point(21.0, 12.0)) > 230.0);
        }
    }

    #[test]
    fn volume_waves_follow_the_master_level_and_have_compact_vertical_spread() {
        for (volume, count) in [(0.0, 0), (0.1, 1), (0.5, 1), (0.51, 2), (1.0, 2)] {
            let (_, output, rect) = render(Icon::volume(volume), 24.0);
            let waves: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Mesh(mesh) => Some(mesh),
                    _ => None,
                })
                .skip(1)
                .collect();
            assert_eq!(waves.len(), count);
            for (index, wave) in waves.into_iter().enumerate() {
                let top = wave
                    .vertices
                    .iter()
                    .map(|vertex| vertex.pos.y)
                    .fold(f32::INFINITY, f32::min);
                let bottom = wave
                    .vertices
                    .iter()
                    .map(|vertex| vertex.pos.y)
                    .fold(f32::NEG_INFINITY, f32::max);
                let height = bottom - top;
                assert!(height < rect.height() * 0.75);
                if index == 1 {
                    assert!(height > rect.height() * 0.65);
                }
            }
        }
    }

    #[test]
    fn microphone_ink_is_centered_in_the_shared_player_icon_box() {
        for size in [20.0, 24.0] {
            let (context, output, rect) = render(Icon::Lyrics, size);
            let primitives = context.tessellate(output.shapes, output.pixels_per_point);
            let mut mass = 0.0;
            let mut moment = Vec2::ZERO;
            for row in 0..48 {
                for column in 0..48 {
                    let point =
                        rect.min + egui::vec2(column as f32 + 0.5, row as f32 + 0.5) * size / 48.0;
                    let alpha = alpha_at(&primitives, point);
                    mass += alpha;
                    moment += point.to_vec2() * alpha;
                }
            }
            let center = (moment / mass).to_pos2();
            assert!(
                center.distance(rect.center()) < size * 0.025,
                "{center:?} at {size}"
            );
        }
    }

    #[test]
    fn filled_heart_has_two_round_lobes_a_clear_cleft_and_a_solid_center() {
        for size in [18.0, 24.0] {
            let (context, output, rect) = render(Icon::HeartFilled, size);
            let body = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Mesh(mesh) => Some(mesh),
                    _ => None,
                })
                .unwrap();
            assert_eq!(body.indices.len(), body.vertices.len() / 2 * 9 - 6);
            let primitives = context.tessellate(output.shapes, output.pixels_per_point);
            let point = |x, y| rect.min + egui::vec2(x, y) * (size / 24.0);
            for (x, y) in [(7.5, 6.5), (16.5, 6.5), (12.0, 12.0), (12.0, 17.0)] {
                assert!(alpha_at(&primitives, point(x, y)) > 240.0);
            }
            assert!(alpha_at(&primitives, point(12.0, 4.2)) < 10.0);
            assert!(alpha_at(&primitives, point(12.0, 21.0)) < 5.0);
            assert!(alpha_at(&primitives, point(2.0, 4.0)) < 5.0);
            let (context, output, rect) = render(Icon::Heart, size);
            let primitives = context.tessellate(output.shapes, output.pixels_per_point);
            assert!(alpha_at(&primitives, rect.center()) < 5.0);
        }
    }

    #[test]
    fn heart_button_keeps_a_22_pixel_hitbox_selected_color_and_checkbox_semantics() {
        for (liked, pending) in [(false, false), (true, false), (true, true)] {
            let context = egui::Context::default();
            let mut response = None;
            let mut frame = |events| {
                let mut output = context.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            egui::Pos2::ZERO,
                            Vec2::splat(100.0),
                        )),
                        time: Some(1.0),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        response = Some(heart_button(ui, "Save track", true, liked, pending));
                    },
                );
                output.textures_delta.clear();
                (output, response.take().unwrap())
            };
            let (output, response) = frame(vec![]);
            assert_eq!(response.rect.size(), Vec2::splat(22.0));
            assert_eq!(response.enabled(), !pending);
            assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Rect(rect) if rect.rect.size()==Vec2::splat(22.0))));
            if !pending {
                let expected_color = if liked {
                    crate::theme::ACCENT
                } else {
                    crate::theme::MUTED
                };
                let primitives = context.tessellate(output.shapes, output.pixels_per_point);
                assert!(primitives.iter().any(|primitive| matches!(&primitive.primitive,egui::epaint::Primitive::Mesh(mesh) if mesh.vertices.iter().any(|vertex| vertex.color == expected_color))));
            }
            let position = response.rect.center();
            let event = |pressed| {
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]
            };
            let _ = frame(event(true));
            let (output, response) = frame(event(false));
            assert_eq!(response.clicked(), !pending);
            if !pending {
                assert!(output.platform_output.events.iter().any(|event| matches!(event,egui::output::OutputEvent::Clicked(info) if info.typ == egui::WidgetType::Checkbox && info.selected == Some(liked))));
            }
        }
    }

    #[test]
    fn playback_toggle_has_a_blue_selected_glyph_without_an_idle_button_background() {
        let context = egui::Context::default();
        crate::theme::install(&context);
        for selected in [false, true] {
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(100.0, 80.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    toggle_button(ui, Icon::Shuffle, "Shuffle", true, selected);
                },
            );
            output.textures_delta.clear();
            assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,egui::Shape::Rect(rect) if rect.rect.size()==Vec2::splat(38.0))));
            let indicator = output.shapes.iter().any(|shape| matches!(&shape.shape,egui::Shape::Circle(circle) if circle.radius==1.5 && circle.fill==crate::theme::ACCENT));
            assert_eq!(indicator, selected);
        }
    }
}
