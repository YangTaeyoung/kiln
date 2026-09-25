//! 화면 구성: 상단 바(명령 바·페이지·도구), 스페이스 레일, 카드 캔버스, 도구 시트, 오버레이.

use kiln_common::widgets::{self, ButtonKind};
use super::*;
use egui::{Align2, Color32, CornerRadius, CursorIcon, Frame, Margin, RichText, Sense, Stroke, StrokeKind, UiBuilder, pos2, vec2};
use icons::Icon;
use kiln_common::fonts;

pub const TOPBAR_H: f32 = 46.0;
const HEADER_H: f32 = 32.0;

fn is_mac() -> bool {
    cfg!(target_os = "macos")
}

/// 카드 한 장의 머리글 정보.
pub(crate) struct CardInfo {
    pub name: String,
    pub detail: String,
    pub right: String,
    pub dot: Color32,
    pub pulse: bool,
    pub icon: Icon,
}

/// 터미널 제목 중 보여줄 만한 것만 고른다. 셸 기본 제목(`user@host:경로`)이나 경로만 있는 제목은 버린다.
pub(crate) fn meaningful_title(title: &str, proc_name: &str, cwd: Option<&str>) -> Option<String> {
    let t = title.trim();
    if t.is_empty() || t == proc_name {
        return None;
    }
    let first = t.split_whitespace().next().unwrap_or("");
    if first.contains('@') && first.contains(':') {
        return None;
    }
    if t.starts_with('/') || t.starts_with('~') {
        return None;
    }
    if let Some(c) = cwd {
        if t == c || t.ends_with(c) || t == short_path(Path::new(c)) {
            return None;
        }
    }
    Some(t.chars().take(120).collect())
}

impl KilnApp {
    pub(crate) fn card_info(&self, pid: PaneId) -> CardInfo {
        let t = self.theme;
        let Some(pane) = self.panes.get(&pid) else {
            return CardInfo { name: String::new(), detail: String::new(), right: String::new(), dot: t.text_faint, pulse: false, icon: Icon::Terminal };
        };
        match &pane.kind {
            PaneKind::Term { session, .. } => match session.and_then(|s| self.conn.infos.get(&s)) {
                Some(i) => {
                    let proc_name = i.fg_process.clone().unwrap_or_else(|| "셸".into());
                    let is_shell = shells().contains(&proc_name.as_str()) || proc_name == "셸";
                    let (dot, pulse) = if i.exited.is_some() {
                        (t.red, false)
                    } else if i.attention {
                        (t.orange, true)
                    } else if is_shell {
                        (t.text_faint, false)
                    } else {
                        (t.green, false)
                    };
                    let detail = match i.exited {
                        Some(code) => format!("종료됨 · 코드 {code}"),
                        None => meaningful_title(&i.title, &proc_name, i.cwd.as_deref()).unwrap_or_default(),
                    };
                    let right = i.cwd.as_deref().map(|c| short_path(Path::new(c))).unwrap_or_default();
                    CardInfo { name: proc_name, detail, right, dot, pulse, icon: Icon::Terminal }
                }
                None => CardInfo { name: "시작하는 중…".into(), detail: String::new(), right: String::new(), dot: t.text_faint, pulse: false, icon: Icon::Terminal },
            },
            PaneKind::Tool(tool) => {
                let dirty = tool.is_dirty();
                let key = tool.key();
                let icon = if key.starts_with("db") {
                    Icon::Database
                } else if key.starts_with("pr:") {
                    Icon::PullRequest
                } else if key.starts_with("diff") || key.starts_with("commit") {
                    Icon::Branch
                } else {
                    Icon::File
                };
                CardInfo {
                    name: tool.title(),
                    detail: if dirty { "저장 안 됨".into() } else { String::new() },
                    right: tool.status_text().unwrap_or_default(),
                    dot: if dirty { t.yellow } else { t.accent },
                    pulse: false,
                    icon,
                }
            }
        }
    }

    /// 페이지 이름: 지정한 제목, 없으면 셸이 아닌 첫 카드의 이름.
    fn page_label(&self, page: &Page) -> String {
        if let Some(t) = &page.title {
            return t.clone();
        }
        let panes = page.root.panes();
        let names: Vec<String> = panes.iter().map(|p| self.card_info(*p).name).collect();
        let main = names.iter().find(|n| !shells().contains(&n.as_str()) && n.as_str() != "셸").cloned().unwrap_or_else(|| names.first().cloned().unwrap_or_default());
        if panes.len() > 1 { format!("{main} +{}", panes.len() - 1) } else { main }
    }

    fn page_attention(&self, page: &Page) -> bool {
        page.root.panes().iter().any(|p| self.panes.get(p).and_then(|x| x.session()).and_then(|s| self.conn.infos.get(&s)).is_some_and(|i| i.attention))
    }

    // ------------------------------------------------------------------ 상단 바

    pub(super) fn ui_topbar(&mut self, root: &mut egui::Ui) {
        let t = self.theme;
        let canvas = widgets::canvas_color(&t);
        egui::Panel::top("topbar").exact_size(TOPBAR_H).frame(Frame::new().fill(canvas)).show(root, |ui| {
            let bar = ui.max_rect();
            // 빈 곳을 끌면 창 이동, 두 번 누르면 최대화 전환.
            let bg = ui.interact(bar, ui.id().with("drag"), Sense::click_and_drag());
            if bg.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if bg.double_clicked() {
                let max = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
            }

            let cy = bar.center().y;
            let mut x = bar.left() + if is_mac() { 80.0 } else { 12.0 };
            let mut lui = ui.new_child(UiBuilder::new().max_rect(egui::Rect::from_min_size(pos2(x, cy - 15.0), vec2(30.0, 30.0))));
            if widgets::icon_button(&mut lui, Icon::Sidebar, 30.0, false, "스페이스 레일 (⌘B)").clicked() {
                self.actions.push(Action::ToggleSidebar);
            }
            x += 40.0;

            // 현재 스페이스와 브랜치.
            let ws = &self.workspaces[self.active];
            let g = ui.painter().layout_no_wrap(ws.name.clone(), fonts::semibold(14.0), t.text);
            ui.painter().galley(pos2(x, cy - g.size().y / 2.0), g.clone(), t.text);
            x += g.size().x + 10.0;
            if let Some(s) = ws.tools.summary() {
                let label = if s.dirty > 0 { format!("{}  ●{}", s.branch, s.dirty) } else { s.branch.clone() };
                let lg = ui.painter().layout_no_wrap(label, fonts::medium(12.0), t.text_dim);
                let chip = egui::Rect::from_min_size(pos2(x, cy - 11.0), vec2(lg.size().x + 30.0, 22.0));
                ui.painter().rect_filled(chip, CornerRadius::same(11), t.bg_elevated);
                icons::paint(ui.painter(), egui::Rect::from_center_size(pos2(chip.left() + 13.0, cy), vec2(12.0, 12.0)), Icon::Branch, t.purple);
                ui.painter().galley(pos2(chip.left() + 23.0, cy - lg.size().y / 2.0), lg, t.text_dim);
                x = chip.right() + 16.0;
            }

            // 오른쪽 도구 아이콘.
            let right_w = (tools::ToolKind::ALL.len() as f32 + 2.0) * 32.0 + 20.0;
            let right_rect = egui::Rect::from_min_max(pos2(bar.right() - right_w - 10.0, cy - 16.0), pos2(bar.right() - 10.0, cy + 16.0));
            let mut rui = ui.new_child(UiBuilder::new().max_rect(right_rect).layout(egui::Layout::right_to_left(egui::Align::Center)));
            rui.spacing_mut().item_spacing.x = 2.0;
            if widgets::icon_button(&mut rui, Icon::Gear, 30.0, self.settings_ui.open, "설정 (⌘,)").clicked() {
                self.actions.push(Action::OpenSettings);
            }
            let unread = self.unread.len();
            let bell = widgets::icon_button(&mut rui, Icon::Bell, 30.0, false, if unread > 0 { "최근 알림으로 이동 (⇧⌘U)" } else { "새 알림 없음" });
            if unread > 0 {
                let c = pos2(bell.rect.right() - 8.0, bell.rect.top() + 8.0);
                rui.painter().circle_filled(c, 7.0, t.orange);
                rui.painter().text(c, Align2::CENTER_CENTER, unread.min(9).to_string(), fonts::semibold(9.5), Color32::BLACK);
            }
            if bell.clicked() && unread > 0 {
                self.actions.push(Action::JumpUnread);
            }
            rui.add_space(6.0);
            let (sep, _) = rui.allocate_exact_size(vec2(1.0, 18.0), Sense::hover());
            rui.painter().rect_filled(sep, 0.0, t.border_strong);
            rui.add_space(6.0);
            let open = self.workspaces[self.active].sheet;
            for k in tools::ToolKind::ALL.iter().rev() {
                let tip = format!("{} ({})", k.label(), k.shortcut());
                if widgets::icon_button(&mut rui, k.vicon(), 30.0, open == Some(*k), &tip).clicked() {
                    self.actions.push(Action::ToggleSheet(*k));
                }
            }

            // 가운데 명령 바.
            let cmd_w = 420.0f32.min((right_rect.left() - x - 24.0).max(160.0));
            let cmd_x = (bar.center().x - cmd_w / 2.0).max(x).min(right_rect.left() - cmd_w - 12.0);
            let cmd = egui::Rect::from_min_size(pos2(cmd_x, cy - 15.0), vec2(cmd_w, 30.0));
            let resp = ui.interact(cmd, ui.id().with("cmdbar"), Sense::click());
            let fill = if resp.hovered() { t.bg_hover } else { t.bg_elevated };
            ui.painter().rect_filled(cmd, CornerRadius::same(8), fill);
            ui.painter().rect_stroke(cmd, CornerRadius::same(8), Stroke::new(1.0, t.border), StrokeKind::Inside);
            icons::paint(ui.painter(), egui::Rect::from_center_size(pos2(cmd.left() + 18.0, cy), vec2(14.0, 14.0)), Icon::Search, t.text_faint);
            ui.painter().with_clip_rect(cmd.shrink(2.0)).text(pos2(cmd.left() + 34.0, cy), Align2::LEFT_CENTER, "명령 · 세션 · 스페이스 검색", fonts::regular(13.0), t.text_faint);
            let mut kui = ui.new_child(UiBuilder::new().max_rect(egui::Rect::from_min_max(pos2(cmd.right() - 60.0, cy - 9.0), pos2(cmd.right() - 8.0, cy + 9.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
            widgets::keycaps(&mut kui, &["K", "⌘"]);
            if resp.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
            if resp.clicked() {
                self.actions.push(Action::OpenPalette);
            }

            // 페이지 알약: 스페이스 이름 뒤, 명령 바 앞.
            let pages_right = cmd.left() - 12.0;
            let mut px = x;
            let active_page = self.workspaces[self.active].active_page;
            let labels: Vec<(String, bool)> = self.workspaces[self.active].pages.iter().map(|p| (self.page_label(p), self.page_attention(p))).collect();
            let n = labels.len();
            if n > 1 {
                for (i, (label, attention)) in labels.into_iter().enumerate() {
                    let g = ui.painter().layout_no_wrap(label, fonts::medium(12.0), t.text);
                    let w = (g.size().x + 30.0).min(180.0);
                    if px + w > pages_right - 34.0 {
                        break;
                    }
                    let r = egui::Rect::from_min_size(pos2(px, cy - 12.0), vec2(w, 24.0));
                    let resp = ui.interact(r, ui.id().with(("page", i)), Sense::click());
                    let sel = i == active_page;
                    let fill = if sel { t.bg_selected } else if resp.hovered() { t.bg_hover } else { Color32::TRANSPARENT };
                    ui.painter().rect_filled(r, CornerRadius::same(12), fill);
                    let dot_c = if attention { t.orange } else if sel { t.accent } else { t.text_faint };
                    ui.painter().circle_filled(pos2(r.left() + 12.0, cy), 3.0, dot_c);
                    ui.painter().with_clip_rect(r.shrink2(vec2(6.0, 0.0))).galley(pos2(r.left() + 21.0, cy - g.size().y / 2.0), g, if sel { t.text } else { t.text_dim });
                    if resp.clicked() {
                        self.actions.push(Action::SelectPage(i));
                    }
                    if resp.middle_clicked() {
                        self.actions.push(Action::ClosePage(i, false));
                    }
                    resp.context_menu(|ui| {
                        if ui.button("페이지 닫기").clicked() {
                            self.actions.push(Action::ClosePage(i, false));
                            ui.close();
                        }
                    });
                    px = r.right() + 4.0;
                }
            }
            if px + 28.0 < pages_right {
                let mut pui = ui.new_child(UiBuilder::new().max_rect(egui::Rect::from_min_size(pos2(px, cy - 13.0), vec2(26.0, 26.0))));
                if widgets::icon_button(&mut pui, Icon::Plus, 26.0, false, "새 페이지 (⌘T)").clicked() {
                    self.actions.push(Action::NewPage);
                }
            }
        });
    }

    // ------------------------------------------------------------------ 스페이스 레일

    pub(super) fn ui_spaces(&mut self, root: &mut egui::Ui) {
        let t = self.theme;
        let canvas = widgets::canvas_color(&t);
        let wide = self.sidebar_open;
        let width = if wide { 244.0 } else { 62.0 };
        egui::Panel::left("spaces").exact_size(width).resizable(false).frame(Frame::new().fill(canvas).inner_margin(Margin { left: 10, right: 6, top: 2, bottom: 10 })).show(root, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 4.0);
            if wide {
                ui.horizontal(|ui| {
                    ui.add_space(6.0);
                    ui.label(RichText::new("스페이스").font(fonts::semibold(11.5)).color(t.text_faint));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::icon_button(ui, Icon::Plus, 24.0, false, "새 스페이스 (⌘N)").clicked() {
                            self.actions.push(Action::NewWorkspace(None));
                        }
                    });
                });
                ui.add_space(4.0);
            }
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for i in 0..self.workspaces.len() {
                    self.space_row(ui, i, wide);
                }
                if !wide {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.add_space(5.0);
                        if widgets::icon_button(ui, Icon::Plus, 32.0, false, "새 스페이스 (⌘N)").clicked() {
                            self.actions.push(Action::NewWorkspace(None));
                        }
                    });
                } else {
                    self.detached_sessions(ui);
                }
            });
        });
    }

    fn space_row(&mut self, ui: &mut egui::Ui, i: usize, wide: bool) {
        let t = self.theme;
        let active = i == self.active;
        let ws = &self.workspaces[i];
        let sessions: Vec<SessionId> = ws.all_panes().iter().filter_map(|p| self.panes.get(p).and_then(|x| x.session())).collect();
        let infos: Vec<&kiln_proto::SessionInfo> = sessions.iter().filter_map(|s| self.conn.infos.get(s)).collect();
        let attention = infos.iter().filter(|x| x.attention).count();
        let mut agents: Vec<(String, bool)> = Vec::new();
        for inf in &infos {
            if let Some(p) = &inf.fg_process {
                if !shells().contains(&p.as_str()) && !agents.iter().any(|(n, _)| n == p) {
                    agents.push((p.clone(), inf.attention));
                }
            }
        }
        let note = infos.iter().filter(|x| x.attention).find_map(|x| x.last_notification.clone());
        let name = ws.name.clone();
        let root_path = ws.root.clone();
        let h = if wide { if note.is_some() { 66.0 } else { 52.0 } } else { 44.0 };
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
        let fill = if active {
            t.bg_elevated
        } else if resp.hovered() {
            widgets::lerp_color(widgets::canvas_color(&t), t.bg_elevated, 0.55)
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, CornerRadius::same(10), fill);
        if active {
            ui.painter().rect_stroke(rect, CornerRadius::same(10), Stroke::new(1.0, t.border), StrokeKind::Inside);
        }
        let av = egui::Rect::from_min_size(pos2(rect.left() + 7.0, rect.top() + if wide { 10.0 } else { 7.0 }), vec2(30.0, 30.0));
        widgets::avatar(ui, av, &name, active);
        if attention > 0 {
            let c = pos2(av.right() - 1.0, av.top() + 1.0);
            ui.painter().circle_filled(c, 6.5, widgets::canvas_color(&t));
            widgets::status_dot(ui, c, t.orange, true);
        }
        if wide {
            let x = av.right() + 11.0;
            let clip = egui::Rect::from_min_max(pos2(x, rect.top()), pos2(rect.right() - 8.0, rect.bottom()));
            let p = ui.painter().with_clip_rect(clip);
            if let Some(buf) = &mut self.workspaces[i].renaming {
                let mut cui = ui.new_child(UiBuilder::new().max_rect(egui::Rect::from_min_size(pos2(x, rect.top() + 7.0), vec2(rect.right() - x - 10.0, 22.0))));
                let te = cui.add(egui::TextEdit::singleline(buf).font(fonts::medium(13.5)).desired_width(f32::INFINITY));
                te.request_focus();
                if te.lost_focus() {
                    let n = buf.clone();
                    self.workspaces[i].renaming = None;
                    self.actions.push(Action::RenameWorkspace(i, n));
                }
            } else {
                p.text(pos2(x, rect.top() + 18.0), Align2::LEFT_CENTER, &name, fonts::medium(13.5), if active { t.text } else { t.text_dim });
                if i < 9 && resp.hovered() {
                    p.text(pos2(rect.right() - 10.0, rect.top() + 18.0), Align2::RIGHT_CENTER, format!("⌘{}", i + 1), fonts::regular(11.0), t.text_faint);
                }
            }
            // 두 번째 줄: 실행 중인 에이전트, 없으면 브랜치.
            let ly = rect.top() + 36.0;
            if agents.is_empty() {
                let s = self.workspaces[i].tools.summary().map(|s| s.branch).unwrap_or_else(|| short_path(&root_path));
                p.text(pos2(x, ly), Align2::LEFT_CENTER, s, fonts::regular(11.5), t.text_faint);
            } else {
                let mut lx = x;
                for (a, att) in agents.iter().take(3) {
                    p.circle_filled(pos2(lx + 3.0, ly), 3.0, if *att { t.orange } else { t.green });
                    let g = ui.painter().layout_no_wrap(a.clone(), fonts::medium(11.5), t.text_dim);
                    let w = g.size().x;
                    p.galley(pos2(lx + 10.0, ly - g.size().y / 2.0), g, t.text_dim);
                    lx += w + 20.0;
                }
            }
            if let Some(n) = note {
                p.text(pos2(x, rect.top() + 53.0), Align2::LEFT_CENTER, n, fonts::regular(11.0), t.orange);
            }
        }
        let resp = resp.on_hover_text(format!("{} — {}", name, short_path(&root_path)));
        if resp.clicked() {
            self.actions.push(Action::SelectWorkspace(i));
        }
        if resp.double_clicked() && wide {
            self.workspaces[i].renaming = Some(name.clone());
        }
        resp.context_menu(|ui| {
            if ui.button("이름 바꾸기").clicked() {
                self.workspaces[i].renaming = Some(name.clone());
                ui.close();
            }
            if ui.button("Finder 에서 열기").clicked() {
                let _ = open::that_detached(&root_path);
                ui.close();
            }
            ui.separator();
            if ui.button(RichText::new("스페이스 닫기").color(t.red)).clicked() {
                self.confirm = Some(Confirm {
                    title: format!("'{name}' 스페이스를 닫을까요?"),
                    body: "이 스페이스의 모든 터미널 세션이 종료됩니다.".into(),
                    ok: "스페이스 닫기".into(),
                    action: Action::CloseWorkspace(i),
                });
                ui.close();
            }
        });
    }

    fn detached_sessions(&mut self, ui: &mut egui::Ui) {
        let t = self.theme;
        let used: std::collections::HashSet<SessionId> = self.panes.values().filter_map(|p| p.session()).collect();
        let mut orphans: Vec<kiln_proto::SessionInfo> = self.conn.infos.values().filter(|i| !used.contains(&i.id)).cloned().collect();
        if orphans.is_empty() {
            return;
        }
        orphans.sort_by_key(|i| i.id);
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.label(RichText::new(format!("분리된 세션 {}", orphans.len())).font(fonts::semibold(11.5)).color(t.text_faint));
        });
        ui.add_space(2.0);
        for o in orphans {
            let label = o.fg_process.clone().unwrap_or_else(|| "셸".into());
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(rect, CornerRadius::same(8), t.bg_hover);
            }
            ui.painter().circle_filled(pos2(rect.left() + 14.0, rect.center().y), 3.0, t.text_faint);
            let cwd = o.cwd.as_deref().map(|c| short_path(Path::new(c))).unwrap_or_default();
            let p = ui.painter().with_clip_rect(rect.shrink2(vec2(4.0, 0.0)));
            let g = ui.painter().layout_no_wrap(label, fonts::medium(12.5), t.text_dim);
            let w = g.size().x;
            p.galley(pos2(rect.left() + 24.0, rect.center().y - g.size().y / 2.0), g, t.text_dim);
            p.text(pos2(rect.left() + 32.0 + w, rect.center().y), Align2::LEFT_CENTER, cwd, fonts::regular(11.5), t.text_faint);
            let resp = resp.on_hover_text("눌러서 현재 스페이스에 붙이기");
            if resp.clicked() {
                self.actions.push(Action::AttachSession(o.id));
            }
            resp.context_menu(|ui| {
                if ui.button("붙이기").clicked() {
                    self.actions.push(Action::AttachSession(o.id));
                    ui.close();
                }
                if ui.button(RichText::new("세션 종료").color(t.red)).clicked() {
                    self.actions.push(Action::KillSession(o.id));
                    ui.close();
                }
            });
        }
    }

    // ------------------------------------------------------------------ 카드 캔버스

    pub(super) fn ui_canvas(&mut self, root: &mut egui::Ui) {
        let t = self.theme;
        let canvas = widgets::canvas_color(&t);
        egui::CentralPanel::default().frame(Frame::new().fill(canvas).inner_margin(Margin { left: 4, right: 10, top: 0, bottom: 10 })).show(root, |ui| {
            let area = ui.max_rect();
            let ws_idx = self.active;
            let (root_node, focused, zoomed) = {
                let p = self.workspaces[ws_idx].page();
                (p.root.clone(), p.focused, p.zoomed)
            };
            let mut rects = Vec::new();
            match zoomed {
                Some(z) if root_node.panes().contains(&z) => rects.push((z, area)),
                _ => root_node.layout(area, &mut rects),
            }
            let multi = rects.len() > 1;
            let focus_req = self.focus_terminal && self.confirm.is_none() && !self.palette.is_open() && !self.settings_ui.open;
            let mut focus_consumed = false;
            let mut new_focus = None;
            let settings = terminal::TermSettings { font_size: self.settings.font_size, option_as_meta: self.settings.option_as_meta, line_height: self.settings.line_height };
            for (pid, rect) in &rects {
                let is_focused = *pid == focused;
                let info = self.card_info(*pid);
                let session = self.panes.get(pid).and_then(|p| p.session());
                let attention = session.and_then(|s| self.conn.infos.get(&s)).is_some_and(|i| i.attention);
                let cwd = session.and_then(|s| self.conn.infos.get(&s)).and_then(|i| i.cwd.clone());
                ui.painter().rect_filled(*rect, CornerRadius::same(10), t.bg);
                let header = egui::Rect::from_min_size(rect.min, vec2(rect.width(), HEADER_H));
                let body = egui::Rect::from_min_max(pos2(rect.left() + 1.0, header.bottom() + 1.0), pos2(rect.right() - 1.0, rect.bottom() - 5.0));
                let hresp = ui.interact(header, ui.id().with(("hdr", *pid)), Sense::click());
                self.card_header(ui, *pid, header, &info, is_focused, multi, zoomed.is_some());
                if hresp.clicked() && !is_focused {
                    new_focus = Some(*pid);
                }
                if hresp.double_clicked() {
                    self.actions.push(Action::ToggleZoom(Some(*pid)));
                }
                let mut child = ui.new_child(UiBuilder::new().max_rect(body).id_salt(("card", *pid)));
                child.set_clip_rect(body);
                match self.panes.get_mut(pid).map(|p| &mut p.kind) {
                    Some(PaneKind::Term { view: Some(view), session: Some(_), .. }) => {
                        view.fill_background = false;
                        let out = view.ui(&mut child, &mut self.conn, &settings, focus_req && is_focused, cwd.as_deref());
                        if focus_req && is_focused {
                            focus_consumed = true;
                        }
                        if out.clicked && !is_focused {
                            new_focus = Some(*pid);
                        }
                        if let Some(target) = out.open {
                            self.actions.push(Action::OpenLink(target));
                        }
                        if out.restart {
                            self.actions.push(Action::RestartPane(*pid));
                        }
                        if out.clicked && attention {
                            if let Some(s) = session {
                                self.conn.send(kiln_proto::ClientMsg::ClearAttention { session: s });
                            }
                        }
                    }
                    Some(PaneKind::Tool(tool)) => {
                        let acts = tool.ui(&mut child);
                        if !is_focused && child.ui_contains_pointer() && ui.input(|i| i.pointer.any_pressed()) {
                            new_focus = Some(*pid);
                        }
                        self.actions.extend(acts);
                    }
                    _ => {
                        let msg = if self.conn.is_connected() { "셸을 시작하는 중…" } else { "데몬에 연결하는 중…" };
                        child.painter().text(body.center(), Align2::CENTER_CENTER, msg, fonts::regular(13.0), t.text_faint);
                    }
                }
                let stroke = if attention {
                    Stroke::new(1.5, t.orange)
                } else if is_focused && multi {
                    Stroke::new(1.5, t.accent_soft(190))
                } else {
                    Stroke::new(1.0, t.border)
                };
                ui.painter().rect_stroke(*rect, CornerRadius::same(10), stroke, StrokeKind::Inside);
            }
            if focus_consumed {
                self.focus_terminal = false;
            }

            // 카드 사이 간격을 끌어 크기를 바꾼다.
            let mut ratio_change = None;
            if zoomed.is_none() {
                let mut seps = Vec::new();
                root_node.splitters(area, 0, &mut seps);
                for (path, dir, sep, parent) in seps {
                    let resp = ui.interact(sep, ui.id().with(("sep", path)), Sense::drag());
                    if resp.hovered() || resp.dragged() {
                        let line = match dir {
                            layout::Dir::Horizontal => egui::Rect::from_center_size(sep.center(), vec2(2.0, (sep.height() - 24.0).max(10.0))),
                            layout::Dir::Vertical => egui::Rect::from_center_size(sep.center(), vec2((sep.width() - 24.0).max(10.0), 2.0)),
                        };
                        ui.painter().rect_filled(line, CornerRadius::same(1), t.accent_soft(170));
                        ui.ctx().set_cursor_icon(if dir == layout::Dir::Horizontal { CursorIcon::ResizeHorizontal } else { CursorIcon::ResizeVertical });
                    }
                    if resp.dragged() {
                        if let Some(p) = resp.interact_pointer_pos() {
                            let r = match dir {
                                layout::Dir::Horizontal => (p.x - parent.left()) / parent.width(),
                                layout::Dir::Vertical => (p.y - parent.top()) / parent.height(),
                            };
                            ratio_change = Some((path, r));
                        }
                    }
                    if resp.double_clicked() {
                        ratio_change = Some((path, 0.5));
                    }
                }
            }
            let page = self.workspaces[ws_idx].page_mut();
            page.rects = rects;
            if let Some((path, r)) = ratio_change {
                page.root.set_ratio(path, r);
            }
            if let Some(f) = new_focus {
                page.focused = f;
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn card_header(&mut self, ui: &mut egui::Ui, pid: PaneId, rect: egui::Rect, info: &CardInfo, focused: bool, multi: bool, zoomed: bool) {
        let t = self.theme;
        let cy = rect.center().y;
        ui.painter().line_segment([pos2(rect.left() + 1.0, rect.bottom()), pos2(rect.right() - 1.0, rect.bottom())], Stroke::new(1.0, t.border));
        let hovered = ui.rect_contains_pointer(rect);
        let mut x = rect.left() + 14.0;
        widgets::status_dot(ui, pos2(x, cy), info.dot, info.pulse);
        x += 11.0;
        icons::paint(ui.painter(), egui::Rect::from_center_size(pos2(x + 6.0, cy), vec2(12.0, 12.0)), info.icon, if focused { t.text_dim } else { t.text_faint });
        x += 19.0;
        let show_buttons = hovered || (focused && multi);
        let buttons_w = if show_buttons { 4.0 * 26.0 + 6.0 } else { 0.0 };
        let right_limit = rect.right() - 10.0 - buttons_w;
        let clip = egui::Rect::from_min_max(pos2(x, rect.top()), pos2(right_limit, rect.bottom()));
        let p = ui.painter().with_clip_rect(clip);
        let g = ui.painter().layout_no_wrap(info.name.clone(), fonts::semibold(12.5), t.text);
        let name_w = g.size().x;
        p.galley(pos2(x, cy - g.size().y / 2.0), g, if focused || !multi { t.text } else { t.text_dim });
        x += name_w + 10.0;
        if !info.detail.is_empty() {
            let g = ui.painter().layout_no_wrap(info.detail.clone(), fonts::regular(12.0), t.text_dim);
            let w = g.size().x;
            p.galley(pos2(x, cy - g.size().y / 2.0), g, t.text_dim);
            x += w + 10.0;
        }
        if !info.right.is_empty() {
            let g = ui.painter().layout_no_wrap(info.right.clone(), fonts::regular(11.5), t.text_faint);
            let rx = (right_limit - g.size().x - 4.0).max(x);
            p.galley(pos2(rx, cy - g.size().y / 2.0), g, t.text_faint);
        }
        if show_buttons {
            let br = egui::Rect::from_min_max(pos2(rect.right() - 8.0 - buttons_w, cy - 12.0), pos2(rect.right() - 8.0, cy + 12.0));
            let mut bui = ui.new_child(UiBuilder::new().max_rect(br).layout(egui::Layout::right_to_left(egui::Align::Center)));
            bui.spacing_mut().item_spacing.x = 2.0;
            if widgets::icon_button(&mut bui, Icon::Close, 24.0, false, "카드 닫기 (⌘W)").clicked() {
                self.actions.push(Action::ClosePane(pid, false));
            }
            let (zi, zt) = if zoomed { (Icon::Restore, "원래 배치로 (⇧⌘↩)") } else { (Icon::Maximize, "이 카드만 크게 (⇧⌘↩)") };
            if widgets::icon_button(&mut bui, zi, 24.0, false, zt).clicked() {
                self.actions.push(Action::ToggleZoom(Some(pid)));
            }
            if widgets::icon_button(&mut bui, Icon::SplitDown, 24.0, false, "아래로 나누기 (⇧⌘D)").clicked() {
                self.actions.push(Action::SplitPane(pid, layout::Dir::Vertical));
            }
            if widgets::icon_button(&mut bui, Icon::SplitRight, 24.0, false, "오른쪽으로 나누기 (⌘D)").clicked() {
                self.actions.push(Action::SplitPane(pid, layout::Dir::Horizontal));
            }
        }
    }

    // ------------------------------------------------------------------ 도구 시트

    pub(super) fn ui_sheet(&mut self, ctx: &egui::Context) {
        let t = self.theme;
        let ws_idx = self.active;
        let open = self.workspaces[ws_idx].sheet;
        let Some(kind) = open else { return };
        let k = ctx.animate_bool_with_time(egui::Id::new(("sheet-open", self.workspaces[ws_idx].id, kind.as_str())), true, 0.14);
        let screen = ctx.content_rect();
        let width = 400.0f32.min(screen.width() * 0.5);
        let top = screen.top() + TOPBAR_H;
        let shift = (1.0 - k) * 24.0;
        let rect = egui::Rect::from_min_max(pos2(screen.right() - width - 10.0 + shift, top), pos2(screen.right() - 10.0 + shift, screen.bottom() - 10.0));
        let mut acts = Vec::new();
        egui::Area::new(egui::Id::new(("sheet", self.workspaces[ws_idx].id))).fixed_pos(rect.min).order(egui::Order::Middle).show(ctx, |ui| {
            ui.set_opacity(k.max(0.2));
            Frame::new().fill(t.bg_panel).stroke(Stroke::new(1.0, t.border_strong)).corner_radius(CornerRadius::same(12)).shadow(t.shadow()).show(ui, |ui| {
                ui.set_min_size(rect.size());
                ui.set_max_size(rect.size());
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for tk in tools::ToolKind::ALL {
                        if tk == kind {
                            let g = ui.painter().layout_no_wrap(tk.label().to_string(), fonts::semibold(12.5), t.text);
                            let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 38.0, 30.0), Sense::hover());
                            ui.painter().rect_filled(r, CornerRadius::same(8), t.bg_selected);
                            icons::paint(ui.painter(), egui::Rect::from_center_size(pos2(r.left() + 16.0, r.center().y), vec2(14.0, 14.0)), tk.vicon(), t.accent);
                            ui.painter().galley(pos2(r.left() + 28.0, r.center().y - g.size().y / 2.0), g, t.text);
                        } else if widgets::icon_button(ui, tk.vicon(), 30.0, false, &format!("{} ({})", tk.label(), tk.shortcut())).clicked() {
                            acts.push(Action::ToggleSheet(tk));
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(6.0);
                        if widgets::icon_button(ui, Icon::Close, 28.0, false, "닫기 (Esc)").clicked() {
                            acts.push(Action::CloseSheet);
                        }
                    });
                });
                ui.add_space(6.0);
                let (sep, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
                ui.painter().rect_filled(sep, 0.0, t.border);
                let body = ui.available_rect_before_wrap();
                let mut child = ui.new_child(UiBuilder::new().max_rect(body.shrink2(vec2(0.0, 4.0))));
                child.set_clip_rect(body);
                acts.extend(self.workspaces[ws_idx].tools.panel_ui(&mut child, kind));
            });
        });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && ctx.memory(|m| m.focused().is_none()) {
            acts.push(Action::CloseSheet);
        }
        self.actions.extend(acts);
    }

    // ------------------------------------------------------------------ 오버레이

    pub(super) fn ui_overlays(&mut self, ctx: &egui::Context) {
        let t = self.theme;
        let acts = self.workspaces[self.active].tools.overlay_ui(ctx);
        self.actions.extend(acts);

        if self.palette.is_open() {
            let items = self.palette_items();
            if let Some(a) = self.palette.ui(ctx, items) {
                self.actions.push(a);
            }
            if !self.palette.is_open() {
                self.focus_terminal = true;
            }
        }

        if self.settings_ui.open {
            let info = settings::AboutInfo { daemon_pid: self.conn.daemon_pid, daemon_build: self.conn.daemon_build.clone(), connected: self.conn.is_connected() };
            let acts = self.settings_ui.ui(ctx, &mut self.settings, &info);
            self.actions.extend(acts);
            if !self.settings_ui.open {
                self.focus_terminal = true;
            }
        }

        let mut close_confirm = false;
        let mut ok = false;
        if let Some(c) = &self.confirm {
            let frame = Frame::new().fill(t.bg_elevated).stroke(Stroke::new(1.0, t.border_strong)).corner_radius(CornerRadius::same(14)).shadow(t.shadow()).inner_margin(Margin::same(22));
            egui::Modal::new(egui::Id::new("confirm")).frame(frame).backdrop_color(Color32::from_black_alpha(110)).show(ctx, |ui| {
                ui.set_width(390.0);
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
                    ui.painter().rect_filled(r, CornerRadius::same(10), Color32::from_rgba_unmultiplied(t.red.r(), t.red.g(), t.red.b(), 36));
                    icons::paint(ui.painter(), r.shrink(9.0), Icon::Warning, t.red);
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        ui.label(RichText::new(&c.title).font(fonts::semibold(15.5)).color(t.text));
                        ui.label(RichText::new(&c.body).size(13.0).color(t.text_dim));
                    });
                });
                ui.add_space(20.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::button(ui, &c.ok, ButtonKind::Danger).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        ok = true;
                    }
                    if widgets::button(ui, "취소", ButtonKind::Secondary).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        close_confirm = true;
                    }
                });
            });
        }
        if ok {
            if let Some(c) = self.confirm.take() {
                self.actions.push(c.action);
            }
        }
        if close_confirm {
            self.confirm = None;
            self.focus_terminal = true;
        }

        self.ui_toasts(ctx);
    }

    fn ui_toasts(&mut self, ctx: &egui::Context) {
        let t = self.theme;
        self.toasts.retain(|x| x.at.elapsed() < Duration::from_secs(if x.kind == ToastKind::Error { 8 } else { 6 }));
        if self.toasts.is_empty() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(200));
        let mut reveal = None;
        let mut dismiss = None;
        egui::Area::new(egui::Id::new("toasts")).anchor(Align2::RIGHT_BOTTOM, vec2(-20.0, -20.0)).order(egui::Order::Tooltip).show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            let n = self.toasts.len();
            for idx in (n.saturating_sub(4)..n).rev() {
                let toast = &self.toasts[idx];
                let (accent, icon) = match toast.kind {
                    ToastKind::Info => (t.accent, Icon::Sparkle),
                    ToastKind::Notify => (t.orange, Icon::Bell),
                    ToastKind::Error => (t.red, Icon::Warning),
                };
                let resp = Frame::new()
                    .fill(t.bg_elevated)
                    .stroke(Stroke::new(1.0, t.border_strong))
                    .corner_radius(CornerRadius::same(12))
                    .shadow(t.shadow())
                    .inner_margin(Margin { left: 14, right: 14, top: 12, bottom: 12 })
                    .show(ui, |ui| {
                        ui.set_width(320.0);
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
                            ui.painter().rect_filled(r, CornerRadius::same(8), Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 34));
                            icons::paint(ui.painter(), r.shrink(7.0), icon, accent);
                            ui.add_space(4.0);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.add(egui::Label::new(RichText::new(&toast.title).font(fonts::semibold(13.0)).color(t.text)).wrap());
                                if !toast.body.is_empty() {
                                    ui.add(egui::Label::new(RichText::new(&toast.body).size(12.5).color(t.text_dim)).wrap());
                                }
                                if toast.session.is_some() {
                                    ui.label(RichText::new("눌러서 이동").size(11.5).color(accent));
                                }
                            });
                        });
                    });
                let r = ui.interact(resp.response.rect, ui.id().with(("toast", idx)), Sense::click());
                if r.clicked() {
                    match toast.session {
                        Some(s) => reveal = Some((idx, s)),
                        None => dismiss = Some(idx),
                    }
                }
            }
        });
        if let Some((idx, s)) = reveal {
            self.toasts.remove(idx);
            self.reveal_session(s);
        } else if let Some(idx) = dismiss {
            self.toasts.remove(idx);
        }
    }

    fn palette_items(&self) -> Vec<palette::Item<Action>> {
        use palette::Group;
        let mut items: Vec<palette::Item<Action>> = Vec::new();
        let mut add = |group: Group, icon: Icon, label: String, hint: &str, a: Action| items.push(palette::Item { group, icon, label, hint: hint.into(), action: a });
        for ws in self.workspaces.iter() {
            for page in &ws.pages {
                for p in page.root.panes() {
                    if let Some(s) = self.panes.get(&p).and_then(|x| x.session()) {
                        let info = self.card_info(p);
                        let label = if info.detail.is_empty() { format!("{} — {}", info.name, ws.name) } else { format!("{} · {} — {}", info.name, info.detail, ws.name) };
                        add(Group::Sessions, Icon::Terminal, label, "", Action::RevealSession(s));
                    }
                }
            }
        }
        add(Group::Commands, Icon::Plus, "새 페이지".into(), "⌘T", Action::NewPage);
        add(Group::Commands, Icon::SplitRight, "오른쪽으로 나누기".into(), "⌘D", Action::Split(layout::Dir::Horizontal));
        add(Group::Commands, Icon::SplitDown, "아래로 나누기".into(), "⇧⌘D", Action::Split(layout::Dir::Vertical));
        add(Group::Commands, Icon::Maximize, "카드 크게 보기 전환".into(), "⇧⌘↩", Action::ToggleZoom(None));
        add(Group::Commands, Icon::Command, "카드 크기 균등하게".into(), "⌥⌘=", Action::Equalize);
        add(Group::Commands, Icon::Close, "카드 닫기".into(), "⌘W", Action::CloseActive);
        add(Group::Commands, Icon::File, "파일 빠르게 열기".into(), "⌘P", Action::QuickOpen);
        add(Group::Commands, Icon::Search, "터미널·에디터에서 찾기".into(), "⌘F", Action::FindInFocused);
        add(Group::Commands, Icon::Folder, "새 스페이스…".into(), "⌘N", Action::NewWorkspace(None));
        add(Group::Commands, Icon::Sidebar, "스페이스 레일 접기/펴기".into(), "⌘B", Action::ToggleSidebar);
        add(Group::Commands, Icon::Bell, "최근 알림으로 이동".into(), "⇧⌘U", Action::JumpUnread);
        for k in tools::ToolKind::ALL {
            add(Group::Tools, k.vicon(), k.label().into(), k.shortcut(), Action::ToggleSheet(k));
        }
        for (i, w) in self.workspaces.iter().enumerate() {
            add(Group::Spaces, Icon::Folder, w.name.clone(), &format!("⌘{}", i + 1), Action::SelectWorkspace(i));
        }
        for th in Theme::ALL {
            add(Group::Settings, Icon::Sparkle, format!("테마: {}", th.label), "", Action::SetTheme(th.name.into()));
        }
        add(Group::Settings, Icon::Gear, "설정 열기".into(), "⌘,", Action::OpenSettings);
        add(Group::Settings, Icon::Command, "글꼴 크게".into(), "⌘=", Action::FontDelta(1.0));
        add(Group::Settings, Icon::Command, "글꼴 작게".into(), "⌘-", Action::FontDelta(-1.0));
        add(Group::Settings, Icon::Sparkle, "데몬을 이 버전으로 교체".into(), "", Action::UpgradeDaemon);
        items
    }
}
