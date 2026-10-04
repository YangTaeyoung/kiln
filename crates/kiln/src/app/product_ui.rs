use super::*;
use kiln_common::{fonts,widgets};
impl KilnApp {
    pub(super) fn ui_product_overlays(&mut self, ctx:&egui::Context) {
        if self.confirm.is_some(){return;}
        if let Some(projects::ProjectAction::Open{path,name,command})=self.projects.ui(ctx) {
            let path=normalize_path(path.canonicalize().unwrap_or(path));
            if !path.is_dir() { self.toast("폴더를 열 수 없습니다",path.display().to_string(),ToastKind::Error,None); return; }
            self.add_workspace(path.clone(),ctx);
            self.workspaces[self.active].name=name.clone();
            if let Some(command)=command { self.actions.push(Action::RunSavedCommand(launchers::SelectedCommand{name,command,cwd:Some(path)})); }
            self.focus_terminal=true;
            self.reveal_work_surface(ctx);
        }
        if self.save_error.is_some() {
            egui::Area::new(egui::Id::new("save-failure-banner")).anchor(egui::Align2::CENTER_BOTTOM,[0.0,-8.0]).show(ctx,|ui|{
                egui::Frame::popup(ui.style()).show(ui,|ui|{ui.horizontal(|ui|{
                    ui.colored_label(self.theme.red,"작업 상태가 저장되지 않았습니다");
                    if ui.button("작업 복구 센터").clicked(){self.recovery_open=true;}
                });});
            });
        }
        if self.recovery_open {
            let mut close=false;
            let mut dismiss_history=false;
            let mut draft_target=None;
            let theme = self.theme;
            let frame=egui::Frame::new().fill(theme.bg_elevated).stroke(egui::Stroke::new(1.0,theme.border_strong)).corner_radius(12).shadow(theme.shadow()).inner_margin(egui::Margin::same(16));
            let response=egui::Modal::new(egui::Id::new("recovery-center")).frame(frame).show(ctx,|ui|{
                ui.set_width((ctx.content_rect().width()-64.0).clamp(240.0,620.0));
                ui.label(egui::RichText::new("작업 복구 센터").font(fonts::semibold(20.0)));
                ui.add_space(8.0);
                ui.label("복원한 초안은 자동으로 저장하거나 전송하지 않습니다. 내용을 확인한 뒤 저장·적용하세요.");
                egui::ScrollArea::vertical().max_height((ctx.content_rect().height()-210.0).max(80.0)).show(ui,|ui|{
                    if !self.recovery_messages.is_empty() {
                        egui::CollapsingHeader::new("이전 복구 안내").default_open(true).show(ui, |ui| {
                            for message in &self.recovery_messages { ui.add(egui::Label::new(message).wrap()); }
                            dismiss_history=ui.button("안내 확인 완료").on_hover_text("이 안내만 닫습니다. 백업 파일과 편집 초안은 유지됩니다.").clicked();
                        });
                    }
                    if let Some(error)=&self.save_error{ui.colored_label(self.theme.red,error);}
                    let mut count=0;
                    for ws in &self.workspaces { for pane in ws.all_panes() {
                        if let Some(draft)=self.terminal_launch_drafts.get(&pane) {
                            count+=1;
                            ui.push_id(("terminal-launch",pane),|ui| {
                                ui.separator();
                                ui.label(format!("{} · 터미널 시작",ws.name));
                                ui.add(egui::Label::new(&draft.reason).wrap());
                                if let Some(cwd)=self.panes.get(&pane).and_then(|pane|pane.cwd.as_deref()) { ui.add(egui::Label::new(egui::RichText::new(cwd).small().color(theme.text_dim)).wrap()); }
                                if let Some(command)=&draft.command {
                                    egui::CollapsingHeader::new("보관한 명령").show(ui,|ui| {
                                        ui.add(egui::Label::new(command).wrap().selectable(true));
                                        if ui.button("명령 복사").clicked(){ui.ctx().copy_text(command.clone());}
                                    });
                                }
                                ui.horizontal_wrapped(|ui| {
                                    if ui.add_enabled(self.conn.is_connected(),egui::Button::new("다시 시작")).clicked(){self.actions.push(Action::RetryTerminalLaunch(pane));close=true;}
                                    if ui.button(if draft.command.is_some(){"명령 버리고 셸 열기"}else{"셸 설정"}).clicked(){
                                        if draft.command.is_some(){self.actions.push(Action::DiscardTerminalLaunch(pane));}else{self.actions.push(Action::OpenTerminalSettings);}
                                        close=true;
                                    }
                                });
                            });
                        }
                    }}
                    for ws in &self.workspaces {for id in ws.all_panes(){
                        if let Some(tool)=self.panes.get(&id).and_then(Pane::tool) {
                            if let Some(notice)=tool.recovery_notice(){
                                count+=1;ui.separator();ui.label(format!("{} · {}",ws.name,tool.title()));ui.label(notice);
                                if ui.button("복원한 내용 열기").clicked(){self.actions.push(Action::RevealPane(id));close=true;}
                            }
                        }
                    }}
                    for (index,ws) in self.workspaces.iter().enumerate() {
                        let drafts=ws.tools.drafts();
                        if !drafts.agent_prompt.is_empty() {
                            count+=1;ui.separator();ui.label(format!("{} · 작성 중인 에이전트 요청",ws.name));
                            if ui.button("에이전트 요청 이어 쓰기").clicked(){draft_target=Some((index,Action::NewAgentTask));close=true;}
                        }
                        if !drafts.repositories.is_empty() {
                            count+=1;ui.separator();ui.label(format!("{} · 작업 공간 초안",ws.name));
                            for text in ws.tools.unsaved_drafts(){ui.add(egui::Label::new(text).wrap());}
                            if ui.button("작업 공간에서 이어 쓰기").clicked(){draft_target=Some((index,Action::OpenSheet(tools::ToolKind::Git)));close=true;}
                        }
                        if !drafts.commit_message.is_empty() {
                            count+=1; ui.separator(); ui.label(format!("{} · 작성 중인 커밋 메시지",ws.name));
                            if ui.button("소스 제어에서 이어 쓰기").clicked(){draft_target=Some((index,Action::OpenSheet(tools::ToolKind::Git)));close=true;}
                        }
                        if !drafts.github.repositories.is_empty() {
                            count+=1; ui.separator(); ui.label(format!("{} · GitHub 작성 초안",ws.name));
                            for item in ws.tools.unsaved_drafts().into_iter().filter(|item|item.starts_with("GitHub")) {ui.add(egui::Label::new(item).wrap());}
                            if ui.button("GitHub에서 이어 쓰기").clicked(){draft_target=Some((index,Action::OpenSheet(tools::ToolKind::PullRequests)));close=true;}
                        }
                    }
                    if count==0 && self.save_error.is_none(){ui.label("확인이 필요한 편집 초안이 없습니다.");}
                    ui.separator();ui.label("작업 배치와 초안은 변경 후 주기적으로 저장합니다. 강제 종료 직전의 변경은 저장되지 않을 수 있습니다.");
                });
                ui.add_space(12.0);widgets::divider(ui);ui.add_space(8.0);
                ui.horizontal(|ui|{
                    if self.save_error.is_some() && widgets::button(ui,"작업 상태 저장 다시 시도",widgets::ButtonKind::Secondary).clicked(){self.save_if_changed(true);}
                    if widgets::button(ui,"닫기",widgets::ButtonKind::Primary).clicked(){close=true;}
                });
            });
            if let Some((index,action))=draft_target { self.active=index; self.actions.push(action); }
            if dismiss_history { self.recovery_messages.clear(); }
            if close || response.should_close(){self.recovery_open=false;}
        }
        if let Some((index,buffer))=&mut self.rename_page {
            let mut save=false;let mut cancel=false;
            let response=egui::Modal::new(egui::Id::new("rename-task")).show(ctx,|ui|{
                (save,cancel)=rename_task_form(ui,buffer);
            });
            if save && !buffer.trim().is_empty(){if let Some(page)=self.workspaces[self.active].pages.get_mut(*index){page.title=Some(buffer.trim().into());}}
            if save||cancel||response.should_close(){self.rename_page=None;ctx.data_mut(|d|d.remove::<bool>(egui::Id::new("rename-task-first-focus")));}
        }
        self.quick.poll();
        if self.quick.open {
            if self.quick.pane.is_none_or(|p|!self.panes.contains_key(&p)) {
                let pane=self.new_term_pane(Some(self.workspaces[self.active].root.to_string_lossy().into_owned()));
                let id=self.id();let mut page=Page::new(id,pane);page.title=Some("빠른 터미널".into());self.ws().pages.push(page);
                self.spawn_for_pane(pane);self.quick.pane=Some(pane);
            }
            let pane=self.quick.pane.unwrap();
            let session=self.panes.get(&pane).and_then(Pane::session);
            if let Some(sid)=session {if self.quick.view.is_none(){self.quick.view=Some(TermView::new(sid));}}
            let settings=terminal::TermSettings{font_size:self.settings.font_size,option_as_meta:self.settings.option_as_meta,line_height:self.settings.line_height,copy_on_select:self.settings.copy_on_select,cursor_blink:self.settings.cursor_blink,close_shortcut:None};
            let mut close=false;let mut attach=false;
            ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("quick-terminal"),egui::ViewportBuilder::default().with_title("Kiln · 빠른 터미널").with_inner_size([900.0,420.0]).with_min_inner_size([400.0,240.0]).with_always_on_top(),|ctx,_|{
                if self.quick.focus_pending { ctx.ctx().send_viewport_cmd(egui::ViewportCommand::Focus); }
                if ctx.input(|i|i.viewport().close_requested()){close=true;}
                egui::CentralPanel::default().show(ctx,|ui|{
                    let actions=quick_terminal_header(ui, self.quick.view.as_ref().is_some_and(TermView::inspector_open));
                    if actions.2 { if let Some(view) = &mut self.quick.view { view.open_history(); } }
                    attach=actions.0; close|=actions.1;
                    widgets::divider(ui);
                    if let Some(view)=&mut self.quick.view{let out=view.ui(ui,&mut self.conn,&settings,self.quick.focus_pending,None);self.quick.focus_pending=false;if let Some(link)=out.open{self.actions.push(Action::OpenLink(link));}if let Some((command,cwd))=out.command_to_run{self.actions.push(Action::RunSavedCommand(launchers::SelectedCommand{name:"명령 다시 실행".into(),command,cwd:cwd.map(PathBuf::from)}));}}else{ui.spinner();ui.label("터미널 시작 중…");ui.ctx().request_repaint_after(Duration::from_millis(100));}
                });
            });
            if attach{self.actions.push(Action::RevealPane(pane));ctx.send_viewport_cmd(egui::ViewportCommand::Focus);close=true;}
            if close{self.quick.open=false;self.quick.view=None;}
        }
    }
}

fn quick_terminal_header(ui:&mut egui::Ui, history_open: bool)->(bool,bool,bool) {
    let mut attach=false; let mut close=false; let mut history=false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("빠른 터미널").font(fonts::medium(13.0)).color(Theme::current().text)).on_hover_text("Ctrl+` · 숨겨도 세션은 유지됩니다");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            close=widgets::icon_button(ui, kiln_common::icons::Icon::Close, 28.0, false, "숨기기").clicked();
            attach=widgets::icon_button(ui, kiln_common::icons::Icon::Maximize, 28.0, false, "프로젝트에서 보기").clicked();
            history=widgets::icon_button(ui, kiln_common::icons::Icon::History, 28.0, history_open, "명령 기록").clicked();
        });
    });
    (attach,close,history)
}

#[cfg(test)]
mod narrow_quick_header_tests {
    use super::*;
    use egui_kittest::{Harness,kittest::Queryable};
    #[test]
    fn minimum_quick_terminal_header_keeps_actions_inside() {
        let mut initialized=false;
        let mut h=Harness::builder().with_size([400.0/1.3,240.0/1.3]).build_ui(|ui| {
            if !initialized{fonts::install(ui.ctx());Theme::current().apply(ui.ctx());initialized=true;return;}
            quick_terminal_header(ui, false);
            ui.separator();ui.label("터미널 출력 영역");
        });
        h.run_steps(4);
        for label in ["빠른 터미널","프로젝트에서 보기","숨기기","명령 기록","터미널 출력 영역"]{assert!(h.ctx.content_rect().contains_rect(h.get_by_label(label).rect()),"{label}");}
        h.render().unwrap().save("/tmp/kiln-quick-header-minimum.png").unwrap();
    }
}

fn rename_task_form(ui:&mut egui::Ui,buffer:&mut String)->(bool,bool) {
    let mut save=false; let mut cancel=false;
    ui.set_width(320.0);ui.heading("작업 이름");
    ui.label("예: 결제 오류 조사, PR #42 검토");
    let input=ui.text_edit_singleline(buffer);
    let focus_id=egui::Id::new("rename-task-first-focus");
    if !ui.data(|d|d.get_temp::<bool>(focus_id).unwrap_or(false)){input.request_focus();ui.data_mut(|d|d.insert_temp(focus_id,true));}
    ui.horizontal(|ui|{
        let enter=ui.input_mut(|i|i.consume_key(Modifiers::NONE,Key::Enter));
        let valid=!buffer.trim().is_empty();
        let clicked=ui.add_enabled(valid,egui::Button::new("저장")).clicked();
        save=valid && (clicked || enter);

        cancel=ui.button("취소").clicked();
    });
    if buffer.trim().is_empty(){ui.colored_label(Theme::current().red,"작업 이름을 입력하세요");}
    (save,cancel)
}

#[cfg(test)]
mod rename_regressions {
    use super::*;
    use egui_kittest::{Harness,kittest::Queryable};
    #[test]
    fn blank_enter_keeps_rename_open_then_valid_enter_saves() {
        let mut initialized=false;
        let mut h=Harness::builder().with_size([550.0,338.0]).build_ui_state(|ui,state:&mut (String,bool)| {
            if !initialized {fonts::install(ui.ctx());Theme::current().apply(ui.ctx());initialized=true;return;}
            if !state.1 {egui::Modal::new(egui::Id::new("test-rename")).show(ui.ctx(),|ui|{state.1=rename_task_form(ui,&mut state.0).0;});}
        },("  ".to_string(),false));
        h.run_steps(3);h.key_press(Key::Enter);h.run_steps(2);
        assert!(!h.state().1);assert!(h.query_by_label("작업 이름을 입력하세요").is_some());
        h.render().unwrap().save("/tmp/kiln-rename-invalid.png").unwrap();
        h.state_mut().0="버그 조사".into();h.key_press(Key::Enter);h.run_steps(2);assert!(h.state().1);
    }
}
