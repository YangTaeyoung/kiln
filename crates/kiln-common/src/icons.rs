//! 벡터로 그리는 UI 아이콘(폰트 글리프 유무와 무관하게 동일하게 보인다).

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Menu,
    More,
    Folder,
    Search,
    Branch,
    PullRequest,
    GitHub,
    Codex,
    Claude,
    Database,
    Gear,
    Command,
    SplitRight,
    SplitDown,
    Plus,
    Close,
    Terminal,
    Bell,
    Warning,
    Sidebar,
    Inspector,
    Maximize,
    Restore,
    File,
    ChevronDown,
    Sparkle,
    ChevronRight,
    Refresh,
    Check,
    Trash,
    Table,
    Filter,
    Play,
    Stop,
    Copy,
    Pencil,
    Key,
    Eye,
    Download,
    Upload,
    ArrowUp,
    ArrowDown,
    Undo,
    Save,
    Code,
    Plug,
    Columns,
    Minus,
    History,
    Info,
    /// 열린 이슈(원 안의 점).
    Issue,
    /// 닫힌 이슈(원 안의 체크).
    IssueClosed,
    /// GitHub Actions(원 안의 재생 삼각형).
    Actions,
    /// 저장소(책).
    Repo,
    Star,
    /// 라벨(태그).
    Tag,
    /// 댓글(말풍선).
    Comment,
    /// 사람(담당자).
    Person,
    Lock,
}

pub fn paint(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height()) / 18.0;
    let st = Stroke::new(1.5 * s.max(0.8), color);
    let at = |x: f32, y: f32| -> Pos2 { pos2(c.x + x * s, c.y + y * s) };
    match icon {
        Icon::GitHub => paint_github(p, rect, color),
        Icon::Codex => paint_agent_mark(p, rect, color, "codex", include_str!("../assets/mark-codex.svg")),
        Icon::Claude => paint_agent_mark(p, rect, color, "claude", include_str!("../assets/mark-claude.svg")),
        Icon::More => {
            for x in [-5.0, 0.0, 5.0] { p.circle_filled(at(x, 0.0), 1.2 * s, color); }
        }
        Icon::Menu => {
            for y in [-5.0, 0.0, 5.0] {
                p.line_segment([at(-6.5, y), at(6.5, y)], st);
            }
        }
        Icon::Folder => {
            let pts = vec![at(-8.0, -5.5), at(-3.0, -5.5), at(-1.0, -3.5), at(8.0, -3.5), at(8.0, 6.5), at(-8.0, 6.5)];
            p.add(Shape::closed_line(pts, st));
        }
        Icon::Search => {
            p.circle_stroke(at(-1.5, -1.5), 5.5 * s, st);
            p.line_segment([at(2.5, 2.5), at(7.5, 7.5)], Stroke::new(st.width * 1.3, color));
        }
        Icon::Branch => {
            p.circle_stroke(at(-4.0, -6.0), 2.2 * s, st);
            p.circle_stroke(at(-4.0, 6.5), 2.2 * s, st);
            p.circle_stroke(at(5.0, -3.0), 2.2 * s, st);
            p.line_segment([at(-4.0, -3.8), at(-4.0, 4.3)], st);
            p.add(Shape::line(vec![at(5.0, -0.8), at(5.0, 1.0), at(0.0, 3.0), at(-4.0, 4.3)], st));
        }
        Icon::PullRequest => {
            p.circle_stroke(at(-5.0, -6.0), 2.2 * s, st);
            p.circle_stroke(at(-5.0, 6.5), 2.2 * s, st);
            p.circle_stroke(at(5.0, 6.5), 2.2 * s, st);
            p.line_segment([at(-5.0, -3.8), at(-5.0, 4.3)], st);
            p.add(Shape::line(vec![at(5.0, 4.3), at(5.0, -4.0), at(1.0, -6.0)], st));
            p.add(Shape::line(vec![at(3.0, -8.2), at(0.8, -6.0), at(3.0, -3.8)], st));
        }
        Icon::Database => {
            let w = 7.0 * s;
            let h = 2.6 * s;
            let top = at(0.0, -5.5);
            p.add(Shape::ellipse_stroke(top, vec2(w, h), st));
            p.line_segment([at(-7.0, -5.5), at(-7.0, 5.5)], st);
            p.line_segment([at(7.0, -5.5), at(7.0, 5.5)], st);
            let arc = |y: f32| -> Vec<Pos2> {
                (0..=16).map(|i| {
                    let t = std::f32::consts::PI * i as f32 / 16.0;
                    pos2(c.x - w * t.cos(), c.y + y * s + h * t.sin())
                }).collect()
            };
            p.add(Shape::line(arc(0.0), st));
            p.add(Shape::line(arc(5.5), st));
        }
        Icon::Gear => {
            p.circle_stroke(c, 3.0 * s, st);
            for i in 0..8 {
                let a = std::f32::consts::TAU * i as f32 / 8.0;
                let (sn, cs) = a.sin_cos();
                p.line_segment([pos2(c.x + cs * 5.2 * s, c.y + sn * 5.2 * s), pos2(c.x + cs * 7.6 * s, c.y + sn * 7.6 * s)], Stroke::new(st.width * 1.4, color));
            }
            p.circle_stroke(c, 5.4 * s, st);
        }
        Icon::Command => {
            let r = 2.2 * s;
            for (x, y) in [(-4.0, -4.0), (4.0, -4.0), (-4.0, 4.0), (4.0, 4.0)] {
                p.circle_stroke(at(x + if x < 0.0 { -r / s } else { r / s }, y + if y < 0.0 { -r / s } else { r / s }), r, st);
            }
            p.rect_stroke(Rect::from_center_size(c, vec2(8.0 * s, 8.0 * s)), 0.0, st, egui::StrokeKind::Middle);
        }
        Icon::SplitRight => {
            let r = Rect::from_center_size(c, vec2(15.0 * s, 12.0 * s));
            p.rect_stroke(r, 2.0 * s, st, egui::StrokeKind::Middle);
            p.line_segment([pos2(c.x, r.top()), pos2(c.x, r.bottom())], st);
        }
        Icon::SplitDown => {
            let r = Rect::from_center_size(c, vec2(15.0 * s, 12.0 * s));
            p.rect_stroke(r, 2.0 * s, st, egui::StrokeKind::Middle);
            p.line_segment([pos2(r.left(), c.y), pos2(r.right(), c.y)], st);
        }
        Icon::Plus => {
            p.line_segment([at(-5.5, 0.0), at(5.5, 0.0)], st);
            p.line_segment([at(0.0, -5.5), at(0.0, 5.5)], st);
        }
        Icon::Close => {
            p.line_segment([at(-4.0, -4.0), at(4.0, 4.0)], st);
            p.line_segment([at(4.0, -4.0), at(-4.0, 4.0)], st);
        }
        Icon::Terminal => {
            p.add(Shape::line(vec![at(-7.0, -4.0), at(-2.5, 0.0), at(-7.0, 4.0)], st));
            p.line_segment([at(0.0, 5.0), at(7.0, 5.0)], st);
        }
        Icon::Sidebar => {
            let r = Rect::from_center_size(c, vec2(16.0 * s, 13.0 * s));
            p.rect_stroke(r, 2.5 * s, st, egui::StrokeKind::Middle);
            p.line_segment([pos2(r.left() + 5.5 * s, r.top()), pos2(r.left() + 5.5 * s, r.bottom())], st);
        }
        Icon::Inspector => {
            let r = Rect::from_center_size(c, vec2(16.0 * s, 13.0 * s));
            p.rect_stroke(r, 2.5 * s, st, egui::StrokeKind::Middle);
            p.line_segment([pos2(r.right() - 5.5 * s, r.top()), pos2(r.right() - 5.5 * s, r.bottom())], st);
        }
        Icon::Maximize => {
            p.add(Shape::line(vec![at(1.5, -7.0), at(7.0, -7.0), at(7.0, -1.5)], st));
            p.add(Shape::line(vec![at(-1.5, 7.0), at(-7.0, 7.0), at(-7.0, 1.5)], st));
            p.line_segment([at(7.0, -7.0), at(2.0, -2.0)], st);
            p.line_segment([at(-7.0, 7.0), at(-2.0, 2.0)], st);
        }
        Icon::Restore => {
            p.add(Shape::line(vec![at(-1.0, -6.5), at(-1.0, -1.0), at(-6.5, -1.0)], st));
            p.add(Shape::line(vec![at(1.0, 6.5), at(1.0, 1.0), at(6.5, 1.0)], st));
            p.line_segment([at(-1.0, -1.0), at(-6.5, -6.5)], st);
            p.line_segment([at(1.0, 1.0), at(6.5, 6.5)], st);
        }
        Icon::File => {
            p.add(Shape::closed_line(vec![at(-5.5, -8.0), at(2.0, -8.0), at(6.0, -4.0), at(6.0, 8.0), at(-5.5, 8.0)], st));
            p.add(Shape::line(vec![at(2.0, -8.0), at(2.0, -4.0), at(6.0, -4.0)], st));
        }
        Icon::ChevronDown => {
            p.add(Shape::line(vec![at(-5.0, -2.0), at(0.0, 3.0), at(5.0, -2.0)], st));
        }
        Icon::Sparkle => {
            let pts = vec![at(0.0, -8.0), at(2.0, -2.0), at(8.0, 0.0), at(2.0, 2.0), at(0.0, 8.0), at(-2.0, 2.0), at(-8.0, 0.0), at(-2.0, -2.0)];
            p.add(Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        Icon::Warning => {
            p.add(Shape::closed_line(vec![at(0.0, -7.5), at(8.0, 6.5), at(-8.0, 6.5)], st));
            p.line_segment([at(0.0, -2.5), at(0.0, 2.0)], st);
            p.circle_filled(at(0.0, 4.3), 0.9 * s, color);
        }
        Icon::ChevronRight => {
            p.add(Shape::line(vec![at(-2.0, -5.0), at(3.0, 0.0), at(-2.0, 5.0)], st));
        }
        Icon::Refresh => {
            let pts: Vec<Pos2> = (0..=20)
                .map(|i| {
                    let a = -0.35 * std::f32::consts::PI + 1.55 * std::f32::consts::PI * i as f32 / 20.0;
                    pos2(c.x + a.cos() * 6.0 * s, c.y + a.sin() * 6.0 * s)
                })
                .collect();
            let end = *pts.first().unwrap_or(&c);
            p.add(Shape::line(pts, st));
            p.add(Shape::line(vec![pos2(end.x - 3.5 * s, end.y - 1.0 * s), end, pos2(end.x + 0.5 * s, end.y - 3.8 * s)], st));
        }
        Icon::Check => {
            p.add(Shape::line(vec![at(-6.0, 0.5), at(-2.0, 4.5), at(6.5, -4.5)], st));
        }
        Icon::Trash => {
            p.line_segment([at(-7.0, -4.5), at(7.0, -4.5)], st);
            p.add(Shape::line(vec![at(-2.5, -4.5), at(-2.0, -7.0), at(2.0, -7.0), at(2.5, -4.5)], st));
            p.add(Shape::line(vec![at(-5.5, -4.5), at(-4.5, 7.5), at(4.5, 7.5), at(5.5, -4.5)], st));
            p.line_segment([at(-1.5, -1.5), at(-1.3, 4.5)], st);
            p.line_segment([at(1.5, -1.5), at(1.3, 4.5)], st);
        }
        Icon::Table => {
            let r = Rect::from_center_size(c, vec2(15.0 * s, 13.0 * s));
            p.rect_stroke(r, 2.0 * s, st, egui::StrokeKind::Middle);
            p.line_segment([pos2(r.left(), r.top() + 4.0 * s), pos2(r.right(), r.top() + 4.0 * s)], st);
            p.line_segment([pos2(r.left(), r.top() + 8.5 * s), pos2(r.right(), r.top() + 8.5 * s)], st);
            p.line_segment([pos2(c.x - 1.5 * s, r.top() + 4.0 * s), pos2(c.x - 1.5 * s, r.bottom())], st);
        }
        Icon::Filter => {
            p.add(Shape::closed_line(vec![at(-7.0, -6.0), at(7.0, -6.0), at(1.5, 0.5), at(1.5, 6.5), at(-1.5, 5.0), at(-1.5, 0.5)], st));
        }
        Icon::Play => {
            p.add(Shape::convex_polygon(vec![at(-4.0, -6.5), at(6.5, 0.0), at(-4.0, 6.5)], color, Stroke::NONE));
        }
        Icon::Stop => {
            p.rect_filled(Rect::from_center_size(c, vec2(11.0 * s, 11.0 * s)), 2.0 * s, color);
        }
        Icon::Copy => {
            p.rect_stroke(Rect::from_min_max(at(-2.5, -2.5), at(7.0, 7.5)), 2.0 * s, st, egui::StrokeKind::Middle);
            p.add(Shape::line(vec![at(-4.5, 4.5), at(-7.0, 4.5), at(-7.0, -7.0), at(4.5, -7.0), at(4.5, -4.5)], st));
        }
        Icon::Pencil => {
            p.add(Shape::closed_line(vec![at(3.5, -7.0), at(7.0, -3.5), at(-3.5, 7.0), at(-7.0, 7.0), at(-7.0, 3.5)], st));
            p.line_segment([at(1.0, -4.5), at(4.5, -1.0)], st);
        }
        Icon::Key => {
            p.circle_stroke(at(-3.5, 3.5), 3.5 * s, st);
            p.line_segment([at(-1.0, 1.0), at(6.5, -6.5)], st);
            p.line_segment([at(4.0, -4.0), at(6.5, -1.5)], st);
            p.line_segment([at(2.0, -2.0), at(3.8, -0.2)], st);
        }
        Icon::Eye => {
            let top: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let x = -8.0 + 16.0 * i as f32 / 16.0;
                    at(x, -5.0 * (1.0 - (x / 8.0).powi(2)))
                })
                .collect();
            let bot: Vec<Pos2> = top.iter().rev().map(|q| pos2(q.x, 2.0 * c.y - q.y)).collect();
            p.add(Shape::closed_line(top.into_iter().chain(bot).collect(), st));
            p.circle_stroke(c, 2.3 * s, st);
        }
        Icon::Download => {
            p.line_segment([at(0.0, -7.0), at(0.0, 3.0)], st);
            p.add(Shape::line(vec![at(-4.0, -1.0), at(0.0, 3.0), at(4.0, -1.0)], st));
            p.add(Shape::line(vec![at(-7.0, 3.5), at(-7.0, 7.0), at(7.0, 7.0), at(7.0, 3.5)], st));
        }
        Icon::Upload => {
            p.line_segment([at(0.0, 3.0), at(0.0, -7.0)], st);
            p.add(Shape::line(vec![at(-4.0, -3.0), at(0.0, -7.0), at(4.0, -3.0)], st));
            p.add(Shape::line(vec![at(-7.0, 3.5), at(-7.0, 7.0), at(7.0, 7.0), at(7.0, 3.5)], st));
        }
        Icon::ArrowUp => {
            p.line_segment([at(0.0, 6.5), at(0.0, -6.5)], st);
            p.add(Shape::line(vec![at(-5.0, -1.5), at(0.0, -6.5), at(5.0, -1.5)], st));
        }
        Icon::ArrowDown => {
            p.line_segment([at(0.0, -6.5), at(0.0, 6.5)], st);
            p.add(Shape::line(vec![at(-5.0, 1.5), at(0.0, 6.5), at(5.0, 1.5)], st));
        }
        Icon::Undo => {
            let pts: Vec<Pos2> = (0..=14)
                .map(|i| {
                    let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / 14.0;
                    pos2(c.x + 1.0 * s + a.cos() * 5.0 * s, c.y + 0.5 * s + a.sin() * 5.0 * s)
                })
                .collect();
            p.add(Shape::line(pts, st));
            p.line_segment([at(1.0, -4.5), at(-6.0, -4.5)], st);
            p.add(Shape::line(vec![at(-3.0, -7.5), at(-6.0, -4.5), at(-3.0, -1.5)], st));
            p.line_segment([at(1.0, 5.5), at(-3.0, 5.5)], st);
        }
        Icon::Save => {
            p.add(Shape::closed_line(vec![at(-7.0, -7.0), at(4.0, -7.0), at(7.0, -4.0), at(7.0, 7.0), at(-7.0, 7.0)], st));
            p.add(Shape::line(vec![at(-4.0, -7.0), at(-4.0, -3.0), at(3.0, -3.0), at(3.0, -7.0)], st));
            p.rect_stroke(Rect::from_min_max(at(-4.0, 1.5), at(4.0, 7.0)), 0.0, st, egui::StrokeKind::Middle);
        }
        Icon::Code => {
            p.add(Shape::line(vec![at(-3.5, -5.5), at(-8.0, 0.0), at(-3.5, 5.5)], st));
            p.add(Shape::line(vec![at(3.5, -5.5), at(8.0, 0.0), at(3.5, 5.5)], st));
            p.line_segment([at(1.5, -7.0), at(-1.5, 7.0)], st);
        }
        Icon::Plug => {
            p.add(Shape::line(vec![at(-5.0, -3.0), at(-5.0, 1.5), at(-2.5, 4.5), at(2.5, 4.5), at(5.0, 1.5), at(5.0, -3.0)], st));
            p.line_segment([at(-6.5, -3.0), at(6.5, -3.0)], st);
            p.line_segment([at(-2.5, -3.0), at(-2.5, -7.5)], st);
            p.line_segment([at(2.5, -3.0), at(2.5, -7.5)], st);
            p.line_segment([at(0.0, 4.5), at(0.0, 8.0)], st);
        }
        Icon::Columns => {
            let r = Rect::from_center_size(c, vec2(15.0 * s, 13.0 * s));
            p.rect_stroke(r, 2.0 * s, st, egui::StrokeKind::Middle);
            p.line_segment([pos2(r.left() + r.width() / 3.0, r.top()), pos2(r.left() + r.width() / 3.0, r.bottom())], st);
            p.line_segment([pos2(r.left() + r.width() * 2.0 / 3.0, r.top()), pos2(r.left() + r.width() * 2.0 / 3.0, r.bottom())], st);
        }
        Icon::Minus => {
            p.line_segment([at(-5.5, 0.0), at(5.5, 0.0)], st);
        }
        Icon::History => {
            p.circle_stroke(c, 7.0 * s, st);
            p.add(Shape::line(vec![at(0.0, -4.0), at(0.0, 0.0), at(3.0, 2.0)], st));
        }
        Icon::Info => {
            p.circle_stroke(c, 7.0 * s, st);
            p.line_segment([at(0.0, -0.5), at(0.0, 4.0)], st);
            p.circle_filled(at(0.0, -3.5), 0.9 * s, color);
        }
        Icon::Issue => {
            p.circle_stroke(c, 7.0 * s, st);
            p.circle_filled(c, 1.8 * s, color);
        }
        Icon::IssueClosed => {
            p.circle_stroke(c, 7.0 * s, st);
            p.add(Shape::line(vec![at(-3.2, 0.2), at(-0.8, 2.6), at(3.4, -2.2)], st));
        }
        Icon::Actions => {
            p.circle_stroke(c, 7.0 * s, st);
            p.add(Shape::convex_polygon(vec![at(-2.0, -3.6), at(3.8, 0.0), at(-2.0, 3.6)], color, Stroke::NONE));
        }
        Icon::Repo => {
            p.add(Shape::line(vec![at(-6.0, 5.0), at(-6.0, -5.5), at(-4.0, -7.0), at(6.0, -7.0), at(6.0, 4.0), at(-4.0, 4.0), at(-6.0, 5.5), at(-4.0, 7.0), at(6.0, 7.0)], st));
            p.line_segment([at(-3.0, -7.0), at(-3.0, 4.0)], st);
        }
        Icon::Star => {
            let pts: Vec<Pos2> = (0..10)
                .map(|i| {
                    let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / 5.0;
                    let r = if i % 2 == 0 { 7.5 } else { 3.3 };
                    at(r * a.cos(), r * a.sin() + 0.6)
                })
                .collect();
            p.add(Shape::closed_line(pts, st));
        }
        Icon::Tag => {
            p.add(Shape::closed_line(vec![at(-7.0, -7.0), at(0.5, -7.0), at(7.5, 0.0), at(0.0, 7.5), at(-7.0, 0.5)], st));
            p.circle_filled(at(-3.3, -3.3), 1.3 * s, color);
        }
        Icon::Comment => {
            p.add(Shape::closed_line(
                vec![at(-7.5, -6.0), at(7.5, -6.0), at(7.5, 3.5), at(-1.0, 3.5), at(-4.5, 7.0), at(-4.5, 3.5), at(-7.5, 3.5)],
                st,
            ));
        }
        Icon::Person => {
            p.circle_stroke(at(0.0, -3.5), 3.3 * s, st);
            let arc: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let a = std::f32::consts::PI + std::f32::consts::PI * i as f32 / 16.0;
                    at(6.5 * a.cos(), 7.5 + 5.5 * a.sin())
                })
                .collect();
            p.add(Shape::line(arc, st));
        }
        Icon::Lock => {
            p.rect_stroke(Rect::from_min_max(at(-6.0, -1.0), at(6.0, 7.5)), 1.5 * s, st, egui::StrokeKind::Middle);
            let arc: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let a = std::f32::consts::PI + std::f32::consts::PI * i as f32 / 16.0;
                    at(3.8 * a.cos(), -3.5 + 3.8 * a.sin())
                })
                .collect();
            let mut pts = vec![at(-3.8, -1.0)];
            pts.extend(arc);
            pts.push(at(3.8, -1.0));
            p.add(Shape::line(pts, st));
        }
        Icon::Bell => {
            let pts: Vec<Pos2> = vec![at(-6.0, 4.0), at(-4.5, 2.0), at(-4.5, -2.5), at(-2.0, -6.0), at(2.0, -6.0), at(4.5, -2.5), at(4.5, 2.0), at(6.0, 4.0)];
            p.add(Shape::closed_line(pts, st));
            p.line_segment([at(-1.5, 6.5), at(1.5, 6.5)], st);
        }
    }
}

/// 아이콘 버튼. 호버 배경과 툴팁 포함.
pub fn button(ui: &mut egui::Ui, icon: Icon, size: egui::Vec2, color: Color32, hover_bg: Color32, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect.shrink(1.0), 4.0, hover_bg);
    }
    let icon_rect = Rect::from_center_size(rect.center(), egui::Vec2::splat(size.y.min(size.x) * 0.62));
    paint(ui.painter(), icon_rect, icon, color);
    if tip.is_empty() { resp } else { resp.on_hover_text(tip) }
}

// GitHub's official Octicons mark (MIT); see assets/OCTICONS-LICENSE.
fn paint_github(p: &Painter, rect: Rect, color: Color32) {
    let px = (rect.width().min(rect.height()) * p.ctx().pixels_per_point()).ceil().clamp(8.0, 256.0) as u32;
    let id = egui::Id::new(("github-mark", px));
    let texture = p.ctx().data_mut(|d| d.get_temp::<egui::TextureHandle>(id));
    let texture = texture.unwrap_or_else(|| {
        let svg = include_str!("../assets/mark-github-16.svg").replace("<svg ", "<svg fill=\"white\" ");
        let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).expect("bundled GitHub mark");
        let mut pixels = resvg::tiny_skia::Pixmap::new(px, px).expect("bounded icon size");
        resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(px as f32 / 16.0, px as f32 / 16.0), &mut pixels.as_mut());
        let image = egui::ColorImage::from_rgba_premultiplied([px as usize, px as usize], pixels.data());
        let texture = p.ctx().load_texture("github-mark", image, egui::TextureOptions::LINEAR);
        p.ctx().data_mut(|d| d.insert_temp(id, texture.clone()));
        texture
    });
    p.image(texture.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), color);
}

fn paint_agent_mark(p: &Painter, rect: Rect, color: Color32, name: &'static str, svg: &str) {
    let px = (rect.width().min(rect.height()) * p.ctx().pixels_per_point()).ceil().clamp(8.0, 256.0) as u32;
    let id = egui::Id::new(("agent-mark", name, px));
    let texture = p.ctx().data_mut(|data| data.get_temp::<egui::TextureHandle>(id)).unwrap_or_else(|| {
        let tree = resvg::usvg::Tree::from_str(&svg.replace("currentColor", "white"), &resvg::usvg::Options::default()).expect("bundled agent mark");
        let mut pixels = resvg::tiny_skia::Pixmap::new(px, px).expect("bounded icon size");
        resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(px as f32 / tree.size().width(), px as f32 / tree.size().height()), &mut pixels.as_mut());
        let image = egui::ColorImage::from_rgba_premultiplied([px as usize, px as usize], pixels.data());
        let texture = p.ctx().load_texture(name, image, egui::TextureOptions::LINEAR);
        p.ctx().data_mut(|data| data.insert_temp(id, texture.clone()));
        texture
    });
    p.image(texture.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), color);
}
