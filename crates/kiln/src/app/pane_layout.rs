//! Runtime-only pane manipulation. Dragging changes the tree, never a session.
use super::{
    KilnApp,
    layout::{Dir, MoveSide, Node, PaneId, ResizeDrag},
};
use egui::{CursorIcon, Pos2, Rect, Stroke, StrokeKind, pos2};

#[derive(Clone)]
struct Scope {
    workspace: u64,
    page: u64,
    viewport: Rect,
    gap: f32,
    original: Node,
    manual_split: bool,
}
enum Drag {
    Move {
        scope: Scope,
        source: PaneId,
        preview: Option<(Node, Vec<(PaneId, Rect)>)>,
    },
    Resize {
        scope: Scope,
        geometry: ResizeDrag,
        offset: f32,
        snapped: bool,
        moved: bool,
    },
    Cross {
        scope: Scope,
        down: Pos2,
        candidates: Vec<ResizeDrag>,
    },
}
impl Drag {
    fn scope(&self) -> &Scope {
        match self {
            Self::Move { scope, .. } | Self::Resize { scope, .. } | Self::Cross { scope, .. } => {
                scope
            }
        }
    }
}
#[derive(Default)]
pub(super) struct PaneLayout {
    drag: Option<Drag>,
}
fn axis(dir: Dir, point: Pos2) -> f32 {
    if dir == Dir::Horizontal {
        point.x
    } else {
        point.y
    }
}

impl KilnApp {
    pub(super) fn cancel_pane_layout(&mut self) {
        if let Some(drag) = self.pane_layout.drag.take() {
            let scope = drag.scope();
            if !matches!(drag, Drag::Move { .. }) {
                if let Some(page) = self
                    .workspaces
                    .iter_mut()
                    .find(|w| w.id == scope.workspace)
                    .and_then(|w| w.pages.iter_mut().find(|p| p.id == scope.page))
                {
                    if pane_set(&page.root) != pane_set(&scope.original) {
                        return;
                    }
                    page.root = scope.original.clone();
                    page.manual_split = scope.manual_split;
                }
            }
        }
    }
    pub(super) fn cancel_layout_escape(&mut self, ctx: &egui::Context) {
        if self.pane_layout.drag.is_some()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.cancel_pane_layout();
        }
    }
    pub(super) fn prepare_pane_layout(&mut self, ui: &egui::Ui, viewport: Rect, enabled: bool) {
        let (workspace, page) = (
            self.workspaces[self.active].id,
            self.workspaces[self.active].page().id,
        );
        let invalid = self.pane_layout.drag.as_ref().is_some_and(|drag| {
            let scope = drag.scope();
            !enabled
                || scope.workspace != workspace
                || scope.page != page
                || scope.viewport != viewport
                || scope.gap != self.settings.card_gap
                || scope
                    .original
                    .panes()
                    .into_iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    != self.workspaces[self.active]
                        .page()
                        .root
                        .panes()
                        .into_iter()
                        .collect::<std::collections::BTreeSet<_>>()
        });
        if invalid || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.cancel_pane_layout();
        }
    }
    pub(super) fn start_pane_move(&mut self, source: PaneId, viewport: Rect) {
        if self.pane_layout.drag.is_some() {
            return;
        }
        let ws = &self.workspaces[self.active];
        let page = ws.page();
        self.pane_layout.drag = Some(Drag::Move {
            scope: Scope {
                workspace: ws.id,
                page: page.id,
                viewport,
                gap: self.settings.card_gap,
                original: page.root.clone(),
                manual_split: page.manual_split,
            },
            source,
            preview: None,
        });
    }

    pub(super) fn interact_pane_layout(
        &mut self,
        ui: &mut egui::Ui,
        viewport: Rect,
        enabled: bool,
    ) {
        if !enabled {
            self.cancel_pane_layout();
            return;
        }
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let alt = ui.input(|i| i.modifiers.alt);
        let ws = &self.workspaces[self.active];
        let (workspace_id, page_id, manual_split) = (ws.id, ws.page().id, ws.page().manual_split);
        let root = ws.page().root.clone();
        let gap = self.settings.card_gap;
        let mut pointer_eligible = false;
        let mut seps = Vec::new();
        root.splitters_with_gap(viewport, 0, gap, &mut seps);
        // Register actual boundary widgets for hit testing and keyboard focus;
        // the raw pointer state retains ownership when the tree is transposed.
        let mut key_resize = None;
        let mut equalize = None;
        let mut focused_scopes = Vec::new();
        for (path, dir, sep, _) in &seps {
            let resp = ui.interact(
                *sep,
                ui.id().with(("pane-boundary", path)),
                egui::Sense::click_and_drag(),
            );
            resp.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Other,
                    true,
                    kiln_common::i18n::tr("패널 크기 조절"),
                )
            });
            pointer_eligible |= resp.contains_pointer();
            if resp.clicked() {
                resp.request_focus();
            }
            if resp.double_clicked() {
                equalize = Some((*path, *dir, sep.center()));
            }
            if resp.has_focus() && self.pane_layout.drag.is_none() {
                if let Some(geometry) =
                    root.begin_resize_at(viewport, gap, *path, sep.center(), false)
                {
                    focused_scopes.push(geometry);
                }
                let (negative, positive) = if *dir == Dir::Horizontal {
                    (egui::Key::ArrowLeft, egui::Key::ArrowRight)
                } else {
                    (egui::Key::ArrowUp, egui::Key::ArrowDown)
                };
                let delta = ui.input_mut(|i| {
                    if i.consume_key(egui::Modifiers::NONE, negative) {
                        -10.0
                    } else if i.consume_key(egui::Modifiers::NONE, positive) {
                        10.0
                    } else {
                        0.0
                    }
                });
                if delta != 0.0 {
                    if let Some(geometry) =
                        root.begin_resize_at(viewport, gap, *path, sep.center(), false)
                    {
                        key_resize = Some((geometry, delta, true));
                    }
                }
            }
        }
        // A junction occupies only the narrow cross arms, never the adjacent
        // header or terminal. Its keyboard arrows move the aligned full axis.
        let mut hovered_junction: Option<Vec<ResizeDrag>> = None;
        let mut junctions = Vec::new();
        for (path, dir, sep, _) in &seps {
            if *dir != Dir::Horizontal {
                continue;
            }
            for (other_path, other_dir, other_sep, _) in &seps {
                if *other_dir != Dir::Vertical {
                    continue;
                }
                let center = pos2(sep.center().x, other_sep.center().y);
                if !sep.expand(gap).contains(center) || !other_sep.expand(gap).contains(center) {
                    continue;
                }
                if junctions
                    .iter()
                    .any(|(p, _, _): &(Pos2, u64, u64)| p.distance(center) < 1.)
                {
                    continue;
                }
                junctions.push((center, *path, *other_path));
            }
        }
        for (center, x_path, y_path) in junctions {
            let mut junction_eligible = false;
            // The arms are hit-test-only; one keyboard/AX control represents
            // the junction, avoiding duplicate tab stops for the same action.
            for (arm, size) in [(0, egui::vec2(8., 40.)), (1, egui::vec2(40., 8.))] {
                let resp = ui.interact(
                    Rect::from_center_size(center, size),
                    ui.id().with(("pane-junction-arm", x_path, y_path, arm)),
                    egui::Sense::CLICK | egui::Sense::DRAG,
                );
                junction_eligible |= resp.contains_pointer();
                pointer_eligible |= resp.contains_pointer();
            }
            let resp = ui.interact(
                Rect::from_center_size(center, egui::vec2(8., 8.)),
                ui.id().with(("pane-junction", x_path, y_path)),
                egui::Sense::click_and_drag(),
            );
            resp.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Other,
                    true,
                    kiln_common::i18n::tr("패널 크기 조절"),
                )
            });
            junction_eligible |= resp.contains_pointer();
            pointer_eligible |= resp.contains_pointer();
            if junction_eligible && !alt && pointer.is_some_and(|point| cross_hit(point, center)) {
                let candidates = [x_path, y_path]
                    .into_iter()
                    .filter_map(|path| root.begin_resize_at(viewport, gap, path, center, true))
                    .collect::<Vec<_>>();
                if candidates.len() == 2 {
                    hovered_junction = Some(candidates);
                }
            }
            if resp.clicked()
                || (ui.input(|i| i.pointer.primary_released())
                    && pointer.is_some_and(|p| cross_hit(p, center))
                    && matches!(self.pane_layout.drag, Some(Drag::Cross { .. })))
            {
                resp.request_focus();
            }
            if resp.has_focus() && self.pane_layout.drag.is_none() {
                focused_scopes.clear();
                for path in [x_path, y_path] {
                    if let Some(geometry) = root.begin_resize_at(viewport, gap, path, center, true)
                    {
                        focused_scopes.push(geometry);
                    }
                }
                let command = ui.input_mut(|i| {
                    for (key, path, delta) in [
                        (egui::Key::ArrowLeft, x_path, -10.),
                        (egui::Key::ArrowRight, x_path, 10.),
                        (egui::Key::ArrowUp, y_path, -10.),
                        (egui::Key::ArrowDown, y_path, 10.),
                    ] {
                        if i.consume_key(egui::Modifiers::NONE, key) {
                            return Some((path, delta));
                        }
                    }
                    None
                });
                if let Some((path, delta)) = command {
                    if let Some(geometry) = root.begin_resize_at(viewport, gap, path, center, true)
                    {
                        key_resize = Some((geometry, delta, false));
                    }
                }
            }
        }
        if let Some((path, dir, point)) = equalize {
            self.cancel_pane_layout();
            let root = self.workspaces[self.active].page().root.clone();
            if let Some(geometry) = root.begin_resize_at(viewport, gap, path, point, false) {
                let delta = axis(dir, geometry.affected_rect(true).center()) - geometry.position;
                key_resize = Some((geometry, delta, true));
            }
        }
        if let Some((geometry, delta, independent)) = key_resize {
            let page = self.workspaces[self.active].page_mut();
            if page
                .root
                .resize_drag_with_mode(&geometry, geometry.position + delta, false, independent)
                .changed
            {
                page.manual_split = true;
            }
        }
        let Some(point) = pointer else {
            if !ui.input(|i| i.pointer.primary_down()) {
                self.cancel_pane_layout();
            }
            if focused_scopes.len() > 1 {
                paint_cross(ui, &focused_scopes, false, &self.theme);
            } else if let Some(geometry) = focused_scopes.first() {
                paint_scope(ui, geometry, true, geometry.position, false, &self.theme);
            }
            return;
        };
        let hover = if let Some(candidates) = &hovered_junction {
            candidates.first().cloned()
        } else if pointer_eligible {
            root.hover_scope(viewport, gap, point, alt)
        } else {
            None
        };
        if self.pane_layout.drag.is_none() && ui.input(|i| i.pointer.primary_pressed()) {
            if let Some(geometry) = hover.clone() {
                let scope = Scope {
                    workspace: workspace_id,
                    page: page_id,
                    viewport,
                    gap,
                    original: root.clone(),
                    manual_split,
                };
                let mut candidates = hovered_junction
                    .clone()
                    .unwrap_or_else(|| vec![geometry.clone()]);
                if !alt && candidates.len() == 1 {
                    if let Some(other) =
                        cross_candidate(&root, viewport, gap, point, &geometry, &seps)
                    {
                        candidates.push(other);
                    }
                }
                self.pane_layout.drag = Some(if candidates.len() > 1 {
                    Drag::Cross {
                        scope,
                        down: point,
                        candidates,
                    }
                } else {
                    Drag::Resize {
                        scope,
                        offset: axis(geometry.dir, point) - geometry.position,
                        geometry,
                        snapped: false,
                        moved: false,
                    }
                });
            }
        }
        if let Some(Drag::Cross { down, .. }) = &self.pane_layout.drag {
            let delta = point - *down;
            if delta.length() >= 2.0 {
                let Some(Drag::Cross {
                    scope,
                    down,
                    candidates,
                }) = self.pane_layout.drag.take()
                else {
                    unreachable!()
                };
                let dir = if delta.x.abs() >= delta.y.abs() {
                    Dir::Horizontal
                } else {
                    Dir::Vertical
                };
                let geometry = candidates.into_iter().find(|c| c.dir == dir).unwrap();
                self.pane_layout.drag = Some(Drag::Resize {
                    scope,
                    offset: axis(dir, down) - geometry.position,
                    geometry,
                    snapped: false,
                    moved: false,
                });
            }
        }
        let mut commit_move = None;
        let released = ui.input(|i| i.pointer.primary_released());
        let theme = self.theme;
        match &mut self.pane_layout.drag {
            Some(Drag::Move {
                scope,
                source,
                preview,
            }) => {
                let mut rects = Vec::new();
                scope.original.layout_with_gap(viewport, gap, &mut rects);
                let target = rects
                    .iter()
                    .find(|(id, rect)| id != source && rect.contains(point));
                let rejected = target.map(|(_, rect)| *rect);
                *preview = target.and_then(|(target, rect)| {
                    let relative = point - rect.min;
                    let candidates = [
                        (relative.x / rect.width(), MoveSide::Left),
                        ((rect.width() - relative.x) / rect.width(), MoveSide::Right),
                        (relative.y / rect.height(), MoveSide::Top),
                        (
                            (rect.height() - relative.y) / rect.height(),
                            MoveSide::Bottom,
                        ),
                    ];
                    let side = candidates
                        .into_iter()
                        .min_by(|a, b| a.0.total_cmp(&b.0))
                        .unwrap()
                        .1;
                    scope
                        .original
                        .move_preview(*source, *target, side, viewport, gap)
                });
                ui.ctx().set_cursor_icon(if preview.is_some() {
                    CursorIcon::Grabbing
                } else {
                    CursorIcon::NotAllowed
                });
                if preview.is_none() {
                    if let Some(rect) = rejected {
                        ui.painter()
                            .rect_filled(rect, 0, theme.red.gamma_multiply(0.12));
                        ui.painter().rect_stroke(
                            rect,
                            0,
                            Stroke::new(2.0, theme.red),
                            StrokeKind::Inside,
                        );
                    }
                }
                if let Some((node, rects)) = preview {
                    for (id, rect) in rects.iter() {
                        if id == source {
                            ui.painter().rect_filled(*rect, 0, theme.accent_soft(35));
                            ui.painter().rect_stroke(
                                *rect,
                                0,
                                Stroke::new(2.0, theme.accent_soft(240)),
                                StrokeKind::Inside,
                            );
                        } else {
                            ui.painter().rect_stroke(
                                *rect,
                                0,
                                Stroke::new(1.0, theme.accent_soft(110)),
                                StrokeKind::Inside,
                            );
                        }
                    }
                    if released {
                        commit_move = Some((node.clone(), *source));
                    }
                }
            }
            Some(Drag::Resize {
                geometry,
                offset,
                snapped,
                moved,
                ..
            }) => {
                let page = self.workspaces[self.active].page_mut();
                let result = page.root.resize_drag_with_mode(
                    geometry,
                    axis(geometry.dir, point) - *offset,
                    !alt,
                    alt,
                );
                *snapped = result.snapped;
                *moved |= (axis(geometry.dir, point) - *offset - geometry.position).abs() > 2.0;
                if result.changed {
                    page.manual_split = true;
                }
                paint_scope(ui, geometry, alt, result.position, result.snapped, &theme);
                for guide in result.guides {
                    let (a, b) = if guide.dir == Dir::Horizontal {
                        (
                            pos2(guide.position, viewport.top()),
                            pos2(guide.position, viewport.bottom()),
                        )
                    } else {
                        (
                            pos2(viewport.left(), guide.position),
                            pos2(viewport.right(), guide.position),
                        )
                    };
                    ui.painter().line_segment(
                        [a, b],
                        Stroke::new(
                            if result.snapped { 4.0 } else { 2.5 },
                            theme.accent_soft(if result.snapped { 255 } else { 175 }),
                        ),
                    );
                }
            }
            Some(Drag::Cross { candidates, .. }) => {
                paint_cross(ui, candidates, alt, &theme);
            }
            None => {
                if let Some(candidates) = &hovered_junction {
                    paint_cross(ui, candidates, false, &theme);
                }
                if hover.is_none() && !focused_scopes.is_empty() {
                    if focused_scopes.len() > 1 {
                        paint_cross(ui, &focused_scopes, false, &theme);
                    } else {
                        let geometry = &focused_scopes[0];
                        paint_scope(ui, geometry, true, geometry.position, false, &theme);
                    }
                }
                if let Some(geometry) = hover.filter(|_| hovered_junction.is_none()) {
                    if !alt {
                        if let Some(other) =
                            cross_candidate(&root, viewport, gap, point, &geometry, &seps)
                        {
                            paint_cross(ui, &[geometry, other], false, &theme);
                        } else {
                            paint_scope(ui, &geometry, alt, geometry.position, false, &theme);
                        }
                    } else {
                        paint_scope(ui, &geometry, alt, geometry.position, false, &theme);
                    }
                }
            }
        }
        if released {
            if let Some((node, source)) = commit_move {
                let page = self.workspaces[self.active].page_mut();
                page.root = node;
                page.focused = source;
                page.manual_split = true;
                page.zoomed = None;
                self.focus_terminal = true;
            }
            if matches!(
                self.pane_layout.drag,
                Some(Drag::Move { .. }) | Some(Drag::Resize { moved: true, .. })
            ) {
                self.focus_terminal = true;
            }
            self.pane_layout.drag = None;
        }
        if self.pane_layout.drag.is_some() {
            ui.ctx().request_repaint();
        }
    }
}

fn paint_scope(
    ui: &egui::Ui,
    geometry: &ResizeDrag,
    independent: bool,
    position: f32,
    snapped: bool,
    theme: &kiln_common::Theme,
) {
    let rect = geometry.boundary_rect(independent);
    let extent = geometry.affected_rect(independent);
    let color = theme.accent_soft(if snapped { 255 } else { 175 });
    let stroke = Stroke::new(if snapped { 4.0 } else { 2.5 }, color);
    let (start, end) = if geometry.dir == Dir::Horizontal {
        (pos2(position, rect.top()), pos2(position, rect.bottom()))
    } else {
        (pos2(rect.left(), position), pos2(rect.right(), position))
    };
    ui.painter().line_segment([start, end], stroke);
    ui.painter().rect_stroke(
        extent,
        0,
        Stroke::new(1.0, theme.accent_soft(100)),
        StrokeKind::Inside,
    );
    ui.ctx()
        .set_cursor_icon(if geometry.dir == Dir::Horizontal {
            CursorIcon::ResizeHorizontal
        } else {
            CursorIcon::ResizeVertical
        });
}

fn pane_set(node: &Node) -> std::collections::BTreeSet<PaneId> {
    node.panes().into_iter().collect()
}
fn cross_candidate(
    root: &Node,
    viewport: Rect,
    gap: f32,
    point: Pos2,
    active: &ResizeDrag,
    seps: &[(u64, Dir, Rect, Rect)],
) -> Option<ResizeDrag> {
    seps.iter()
        .filter(|(_, dir, _, _)| *dir != active.dir)
        .filter_map(|(path, _dir, sep, _)| {
            let crossing = if active.dir == Dir::Horizontal {
                pos2(active.position, sep.center().y)
            } else {
                pos2(sep.center().x, active.position)
            };
            if !cross_hit(point, crossing)
                || !sep.expand(gap).contains(crossing)
                || !active.boundary_rect(false).expand(gap).contains(crossing)
            {
                return None;
            }
            root.begin_resize_at(viewport, gap, *path, crossing, true)
                .map(|drag| (point.distance(crossing), drag))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, drag)| drag)
}
fn paint_cross(
    ui: &egui::Ui,
    candidates: &[ResizeDrag],
    independent: bool,
    theme: &kiln_common::Theme,
) {
    let mut affected = Rect::NOTHING;
    for geometry in candidates {
        let rect = geometry.boundary_rect(independent);
        let (a, b) = if geometry.dir == Dir::Horizontal {
            (rect.center_top(), rect.center_bottom())
        } else {
            (rect.left_center(), rect.right_center())
        };
        ui.painter()
            .line_segment([a, b], Stroke::new(2.5, theme.accent_soft(175)));
        affected = affected.union(geometry.affected_rect(independent));
    }
    ui.painter().rect_stroke(
        affected,
        0,
        Stroke::new(1.0, theme.accent_soft(100)),
        StrokeKind::Inside,
    );
    ui.ctx().set_cursor_icon(CursorIcon::Move);
}

fn cross_hit(point: Pos2, crossing: Pos2) -> bool {
    let d = point - crossing;
    (d.x.abs() <= 4. && d.y.abs() <= 20.) || (d.y.abs() <= 4. && d.x.abs() <= 20.)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn junction_hit_does_not_steal_diagonal_headers() {
        let c = pos2(500., 300.);
        assert!(cross_hit(pos2(503., 318.), c));
        assert!(cross_hit(pos2(518., 303.), c));
        assert!(!cross_hit(pos2(506., 314.), c));
        assert!(!cross_hit(pos2(515., 315.), c));
        assert!(!cross_hit(pos2(500., 321.), c));
    }
}
