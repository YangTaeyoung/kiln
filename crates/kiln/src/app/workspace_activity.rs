//! Workspace navigation surfaces reported task state without guessing from CPU
//! output, process existence, or a previously successful shell command.
use super::*;
use egui::{Align2, Color32, FontId, Sense, pos2, vec2};
use kiln_common::{fonts, icons::Icon, widgets};
use kiln_proto::{AgentActivity, SessionInfo, SessionTelemetry};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TaskPhase { Failed, Waiting, Attention, Running, Done, Unknown }

impl TaskPhase {
    fn label(self) -> &'static str { match self {
        Self::Failed=>kiln_common::i18n::tr("실패"), Self::Waiting=>kiln_common::i18n::tr("입력 대기"), Self::Attention=>kiln_common::i18n::tr("확인 필요"),
        Self::Running=>kiln_common::i18n::tr("작업 중"), Self::Done=>kiln_common::i18n::tr("완료"), Self::Unknown=>kiln_common::i18n::tr("세션 열림"),
    }}
    fn priority(self) -> u8 { match self {
        Self::Waiting=>0, Self::Running=>1, Self::Attention=>2, Self::Failed=>3, Self::Done=>4, Self::Unknown=>5,
    }}
    fn appearance(self, t: &Theme) -> (Icon, Color32) { match self {
        Self::Failed=>(Icon::Warning,t.red), Self::Waiting|Self::Attention=>(Icon::Bell,t.orange),
        Self::Running=>(Icon::Play,t.blue), Self::Done=>(Icon::Check,t.green), Self::Unknown=>(Icon::Terminal,t.text_dim),
    }}
}

#[derive(Clone)]
pub(super) struct WorkspaceTask {
    pub pane: PaneId,
    pub title: String,
    /// Compact visible distinction; reserve its width instead of truncating it.
    pub qualifier: String,
    pub duplicate_index: usize,
    pub phase: TaskPhase,
    pub agent: Option<kiln_accounts::Tool>,
    pub updated: u64,
    /// Only real command/notification records have a last-activity timestamp.
    pub recorded: Option<u64>,
}

fn phase(info: &SessionInfo, telemetry: Option<&SessionTelemetry>) -> TaskPhase {
    let activity = super::ui::title_activity(&info.title,telemetry.map(|t|t.activity).unwrap_or_default(),info.exited);
    match activity {
        AgentActivity::Failed=>TaskPhase::Failed,
        AgentActivity::Waiting=>TaskPhase::Waiting,
        AgentActivity::Done=>TaskPhase::Done,
        _ if info.attention=>TaskPhase::Attention,
        AgentActivity::Running=>TaskPhase::Running,
        _=>TaskPhase::Unknown,
    }
}

impl KilnApp {
    pub(super) fn workspace_tasks(&self, index: usize) -> Vec<WorkspaceTask> {
        let mut tasks=Vec::new();
        let mut locations=Vec::new();
        for page in &self.workspaces[index].pages {
            let panes=page.root.panes();
            for &pane in &panes {
                let Some(session)=self.panes.get(&pane).and_then(Pane::session) else {continue};
                let Some(info)=self.conn.infos.get(&session) else {continue};
                let telemetry=self.conn.telemetry.get(&session);
                let command_time=telemetry.into_iter().flat_map(|t|&t.commands)
                    .map(|c|c.started_unix.max(c.finished_unix.unwrap_or(0))).max().unwrap_or(0);
                let notice_time=self.notifications.items.iter().filter(|n|n.session==Some(session)).map(|n|n.timestamp).max().unwrap_or(0);
                let mut title=if panes.len()==1 {page.title.clone()} else {None}
                    .or_else(||info.name.clone().filter(|name|!name.trim().is_empty()))
                    .unwrap_or_else(|| self.card_info(pane).name);
                if title.is_empty() || super::shells().contains(&title.as_str()) || title==kiln_common::i18n::tr("셸") {
                    title=telemetry.and_then(|t|t.commands.last()).map(|c|c.command.lines().next().unwrap_or("").chars().take(120).collect())
                        .filter(|s:&String|!s.is_empty()).unwrap_or_else(||kiln_common::i18n::tr("터미널").into());
                }
                let recorded=Some(command_time.max(notice_time)).filter(|time|*time>0);
                tasks.push(WorkspaceTask {pane,title,qualifier:String::new(),duplicate_index:0,phase:phase(info,telemetry),agent:super::ui::session_agent(info),updated:info.created_unix.max(command_time).max(notice_time),recorded});
                locations.push(info.cwd.as_deref().map(|cwd| {
                    let cwd=Path::new(cwd);
                    cwd.strip_prefix(&self.workspaces[index].root).ok().filter(|p|!p.as_os_str().is_empty())
                        .map(|p|p.to_string_lossy().into_owned())
                        .unwrap_or_else(||cwd.file_name().unwrap_or_default().to_string_lossy().into_owned())
                }).unwrap_or_default());
            }
        }
        // Stable ties preserve page/split order. Animated OSC title frames must
        // never make the list shuffle or imply a new task completion.
        // Name in navigation order before activity sorting: internal pane IDs
        // have no user meaning, and status changes must not swap display numbers.
        distinguish_titles(&mut tasks,&locations);
        tasks.sort_by_key(|t|(t.phase.priority(),std::cmp::Reverse(t.updated)));
        tasks
    }
}

fn distinguish_titles(tasks:&mut [WorkspaceTask],locations:&[String]) {
    let names:Vec<_>=tasks.iter().map(|t|t.title.clone()).collect();
    for (i,task) in tasks.iter_mut().enumerate() {
        let peers:Vec<_>=names.iter().enumerate().filter(|(_,name)|**name==names[i]).map(|(index,_)|index).collect();
        if peers.len()<2 {continue;}
        task.duplicate_index=peers.iter().position(|&peer|peer==i).unwrap()+1;
        if !locations[i].is_empty() && peers.iter().any(|&j|locations[j]!=locations[i]) {
            task.qualifier=Path::new(&locations[i]).file_name().unwrap_or_default().to_string_lossy().into_owned();
            task.title=format!("{} · {}",names[i],task.qualifier);
        }
    }
    let names:Vec<_>=tasks.iter().map(|t|t.title.clone()).collect();
    for (i,task) in tasks.iter_mut().enumerate() {
        if names.iter().filter(|name|**name==names[i]).count()>1 {
            let number=names[..=i].iter().filter(|name|**name==names[i]).count();
            task.title=format!("{} {number}",names[i]);
            task.qualifier=if task.qualifier.is_empty(){number.to_string()}else{format!("{} {number}",task.qualifier)};
        }
    }
}

fn relative_time(updated:u64)->String {
    if updated==0 {return String::new()}
    let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    match now.saturating_sub(updated) {
        0..=59=>kiln_common::i18n::tr("방금").into(), n@60..=3599=>kiln_common::trf!("{}분 전",n/60),
        n@3600..=86399=>kiln_common::trf!("{}시간 전",n/3600), n=>kiln_common::trf!("{}일 전",n/86400),
    }
}

pub(super) fn summary(tasks:&[WorkspaceTask])->String {
    [(TaskPhase::Failed,kiln_common::i18n::tr("실패")),(TaskPhase::Waiting,kiln_common::i18n::tr("대기")),(TaskPhase::Attention,kiln_common::i18n::tr("확인")),(TaskPhase::Running,kiln_common::i18n::tr("진행")),(TaskPhase::Done,kiln_common::i18n::tr("완료")),(TaskPhase::Unknown,kiln_common::i18n::tr("열림"))]
        .into_iter().filter_map(|(phase,label)| {let n=tasks.iter().filter(|t|t.phase==phase).count();(n>0).then(||format!("{label} {n}"))}).collect::<Vec<_>>().join(" · ")
}

fn elided(ui:&egui::Ui,text:&str,font:FontId,color:Color32,width:f32)->std::sync::Arc<egui::Galley> {
    let mut job=egui::text::LayoutJob::simple_singleline(text.into(),font,color);
    job.wrap.max_width=width.max(1.0); job.wrap.max_rows=1; job.wrap.break_anywhere=true; job.wrap.overflow_character=Some('…');
    ui.fonts_mut(|f|f.layout_job(job))
}

// A clipped folder must still distinguish peers with the same long prefix.
fn visible_qualifier(ui:&egui::Ui,task:&WorkspaceTask,width:f32,theme:&Theme)->String {
    let measure=|text:String|ui.painter().layout_no_wrap(text,fonts::regular(11.0),theme.text_dim).size().x;
    if measure(task.qualifier.clone())<=width {return task.qualifier.clone();}
    let number=task.duplicate_index.to_string();
    let mut prefix=task.qualifier.clone();
    while !prefix.is_empty() {
        prefix.pop();
        let candidate=format!("{prefix}… {number}");
        if measure(candidate.clone())<=width {return candidate;}
    }
    number
}

pub(super) fn task_rows(ui:&mut egui::Ui,tasks:&[WorkspaceTask],selected:Option<PaneId>,theme:&Theme)->Option<PaneId> {
    if tasks.is_empty() {return None}
    let key=ui.id().with("workspace-tasks-expanded");
    let expanded=ui.data(|d|d.get_temp::<bool>(key)).unwrap_or(false);
    let mut reveal=None;
    for task in tasks.iter().take(if expanded {usize::MAX}else{3}) {
        let (rect,response)=ui.allocate_exact_size(vec2(ui.available_width(),40.0),Sense::click());
        let chosen=selected==Some(task.pane);
        let label=format!("{} · {}",task.title,task.phase.label());
        response.widget_info(||egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel,true,chosen,&label));
        if chosen || response.hovered() {ui.painter().rect_filled(rect,4,if response.hovered(){theme.bg_hover}else{theme.bg_panel});}
        let (icon,color)=task.phase.appearance(theme);
        let color=if task.phase==TaskPhase::Running {super::ui::running_color(task.agent,theme)}else{color};
        let center=pos2(rect.left()+17.0,rect.top()+13.0);
        if task.phase==TaskPhase::Running {super::ui::paint_running(ui,center,color);}
        else {icons::paint(ui.painter(),egui::Rect::from_center_size(center,vec2(13.0,13.0)),icon,color);}
        let x=rect.left()+30.0;
        let text_color=if chosen || matches!(task.phase,TaskPhase::Running|TaskPhase::Waiting|TaskPhase::Failed|TaskPhase::Attention){theme.text}else{theme.text_dim};
        let width=rect.right()-x-8.0;
        let qualifier=visible_qualifier(ui,task,width*0.45,theme);
        let suffix=elided(ui,&qualifier,fonts::regular(11.0),theme.text_dim,width*0.45);
        let reserved=if task.qualifier.is_empty(){0.0}else{suffix.size().x+10.0};
        let base=if task.qualifier.is_empty(){task.title.as_str()}else{
            task.title.strip_suffix(&task.qualifier).unwrap_or(&task.title).trim_end_matches([' ','·'])
        };
        let title=elided(ui,base,fonts::medium(12.0),text_color,width-reserved);
        ui.painter().galley(pos2(x,rect.top()+4.0),title,text_color);
        if !task.qualifier.is_empty() {
            ui.painter().galley(pos2(rect.right()-8.0-suffix.size().x,rect.top()+5.0),suffix,theme.text_dim);
        }
        ui.painter().text(pos2(x,rect.top()+26.0),Align2::LEFT_CENTER,task.phase.label(),fonts::regular(11.0),color);
        let age=task.recorded.filter(|_|task.phase!=TaskPhase::Running).map(relative_time).unwrap_or_default();
        let age_width=ui.painter().layout_no_wrap(age.clone(),fonts::regular(11.0),theme.text_dim).size().x;
        // At narrow widths keep the actionable status, move the timestamp to hover.
        if rect.right()-x > age_width+70.0 {ui.painter().text(pos2(rect.right()-8.0,rect.top()+26.0),Align2::RIGHT_CENTER,&age,fonts::regular(11.0),theme.text_dim);}
        widgets::focus_ring(ui,&response,4);
        if response.clicked() {reveal=Some(task.pane);}
        response.on_hover_text(kiln_common::trf!("{}\n{}{}\n클릭하여 작업으로 이동",task.title,task.phase.label(),if age.is_empty(){String::new()}else{kiln_common::trf!("\n최근 명령·알림 기록: {age}")}));
    }
    if tasks.len()>3 {
        let text=if expanded {kiln_common::i18n::tr("간단히 보기").into()} else {kiln_common::trf!("작업 {}개 모두 보기",tasks.len())};
        if ui.add_sized([ui.available_width(),24.0],egui::Button::new(egui::RichText::new(text).font(fonts::regular(11.5)).color(theme.text_dim)).frame(false)).clicked() {
            ui.data_mut(|d|d.insert_temp(key,!expanded));
        }
    }
    reveal
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_names_keep_human_distinctions_when_activity_order_changes() {
        use egui_kittest::{Harness,kittest::Queryable};
        let task=|pane,title:&str|WorkspaceTask{pane,title:title.into(),qualifier:String::new(),duplicate_index:0,phase:TaskPhase::Unknown,agent:None,updated:0,recorded:None};
        let mut tasks=vec![task(1004,"터미널"),task(1001,"터미널"),
            task(1051,"동일한 긴 작업 제목으로 프론트와 백엔드 API 연결 상태 확인"),
            task(1062,"동일한 긴 작업 제목으로 프론트와 백엔드 API 연결 상태 확인")];
        distinguish_titles(&mut tasks,&["workspace".into(),"workspace".into(),"frontend-staging".into(),"frontend-production".into()]);
        assert_eq!(tasks[0].title,"터미널 1");assert_eq!(tasks[1].title,"터미널 2");
        assert_eq!(tasks[2].qualifier,"frontend-staging");assert_eq!(tasks[3].qualifier,"frontend-production");
        tasks[1].phase=TaskPhase::Waiting;
        tasks.sort_by_key(|t|t.phase.priority());
        assert_eq!((tasks[0].pane,tasks[0].title.as_str()),(1001,"터미널 2"));
        assert!(tasks.iter().all(|t|!t.title.contains("패널")&&!t.title.contains(&t.pane.to_string())));
        let mut installed=false;
        let mut h=Harness::builder().with_size([180.0,240.0]).build_ui(move|ui|{
            if !installed{kiln_common::fonts::install(ui.ctx());Theme::current().apply(ui.ctx());installed=true;return;}
            let first=visible_qualifier(ui,&tasks[2],60.0,&Theme::current());
            let second=visible_qualifier(ui,&tasks[3],60.0,&Theme::current());
            assert_ne!(first,second);assert!(first.ends_with('1'));assert!(second.ends_with('2'));
            task_rows(ui,&tasks,Some(1001),&Theme::current());
        });
        h.run_steps(3);h.get_by_label("작업 4개 모두 보기").click();h.run_steps(2);
        h.render().unwrap().save("/tmp/kiln-readable-task-names-180.png").unwrap();
    }

    #[test]
    fn task_status_does_not_confuse_old_command_success_with_agent_completion() {
        assert!(TaskPhase::Waiting.priority()<TaskPhase::Failed.priority());
        assert!(TaskPhase::Running.priority()<TaskPhase::Failed.priority());
        let mut info=SessionInfo {title:"Claude Code".into(),..Default::default()};
        let mut t=SessionTelemetry {commands:vec![kiln_proto::CommandRecord {exit_code:Some(0),finished_unix:Some(10),..Default::default()}],..Default::default()};
        assert_eq!(phase(&info,Some(&t)),TaskPhase::Unknown);
        info.attention=true; assert_eq!(phase(&info,Some(&t)),TaskPhase::Attention);
        t.activity=AgentActivity::Waiting; assert_eq!(phase(&info,Some(&t)),TaskPhase::Waiting);
        t.activity=AgentActivity::Done; assert_eq!(phase(&info,Some(&t)),TaskPhase::Done);
        info.exited=Some(1); assert_eq!(phase(&info,Some(&t)),TaskPhase::Failed);
        info.exited=None;info.attention=false;t.activity=AgentActivity::Unknown;info.title="⠼ API 구현".into();
        assert_eq!(phase(&info,Some(&t)),TaskPhase::Running);
        info.title="◑ Claude task".into(); assert_eq!(phase(&info,Some(&t)),TaskPhase::Running);
        info.title="✳ Claude task".into(); assert_eq!(phase(&info,Some(&t)),TaskPhase::Unknown);
    }

    #[test]
    fn workspace_task_rows_are_bounded_and_open_the_selected_split() {
        use egui_kittest::{Harness,kittest::Queryable};
        let tasks=vec![
            WorkspaceTask{pane:11,title:"결제 API 인증 방식 확인".into(),qualifier:String::new(),duplicate_index:0,phase:TaskPhase::Waiting,agent:None,updated:0,recorded:None},
            WorkspaceTask{pane:12,title:"프론트·백엔드 로그인 연결 구현".into(),qualifier:String::new(),duplicate_index:0,phase:TaskPhase::Running,agent:None,updated:0,recorded:None},
            WorkspaceTask{pane:13,title:"캐시 무효화 문제 수정".into(),qualifier:String::new(),duplicate_index:0,phase:TaskPhase::Done,agent:None,updated:0,recorded:None},
            WorkspaceTask{pane:14,title:"아주 긴 최근 작업 제목이 좁은 작업 공간 목록을 밀어내면 안 됩니다".into(),qualifier:String::new(),duplicate_index:0,phase:TaskPhase::Unknown,agent:None,updated:0,recorded:None},
        ];
        for width in [180.0,280.0] {
            let tasks=tasks.clone(); let mut installed=false;
            let mut h=Harness::builder().with_size([width,260.0]).build_ui_state(move |ui,selected:&mut Option<PaneId>|{
                if !installed {kiln_common::fonts::install(ui.ctx());Theme::current().apply(ui.ctx());installed=true;return;}
                ui.set_max_width(width-16.0);
                if let Some(pane)=task_rows(ui,&tasks,*selected,&Theme::current()){*selected=Some(pane);}
            },None);
            h.run_steps(3);
            assert!(h.query_by_label("캐시 무효화 문제 수정 · 완료").is_some());
            assert!(h.query_by_label("아주 긴 최근 작업 제목이 좁은 작업 공간 목록을 밀어내면 안 됩니다 · 세션 열림").is_none());
            h.get_by_label("결제 API 인증 방식 확인 · 입력 대기").click();h.run_steps(2);assert_eq!(*h.state(),Some(11));
            h.get_by_label("작업 4개 모두 보기").click();h.run_steps(2);
            let last=h.get_by_label("아주 긴 최근 작업 제목이 좁은 작업 공간 목록을 밀어내면 안 됩니다 · 세션 열림");
            assert!(h.ctx.content_rect().contains_rect(last.rect()));last.click();h.run_steps(2);assert_eq!(*h.state(),Some(14));
            h.render().unwrap().save(format!("/tmp/kiln-workspace-tasks-{}.png",width as u32)).unwrap();
        }
    }
}
