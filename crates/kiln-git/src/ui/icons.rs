//! 폰트에 의존하지 않는 선 아이콘. 모두 painter 로 직접 그린다.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Icon {
    Plus,
    Minus,
    Discard,
    Open,
    Check,
    Close,
    Trash,
    Refresh,
    ArrowDown,
    ArrowUp,
    Branch,
    ChevronDown,
    ChevronRight,
    Warning,
    Stash,
    Apply,
    Pop,
    PullRequest,
    CheckCircle,
    XCircle,
    PendingCircle,
    Skip,
    External,
    Download,
    Issue,
    IssueClosed,
    Actions,
    Repo,
    Star,
    Tag,
    Comment,
    Person,
    Lock,
    Play,
    Stop,
}

/// `rect` 중앙에 아이콘을 그린다. 아이콘 크기는 `rect` 의 짧은 변 기준.
pub(crate) fn paint_icon(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width().min(rect.height());
    let c = rect.center();
    let u = s / 16.0;
    let pt = |x: f32, y: f32| pos2(c.x + (x - 8.0) * u, c.y + (y - 8.0) * u);
    let w = (1.35 * u).max(1.1);
    let st = Stroke::new(w, color);
    let line = |pts: &[Pos2]| {
        p.add(Shape::line(pts.to_vec(), st));
    };
    let common = match icon {
        Icon::Issue => Some(kiln_common::icons::Icon::Issue),
        Icon::IssueClosed => Some(kiln_common::icons::Icon::IssueClosed),
        Icon::Actions => Some(kiln_common::icons::Icon::Actions),
        Icon::Repo => Some(kiln_common::icons::Icon::Repo),
        Icon::Star => Some(kiln_common::icons::Icon::Star),
        Icon::Tag => Some(kiln_common::icons::Icon::Tag),
        Icon::Comment => Some(kiln_common::icons::Icon::Comment),
        Icon::Person => Some(kiln_common::icons::Icon::Person),
        Icon::Lock => Some(kiln_common::icons::Icon::Lock),
        Icon::Play => Some(kiln_common::icons::Icon::Play),
        Icon::Stop => Some(kiln_common::icons::Icon::Stop),
        _ => None,
    };
    if let Some(ci) = common {
        // 공용 아이콘 세트로 그린다.
        kiln_common::icons::paint(p, Rect::from_center_size(c, vec2(s * 1.12, s * 1.12)), ci, color);
        return;
    }
    match icon {
        Icon::Plus => {
            line(&[pt(8.0, 3.0), pt(8.0, 13.0)]);
            line(&[pt(3.0, 8.0), pt(13.0, 8.0)]);
        }
        Icon::Minus => line(&[pt(3.0, 8.0), pt(13.0, 8.0)]),
        Icon::Discard => {
            // 되돌리기(undo) 모양: 왼쪽 화살촉 + 오른쪽으로 감기는 곡선
            line(&[pt(6.0, 3.0), pt(3.0, 6.0), pt(6.0, 9.0)]);
            let mut pts = vec![pt(3.0, 6.0), pt(9.5, 6.0)];
            pts.extend((1..=10).map(|i| {
                let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 10.0;
                pt(9.5 + 3.5 * a.cos(), 9.5 + 3.5 * a.sin())
            }));
            pts.push(pt(6.0, 13.0));
            line(&pts);
        }
        Icon::Open | Icon::External => {
            line(&[pt(9.0, 3.0), pt(13.0, 3.0), pt(13.0, 7.0)]);
            line(&[pt(13.0, 3.0), pt(7.5, 8.5)]);
            line(&[pt(11.0, 9.5), pt(11.0, 13.0), pt(3.0, 13.0), pt(3.0, 5.0), pt(6.5, 5.0)]);
        }
        Icon::Check => line(&[pt(3.0, 8.5), pt(6.5, 12.0), pt(13.0, 4.5)]),
        Icon::Close => {
            line(&[pt(4.0, 4.0), pt(12.0, 12.0)]);
            line(&[pt(12.0, 4.0), pt(4.0, 12.0)]);
        }
        Icon::Trash => {
            line(&[pt(2.5, 4.5), pt(13.5, 4.5)]);
            line(&[pt(6.0, 4.5), pt(6.0, 2.5), pt(10.0, 2.5), pt(10.0, 4.5)]);
            line(&[pt(4.0, 4.5), pt(4.8, 13.5), pt(11.2, 13.5), pt(12.0, 4.5)]);
            line(&[pt(6.8, 7.0), pt(6.8, 11.0)]);
            line(&[pt(9.2, 7.0), pt(9.2, 11.0)]);
        }
        Icon::Refresh => {
            let pts: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let a = -0.35 + i as f32 * 0.33;
                    pt(8.0 + 5.0 * a.cos(), 8.0 + 5.0 * a.sin())
                })
                .collect();
            line(&pts);
            let e = pts[0];
            line(&[e + vec2(-3.0 * u, -0.6 * u), e, e + vec2(0.6 * u, -3.0 * u)]);
        }
        Icon::ArrowDown => {
            line(&[pt(8.0, 2.5), pt(8.0, 13.0)]);
            line(&[pt(3.5, 8.5), pt(8.0, 13.0), pt(12.5, 8.5)]);
        }
        Icon::ArrowUp => {
            line(&[pt(8.0, 13.5), pt(8.0, 3.0)]);
            line(&[pt(3.5, 7.5), pt(8.0, 3.0), pt(12.5, 7.5)]);
        }
        Icon::Download => {
            line(&[pt(8.0, 2.5), pt(8.0, 10.0)]);
            line(&[pt(4.5, 6.5), pt(8.0, 10.0), pt(11.5, 6.5)]);
            line(&[pt(3.0, 13.0), pt(13.0, 13.0)]);
        }
        Icon::Branch => {
            let r = 1.9 * u;
            p.circle_stroke(pt(5.0, 3.5), r, st);
            p.circle_stroke(pt(5.0, 12.5), r, st);
            p.circle_stroke(pt(11.5, 5.0), r, st);
            line(&[pt(5.0, 5.4), pt(5.0, 10.6)]);
            let bez = [pt(11.5, 6.9), pt(11.5, 9.5), pt(5.5, 8.0), pt(5.0, 10.6)];
            p.add(Shape::CubicBezier(egui::epaint::CubicBezierShape::from_points_stroke(
                bez,
                false,
                Color32::TRANSPARENT,
                st,
            )));
        }
        Icon::PullRequest => {
            let r = 1.9 * u;
            p.circle_stroke(pt(4.5, 3.5), r, st);
            p.circle_stroke(pt(4.5, 12.5), r, st);
            p.circle_stroke(pt(11.5, 12.5), r, st);
            line(&[pt(4.5, 5.4), pt(4.5, 10.6)]);
            line(&[pt(11.5, 10.6), pt(11.5, 5.0), pt(7.5, 5.0)]);
            line(&[pt(9.2, 3.2), pt(7.3, 5.0), pt(9.2, 6.8)]);
        }
        Icon::ChevronDown => line(&[pt(4.5, 6.5), pt(8.0, 10.0), pt(11.5, 6.5)]),
        Icon::ChevronRight => line(&[pt(6.5, 4.5), pt(10.0, 8.0), pt(6.5, 11.5)]),
        Icon::Warning => {
            line(&[pt(8.0, 2.0), pt(14.5, 13.5), pt(1.5, 13.5), pt(8.0, 2.0)]);
            line(&[pt(8.0, 6.0), pt(8.0, 9.5)]);
            p.circle_filled(pt(8.0, 11.5), 0.9 * u, color);
        }
        Icon::Stash => {
            line(&[pt(2.5, 3.5), pt(13.5, 3.5), pt(13.5, 6.5), pt(2.5, 6.5), pt(2.5, 3.5)]);
            line(&[pt(3.5, 6.5), pt(3.5, 13.0), pt(12.5, 13.0), pt(12.5, 6.5)]);
            line(&[pt(6.5, 9.0), pt(9.5, 9.0)]);
        }
        Icon::Apply => {
            line(&[pt(8.0, 2.5), pt(8.0, 9.5)]);
            line(&[pt(5.0, 6.8), pt(8.0, 9.8), pt(11.0, 6.8)]);
            line(&[pt(2.5, 9.5), pt(2.5, 13.0), pt(13.5, 13.0), pt(13.5, 9.5)]);
        }
        Icon::Pop => {
            line(&[pt(8.0, 10.0), pt(8.0, 2.8)]);
            line(&[pt(5.0, 5.8), pt(8.0, 2.8), pt(11.0, 5.8)]);
            line(&[pt(2.5, 9.5), pt(2.5, 13.0), pt(13.5, 13.0), pt(13.5, 9.5)]);
        }
        Icon::CheckCircle => {
            p.circle_filled(c, 6.8 * u, color);
            let st2 = Stroke::new(w * 1.1, Color32::from_rgb(0x10, 0x14, 0x1c));
            p.add(Shape::line(vec![pt(5.0, 8.3), pt(7.2, 10.5), pt(11.2, 5.9)], st2));
        }
        Icon::XCircle => {
            p.circle_filled(c, 6.8 * u, color);
            let st2 = Stroke::new(w * 1.1, Color32::from_rgb(0x10, 0x14, 0x1c));
            p.add(Shape::line(vec![pt(5.6, 5.6), pt(10.4, 10.4)], st2));
            p.add(Shape::line(vec![pt(10.4, 5.6), pt(5.6, 10.4)], st2));
        }
        Icon::PendingCircle => {
            p.circle_stroke(c, 6.0 * u, Stroke::new(w * 1.2, color));
            p.circle_filled(c, 2.4 * u, color);
        }
        Icon::Skip => {
            p.circle_stroke(c, 6.0 * u, st);
            line(&[pt(4.0, 12.0), pt(12.0, 4.0)]);
        }
        _ => {}
    }
}
