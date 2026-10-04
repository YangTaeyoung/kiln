//! Bounded, persisted notification inbox. Toasts are transient; this is the record.
use super::{Action, KilnApp, ToastKind};
use egui::{Color32, CornerRadius, Frame, Margin, RichText, Stroke, vec2};
use kiln_common::{fonts, widgets::{self, ButtonKind}, icons::Icon};
use serde::{Deserialize, Serialize};

const MAX_NOTIFICATIONS: usize = 200;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum NotificationCategory { #[default] Attention, Completed, Error, Info }
impl NotificationCategory {
    fn label(self) -> &'static str { match self { Self::Attention=>"확인 필요", Self::Completed=>"완료", Self::Error=>"오류", Self::Info=>"안내" } }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    pub title: String,
    pub body: String,
    pub session: Option<u64>,
    pub workspace: String,
    pub timestamp: u64,
    pub read: bool,
    pub error: bool,
    #[serde(default)]
    pub category: NotificationCategory,
    #[serde(default)]
    pub rotate_tool: Option<kiln_accounts::Tool>,
}

#[derive(Default)]
pub(super) struct NotificationCenter {
    pub items: Vec<Notification>,
    pub open: bool,
    unread_only: bool,
    category: Option<NotificationCategory>,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
}

impl NotificationCenter {
    pub fn new(mut items: Vec<Notification>) -> Self {
        for item in &mut items { if item.error { item.category = NotificationCategory::Error; } }
        if items.len() > MAX_NOTIFICATIONS { items.drain(..items.len() - MAX_NOTIFICATIONS); }
        Self { items, ..Default::default() }
    }

    pub fn push(&mut self, title: &str, body: &str, kind: ToastKind, session: Option<u64>, workspace: String) {
        // A noisy process cannot grow the app state without bound.
        if self.items.len() >= MAX_NOTIFICATIONS { self.items.remove(0); }
        self.items.push(Notification {
            title: title.chars().take(240).collect(), body: body.chars().take(4000).collect(), session,
            category: match kind { ToastKind::Error=>NotificationCategory::Error, ToastKind::Info=>NotificationCategory::Info, _=>NotificationCategory::Attention },
            workspace, timestamp: now(), read: kind == ToastKind::Info, error: kind == ToastKind::Error, rotate_tool: None,
        });
    }

    pub fn push_activity(&mut self, activity: kiln_proto::AgentActivity, session: u64, workspace: String, body: &str) {
        use kiln_proto::AgentActivity;
        let category = match activity { AgentActivity::Waiting=>NotificationCategory::Attention,AgentActivity::Done=>NotificationCategory::Completed,AgentActivity::Failed=>NotificationCategory::Error,_=>return };
        self.push(activity.label(),body,if category==NotificationCategory::Error {ToastKind::Error}else{ToastKind::Notify},Some(session),workspace);
        if let Some(item)=self.items.last_mut(){item.category=category;}
    }

    pub fn unread_count(&self) -> usize { self.items.iter().filter(|n| !n.read).count() }

    pub fn mark_session_read(&mut self, session: u64) {
        for item in &mut self.items { if item.session == Some(session) { item.read = true; } }
    }
}

fn age(timestamp: u64) -> String {
    let elapsed = now().saturating_sub(timestamp);
    match elapsed {
        0..=59 => "방금".into(),
        60..=3599 => format!("{}분 전", elapsed / 60),
        3600..=86399 => format!("{}시간 전", elapsed / 3600),
        _ => format!("{}일 전", elapsed / 86400),
    }
}

#[derive(Default, PartialEq, Debug)]
enum RowAction { #[default] None, Reveal, Rotate, Dismiss }

fn notification_row(ui: &mut egui::Ui, item: &mut Notification, available: bool, can_rotate: bool) -> RowAction {
    let t = kiln_common::Theme::current();
    let mut action = RowAction::None;
    let color = if item.error { t.red } else if item.read { t.text_dim } else { t.accent };
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(16.0, 20.0), egui::Sense::hover());
        kiln_common::icons::paint(ui.painter(), r.shrink2(vec2(0.0, 2.0)), if item.error { Icon::Warning } else { Icon::Bell }, color);
        let title_width=(ui.available_width()-60.0).max(40.0);
        let galley=ui.painter().layout(item.title.clone(),fonts::semibold(13.5),t.text,(title_width-8.0).max(32.0));
        let (rect,response)=ui.allocate_exact_size(vec2(title_width,(galley.size().y+8.0).max(24.0)),if available{egui::Sense::click()}else{egui::Sense::hover()});
        response.widget_info(||egui::WidgetInfo::labeled(if available{egui::WidgetType::Button}else{egui::WidgetType::Label},ui.is_enabled(),&item.title));
        if available && response.hovered(){ui.painter().rect_filled(rect,4,t.bg_hover);ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);}
        ui.painter().galley(egui::pos2(rect.left()+4.0,rect.center().y-galley.size().y/2.0),galley,t.text);
        widgets::focus_ring(ui,&response,4);
        if available && response.on_hover_text("세션으로 이동").clicked(){item.read=true;action=RowAction::Reveal;}
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::icon_button(ui, Icon::Close, 24.0, false, "알림 지우기").clicked() { action = RowAction::Dismiss; }
            if widgets::icon_button(ui, Icon::Check, 24.0, item.read, if item.read { "읽지 않음으로 표시" } else { "읽음으로 표시" }).clicked() { item.read = !item.read; }
        });
    });
    if !item.body.is_empty() && item.body != item.title {
        ui.add(egui::Label::new(RichText::new(&item.body).size(12.5).color(t.text_dim)).wrap());
    }
    ui.horizontal_wrapped(|ui| {
        let context = if item.workspace.is_empty() { age(item.timestamp) } else { format!("{} · {}", item.workspace, age(item.timestamp)) };
        ui.label(RichText::new(context).size(11.5).color(t.text_dim));
        if item.title != item.category.label() { ui.label(RichText::new(item.category.label()).size(11.5).color(color)); }
        if item.session.is_some() && !available { ui.label(RichText::new("세션 연결 안 됨").size(11.5).color(t.text_dim)); }
        if item.rotate_tool.is_some() && item.session.is_some() {
            ui.add_enabled_ui(can_rotate, |ui| {
                if widgets::button(ui, "다음 계정으로 전환", ButtonKind::Secondary).clicked() { item.read = true; action = RowAction::Rotate; }
            });
        }
    });
    action
}

impl KilnApp {
    pub(super) fn ui_notifications(&mut self, ctx: &egui::Context) {
        if !self.notifications.open { return; }
        let t = self.theme;
        let width = 520.0f32.min(ctx.content_rect().width() - 64.0);
        let mut close = false;
        let mut reveal = None;
        let mut mark_all = false;
        let mut rotate = None;
        let mut dismiss = None;
        let unread_before: std::collections::HashSet<_> = self.notifications.items.iter().filter(|n| !n.read).filter_map(|n| n.session).collect();
        let frame = Frame::new().fill(t.bg_panel).stroke(Stroke::new(1.0, t.border_strong))
            .corner_radius(CornerRadius::same(12)).shadow(t.shadow()).inner_margin(Margin::same(20));
        let modal = egui::Modal::new(egui::Id::new("notification-center")).frame(frame)
            .backdrop_color(Color32::from_black_alpha(if t.dark { 120 } else { 50 })).show(ctx, |ui| {
                ui.set_width(width);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("알림").font(fonts::semibold(22.0)));
                    if self.notifications.unread_count() > 0 { widgets::pill(ui, &self.notifications.unread_count().to_string(), t.accent); }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        close = widgets::icon_button(ui, Icon::Close, 30.0, false, "알림 닫기 (Esc)").clicked();
                    });
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    widgets::segmented(ui, &mut self.notifications.unread_only, &[(false, "전체"), (true, "읽지 않음")]);
                let filter = widgets::icon_button(ui, Icon::Filter, 28.0, self.notifications.category.is_some(), "알림 종류 필터");
                egui::Popup::menu(&filter).show(|ui| {
                    ui.selectable_value(&mut self.notifications.category, None, "모든 종류");
                    for c in [NotificationCategory::Attention, NotificationCategory::Completed, NotificationCategory::Error, NotificationCategory::Info] {
                        ui.selectable_value(&mut self.notifications.category, Some(c), c.label());
                    }
                });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_enabled_ui(self.notifications.unread_count() > 0, |ui| {
                            mark_all = widgets::icon_button(ui, Icon::Check, 28.0, false, "모두 읽음").clicked();
                        });
                    });
                });
                ui.add_space(4.0);
                widgets::divider(ui);
                egui::ScrollArea::vertical().id_salt("inbox-list").max_height((ctx.content_rect().height() - 290.0).clamp(48.0, 480.0)).auto_shrink([false, true]).show(ui, |ui| {
                    let mut visible = 0;
                    for (index, item) in self.notifications.items.iter_mut().enumerate().rev() {
                        if self.notifications.unread_only && item.read { continue; }
                        if self.notifications.category.is_some_and(|c| c != item.category) { continue; }
                        visible += 1;
                        ui.push_id(index, |ui| {
                            Frame::new().fill(if item.read { Color32::TRANSPARENT } else { t.bg_elevated })
                                .inner_margin(Margin::same(12)).corner_radius(6).show(ui, |ui| {
                                ui.set_width(width - 38.0);
                                let available = item.session.is_some_and(|session| self.conn.infos.contains_key(&session));
                                let can_rotate = item.session.is_some_and(|session| self.conn.infos.get(&session).is_some_and(|s| s.exited.is_none()));
                                let action = notification_row(ui, item, available, can_rotate);
                                match action {
                                    RowAction::Reveal => reveal = item.session,
                                    RowAction::Rotate => rotate = item.session.zip(item.rotate_tool),
                                    RowAction::Dismiss => dismiss = Some(index),
                                    RowAction::None => {}
                                }
                            });
                            ui.add_space(4.0);
                        });
                    }
                    if visible == 0 {
                        ui.add_space(36.0);
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new(if self.notifications.category.is_some() { "현재 필터에 맞는 알림이 없습니다" } else if self.notifications.unread_only { "모든 알림을 확인했습니다" } else { "아직 알림이 없습니다" }).font(fonts::semibold(15.0)));
                        });
                        ui.add_space(36.0);
                    }
                });
                widgets::divider(ui);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if widgets::icon_button(ui, Icon::Bell, 28.0, self.settings.do_not_disturb, if self.settings.do_not_disturb { "방해 금지 끄기" } else { "방해 금지 켜기" }).clicked() {
                        self.settings.do_not_disturb = !self.settings.do_not_disturb;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::icon_button(ui, Icon::Trash, 28.0, false, "읽은 알림 지우기").clicked() {
                            self.notifications.items.retain(|item| !item.read);
                        }
                    });
                });
            });
        if let Some(index) = dismiss { self.notifications.items.remove(index); }
        if mark_all { for item in &mut self.notifications.items { item.read = true; } }
        for session in unread_before {
            if !self.notifications.items.iter().any(|n| !n.read && n.session == Some(session)) {
                self.conn.send(kiln_proto::ClientMsg::ClearAttention { session });
            }
        }
        if let Some((session, tool)) = rotate { self.actions.push(Action::RotateAccount(session, tool)); }
        if close || modal.should_close() || reveal.is_some() || rotate.is_some() {
            self.notifications.open = false;
            self.focus_terminal = true;
        }
        if let Some(session) = reveal {
            if self.panes.values().any(|pane| pane.session() == Some(session)) { self.reveal_session(session); self.reveal_work_surface(ctx); }
            else { self.actions.push(Action::AttachSession(session)); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notification_title_navigates_but_read_and_dismiss_stay_independent() {
        use egui_kittest::{Harness, kittest::Queryable};
        let mut center = NotificationCenter::default();
        center.push_activity(kiln_proto::AgentActivity::Done, 7, "Workspace".into(), "Build complete");
        let item = center.items.pop().unwrap();
        let mut initialized = false;
        let mut h = Harness::builder().with_size([400.0, 240.0]).build_ui_state(|ui, s: &mut (Notification, RowAction)| {
            if !initialized { kiln_common::fonts::install(ui.ctx()); kiln_common::Theme::current().apply(ui.ctx()); initialized = true; return; }
            let action = notification_row(ui, &mut s.0, true, false);
            if action != RowAction::None { s.1 = action; }
        }, (item, RowAction::None));
        h.run_steps(3);
        h.get_by_label("읽음으로 표시").click(); h.run_steps(2);
        assert!(h.state().0.read);
        assert_eq!(h.state().1, RowAction::None);
        h.get_by_label("읽지 않음으로 표시").click(); h.run_steps(2);
        assert!(!h.state().0.read);
        h.get_by_label("완료").click(); h.run_steps(2);
        assert!(h.state().0.read);
        assert_eq!(h.state().1, RowAction::Reveal);
        h.get_by_label("알림 지우기").click(); h.run_steps(2);
        assert_eq!(h.state().1, RowAction::Dismiss);
    }

    #[test]
    fn explicit_agent_events_are_classified_without_guessing_completion() {
        use kiln_proto::AgentActivity;
        let mut center=NotificationCenter::default();
        for activity in [AgentActivity::Unknown,AgentActivity::Running,AgentActivity::Waiting,AgentActivity::Done,AgentActivity::Failed] {center.push_activity(activity,1,"Project".into(),"Agent");}
        assert_eq!(center.items.iter().map(|n|n.category).collect::<Vec<_>>(),vec![NotificationCategory::Attention,NotificationCategory::Completed,NotificationCategory::Error]);
        assert_eq!(center.unread_count(),3);
        center.mark_session_read(1);assert_eq!(center.unread_count(),0);
        let old=r#"[{"title":"old error","body":"failed","session":null,"workspace":"","timestamp":0,"read":false,"error":true}]"#;
        assert_eq!(NotificationCenter::new(serde_json::from_str(old).unwrap()).items[0].category,NotificationCategory::Error);
    }
    #[test]
    fn inbox_is_bounded_and_restores_read_state() {
        let mut center = NotificationCenter::default();
        for id in 0..220 { center.push("Agent", "Ready", ToastKind::Notify, Some(id), "Project".into()); }
        assert_eq!(center.items.len(), MAX_NOTIFICATIONS);
        assert_eq!(center.items[0].session, Some(20));
        center.mark_session_read(219);
        center.items.last_mut().unwrap().rotate_tool = Some(kiln_accounts::Tool::Claude);
        let json = serde_json::to_string(&center.items).unwrap();
        let restored = NotificationCenter::new(serde_json::from_str(&json).unwrap());
        assert_eq!(restored.unread_count(), 199);
        assert!(restored.items.last().unwrap().read);
        assert_eq!(restored.items.last().unwrap().rotate_tool, Some(kiln_accounts::Tool::Claude));
    }
}
