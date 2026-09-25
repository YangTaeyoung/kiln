//! 화면 구성: 액티비티 바, 워크스페이스 사이드바, 도구 패널, 탭, 분할 창, 상태 바, 오버레이.

use super::*;
use egui::{Align2, CornerRadius, CursorIcon, Frame, Margin, UiBuilder};

const ACTIVITY_W: f32 = 44.0;

fn icon_button(ui: &mut egui::Ui, icon: icons::Icon, active: bool, tip: &str, theme: &Theme) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ACTIVITY_W, 40.0), Sense::click());
    let color = if active { theme.text } else if resp.hovered() { theme.text_dim } else { theme.text_faint };
    if active {
        ui.painter().rect_filled(Rect::from_min_size(rect.min + vec2(0.0, 6.0), vec2(2.0, rect.height() - 12.0)), 1.0, theme.accent);
    }
    icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(20.0, 20.0)), icon, color);
    resp.on_hover_text(tip)
}

impl KilnApp {
    pub(super) fn ui_sidebar(&mut self, root: &mut egui::Ui) {
        let theme = self.theme;
        egui::Panel::left("activity")
            .exact_size(ACTIVITY_W)
            .resizable(false)
            .frame(Frame::new().fill(theme.bg).stroke(Stroke::NONE))
            .show(root, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                ui.add_space(6.0);
                if icon_button(ui, icons::Icon::Menu, self.sidebar_open, "워크스페이스 (⌘B)", &theme).clicked() {
                    self.actions.push(Action::ToggleSidebar);
                }
                ui.add_space(6.0);
                let (tool, open) = self.workspaces.get(self.active).map(|w| (w.tool, w.tool_open)).unwrap_or((tools::ToolKind::Explorer, false));
                for k in tools::ToolKind::ALL {
                    let sc = match k {
                        tools::ToolKind::Explorer => "⇧⌘E",
                        tools::ToolKind::Search => "⇧⌘F",
                        tools::ToolKind::Git => "⇧⌘G",
                        tools::ToolKind::PullRequests => "⇧⌘R",
                        tools::ToolKind::Database => "⇧⌘B",
                    };
                    if icon_button(ui, k.vicon(), open && tool == k, &format!("{} ({sc})", k.label()), &theme).clicked() {
                        self.actions.push(Action::ToggleTool(k));
                    }
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                    ui.add_space(6.0);
                    if icon_button(ui, icons::Icon::Gear, self.settings_open, "설정 (⌘,)", &theme).clicked() {
                        self.actions.push(Action::OpenSettings);
                    }
                    if icon_button(ui, icons::Icon::Command, self.palette.is_open(), "명령 팔레트 (⇧⌘P)", &theme).clicked() {
                        self.actions.push(Action::OpenPalette);
                    }
                });
            });

        if !self.sidebar_open {
            return;
        }
        egui::Panel::left("workspaces")
            .default_size(232.0)
            .size_range(170.0..=420.0)
            .frame(Frame::new().fill(theme.bg_panel).inner_margin(Margin::symmetric(8, 8)))
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("워크스페이스").size(11.5).color(theme.text_faint).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icons::button(ui, icons::Icon::Plus, vec2(22.0, 22.0), theme.text_dim, theme.bg_hover, "새 워크스페이스 (⌘N)").clicked() {
                            self.actions.push(Action::NewWorkspace(None));
                        }
                    });
                });
                ui.add_space(6.0);
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    for i in 0..self.workspaces.len() {
                        self.workspace_card(ui, i);
                        ui.add_space(4.0);
                    }
                    self.detached_sessions(ui);
                });
            });
    }

    fn workspace_sessions(&self, i: usize) -> Vec<SessionId> {
        let ws = &self.workspaces[i];
        ws.tabs.iter().filter_map(|t| match &t.kind {
            TabKind::Terminal(tt) => Some(tt.root.panes()),
            _ => None,
        }).flatten().filter_map(|p| self.panes.get(&p).and_then(|x| x.session)).collect()
    }

    fn workspace_card(&mut self, ui: &mut egui::Ui, i: usize) {
        let theme = self.theme;
        let active = i == self.active;
        let sessions = self.workspace_sessions(i);
        let infos: Vec<&kiln_proto::SessionInfo> = sessions.iter().filter_map(|s| self.conn.infos.get(s)).collect();
        let attention = infos.iter().filter(|x| x.attention).count();
        let last_note = infos.iter().filter(|x| x.attention).filter_map(|x| x.last_notification.clone()).next();
        let procs: Vec<(String, bool)> = {
            let mut v: Vec<(String, bool)> = Vec::new();
            for inf in &infos {
                if let Some(p) = &inf.fg_process
                    && !shells().contains(&p.as_str()) && !v.iter().any(|(n, _)| n == p) {
                        v.push((p.clone(), inf.attention));
                    }
            }
            v
        };
        let summary = self.workspaces[i].tools.summary();
        let ws = &mut self.workspaces[i];

        let fill = if active { theme.bg_selected } else { theme.bg_panel };
        let frame = Frame::new().fill(fill).corner_radius(CornerRadius::same(6)).inner_margin(Margin { left: 10, right: 8, top: 7, bottom: 7 });
        let resp = frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.horizontal(|ui| {
                if i < 9 {
                    ui.label(RichText::new(format!("{}", i + 1)).size(10.5).color(theme.text_faint).monospace());
                }
                if let Some(buf) = &mut ws.renaming {
                    let te = ui.add(egui::TextEdit::singleline(buf).desired_width(ui.available_width() - 30.0));
                    te.request_focus();
                    if te.lost_focus() {
                        let name = buf.clone();
                        ws.renaming = None;
                        self.actions.push(Action::RenameWorkspace(i, name));
                    }
                } else {
                    ui.label(RichText::new(&ws.name).size(13.0).strong().color(if active { theme.text } else { theme.text_dim }));
                }
                if attention > 0 {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(18.0, 16.0), Sense::hover());
                        ui.painter().rect_filled(r, 8.0, theme.orange);
                        ui.painter().text(r.center(), Align2::CENTER_CENTER, attention.to_string(), FontId::proportional(10.5), Color32::BLACK);
                    });
                }
            });
            ui.label(RichText::new(short_path(&ws.root)).size(11.0).color(theme.text_faint));
            if let Some(s) = &summary {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let (r, _) = ui.allocate_exact_size(vec2(12.0, 14.0), Sense::hover());
                    icons::paint(ui.painter(), r, icons::Icon::Branch, theme.purple);
                    ui.label(RichText::new(&s.branch).size(11.0).color(theme.purple));
                    if s.dirty > 0 {
                        ui.label(RichText::new(format!("●{}", s.dirty)).size(10.5).color(theme.yellow));
                    }
                    if s.ahead > 0 {
                        ui.label(RichText::new(format!("↑{}", s.ahead)).size(10.5).color(theme.text_dim));
                    }
                    if s.behind > 0 {
                        ui.label(RichText::new(format!("↓{}", s.behind)).size(10.5).color(theme.text_dim));
                    }
                    if let Some((n, st)) = &s.pr {
                        ui.label(RichText::new(format!("#{n} {st}")).size(10.5).color(theme.green));
                    }
                });
            }
            if !procs.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (p, att) in &procs {
                        let color = if *att { theme.orange } else { theme.green };
                        let text = RichText::new(format!("● {p}")).size(10.5).color(color);
                        ui.label(text);
                    }
                });
            }
            if let Some(n) = last_note {
                ui.label(RichText::new(n).size(10.5).italics().color(theme.text_dim)).on_hover_text("알림");
            }
        });
        let resp = ui.interact(resp.response.rect, ui.id().with(("ws", i)), Sense::click());
        if resp.clicked() {
            self.actions.push(Action::SelectWorkspace(i));
        }
        if resp.double_clicked() {
            self.workspaces[i].renaming = Some(self.workspaces[i].name.clone());
        }
        if resp.hovered() && !active {
            ui.painter().rect_stroke(resp.rect, 6.0, Stroke::new(1.0, theme.border), egui::StrokeKind::Inside);
        }
        if active {
            ui.painter().rect_filled(Rect::from_min_size(resp.rect.min + vec2(0.0, 8.0), vec2(3.0, resp.rect.height() - 16.0)), 2.0, theme.accent);
        }
        resp.context_menu(|ui| {
            if ui.button("이름 바꾸기").clicked() {
                self.workspaces[i].renaming = Some(self.workspaces[i].name.clone());
                ui.close();
            }
            if ui.button("폴더 열기").clicked() {
                let _ = open::that_detached(&self.workspaces[i].root);
                ui.close();
            }
            ui.separator();
            if ui.button(RichText::new("워크스페이스 닫기 (세션 종료)").color(theme.red)).clicked() {
                self.confirm = Some(Confirm {
                    title: "워크스페이스 닫기".into(),
                    body: format!("'{}' 의 모든 터미널 세션이 종료됩니다.", self.workspaces[i].name),
                    ok: "닫기".into(),
                    action: Action::CloseWorkspace(i),
                });
                ui.close();
            }
        });
    }

    fn detached_sessions(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme;
        let used: std::collections::HashSet<SessionId> = self.panes.values().filter_map(|p| p.session).collect();
        let mut orphans: Vec<&kiln_proto::SessionInfo> = self.conn.infos.values().filter(|i| !used.contains(&i.id)).collect();
        if orphans.is_empty() {
            return;
        }
        orphans.sort_by_key(|i| i.id);
        ui.add_space(10.0);
        ui.label(RichText::new(format!("분리된 세션 ({})", orphans.len())).size(11.5).color(theme.text_faint).strong());
        let mut acts = Vec::new();
        for o in orphans {
            let label = format!(
                "#{} {} {}",
                o.id,
                o.fg_process.clone().unwrap_or_else(|| "shell".into()),
                o.cwd.as_deref().map(|c| short_path(Path::new(c))).unwrap_or_default()
            );
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new(label).size(11.5)).frame(false)).on_hover_text("현재 워크스페이스에 붙이기").clicked() {
                    acts.push(Action::AttachSession(o.id));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icons::button(ui, icons::Icon::Close, vec2(18.0, 18.0), theme.text_faint, theme.bg_hover, "세션 종료").clicked() {
                        acts.push(Action::KillSession(o.id));
                    }
                });
            });
        }
        self.actions.extend(acts);
    }

    pub(super) fn ui_statusbar(&mut self, root: &mut egui::Ui) {
        let theme = self.theme;
        egui::Panel::bottom("status").exact_size(24.0).frame(Frame::new().fill(theme.bg).inner_margin(Margin::symmetric(10, 3))).show(root, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                let (dot, text) = if self.conn.is_connected() {
                    (theme.green, format!("데몬 pid {}", self.conn.daemon_pid))
                } else {
                    (theme.red, "데몬 재연결 중…".to_string())
                };
                ui.label(RichText::new(format!("● {text}")).size(11.0).color(dot)).on_hover_text(format!("build {}", self.conn.daemon_build));
                if let Some(ws) = self.workspaces.get(self.active)
                    && let Some(s) = ws.tools.summary() {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let (r, _) = ui.allocate_exact_size(vec2(12.0, 14.0), Sense::hover());
                        icons::paint(ui.painter(), r, icons::Icon::Branch, theme.text_dim);
                        ui.label(RichText::new(s.branch).size(11.0).color(theme.text_dim));
                        ui.spacing_mut().item_spacing.x = 14.0;
                    }
                let unread = self.unread.len();
                if unread > 0 && ui.add(egui::Button::new(RichText::new(format!("알림 {unread}")).size(11.0).color(theme.orange)).frame(false)).on_hover_text("최근 알림으로 이동 (⇧⌘U)").clicked() {
                    self.actions.push(Action::JumpUnread);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let tool_status = self.workspaces.get(self.active).and_then(|w| match w.tabs.get(w.active_tab).map(|t| &t.kind) {
                        Some(TabKind::Tool(t)) => t.status_text(),
                        _ => None,
                    });
                    if let Some(st) = tool_status {
                        ui.label(RichText::new(st).size(11.0).color(theme.text_dim));
                        return;
                    }
                    ui.label(RichText::new(format!("{:.1}pt", self.settings.font_size)).size(11.0).color(theme.text_faint));
                    if let Some(info) = self.focused_session().and_then(|s| self.conn.infos.get(&s)) {
                        ui.label(RichText::new(format!("{}×{}", info.cols, info.rows)).size(11.0).color(theme.text_faint));
                        if let Some(c) = &info.cwd {
                            ui.label(RichText::new(short_path(Path::new(c))).size(11.0).color(theme.text_dim));
                        }
                        if let Some(p) = &info.fg_process {
                            ui.label(RichText::new(p).size(11.0).color(theme.text));
                        }
                    }
                });
            });
        });
    }

    pub(super) fn ui_tool_panel(&mut self, root: &mut egui::Ui) {
        let theme = self.theme;
        let Some(ws) = self.workspaces.get_mut(self.active) else { return };
        if !ws.tool_open {
            return;
        }
        let kind = ws.tool;
        let mut acts = Vec::new();
        egui::Panel::left(egui::Id::new(("tool", ws.id)))
            .default_size(300.0)
            .size_range(200.0..=700.0)
            .frame(Frame::new().fill(theme.bg_panel).inner_margin(Margin { left: 0, right: 0, top: 6, bottom: 0 }).stroke(Stroke::new(1.0, theme.border)))
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.label(RichText::new(kind.label().to_uppercase()).size(11.0).strong().color(theme.text_faint));
                });
                ui.add_space(4.0);
                acts = ws.tools.panel_ui(ui, kind);
            });
        self.actions.extend(acts);
    }

    fn tab_title(&self, t: &Tab) -> (String, bool) {
        match &t.kind {
            TabKind::Terminal(tt) => {
                let attention = tt.root.panes().iter().any(|p| self.panes.get(p).and_then(|x| x.session).and_then(|s| self.conn.infos.get(&s)).is_some_and(|i| i.attention));
                if let Some(title) = &tt.title {
                    return (title.clone(), attention);
                }
                let info = self.panes.get(&tt.focused).and_then(|p| p.session).and_then(|s| self.conn.infos.get(&s));
                let title = match info {
                    Some(i) => {
                        let dir = i.cwd.as_deref().and_then(|c| Path::new(c).file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                        let proc_name = i.fg_process.clone().unwrap_or_else(|| "shell".into());
                        let n = tt.root.panes().len();
                        let base = if shells().contains(&proc_name.as_str()) { format!("{proc_name} · {dir}") } else { proc_name };
                        if n > 1 { format!("{base}  ({n})") } else { base }
                    }
                    None => "터미널".into(),
                };
                (title, attention)
            }
            TabKind::Tool(x) => (format!("{} {}", x.icon(), x.title()), false),
        }
    }

    pub(super) fn ui_center(&mut self, root: &mut egui::Ui) {
        let theme = self.theme;
        egui::CentralPanel::default().frame(Frame::new().fill(theme.bg)).show(root, |ui| {
            if self.workspaces.is_empty() {
                return;
            }
            // 탭 바.
            let bar = Rect::from_min_size(ui.max_rect().min, vec2(ui.max_rect().width(), 34.0));
            ui.painter().rect_filled(bar, 0.0, theme.bg_panel);
            ui.painter().line_segment([bar.left_bottom(), bar.right_bottom()], Stroke::new(1.0, theme.border));
            let titles: Vec<(String, bool, bool)> = {
                let ws = &self.workspaces[self.active];
                ws.tabs.iter().map(|t| {
                    let (s, a) = self.tab_title(t);
                    let dirty = matches!(&t.kind, TabKind::Tool(x) if x.is_dirty());
                    (s, a, dirty)
                }).collect()
            };
            let active_tab = self.workspaces[self.active].active_tab;
            let mut bar_ui = ui.new_child(UiBuilder::new().max_rect(bar.shrink2(vec2(6.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
            egui::ScrollArea::horizontal().id_salt("tabs").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(&mut bar_ui, |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (i, (title, attention, dirty)) in titles.iter().enumerate() {
                    let is_active = i == active_tab;
                    let font = FontId::proportional(12.5);
                    let galley = ui.painter().layout_no_wrap(title.clone(), font.clone(), theme.text);
                    let w = (galley.size().x + 44.0).clamp(90.0, 260.0);
                    let (rect, resp) = ui.allocate_exact_size(vec2(w, 30.0), Sense::click());
                    let fill = if is_active { theme.bg } else if resp.hovered() { theme.bg_hover } else { theme.bg_panel };
                    ui.painter().rect_filled(rect.shrink2(vec2(0.0, 2.0)).translate(vec2(0.0, 2.0)), CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }, fill);
                    if is_active {
                        ui.painter().line_segment([rect.left_top() + vec2(4.0, 2.0), rect.right_top() + vec2(-4.0, 2.0)], Stroke::new(2.0, theme.accent));
                    }
                    let color = if is_active { theme.text } else { theme.text_dim };
                    let mut x = rect.left() + 12.0;
                    if *attention {
                        ui.painter().circle_filled(pos2(x + 2.0, rect.center().y + 1.0), 3.5, theme.orange);
                        x += 12.0;
                    }
                    let text_rect = Rect::from_min_max(pos2(x, rect.top()), pos2(rect.right() - 24.0, rect.bottom()));
                    ui.painter().with_clip_rect(text_rect).text(pos2(x, rect.center().y + 1.0), Align2::LEFT_CENTER, title, font, color);
                    let close_rect = Rect::from_center_size(pos2(rect.right() - 13.0, rect.center().y + 1.0), vec2(16.0, 16.0));
                    let close_resp = ui.interact(close_rect, ui.id().with(("close", i)), Sense::click());
                    if *dirty && !close_resp.hovered() {
                        ui.painter().circle_filled(close_rect.center(), 4.0, theme.text_dim);
                    } else if is_active || resp.hovered() || close_resp.hovered() {
                        if close_resp.hovered() {
                            ui.painter().rect_filled(close_rect, 3.0, theme.bg_hover);
                        }
                        ui.painter().text(close_rect.center(), Align2::CENTER_CENTER, "×", FontId::proportional(14.0), theme.text_dim);
                    }
                    if close_resp.clicked() || resp.middle_clicked() {
                        self.actions.push(Action::CloseTab(i, false));
                    } else if resp.clicked() {
                        self.actions.push(Action::SelectTab(i));
                    }
                    resp.context_menu(|ui| {
                        if ui.button("탭 닫기").clicked() {
                            self.actions.push(Action::CloseTab(i, false));
                            ui.close();
                        }
                    });
                }
                if icons::button(ui, icons::Icon::Plus, vec2(28.0, 28.0), theme.text_dim, theme.bg_hover, "새 터미널 탭 (⌘T)").clicked() {
                    self.actions.push(Action::NewTermTab);
                }
            });
            // 분할 버튼.
            let right = Rect::from_min_max(pos2(bar.right() - 70.0, bar.top()), bar.right_bottom());
            let mut rui = ui.new_child(UiBuilder::new().max_rect(right).layout(egui::Layout::right_to_left(egui::Align::Center)));
            if icons::button(&mut rui, icons::Icon::SplitDown, vec2(28.0, 28.0), theme.text_dim, theme.bg_hover, "아래로 분할 (⇧⌘D)").clicked() {
                self.actions.push(Action::Split(Dir::Vertical));
            }
            if icons::button(&mut rui, icons::Icon::SplitRight, vec2(28.0, 28.0), theme.text_dim, theme.bg_hover, "오른쪽으로 분할 (⌘D)").clicked() {
                self.actions.push(Action::Split(Dir::Horizontal));
            }

            let content = Rect::from_min_max(pos2(ui.max_rect().left(), bar.bottom()), ui.max_rect().max);
            let ws_idx = self.active;
            let tab_idx = self.workspaces[ws_idx].active_tab;
            let is_term = matches!(self.workspaces[ws_idx].tabs.get(tab_idx).map(|t| &t.kind), Some(TabKind::Terminal(_)));
            if is_term {
                self.ui_terminal_tab(ui, content);
            } else if let Some(Tab { kind: TabKind::Tool(t), .. }) = self.workspaces[ws_idx].tabs.get_mut(tab_idx) {
                let mut child = ui.new_child(UiBuilder::new().max_rect(content));
                let acts = t.ui(&mut child);
                self.actions.extend(acts);
            }
        });
    }

    fn ui_terminal_tab(&mut self, ui: &mut egui::Ui, content: Rect) {
        let theme = self.theme;
        let settings = TermSettings { font_size: self.settings.font_size, option_as_meta: self.settings.option_as_meta, line_height: 1.2 };
        let ws_idx = self.active;
        let tab_idx = self.workspaces[ws_idx].active_tab;
        let (root, focused) = match &self.workspaces[ws_idx].tabs[tab_idx].kind {
            TabKind::Terminal(tt) => (tt.root.clone(), tt.focused),
            _ => return,
        };
        let mut rects = Vec::new();
        root.layout(content, &mut rects);
        let multi = rects.len() > 1;
        let focus_req = self.focus_terminal && self.confirm.is_none() && !self.palette.is_open();
        let mut focus_consumed = false;
        let mut new_focus = None;
        for (pid, rect) in &rects {
            let Some(pane) = self.panes.get_mut(pid) else { continue };
            let mut child = ui.new_child(UiBuilder::new().max_rect(*rect).id_salt(("pane", *pid)));
            let is_focused = *pid == focused;
            let info = pane.session.and_then(|s| self.conn.infos.get(&s));
            let attention = info.is_some_and(|i| i.attention);
            let cwd = info.and_then(|i| i.cwd.clone());
            match (&mut pane.view, pane.session) {
                (Some(view), Some(_)) => {
                    let out = view.ui(&mut child, &mut self.conn, &settings, focus_req && is_focused, cwd.as_deref());
                    if focus_req && is_focused {
                        focus_consumed = true;
                    }
                    if out.clicked && !is_focused {
                        new_focus = Some(*pid);
                    }
                    if let Some(t) = out.open {
                        self.actions.push(Action::OpenLink(t, cwd.clone()));
                    }
                    if out.restart {
                        self.actions.push(Action::RestartPane(*pid));
                    }
                    if out.clicked && attention
                        && let Some(s) = pane.session {
                            self.conn.send(kiln_proto::ClientMsg::ClearAttention { session: s });
                        }
                }
                _ => {
                    child.painter().rect_filled(*rect, 0.0, theme.bg);
                    let msg = if self.conn.is_connected() { "세션 시작 중…" } else { "데몬 연결 대기 중…" };
                    child.painter().text(rect.center(), Align2::CENTER_CENTER, msg, FontId::proportional(13.0), theme.text_faint);
                }
            }
            if multi && is_focused {
                ui.painter().rect_stroke(*rect, 0.0, Stroke::new(1.0, theme.accent.gamma_multiply(0.7)), egui::StrokeKind::Inside);
            }
            if attention {
                ui.painter().rect_stroke(rect.shrink(1.0), 3.0, Stroke::new(2.0, theme.orange), egui::StrokeKind::Inside);
            }
        }
        // 분할선 드래그.
        let mut seps = Vec::new();
        root.splitters(content, 0, &mut seps);
        let mut ratio_change = None;
        for (path, dir, sep, parent) in seps {
            let hit = sep.expand2(if dir == Dir::Horizontal { vec2(3.0, 0.0) } else { vec2(0.0, 3.0) });
            let resp = ui.interact(hit, ui.id().with(("sep", path)), Sense::drag());
            let hovered = resp.hovered() || resp.dragged();
            ui.painter().rect_filled(sep, 0.0, if hovered { theme.accent.gamma_multiply(0.6) } else { theme.border });
            if hovered {
                ui.ctx().set_cursor_icon(if dir == Dir::Horizontal { CursorIcon::ResizeHorizontal } else { CursorIcon::ResizeVertical });
            }
            if resp.dragged()
                && let Some(p) = resp.interact_pointer_pos() {
                    let r = match dir {
                        Dir::Horizontal => (p.x - parent.left()) / parent.width(),
                        Dir::Vertical => (p.y - parent.top()) / parent.height(),
                    };
                    ratio_change = Some((path, r));
                }
            if resp.double_clicked() {
                ratio_change = Some((path, 0.5));
            }
        }
        if focus_consumed {
            self.focus_terminal = false;
        }
        if let TabKind::Terminal(tt) = &mut self.workspaces[ws_idx].tabs[tab_idx].kind {
            tt.rects = rects;
            if let Some((path, r)) = ratio_change {
                tt.root.set_ratio(path, r);
            }
            if let Some(f) = new_focus {
                tt.focused = f;
            }
        }
    }

    pub(super) fn ui_overlays(&mut self, root: &mut egui::Ui) {
        let ctx = &root.ctx().clone();
        let theme = self.theme;
        // 도구 오버레이(빠른 열기 등).
        if let Some(ws) = self.workspaces.get_mut(self.active) {
            let acts = ws.tools.overlay_ui(ctx);
            self.actions.extend(acts);
        }

        // 명령 팔레트.
        if self.palette.is_open() {
            let mut items: Vec<palette::Item<Action>> = Vec::new();
            let mut add = |label: &str, hint: &str, a: Action| items.push(palette::Item { label: label.into(), hint: hint.into(), action: a });
            add("새 터미널 탭", "⌘T", Action::NewTermTab);
            add("오른쪽으로 분할", "⌘D", Action::Split(Dir::Horizontal));
            add("아래로 분할", "⇧⌘D", Action::Split(Dir::Vertical));
            add("분할 크기 균등화", "⌥⌘=", Action::Equalize);
            add("창/탭 닫기", "⌘W", Action::CloseActive);
            add("새 워크스페이스…", "⌘N", Action::NewWorkspace(None));
            add("파일 빠르게 열기", "⌘P", Action::QuickOpen);
            add("터미널에서 찾기", "⌘F", Action::FindInTerminal);
            add("워크스페이스 사이드바 토글", "⌘B", Action::ToggleSidebar);
            for k in tools::ToolKind::ALL {
                add(&format!("{} 열기/닫기", k.label()), "", Action::ToggleTool(k));
            }
            add("최근 알림으로 이동", "⇧⌘U", Action::JumpUnread);
            add("글꼴 크게", "⌘=", Action::FontDelta(1.0));
            add("글꼴 작게", "⌘-", Action::FontDelta(-1.0));
            add("글꼴 크기 초기화", "⌘0", Action::FontDelta(0.0));
            add("설정", "⌘,", Action::OpenSettings);
            add("데몬 업그레이드 (세션 유지)", "", Action::UpgradeDaemon);
            for (i, w) in self.workspaces.iter().enumerate() {
                add(&format!("워크스페이스로 이동: {}", w.name), &format!("⌘{}", i + 1), Action::SelectWorkspace(i));
            }
            if let Some(a) = self.palette.ui(ctx, items) {
                self.actions.push(a);
            }
            if !self.palette.is_open() {
                self.focus_terminal = true;
            }
        }

        // 확인 대화상자.
        let mut close_confirm = false;
        if let Some(c) = &self.confirm {
            let mut ok = false;
            egui::Modal::new(egui::Id::new("confirm")).show(ctx, |ui| {
                ui.set_width(380.0);
                ui.label(RichText::new(&c.title).size(15.0).strong());
                ui.add_space(6.0);
                ui.label(RichText::new(&c.body).color(theme.text_dim));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new(RichText::new(&c.ok).color(Color32::WHITE)).fill(theme.red)).clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                            ok = true;
                        }
                        if ui.button("취소").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                            close_confirm = true;
                        }
                    });
                });
            });
            if ok
                && let Some(c) = self.confirm.take() {
                    self.actions.push(c.action);
                }
        }
        if close_confirm {
            self.confirm = None;
            self.focus_terminal = true;
        }

        // 설정.
        if self.settings_open {
            let mut open = true;
            egui::Window::new("설정").open(&mut open).collapsible(false).resizable(false).default_width(380.0).show(ctx, |ui| {
                egui::Grid::new("settings").num_columns(2).spacing(vec2(16.0, 10.0)).show(ui, |ui| {
                    ui.label("터미널 글꼴 크기");
                    ui.add(egui::Slider::new(&mut self.settings.font_size, 8.0..=32.0).step_by(0.5));
                    ui.end_row();
                    ui.label("UI 배율");
                    if ui.add(egui::Slider::new(&mut self.settings.ui_scale, 0.7..=1.8).step_by(0.05)).drag_stopped() {
                        ctx.set_zoom_factor(self.settings.ui_scale);
                    }
                    ui.end_row();
                    ui.label("Option 을 Meta 로");
                    ui.checkbox(&mut self.settings.option_as_meta, "Option+키 → ESC 접두");
                    ui.end_row();
                    ui.label("실행 중 닫기 확인");
                    ui.checkbox(&mut self.settings.confirm_close_running, "claude 등 실행 중이면 묻기");
                    ui.end_row();
                    ui.label("OS 알림");
                    ui.checkbox(&mut self.settings.os_notifications, "창이 비활성일 때 시스템 알림");
                    ui.end_row();
                    ui.label("셸");
                    ui.add(egui::TextEdit::singleline(&mut self.settings.shell).hint_text("기본: 로그인 셸"));
                    ui.end_row();
                });
                ui.add_space(8.0);
                ui.separator();
                ui.label(RichText::new(format!("데몬 pid {} · build {}", self.conn.daemon_pid, self.conn.daemon_build)).size(11.0).color(theme.text_faint));
                ui.label(RichText::new(format!("앱 build {}", kiln_daemon::build_id())).size(11.0).color(theme.text_faint));
            });
            if !open {
                self.settings_open = false;
                self.focus_terminal = true;
            }
        }

        // 토스트.
        self.toasts.retain(|t| t.at.elapsed() < Duration::from_secs(if t.kind == ToastKind::Error { 8 } else { 5 }));
        if !self.toasts.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(250));
            let screen = ctx.content_rect();
            egui::Area::new(egui::Id::new("toasts")).anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -36.0)).order(egui::Order::Tooltip).interactable(false).show(ctx, |ui| {
                ui.set_max_width(screen.width().min(380.0));
                for t in self.toasts.iter().rev().take(4) {
                    let fg = match t.kind {
                        ToastKind::Info => theme.accent,
                        ToastKind::Notify => theme.orange,
                        ToastKind::Error => theme.red,
                    };
                    let r = Frame::popup(ui.style()).fill(theme.bg_elevated).corner_radius(8.0).inner_margin(Margin { left: 16, right: 12, top: 8, bottom: 8 }).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if t.kind == ToastKind::Notify {
                                let (ir, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                                icons::paint(ui.painter(), ir, icons::Icon::Bell, fg);
                            }
                            ui.add(egui::Label::new(RichText::new(&t.text).color(theme.text)).wrap());
                        });
                    });
                    let rr = r.response.rect;
                    ui.painter().rect_filled(Rect::from_min_size(rr.min + vec2(5.0, 8.0), vec2(3.0, rr.height() - 16.0)), 2.0, fg);
                    ui.add_space(6.0);
                }
            });
        }
    }
}
