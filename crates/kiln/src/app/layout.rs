//! 터미널 분할 트리.

use egui::{Rect, pos2};
use serde::{Deserialize, Serialize};

pub type PaneId = u64;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    /// 좌우로 나눈다.
    Horizontal,
    /// 위아래로 나눈다.
    Vertical,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Node {
    Leaf(PaneId),
    Split { dir: Dir, ratio: f32, a: Box<Node>, b: Box<Node> },
}

pub const SPLITTER: f32 = 4.0;

impl Node {
    /// Balanced two-column workspace without discarding or recreating sessions.
    pub fn grid(panes: &[PaneId]) -> Option<Self> {
        if panes.len()<3 {return Self::arranged(panes,Dir::Horizontal);}
        let mid=panes.len().div_ceil(2);
        Some(Self::Split{dir:Dir::Horizontal,ratio:0.5,
            a:Box::new(Self::arranged(&panes[..mid],Dir::Vertical)?),
            b:Box::new(Self::arranged(&panes[mid..],Dir::Vertical)?)})
    }
    /// Equal-area strips, preserving every pane and its order.
    pub fn arranged(panes: &[PaneId], dir: Dir) -> Option<Self> {
        let (&first, rest) = panes.split_first()?;
        Some(if rest.is_empty() { Self::Leaf(first) } else {
            Self::Split { dir, ratio: 1.0 / panes.len() as f32,
                a: Box::new(Self::Leaf(first)), b: Box::new(Self::arranged(rest, dir)?) }
        })
    }

    pub fn panes(&self) -> Vec<PaneId> {
        let mut v = Vec::new();
        self.collect(&mut v);
        v
    }

    fn collect(&self, v: &mut Vec<PaneId>) {
        match self {
            Node::Leaf(p) => v.push(*p),
            Node::Split { a, b, .. } => {
                a.collect(v);
                b.collect(v);
            }
        }
    }

    /// `target` 을 `dir` 방향으로 나누고 새 창을 뒤쪽(오른쪽/아래)에 둔다.
    pub fn split(&mut self, target: PaneId, dir: Dir, new: PaneId) -> bool {
        match self {
            Node::Leaf(p) if *p == target => {
                *self = Node::Split { dir, ratio: 0.5, a: Box::new(Node::Leaf(target)), b: Box::new(Node::Leaf(new)) };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => a.split(target, dir, new) || b.split(target, dir, new),
        }
    }

    /// 창을 제거한다. 트리에 창이 하나도 남지 않으면 false.
    pub fn remove(&mut self, target: PaneId) -> bool {
        match self {
            Node::Leaf(p) => *p != target,
            Node::Split { a, b, .. } => {
                if matches!(**a, Node::Leaf(p) if p == target) {
                    *self = (**b).clone();
                    return true;
                }
                if matches!(**b, Node::Leaf(p) if p == target) {
                    *self = (**a).clone();
                    return true;
                }
                a.remove(target) && b.remove(target)
            }
        }
    }

    /// 각 창의 영역을 계산한다.
    pub fn layout(&self, rect: Rect, out: &mut Vec<(PaneId, Rect)>) {
        self.layout_with_gap(rect, SPLITTER, out);
    }

    pub fn layout_with_gap(&self, rect: Rect, gap: f32, out: &mut Vec<(PaneId, Rect)>) {
        match self {
            Node::Leaf(p) => out.push((*p, rect)),
            Node::Split { dir, ratio, a, b } => {
                let (ra, rb) = split_rect_with_gap(rect, *dir, *ratio, gap);
                a.layout_with_gap(ra, gap, out);
                b.layout_with_gap(rb, gap, out);
            }
        }
    }

    /// 분할선 영역들(경로 id, 방향, 분할선 rect, 부모 rect).
    pub fn splitters(&self, rect: Rect, path: u64, out: &mut Vec<(u64, Dir, Rect, Rect)>) {
        self.splitters_with_gap(rect, path, SPLITTER, out);
    }

    pub fn splitters_with_gap(&self, rect: Rect, path: u64, gap: f32, out: &mut Vec<(u64, Dir, Rect, Rect)>) {
        if let Node::Split { dir, ratio, a, b } = self {
            let (ra, rb) = split_rect_with_gap(rect, *dir, *ratio, gap);
            let sep = match dir {
                Dir::Horizontal => Rect::from_min_max(pos2(ra.right(), rect.top()), pos2(rb.left(), rect.bottom())),
                Dir::Vertical => Rect::from_min_max(pos2(rect.left(), ra.bottom()), pos2(rect.right(), rb.top())),
            };
            out.push((path, *dir, sep, rect));
            a.splitters_with_gap(ra, path * 2 + 1, gap, out);
            b.splitters_with_gap(rb, path * 2 + 2, gap, out);
        }
    }

    pub fn set_ratio(&mut self, path: u64, value: f32) {
        self.set_ratio_at(path, 0, value);
    }

    fn set_ratio_at(&mut self, path: u64, here: u64, value: f32) {
        if let Node::Split { ratio, a, b, .. } = self {
            if here == path {
                *ratio = value.clamp(0.08, 0.92);
                return;
            }
            a.set_ratio_at(path, here * 2 + 1, value);
            b.set_ratio_at(path, here * 2 + 2, value);
        }
    }

    /// 모든 분할 비율을 0.5 로 되돌린다.
    pub fn equalize(&mut self) {
        if let Node::Split { ratio, a, b, .. } = self {
            *ratio = 0.5;
            a.equalize();
            b.equalize();
        }
    }
}

#[allow(dead_code)]
pub fn split_rect(rect: Rect, dir: Dir, ratio: f32) -> (Rect, Rect) {
    split_rect_with_gap(rect, dir, ratio, SPLITTER)
}

pub fn split_rect_with_gap(rect: Rect, dir: Dir, ratio: f32, gap: f32) -> (Rect, Rect) {
    let gap = gap.clamp(2.0, 16.0).min(rect.width().min(rect.height()).max(0.0));
    match dir {
        Dir::Horizontal => {
            let x = rect.left() + (rect.width() - gap) * ratio;
            (Rect::from_min_max(rect.min, pos2(x, rect.bottom())), Rect::from_min_max(pos2(x + gap, rect.top()), rect.max))
        }
        Dir::Vertical => {
            let y = rect.top() + (rect.height() - gap) * ratio;
            (Rect::from_min_max(rect.min, pos2(rect.right(), y)), Rect::from_min_max(pos2(rect.left(), y + gap), rect.max))
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Nav {
    Left,
    Right,
    Up,
    Down,
}

/// 현재 창에서 `nav` 방향으로 가장 가까운 창.
pub fn neighbor(rects: &[(PaneId, Rect)], from: PaneId, nav: Nav) -> Option<PaneId> {
    let cur = rects.iter().find(|(p, _)| *p == from)?.1;
    let c = cur.center();
    rects
        .iter()
        .filter(|(p, _)| *p != from)
        .filter(|(_, r)| match nav {
            Nav::Left => r.right() <= cur.left() + 1.0 && r.top() < cur.bottom() && r.bottom() > cur.top(),
            Nav::Right => r.left() >= cur.right() - 1.0 && r.top() < cur.bottom() && r.bottom() > cur.top(),
            Nav::Up => r.bottom() <= cur.top() + 1.0 && r.left() < cur.right() && r.right() > cur.left(),
            Nav::Down => r.top() >= cur.bottom() - 1.0 && r.left() < cur.right() && r.right() > cur.left(),
        })
        .min_by(|(_, a), (_, b)| a.center().distance(c).total_cmp(&b.center().distance(c)))
        .map(|(p, _)| *p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_and_remove_collapse_tree() {
        let mut n = Node::Leaf(1);
        assert!(n.split(1, Dir::Horizontal, 2));
        assert!(n.split(2, Dir::Vertical, 3));
        assert_eq!(n.panes(), vec![1, 2, 3]);
        assert!(n.remove(2));
        assert_eq!(n.panes(), vec![1, 3]);
        assert!(n.remove(1));
        assert_eq!(n, Node::Leaf(3));
        assert!(!n.remove(3));
    }

    #[test]
    fn layout_and_navigation() {
        let mut n = Node::Leaf(1);
        n.split(1, Dir::Horizontal, 2);
        n.split(2, Dir::Vertical, 3);
        let mut rects = vec![];
        n.layout(Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(1000.0, 600.0)), &mut rects);
        assert_eq!(neighbor(&rects, 1, Nav::Right), Some(2));
        assert_eq!(neighbor(&rects, 2, Nav::Down), Some(3));
        assert_eq!(neighbor(&rects, 3, Nav::Left), Some(1));
        assert_eq!(neighbor(&rects, 1, Nav::Left), None);
    }

    #[test]
    fn ratio_is_clamped_by_path() {
        let mut n = Node::Leaf(1);
        n.split(1, Dir::Horizontal, 2);
        n.set_ratio(0, 0.99);
        match n {
            Node::Split { ratio, .. } => assert!((ratio - 0.92).abs() < 1e-6),
            _ => panic!(),
        }
    }
}

#[cfg(test)] mod gap_regression {
    use super::*;
    #[test] fn gap_setting_changes_geometry_and_drag_target_together() {
        let mut n=Node::Leaf(1); n.split(1,Dir::Horizontal,2);
        let area=Rect::from_min_size(pos2(0.,0.),egui::vec2(800.,400.));
        let mut panes=vec![]; n.layout_with_gap(area,12.,&mut panes);
        assert_eq!(panes[1].1.left()-panes[0].1.right(),12.);
        let mut splitters=vec![];n.splitters_with_gap(area,0,12.,&mut splitters);
        assert_eq!(splitters[0].2.width(),12.);
        assert_eq!(splitters[0].2.left(),panes[0].1.right());
    }
}
