//! 터미널 분할 트리.

use egui::{Pos2, Rect, pos2};
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::collections::HashSet;

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
    Split {
        dir: Dir,
        ratio: f32,
        a: Box<Node>,
        b: Box<Node>,
    },
}

pub const SPLITTER: f32 = 4.0;

/// The side of the destination pane where an existing pane is inserted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveSide {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Clone, Debug)]
pub struct AlignmentGuide {
    pub dir: Dir,
    /// Divider center in workspace coordinates, including half the splitter gap.
    pub position: f32,
    pub label: &'static str,
}

#[derive(Clone, Debug)]
struct Divider {
    path: u64,
    dir: Dir,
    parent: Rect,
    position: f32,
    min: f32,
    max: f32,
}

/// Geometry is captured once at drag start. Do not re-create this each frame:
/// aligned dividers must remain linked even after leaving their original axis.
#[derive(Clone, Debug)]
pub struct ResizeDrag {
    pub dir: Dir,
    pub position: f32,
    viewport: Rect,
    gap: f32,
    active: Divider,
    linked: Vec<Divider>,
    peers: Vec<Divider>,
    prepared: Option<Box<Node>>,
    snap_target: Cell<Option<(f32, &'static str)>>,
}

#[derive(Clone, Debug)]
pub struct ResizeResult {
    pub position: f32,
    pub guides: Vec<AlignmentGuide>,
    pub changed: bool,
    pub snapped: bool,
}

const SNAP_DISTANCE: f32 = 18.0;
const SNAP_RELEASE: f32 = 30.0;
const JOIN_DISTANCE: f32 = 64.0;
const MIN_PANE_EXTENT: f32 = 72.0;
const FRACTIONS: [(f32, &str); 5] = [
    (0.25, "1/4"),
    (1.0 / 3.0, "1/3"),
    (0.5, "1/2"),
    (2.0 / 3.0, "2/3"),
    (0.75, "3/4"),
];

impl ResizeDrag {
    pub fn path(&self) -> u64 {
        self.active.path
    }

    pub fn is_linked(&self) -> bool {
        self.linked.len() > 1
    }

    pub fn affected_rect(&self, independent: bool) -> Rect {
        if independent {
            return self.active.parent;
        }
        self.linked
            .iter()
            .fold(self.active.parent, |r, d| r.union(d.parent))
    }

    /// One continuous highlight, including the gap between linked segments.
    pub fn boundary_rect(&self, independent: bool) -> Rect {
        self.boundary_rect_at(self.position, independent)
    }

    pub fn boundary_rect_at(&self, position: f32, independent: bool) -> Rect {
        let region = self.affected_rect(independent);
        match self.dir {
            Dir::Horizontal => Rect::from_min_max(
                pos2(position - self.gap * 0.5, region.top()),
                pos2(position + self.gap * 0.5, region.bottom()),
            ),
            Dir::Vertical => Rect::from_min_max(
                pos2(region.left(), position - self.gap * 0.5),
                pos2(region.right(), position + self.gap * 0.5),
            ),
        }
    }
}

impl Node {
    /// Balanced two-column workspace without discarding or recreating sessions.
    pub fn grid(panes: &[PaneId]) -> Option<Self> {
        if panes.len() < 3 {
            return Self::arranged(panes, Dir::Horizontal);
        }
        let mid = panes.len().div_ceil(2);
        Some(Self::Split {
            dir: Dir::Horizontal,
            ratio: 0.5,
            a: Box::new(Self::arranged(&panes[..mid], Dir::Vertical)?),
            b: Box::new(Self::arranged(&panes[mid..], Dir::Vertical)?),
        })
    }
    /// Equal-area strips, preserving every pane and its order.
    pub fn arranged(panes: &[PaneId], dir: Dir) -> Option<Self> {
        let (&first, rest) = panes.split_first()?;
        Some(if rest.is_empty() {
            Self::Leaf(first)
        } else {
            Self::Split {
                dir,
                ratio: 1.0 / panes.len() as f32,
                a: Box::new(Self::Leaf(first)),
                b: Box::new(Self::arranged(rest, dir)?),
            }
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

    /// Drop an existing pane beside another without creating/removing any PTY.
    /// Work on a copy so rejected drops never mutate the persisted layout.
    pub fn move_pane(&mut self, source: PaneId, target: PaneId, side: MoveSide) -> bool {
        let panes = self.panes();
        if source == target || !panes.contains(&source) || !panes.contains(&target) {
            return false;
        }
        let Some(mut next) = self.normalized().and_then(|n| n.without(source)) else {
            return false;
        };
        next.insert_beside(target, source, side);
        *self = next;
        true
    }

    /// Final candidate geometry, never the destination's old half rectangle.
    /// Rejected previews leave both the tree and every terminal identity intact.
    pub fn move_preview(
        &self,
        source: PaneId,
        target: PaneId,
        side: MoveSide,
        viewport: Rect,
        gap: f32,
    ) -> Option<(Node, Vec<(PaneId, Rect)>)> {
        let mut next = self.clone();
        if !viewport.is_finite() || !next.move_pane(source, target, side) {
            return None;
        }
        let mut rects = Vec::new();
        next.layout_with_gap(viewport, gap, &mut rects);
        if rects.iter().any(|(_, r)| {
            !r.is_finite() || r.width() < MIN_PANE_EXTENT || r.height() < MIN_PANE_EXTENT
        }) {
            return None;
        }
        Some((next, rects))
    }

    /// Recover duplicate leaves and invalid ratios from legacy saved layouts.
    /// Surviving panes keep their original traversal order.
    pub fn normalized(&self) -> Option<Self> {
        fn visit(node: &Node, seen: &mut HashSet<PaneId>) -> Option<Node> {
            match node {
                Node::Leaf(id) => seen.insert(*id).then_some(Node::Leaf(*id)),
                Node::Split { dir, ratio, a, b } => match (visit(a, seen), visit(b, seen)) {
                    (Some(a), Some(b)) => Some(Node::Split {
                        dir: *dir,
                        ratio: if ratio.is_finite() {
                            ratio.clamp(0.08, 0.92)
                        } else {
                            0.5
                        },
                        a: Box::new(a),
                        b: Box::new(b),
                    }),
                    (a, b) => a.or(b),
                },
            }
        }
        visit(self, &mut HashSet::new())
    }

    fn without(self, target: PaneId) -> Option<Self> {
        match self {
            Self::Leaf(id) => (id != target).then_some(Self::Leaf(id)),
            Self::Split { dir, ratio, a, b } => match (a.without(target), b.without(target)) {
                (Some(a), Some(b)) => Some(Self::Split {
                    dir,
                    ratio,
                    a: Box::new(a),
                    b: Box::new(b),
                }),
                (a, b) => a.or(b),
            },
        }
    }

    fn insert_beside(&mut self, target: PaneId, source: PaneId, side: MoveSide) -> bool {
        match self {
            Self::Leaf(id) if *id == target => {
                let (dir, before) = match side {
                    MoveSide::Left => (Dir::Horizontal, true),
                    MoveSide::Right => (Dir::Horizontal, false),
                    MoveSide::Top => (Dir::Vertical, true),
                    MoveSide::Bottom => (Dir::Vertical, false),
                };
                let (a, b) = if before {
                    (source, target)
                } else {
                    (target, source)
                };
                *self = Self::Split {
                    dir,
                    ratio: 0.5,
                    a: Box::new(Self::Leaf(a)),
                    b: Box::new(Self::Leaf(b)),
                };
                true
            }
            Self::Leaf(_) => false,
            Self::Split { a, b, .. } => {
                a.insert_beside(target, source, side) || b.insert_beside(target, source, side)
            }
        }
    }

    /// Start a resize from the actual divider center, not the raw parent ratio.
    pub fn begin_resize(
        &self,
        viewport: Rect,
        gap: f32,
        path: u64,
        link_aligned: bool,
    ) -> Option<ResizeDrag> {
        let mut peers = Vec::new();
        self.dividers(viewport, 0, gap, &mut peers);
        let active = peers.iter().find(|d| d.path == path)?.clone();
        let mut linked = vec![active.clone()];
        if link_aligned {
            loop {
                let candidates: Vec<_> = peers
                    .iter()
                    .filter(|d| {
                        d.dir == active.dir
                            && (d.position - active.position).abs() <= 1.0
                            && linked.iter().all(|known| independent(known.path, d.path))
                            && linked.iter().any(|known| {
                                adjacent_spans(known.parent, d.parent, active.dir, gap)
                            })
                    })
                    .cloned()
                    .collect();
                if candidates.is_empty() {
                    break;
                }
                linked.extend(candidates);
            }
        }
        Some(ResizeDrag {
            dir: active.dir,
            position: active.position,
            viewport,
            gap,
            active,
            linked,
            peers,
            prepared: None,
            snap_target: Cell::new(None),
        })
    }

    /// Hover and pointer-down share this non-mutating scope calculation.
    /// Away from a junction an equivalent tree rotation exposes a local
    /// segment of a continuous divider without changing any pane rectangle.
    pub fn begin_resize_at(
        &self,
        viewport: Rect,
        gap: f32,
        path: u64,
        point: Pos2,
        link_aligned: bool,
    ) -> Option<ResizeDrag> {
        let mut peers = Vec::new();
        self.dividers(viewport, 0, gap, &mut peers);
        let active = peers.iter().find(|d| d.path == path)?;
        if let Some(node) = self.node_at(path) {
            if let Some((candidate, child_dir, child_ratio)) = node.transposed() {
                let cross = axis_start(active.parent, child_dir)
                    + (axis_extent(active.parent, child_dir) - effective_gap(active.parent, gap))
                        * child_ratio
                    + effective_gap(active.parent, gap) * 0.5;
                let tangent = axis_point(point, child_dir);
                let threshold = if link_aligned { JOIN_DISTANCE } else { 20.0 };
                if (tangent - cross).abs() > threshold {
                    let mut prepared = self.clone();
                    prepared.replace_at(path, candidate);
                    let child_path = path * 2 + if tangent < cross { 1 } else { 2 };
                    let mut drag = prepared.begin_resize(viewport, gap, child_path, false)?;
                    drag.prepared = Some(Box::new(prepared));
                    return Some(drag);
                }
            }
        }
        let near_junction = peers.iter().any(|other| {
            other.path != path
                && other.dir == active.dir
                && independent(path, other.path)
                && (other.position - active.position).abs() <= 1.0
                && adjacent_spans(active.parent, other.parent, active.dir, gap)
                && (axis_point(point, opposite(active.dir))
                    - shared_tangent(active.parent, other.parent, active.dir))
                .abs()
                    <= JOIN_DISTANCE
        });
        self.begin_resize(viewport, gap, path, link_aligned && near_junction)
    }

    pub fn hover_scope(
        &self,
        viewport: Rect,
        gap: f32,
        point: Pos2,
        alt: bool,
    ) -> Option<ResizeDrag> {
        let mut splitters = Vec::new();
        self.splitters_with_gap(viewport, 0, gap, &mut splitters);
        let (path, _, _, _) = splitters
            .iter()
            .filter(|(_, _, rect, _)| rect.expand(2.0).contains(point))
            .min_by(|a, b| {
                a.2.distance_to_pos(point)
                    .total_cmp(&b.2.distance_to_pos(point))
            })?;
        self.begin_resize_at(viewport, gap, *path, point, !alt)
    }

    fn node_at(&self, path: u64) -> Option<&Node> {
        if path == 0 {
            return Some(self);
        }
        let parent = self.node_at((path - 1) / 2)?;
        match parent {
            Self::Split { a, b, .. } => Some(if path % 2 == 1 { a } else { b }),
            Self::Leaf(_) => None,
        }
    }

    fn replace_at(&mut self, path: u64, replacement: Node) {
        fn replace(node: &mut Node, path: u64, here: u64, replacement: &Node) -> bool {
            if path == here {
                *node = replacement.clone();
                return true;
            }
            match node {
                Node::Split { a, b, .. } => {
                    replace(a, path, here * 2 + 1, replacement)
                        || replace(b, path, here * 2 + 2, replacement)
                }
                Node::Leaf(_) => false,
            }
        }
        replace(self, path, 0, &replacement);
    }

    fn transposed(&self) -> Option<(Node, Dir, f32)> {
        let Self::Split { dir, ratio, a, b } = self else {
            return None;
        };
        let (
            Self::Split {
                dir: ad,
                ratio: ar,
                a: aa,
                b: ab,
            },
            Self::Split {
                dir: bd,
                ratio: br,
                a: ba,
                b: bb,
            },
        ) = (&**a, &**b)
        else {
            return None;
        };
        if ad != bd || ad == dir || (ar - br).abs() >= 0.00001 {
            return None;
        }
        Some((
            Self::Split {
                dir: *ad,
                ratio: *ar,
                a: Box::new(Self::Split {
                    dir: *dir,
                    ratio: *ratio,
                    a: aa.clone(),
                    b: ba.clone(),
                }),
                b: Box::new(Self::Split {
                    dir: *dir,
                    ratio: *ratio,
                    a: ab.clone(),
                    b: bb.clone(),
                }),
            },
            *ad,
            *ar,
        ))
    }

    /// Apply an absolute workspace divider coordinate. Hold the UI's modifier
    /// to start with `link_aligned=false` and pass `snap=false` for free resize.
    pub fn resize_drag(&mut self, drag: &ResizeDrag, requested: f32, snap: bool) -> ResizeResult {
        self.resize_drag_with_mode(drag, requested, snap, false)
    }

    /// Option can temporarily bypass the captured linked group and snap lock.
    /// Releasing it resumes the original group; proximity never changes scope.
    pub fn resize_drag_with_mode(
        &mut self,
        drag: &ResizeDrag,
        requested: f32,
        snap: bool,
        free: bool,
    ) -> ResizeResult {
        let members = if free {
            std::slice::from_ref(&drag.active)
        } else {
            &drag.linked
        };
        let min = members
            .iter()
            .map(|d| d.min)
            .fold(f32::NEG_INFINITY, f32::max);
        let max = members.iter().map(|d| d.max).fold(f32::INFINITY, f32::min);
        // Legacy over-constrained layouts have no shared legal interval. Keep
        // them stable instead of panicking or moving just half a linked axis.
        if !requested.is_finite() || min > max {
            return ResizeResult {
                position: drag.position,
                guides: Vec::new(),
                changed: false,
                snapped: false,
            };
        }
        let mut position = requested.clamp(min, max);
        let mut guides = Vec::new();
        if snap && !free {
            let mut candidates = Vec::new();
            for peer in &drag.peers {
                if peer.dir == drag.dir
                    && members.iter().all(|d| independent(d.path, peer.path))
                    && adjacent_spans(drag.active.parent, peer.parent, drag.dir, drag.gap)
                {
                    candidates.push((peer.position, ""));
                }
            }
            // Fractions are always relative to the whole terminal board.
            // Local half/quarter guides are misleading in nested layouts.
            for (fraction, label) in FRACTIONS {
                candidates.push((
                    axis_start(drag.viewport, drag.dir)
                        + axis_extent(drag.viewport, drag.dir) * fraction,
                    label,
                ));
            }
            let held = drag
                .snap_target
                .get()
                .filter(|(p, _)| *p >= min && *p <= max && (*p - requested).abs() <= SNAP_RELEASE);
            let acquired = candidates
                .into_iter()
                .filter(|(p, _)| *p >= min && *p <= max && (*p - requested).abs() <= SNAP_DISTANCE)
                .min_by(|(a, _), (b, _)| (a - requested).abs().total_cmp(&(b - requested).abs()));
            drag.snap_target.set(held.or(acquired));
            if let Some((candidate, label)) = drag.snap_target.get() {
                position = candidate;
                guides.push(AlignmentGuide {
                    dir: drag.dir,
                    position,
                    label,
                });
            }
        } else {
            drag.snap_target.set(None);
        }
        let mut changed = false;
        let before = drag.prepared.as_ref().map(|_| self.clone());
        if let Some(prepared) = &drag.prepared {
            *self = (**prepared).clone();
        }
        for d in members {
            let gap = effective_gap(d.parent, drag.gap);
            let available = axis_extent(d.parent, d.dir) - gap;
            if available > 0.0 {
                let ratio = (position - axis_start(d.parent, d.dir) - gap * 0.5) / available;
                changed |= self.update_ratio(d.path, ratio);
            }
        }
        if let Some(before) = before {
            changed = before != *self;
        }
        let snapped = !guides.is_empty();
        ResizeResult {
            position,
            guides,
            changed,
            snapped,
        }
    }

    fn update_ratio(&mut self, path: u64, value: f32) -> bool {
        fn update(node: &mut Node, path: u64, here: u64, value: f32) -> bool {
            match node {
                Node::Split { ratio, a, b, .. } => {
                    if here == path {
                        let changed = (*ratio - value).abs() > f32::EPSILON;
                        *ratio = value;
                        changed
                    } else {
                        update(a, path, here * 2 + 1, value) || update(b, path, here * 2 + 2, value)
                    }
                }
                Node::Leaf(_) => false,
            }
        }
        update(self, path, 0, value)
    }

    fn minimum_extent(&self, dir: Dir, gap: f32) -> f32 {
        match self {
            Self::Leaf(_) => MIN_PANE_EXTENT,
            Self::Split {
                dir: split_dir,
                ratio,
                a,
                b,
            } => {
                let (a, b) = (a.minimum_extent(dir, gap), b.minimum_extent(dir, gap));
                if *split_dir == dir {
                    // Resizing an ancestor preserves this split's ratio. The
                    // smaller side of a 75/25 split therefore needs four leaf
                    // minima of available space, rather than two.
                    let ratio = if ratio.is_finite() {
                        ratio.clamp(0.001, 0.999)
                    } else {
                        0.5
                    };
                    (a / ratio).max(b / (1.0 - ratio)) + gap.clamp(2.0, 16.0)
                } else {
                    a.max(b)
                }
            }
        }
    }

    fn dividers(&self, rect: Rect, path: u64, gap: f32, out: &mut Vec<Divider>) {
        if let Self::Split { dir, ratio, a, b } = self {
            let gap_px = effective_gap(rect, gap);
            let available = (axis_extent(rect, *dir) - gap_px).max(0.0);
            let (ra, rb) = split_rect_with_gap(rect, *dir, *ratio, gap);
            let position = axis_start(rect, *dir) + available * ratio + gap_px * 0.5;
            let need_a = a.minimum_extent(*dir, gap).max(available * 0.08);
            let need_b = b.minimum_extent(*dir, gap).max(available * 0.08);
            let (min_a, min_b) = if need_a + need_b > available {
                let share = need_a / (need_a + need_b);
                (available * share, available * (1.0 - share))
            } else {
                (need_a, need_b)
            };
            out.push(Divider {
                path,
                dir: *dir,
                parent: rect,
                position,
                min: axis_start(rect, *dir) + min_a + gap_px * 0.5,
                max: axis_start(rect, *dir) + available - min_b + gap_px * 0.5,
            });
            a.dividers(ra, path * 2 + 1, gap, out);
            b.dividers(rb, path * 2 + 2, gap, out);
        }
    }

    /// `target` 을 `dir` 방향으로 나누고 새 창을 뒤쪽(오른쪽/아래)에 둔다.
    pub fn split(&mut self, target: PaneId, dir: Dir, new: PaneId) -> bool {
        if self.panes().contains(&new) {
            return false;
        }
        self.split_new(target, dir, new)
    }

    fn split_new(&mut self, target: PaneId, dir: Dir, new: PaneId) -> bool {
        match self {
            Node::Leaf(p) if *p == target => {
                *self = Node::Split {
                    dir,
                    ratio: 0.5,
                    a: Box::new(Node::Leaf(target)),
                    b: Box::new(Node::Leaf(new)),
                };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => {
                a.split_new(target, dir, new) || b.split_new(target, dir, new)
            }
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

    pub fn splitters_with_gap(
        &self,
        rect: Rect,
        path: u64,
        gap: f32,
        out: &mut Vec<(u64, Dir, Rect, Rect)>,
    ) {
        if let Node::Split { dir, ratio, a, b } = self {
            let (ra, rb) = split_rect_with_gap(rect, *dir, *ratio, gap);
            let sep = match dir {
                Dir::Horizontal => {
                    Rect::from_min_max(pos2(ra.right(), rect.top()), pos2(rb.left(), rect.bottom()))
                }
                Dir::Vertical => {
                    Rect::from_min_max(pos2(rect.left(), ra.bottom()), pos2(rect.right(), rb.top()))
                }
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
                *ratio = if value.is_finite() {
                    value.clamp(0.08, 0.92)
                } else {
                    0.5
                };
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

fn effective_gap(rect: Rect, gap: f32) -> f32 {
    gap.clamp(2.0, 16.0)
        .min(rect.width().min(rect.height()).max(0.0))
}

fn axis_start(rect: Rect, dir: Dir) -> f32 {
    match dir {
        Dir::Horizontal => rect.left(),
        Dir::Vertical => rect.top(),
    }
}

fn axis_extent(rect: Rect, dir: Dir) -> f32 {
    match dir {
        Dir::Horizontal => rect.width(),
        Dir::Vertical => rect.height(),
    }
}

fn axis_point(point: Pos2, dir: Dir) -> f32 {
    match dir {
        Dir::Horizontal => point.x,
        Dir::Vertical => point.y,
    }
}

fn opposite(dir: Dir) -> Dir {
    match dir {
        Dir::Horizontal => Dir::Vertical,
        Dir::Vertical => Dir::Horizontal,
    }
}

fn shared_tangent(a: Rect, b: Rect, dir: Dir) -> f32 {
    let (a0, a1, b0, b1) = match dir {
        Dir::Horizontal => (a.top(), a.bottom(), b.top(), b.bottom()),
        Dir::Vertical => (a.left(), a.right(), b.left(), b.right()),
    };
    if a0 < b0 {
        (a1 + b0) * 0.5
    } else {
        (b1 + a0) * 0.5
    }
}

fn independent(a: u64, b: u64) -> bool {
    fn ancestor(a: u64, mut b: u64) -> bool {
        loop {
            if a == b {
                return true;
            }
            if b == 0 {
                return false;
            }
            b = (b - 1) / 2;
        }
    }
    !ancestor(a, b) && !ancestor(b, a)
}

fn adjacent_spans(a: Rect, b: Rect, dir: Dir, gap: f32) -> bool {
    let (a0, a1, b0, b1) = match dir {
        Dir::Horizontal => (a.top(), a.bottom(), b.top(), b.bottom()),
        Dir::Vertical => (a.left(), a.right(), b.left(), b.right()),
    };
    (b0 - a1).max(a0 - b1) <= gap.clamp(2.0, 16.0) + 2.0
}

#[allow(dead_code)]
pub fn split_rect(rect: Rect, dir: Dir, ratio: f32) -> (Rect, Rect) {
    split_rect_with_gap(rect, dir, ratio, SPLITTER)
}

pub fn split_rect_with_gap(rect: Rect, dir: Dir, ratio: f32, gap: f32) -> (Rect, Rect) {
    let gap = gap
        .clamp(2.0, 16.0)
        .min(rect.width().min(rect.height()).max(0.0));
    match dir {
        Dir::Horizontal => {
            let x = rect.left() + (rect.width() - gap) * ratio;
            (
                Rect::from_min_max(rect.min, pos2(x, rect.bottom())),
                Rect::from_min_max(pos2(x + gap, rect.top()), rect.max),
            )
        }
        Dir::Vertical => {
            let y = rect.top() + (rect.height() - gap) * ratio;
            (
                Rect::from_min_max(rect.min, pos2(rect.right(), y)),
                Rect::from_min_max(pos2(rect.left(), y + gap), rect.max),
            )
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
            Nav::Left => {
                r.right() <= cur.left() + 1.0 && r.top() < cur.bottom() && r.bottom() > cur.top()
            }
            Nav::Right => {
                r.left() >= cur.right() - 1.0 && r.top() < cur.bottom() && r.bottom() > cur.top()
            }
            Nav::Up => {
                r.bottom() <= cur.top() + 1.0 && r.left() < cur.right() && r.right() > cur.left()
            }
            Nav::Down => {
                r.top() >= cur.bottom() - 1.0 && r.left() < cur.right() && r.right() > cur.left()
            }
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
        n.layout(
            Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(1000.0, 600.0)),
            &mut rects,
        );
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

#[cfg(test)]
mod gap_regression {
    use super::*;
    #[test]
    fn gap_setting_changes_geometry_and_drag_target_together() {
        let mut n = Node::Leaf(1);
        n.split(1, Dir::Horizontal, 2);
        let area = Rect::from_min_size(pos2(0., 0.), egui::vec2(800., 400.));
        let mut panes = vec![];
        n.layout_with_gap(area, 12., &mut panes);
        assert_eq!(panes[1].1.left() - panes[0].1.right(), 12.);
        let mut splitters = vec![];
        n.splitters_with_gap(area, 0, 12., &mut splitters);
        assert_eq!(splitters[0].2.width(), 12.);
        assert_eq!(splitters[0].2.left(), panes[0].1.right());
    }
}

#[cfg(test)]
mod movement_geometry_regression {
    use super::*;
    use egui::vec2;

    fn viewport() -> Rect {
        Rect::from_min_size(pos2(31.0, 47.0), vec2(1000.0, 800.0))
    }
    fn divider(node: &Node, path: u64, gap: f32) -> (Dir, Rect, Rect) {
        let mut dividers = vec![];
        node.splitters_with_gap(viewport(), 0, gap, &mut dividers);
        let (_, dir, rect, parent) = dividers.into_iter().find(|d| d.0 == path).unwrap();
        (dir, rect, parent)
    }
    fn approx(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 0.01, "{actual} != {expected}");
    }
    fn assert_valid(node: &Node, expected: &[PaneId]) {
        let panes = node.panes();
        assert_eq!(panes.len(), expected.len());
        assert_eq!(
            panes.iter().copied().collect::<HashSet<_>>(),
            expected.iter().copied().collect()
        );
        let mut rects = vec![];
        node.layout_with_gap(viewport(), 10.0, &mut rects);
        assert!(
            rects
                .iter()
                .all(|(_, r)| r.is_finite() && r.width() > 0.0 && r.height() > 0.0)
        );
    }

    #[test]
    fn dragging_across_nested_branches_preserves_every_session_and_collapses_old_branch() {
        for side in [
            MoveSide::Left,
            MoveSide::Right,
            MoveSide::Top,
            MoveSide::Bottom,
        ] {
            let mut n = Node::grid(&[1, 2, 3, 4]).unwrap();
            assert!(n.move_pane(1, 4, side));
            assert_valid(&n, &[1, 2, 3, 4]);
            let mut rects = vec![];
            n.layout(viewport(), &mut rects);
            let source = rects.iter().find(|(id, _)| *id == 1).unwrap().1;
            let target = rects.iter().find(|(id, _)| *id == 4).unwrap().1;
            match side {
                MoveSide::Left => approx(source.right() + SPLITTER, target.left()),
                MoveSide::Right => approx(target.right() + SPLITTER, source.left()),
                MoveSide::Top => approx(source.bottom() + SPLITTER, target.top()),
                MoveSide::Bottom => approx(target.bottom() + SPLITTER, source.top()),
            }
            // Repeated moves in both directions are changes to geometry only.
            for (source, target, side) in [
                (4, 2, MoveSide::Top),
                (2, 3, MoveSide::Right),
                (1, 4, MoveSide::Bottom),
            ] {
                assert!(n.move_pane(source, target, side));
                assert_valid(&n, &[1, 2, 3, 4]);
            }
        }
    }

    #[test]
    fn rejected_drop_is_atomic_and_duplicates_are_normalized() {
        let mut n = Node::grid(&[1, 2, 3]).unwrap();
        let original = n.clone();
        for (source, target) in [(1, 1), (99, 1), (1, 99)] {
            assert!(!n.move_pane(source, target, MoveSide::Left));
            assert_eq!(n, original);
        }
        assert!(!n.split(1, Dir::Vertical, 2));
        assert_eq!(n, original);
        let mut legacy = Node::Split {
            dir: Dir::Horizontal,
            ratio: f32::NAN,
            a: Box::new(Node::Leaf(1)),
            b: Box::new(Node::Split {
                dir: Dir::Vertical,
                ratio: 0.5,
                a: Box::new(Node::Leaf(1)),
                b: Box::new(Node::Leaf(2)),
            }),
        };
        assert!(legacy.move_pane(1, 2, MoveSide::Left));
        assert_valid(&legacy, &[1, 2]);
        assert_eq!(legacy.panes(), vec![1, 2]);
    }

    #[test]
    fn quarter_third_and_half_guides_use_actual_divider_center_with_any_gap() {
        for gap in [4.0, 12.0, 16.0] {
            for (fraction, label) in FRACTIONS {
                let mut n = Node::arranged(&[1, 2], Dir::Horizontal).unwrap();
                let drag = n.begin_resize(viewport(), gap, 0, true).unwrap();
                let expected = viewport().left() + viewport().width() * fraction;
                let result = n.resize_drag(&drag, expected + 6.0, true);
                assert!(result.changed || fraction == 0.5);
                approx(result.position, expected);
                assert_eq!(result.guides.len(), 1);
                assert_eq!(result.guides[0].label, label);
                approx(divider(&n, 0, gap).1.center().x, expected);
            }
        }
    }

    #[test]
    fn near_sibling_axis_snaps_then_future_drags_move_both_branches_together() {
        let mut n = Node::grid(&[1, 2, 3, 4]).unwrap();
        n.set_ratio(1, 0.37);
        n.set_ratio(2, 0.44);
        let gap = 12.0;
        let peer_position = divider(&n, 1, gap).1.center().y;
        let drag = n.begin_resize(viewport(), gap, 2, true).unwrap();
        let snap = n.resize_drag(&drag, peer_position + 5.0, true);
        assert_eq!(snap.guides.len(), 1);
        assert_eq!(snap.guides[0].label, "");
        approx(
            divider(&n, 1, gap).1.center().y,
            divider(&n, 2, gap).1.center().y,
        );
        let linked = n.begin_resize(viewport(), gap, 1, true).unwrap();
        assert_eq!(linked.linked.len(), 2);
        // Same drag snapshot across multiple frames: one axis, no drift.
        for position in [480.0, 530.0, 580.0, 612.0] {
            n.resize_drag(&linked, position, false);
            approx(divider(&n, 1, gap).1.center().y, position);
            approx(divider(&n, 2, gap).1.center().y, position);
        }
        assert_valid(&n, &[1, 2, 3, 4]);
    }

    #[test]
    fn free_resize_breaks_axis_link_and_fraction_snap() {
        let mut n = Node::grid(&[1, 2, 3, 4]).unwrap();
        let peer_before = divider(&n, 2, 12.0).1.center().y;
        let drag = n.begin_resize(viewport(), 12.0, 1, false).unwrap();
        let result = n.resize_drag(&drag, peer_before + 6.0, false);
        assert!(result.guides.is_empty());
        approx(divider(&n, 1, 12.0).1.center().y, peer_before + 6.0);
        approx(divider(&n, 2, 12.0).1.center().y, peer_before);
    }

    #[test]
    fn nested_dividers_convert_global_position_to_distinct_parent_ratios() {
        // Two independent horizontal dividers have distinct parent x bounds.
        let row_a = Node::Split {
            dir: Dir::Horizontal,
            ratio: 0.5,
            a: Box::new(Node::Leaf(1)),
            b: Box::new(Node::Leaf(2)),
        };
        let row_b = Node::Split {
            dir: Dir::Horizontal,
            ratio: 0.25,
            a: Box::new(Node::Leaf(3)),
            b: Box::new(Node::Split {
                dir: Dir::Horizontal,
                ratio: 0.5,
                a: Box::new(Node::Leaf(4)),
                b: Box::new(Node::Leaf(5)),
            }),
        };
        let mut n = Node::Split {
            dir: Dir::Vertical,
            ratio: 0.5,
            a: Box::new(row_a),
            b: Box::new(row_b),
        };
        let gap = 10.0;
        let target = divider(&n, 1, gap).1.center().x;
        let second = n.begin_resize(viewport(), gap, 6, false).unwrap();
        n.resize_drag(&second, target, false);
        approx(divider(&n, 6, gap).1.center().x, target);
        let drag = n.begin_resize(viewport(), gap, 1, true).unwrap();
        assert_eq!(drag.linked.len(), 2);
        n.resize_drag(&drag, 671.0, false);
        approx(divider(&n, 1, gap).1.center().x, 671.0);
        approx(divider(&n, 6, gap).1.center().x, 671.0);
    }

    #[test]
    fn nested_minimum_sizes_clamp_linked_axis_together_and_reject_nonfinite_input() {
        let mut n = Node::grid(&[1, 2, 3, 4]).unwrap();
        n.split(2, Dir::Vertical, 5);
        let gap = 10.0;
        let drag = n.begin_resize(viewport(), gap, 1, true).unwrap();
        assert_eq!(drag.linked.len(), 2);
        let before = n.clone();
        assert!(!n.resize_drag(&drag, f32::NAN, true).changed);
        assert_eq!(before, n);
        let result = n.resize_drag(&drag, 10000.0, true);
        approx(
            divider(&n, 1, gap).1.center().y,
            divider(&n, 2, gap).1.center().y,
        );
        assert!(result.position < viewport().bottom());
        let mut rects = vec![];
        n.layout_with_gap(viewport(), gap, &mut rects);
        assert!(
            rects
                .iter()
                .all(|(_, r)| r.height() >= MIN_PANE_EXTENT - 0.01)
        );
    }

    #[test]
    fn nested_parent_midpoint_is_not_a_fraction_guide_but_board_quarter_is() {
        let mut n = Node::arranged(&[1, 2], Dir::Horizontal).unwrap();
        n.set_ratio(0, 0.61);
        n.split(1, Dir::Horizontal, 3);
        let gap = 12.0;
        let drag = n.begin_resize(viewport(), gap, 1, false).unwrap();
        let parent_midpoint = divider(&n, 1, gap).2.center().x;
        let result = n.resize_drag(&drag, parent_midpoint, true);
        assert!(
            result.guides.is_empty(),
            "a nested parent half is not a board guide"
        );
        approx(result.position, parent_midpoint);
        let board_quarter = viewport().left() + viewport().width() * 0.25;
        let result = n.resize_drag(&drag, board_quarter + 5.0, true);
        assert_eq!(result.guides.len(), 1);
        assert_eq!(result.guides[0].label, "1/4");
        approx(divider(&n, 1, gap).1.center().x, board_quarter);
    }

    #[test]
    fn ancestor_resize_preserves_unequal_subtree_ratios_and_every_leaf_minimum() {
        for ratio in [0.25, 0.75, 0.08, 0.92] {
            let mut n = Node::grid(&[1, 2, 3, 4]).unwrap();
            n.split(2, Dir::Vertical, 5);
            n.set_ratio(4, ratio);
            let gap = 10.0;
            // Use a sufficiently large board for even an 8/92 nested split.
            let board = Rect::from_min_size(pos2(31.0, 47.0), vec2(1400.0, 1800.0));
            let drag = n.begin_resize(board, gap, 1, true).unwrap();
            assert_eq!(drag.linked.len(), 2);
            n.resize_drag(&drag, board.bottom() + 200.0, false);
            let mut rects = vec![];
            n.layout_with_gap(board, gap, &mut rects);
            assert!(
                rects
                    .iter()
                    .all(|(_, r)| r.height() >= MIN_PANE_EXTENT - 0.01),
                "ratio {ratio}: {rects:?}"
            );
            let leaf_2 = rects.iter().find(|(id, _)| *id == 2).unwrap().1;
            let leaf_5 = rects.iter().find(|(id, _)| *id == 5).unwrap().1;
            approx(leaf_2.height() / (leaf_2.height() + leaf_5.height()), ratio);
        }
    }

    #[test]
    fn distant_or_ancestor_dividers_never_form_a_link() {
        assert!(!independent(0, 5));
        assert!(!independent(2, 6));
        assert!(independent(1, 6));
        let top = Rect::from_min_max(pos2(0., 0.), pos2(1000., 200.));
        let bottom = Rect::from_min_max(pos2(0., 400.), pos2(1000., 600.));
        assert!(!adjacent_spans(top, bottom, Dir::Horizontal, 12.0));
    }

    fn rects(node: &Node, gap: f32) -> Vec<(PaneId, Rect)> {
        let mut rects = Vec::new();
        node.layout_with_gap(viewport(), gap, &mut rects);
        rects.sort_by_key(|(id, _)| *id);
        rects
    }

    #[test]
    fn hover_and_pointer_down_preview_both_local_orientations_without_mutation() {
        let gap = 10.0;
        let column_tree = Node::grid(&[1, 2, 3, 4]).unwrap();
        let row_tree = column_tree.transposed().unwrap().0;
        for original in [column_tree, row_tree] {
            let (dir, bar, _) = divider(&original, 0, gap);
            let center = bar.center();
            let tangent = opposite(dir);
            let pointer = |distance| match tangent {
                Dir::Horizontal => pos2(center.x + distance, center.y),
                Dir::Vertical => pos2(center.x, center.y + distance),
            };
            let local = original
                .begin_resize_at(viewport(), gap, 0, pointer(-100.0), true)
                .unwrap();
            let nearby = original
                .begin_resize_at(viewport(), gap, 0, pointer(-45.0), true)
                .unwrap();
            assert!(
                axis_extent(local.affected_rect(false), tangent) < axis_extent(viewport(), tangent)
            );
            approx(
                axis_extent(nearby.affected_rect(false), tangent),
                axis_extent(viewport(), tangent),
            );
            let hover = original
                .hover_scope(viewport(), gap, pointer(-100.0), false)
                .unwrap();
            assert_eq!(hover.boundary_rect(false), local.boundary_rect(false));
            let mut changed = original.clone();
            changed.resize_drag(&local, local.position, false);
            // Rotation is geometrically equivalent before the first movement.
            let before = rects(&original, gap);
            let after = rects(&changed, gap);
            assert_eq!(before, after);
            changed.resize_drag(&local, local.position + 50.0, false);
            let resized = rects(&changed, gap);
            assert!(
                !changed
                    .resize_drag(&local, local.position + 50.0, false)
                    .changed,
                "an identical absolute drag frame is not another layout change"
            );
            for ((_, old), (_, new)) in before.iter().zip(&resized) {
                if axis_start(*old, tangent) > axis_point(center, tangent) {
                    assert_eq!(old, new, "opposite local segment moved");
                }
            }
            assert_eq!(rects(&original, gap), before, "hover mutated tree");
        }
    }

    #[test]
    fn segmented_hover_links_at_sixty_four_and_scope_stays_captured() {
        let gap = 10.0;
        let mut n = Node::grid(&[1, 2, 3, 4]).unwrap();
        let (_, bar, _) = divider(&n, 1, gap);
        let cross_x = divider(&n, 0, gap).1.center().x;
        for (distance, linked) in [(100.0, false), (64.0, true), (45.0, true), (65.0, false)] {
            let scope = n
                .begin_resize_at(
                    viewport(),
                    gap,
                    1,
                    pos2(cross_x - distance, bar.center().y),
                    true,
                )
                .unwrap();
            assert_eq!(scope.is_linked(), linked, "distance {distance}");
        }
        let drag = n
            .begin_resize_at(
                viewport(),
                gap,
                1,
                pos2(cross_x - 45.0, bar.center().y),
                true,
            )
            .unwrap();
        for requested in [bar.center().y + 100.0, bar.center().y - 90.0] {
            n.resize_drag(&drag, requested, false);
            approx(divider(&n, 1, gap).1.center().y, requested);
            approx(divider(&n, 2, gap).1.center().y, requested);
        }
        assert_eq!(drag.boundary_rect(false).width(), viewport().width());
    }

    #[test]
    fn nested_t_junction_does_not_link_a_distant_parallel_segment() {
        let gap = 10.0;
        let mut n = Node::grid(&[1, 2, 3, 4]).unwrap();
        n.split(1, Dir::Horizontal, 5);
        let (_, nested, _) = divider(&n, 3, gap);
        let (_, active, _) = divider(&n, 1, gap);
        let (_, root, _) = divider(&n, 0, gap);
        let pointer = pos2(nested.center().x, active.center().y);
        assert!((pointer.x - root.center().x).abs() > JOIN_DISTANCE);
        let scope = n
            .begin_resize_at(viewport(), gap, 1, pointer, true)
            .unwrap();
        assert!(
            !scope.is_linked(),
            "an internal T is not the shared sibling junction"
        );
        approx(scope.affected_rect(false).width(), active.width());
        let hover = n.hover_scope(viewport(), gap, pointer, false).unwrap();
        assert_eq!(hover.boundary_rect(false), scope.boundary_rect(false));
        let true_junction = pos2(root.center().x - 45.0, active.center().y);
        assert!(
            n.begin_resize_at(viewport(), gap, 1, true_junction, true)
                .unwrap()
                .is_linked()
        );
    }

    #[test]
    fn snap_acquires_holds_releases_and_clears_for_free_resize_both_axes() {
        for dir in [Dir::Horizontal, Dir::Vertical] {
            let mut n = Node::arranged(&[1, 2], dir).unwrap();
            let drag = n.begin_resize(viewport(), 10.0, 0, true).unwrap();
            let quarter = axis_start(viewport(), dir) + axis_extent(viewport(), dir) * 0.25;
            for delta in [18.0, 24.0, 30.0] {
                let result = n.resize_drag(&drag, quarter + delta, true);
                assert!(result.snapped);
                approx(result.position, quarter);
            }
            let released = n.resize_drag(&drag, quarter + 31.0, true);
            assert!(!released.snapped);
            approx(released.position, quarter + 31.0);
            assert!(n.resize_drag(&drag, quarter + 12.0, true).snapped);
            let free = n.resize_drag_with_mode(&drag, quarter + 24.0, true, true);
            assert!(!free.snapped);
            approx(free.position, quarter + 24.0);
            assert!(
                !n.resize_drag(&drag, quarter + 24.0, true).snapped,
                "Option failed to clear lock"
            );
            assert!(n.resize_drag(&drag, quarter + 12.0, true).snapped);
            assert!(!n.resize_drag(&drag, quarter + 24.0, false).snapped);
            assert!(
                !n.resize_drag(&drag, quarter + 24.0, true).snapped,
                "disabled snap failed to clear lock"
            );
        }
    }

    #[test]
    fn option_preview_and_mid_drag_moves_only_active_segment() {
        let gap = 10.0;
        let mut n = Node::grid(&[1, 2, 3, 4]).unwrap();
        let (_, bar, _) = divider(&n, 1, gap);
        let center = divider(&n, 0, gap).1.center().x;
        let local = n
            .hover_scope(viewport(), gap, pos2(center - 45.0, bar.center().y), true)
            .unwrap();
        assert!(!local.is_linked());
        let linked = n
            .begin_resize_at(
                viewport(),
                gap,
                1,
                pos2(center - 45.0, bar.center().y),
                true,
            )
            .unwrap();
        n.resize_drag_with_mode(&linked, bar.center().y + 80.0, true, true);
        approx(divider(&n, 1, gap).1.center().y, bar.center().y + 80.0);
        approx(divider(&n, 2, gap).1.center().y, bar.center().y);
        n.resize_drag_with_mode(&linked, bar.center().y + 90.0, false, false);
        approx(divider(&n, 1, gap).1.center().y, bar.center().y + 90.0);
        approx(divider(&n, 2, gap).1.center().y, bar.center().y + 90.0);
    }

    #[test]
    fn move_preview_uses_candidate_geometry_and_rejects_too_small_without_mutation() {
        let original = Node::grid(&[1, 2, 3, 4]).unwrap();
        for side in [
            MoveSide::Left,
            MoveSide::Right,
            MoveSide::Top,
            MoveSide::Bottom,
        ] {
            let (candidate, preview) = original.move_preview(1, 4, side, viewport(), 10.0).unwrap();
            let mut actual = Vec::new();
            candidate.layout_with_gap(viewport(), 10.0, &mut actual);
            assert_eq!(preview, actual);
            assert_valid(&candidate, &[1, 2, 3, 4]);
            let small = Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 180.0));
            assert!(original.move_preview(1, 4, side, small, 10.0).is_none());
        }
        assert_eq!(original, Node::grid(&[1, 2, 3, 4]).unwrap());
        assert!(
            original
                .move_preview(1, 1, MoveSide::Left, viewport(), 10.0)
                .is_none()
        );
    }
}
