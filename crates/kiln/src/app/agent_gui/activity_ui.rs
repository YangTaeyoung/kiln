//! Fixed activity strip and task details, independent of history scroll position.
use super::*;
use std::time::{Duration, Instant};
#[derive(Default)]
pub(super) struct ActivityUi {
    pub jump: bool,
    pub away: bool,
    started: Option<Instant>,
    elapsed: Duration,
    clocks: BTreeMap<String, Instant>,
    generation: String,
    #[cfg(test)]
    pub offset: f32,
}
fn duration(seconds: u64) -> String {
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}
impl AgentPane {
    pub(super) fn activity_height(&self, available: f32) -> f32 {
        if self.controls_ui.surface != Surface::Conversation {
            return 0.;
        }
        if self.transcript.entries.is_empty()
            && self.transcript.activity.tasks.is_empty()
            && self.lifecycle() == Lifecycle::Ready
        {
            return 0.;
        }
        let rows=self.transcript.activity.live().count()+self.transcript.entries.iter().filter(|e|matches!(e,Entry::Tool(t) if matches!(t.status,ToolStatus::Running|ToolStatus::Pending))).count();
        (if rows == 0 && self.transcript.activity.tasks.is_empty() {42_f32} else {176_f32})
        .min((available * 0.3).max(42.))
    }
    pub(super) fn activity_ui(&mut self, ui: &mut Ui) {
        let tr = kiln_common::i18n::tr;
        let theme = Theme::current();
        let lifecycle = self.lifecycle();
        if self.activity_ui.generation != self.transcript.generation {
            self.activity_ui = ActivityUi {
                generation: self.transcript.generation.clone(),
                ..Default::default()
            };
        }
        let running = lifecycle == Lifecycle::Running;
        if running {
            self.activity_ui.started.get_or_insert_with(Instant::now);
        } else if let Some(start) = self.activity_ui.started.take() {
            self.activity_ui.elapsed = start.elapsed();
        }
        let live = self.transcript.activity.live().collect::<Vec<_>>();
        let tools = self
            .transcript
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::Tool(t)
                    if matches!(t.status, ToolStatus::Pending | ToolStatus::Running)
                        && !live.iter().any(|task| task.tool_id == t.id) =>
                {
                    Some(t)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let main_label = match lifecycle {
            Lifecycle::Starting => "연결 중",
            Lifecycle::Waiting => "입력 필요",
            Lifecycle::Running if !tools.is_empty() => "도구 실행 중",
            Lifecycle::Running => "응답 작성 중",
            Lifecycle::Ready if live.iter().any(|t| !t.ambient) => {
                "응답 완료 · 백그라운드 작업 실행 중"
            }
            Lifecycle::Ready => "응답 완료",
            Lifecycle::Failed => "작업 실패",
            Lifecycle::Disconnected => "연결 끊김",
            Lifecycle::Stopped => "중지됨",
        };
        let animated =
            running || lifecycle == Lifecycle::Starting || (matches!(lifecycle,Lifecycle::Ready|Lifecycle::Waiting) && live.iter().any(|t| !t.ambient));
        if animated {
            ui.ctx().request_repaint_after(Duration::from_millis(80).max(Duration::from_secs_f32(ui.input(|i| i.predicted_dt).max(0.) + 0.001)));
        }
        let max_h = ui.available_height();
        let mut output_request=None;
        Frame::new().fill(theme.bg_panel).stroke(Stroke::new(1.,theme.border)).corner_radius(10).inner_margin(10).show(ui,|ui|{
            ui.set_width(ui.available_width());
            ui.set_min_height((max_h-20.).max(0.));
            ui.horizontal_wrapped(|ui|{
                if animated {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(16.,16.),egui::Sense::hover());
                    ui.painter().circle_stroke(rect.center(),6.,Stroke::new(2.,theme.border));
                    let angle=ui.input(|i|i.time) as f32*4.;
                    let points=(0..18).map(|n| {let a=angle+n as f32/17.*4.4;rect.center()+egui::vec2(a.cos(),a.sin())*6.}).collect::<Vec<_>>();
                    ui.painter().add(egui::Shape::line(points,Stroke::new(2.,theme.accent)));
                } else {ui.label(RichText::new(match lifecycle {Lifecycle::Ready=>"✓",Lifecycle::Waiting=>"!",Lifecycle::Failed=>"×",Lifecycle::Stopped=>"■",_=>"?"}).color(match lifecycle {Lifecycle::Ready=>theme.green,Lifecycle::Waiting=>theme.orange,Lifecycle::Failed=>theme.red,_=>theme.text_dim}));}
                ui.strong(tr(main_label));
                if let Some(start)=self.activity_ui.started {ui.small(format!("{} {}",tr("표시 경과"),duration(start.elapsed().as_secs()))).on_hover_text(tr("GUI에서 현재 작업을 관찰한 시간입니다."));}
            });
            if !live.is_empty() || !tools.is_empty() || !self.transcript.activity.tasks.is_empty() {
                egui::ScrollArea::vertical().id_salt("activity-tasks").max_height((max_h-45.).max(20.)).show(ui,|ui|{
                    ui.visuals_mut().collapsing_header_frame=true;
                    let shells=live.iter().filter(|t|matches!(t.kind.as_str(),"local_bash"|"shell")).count();
                    let monitors=live.iter().filter(|t|t.kind.contains("monitor")).count();
                    if self.transcript.activity.background.is_some() || !self.transcript.activity.tasks.is_empty() {
                        ui.small(format!("{} {} · {} {} · {} {}",tr("셸"),shells,tr("Monitor"),monitors,tr("기타 작업"),live.len()-shells-monitors));
                    } else {ui.small(format!("{}: {}",tr("도구 실행 중"),tools.len()));}
                    for task in &live {
                        let kind=task_kind(&task.kind);
                        let label=if task.description.is_empty(){kind.clone()}else{task.description.clone()};
                        let status=tr(if matches!(lifecycle,Lifecycle::Disconnected|Lifecycle::Failed|Lifecycle::Stopped) {"상태 확인 불가"}else if task.status=="completed" {"완료"}else if task.ambient {"상시 작업"}else if task.status=="paused" {"일시 정지"}else{"실행 중"});
                        let badge=egui::WidgetText::from(RichText::new(status).small().color(if task.ambient {theme.text_dim}else{theme.accent})).into_galley(ui,None,f32::INFINITY,egui::TextStyle::Button);
                        let title=egui::WidgetText::from(format!("{kind} · {label}")).into_galley(ui,Some(egui::TextWrapMode::Truncate),(ui.available_width()-badge.size().x-34.).max(24.),egui::TextStyle::Button);
                        let response=egui::CollapsingHeader::new(title).id_salt(("native-task",&task.id)).show(ui,|ui|{
                            output_ui(ui,task,&self.transcript.activity,&mut output_request);
                            ui.add(egui::Label::new(&task.description).wrap());
                            if let Some(Entry::Tool(tool))=self.transcript.entries.iter().find(|e|matches!(e,Entry::Tool(t) if t.id==task.tool_id)) {let cmd=tool.input.get("command").and_then(serde_json::Value::as_str);if let Some(cmd)=cmd {ui.add(egui::Label::new(RichText::new(bounded(cmd)).monospace()).wrap());}}
                            if !task.summary.is_empty(){ui.add(egui::Label::new(&task.summary).wrap());}
                            if let Some(ms)=task.duration_ms {ui.small(format!("{} {}",tr("실행 시간"),duration(ms/1000)));}
                            if !task.output_file.is_empty(){ui.add(egui::Label::new(format!("{}: {}",tr("출력 파일"),task.output_file)).wrap());}

                            ui.small(tr(if task.ambient {"상시 · 감시 작업"}else if task.status=="paused" {"일시 정지"} else {"실행 중"}));
                        });
                        ui.painter().galley(egui::pos2(response.header_response.rect.right()-badge.size().x-4.,response.header_response.rect.center().y-badge.size().y/2.),badge,theme.text_dim);
                        if !task.summary.is_empty(){ui.add(egui::Label::new(RichText::new(&task.summary).small().color(theme.text_dim)).wrap());}
                    }
                    for tool in &tools {
                        let key=format!("tool:{}",tool.id);let clock=self.activity_ui.clocks.entry(key).or_insert_with(Instant::now);
                        ui.horizontal_wrapped(|ui|{ui.small(format!("{} · {}",tr("도구 실행 중"),duration(clock.elapsed().as_secs())));ui.label(super::super::approval::tool_title(tool,self.descriptor.tool));});
                        if let Some(command)=tool.input.get("command").and_then(serde_json::Value::as_str){ui.add(egui::Label::new(RichText::new(bounded(command)).monospace().small()).wrap());}
                    }
                    let ended=self.transcript.activity.tasks.values().filter(|t|!live.iter().any(|active|active.id==t.id)).collect::<Vec<_>>();
                    if !ended.is_empty(){egui::CollapsingHeader::new(format!("{} ({})",tr("종료된 작업"),ended.len())).show(ui,|ui|{for task in ended {ui.add(egui::Label::new(format!("{} · {} · {}",task_kind(&task.kind),task.description,tr(match task.status.as_str(){"completed"=>"완료","failed"=>"실패","stopped"|"killed"=>"중지됨",_=>"결과 확인 대기"}))).wrap());if !task.summary.is_empty(){ui.add(egui::Label::new(&task.summary).wrap());}output_ui(ui,task,&self.transcript.activity,&mut output_request);}});}
                });
            }
        });
        if let Some((task_id,run_id))=output_request {let _=self.send(Command::Discover(Discovery::TaskOutput{task_id,run_id}));}
    }
}
fn task_kind(kind: &str) -> String {
    let tr = kiln_common::i18n::tr;
    match kind {
        "local_bash" | "shell" => tr("셸").into(),
        "monitor" | "local_monitor" => tr("Monitor").into(),
        "local_agent" | "subagent" => tr("하위 에이전트").into(),
        "local_workflow" | "workflow" => tr("워크플로").into(),
        "mcp_task" => tr("MCP 작업").into(),
        "" => tr("작업").into(),
        other => other.into(),
    }
}

fn output_ui(ui:&mut Ui,task:&super::super::activity::Task,state:&super::super::activity::State,request:&mut Option<(String,String)>) {
    let tr=kiln_common::i18n::tr;
    if ui.small_button(tr("출력 보기 · 새로고침")).clicked(){*request=Some((task.id.clone(),task.run.clone()));}
    if let Some((text,truncated,unavailable))=state.outputs.get(&(task.id.clone(),task.run.clone())) {
        if *unavailable {ui.add(egui::Label::new(tr("이 CLI에서 작업 출력을 가져올 수 없습니다.")).wrap());}
        else if text.is_empty(){ui.label(tr("아직 출력이 없습니다."));}
        else {egui::ScrollArea::both().id_salt(("task-output",&task.id)).max_height(100.).show(ui,|ui|{ui.add(egui::Label::new(RichText::new(text).monospace()).wrap());});}
        if *truncated {ui.small(tr("출력의 마지막 부분을 표시합니다."));}
    }
}
