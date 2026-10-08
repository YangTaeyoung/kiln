//! 화면 구성: 상단 바(명령 바·페이지·도구), 스페이스 레일, 카드 캔버스, 도구 시트, 오버레이.

use kiln_common::widgets::{self, ButtonKind};
use super::*;
use egui::{Align2, Color32, CornerRadius, Frame, Margin, RichText, Sense, Stroke, StrokeKind, UiBuilder, pos2, vec2};
use icons::Icon;
use kiln_common::fonts;

pub const TOPBAR_H: f32 = 42.0;
const HEADER_H: f32 = 30.0;

/// One complete tool list, shared by the titlebar and workspace context menu.
fn tool_menu(ui: &mut egui::Ui, selected: Option<tools::ToolKind>) -> Option<tools::ToolKind> {
    let t = kiln_common::Theme::current();
    ui.set_width(220.0);
    let mut chosen = None;
    for kind in tools::ToolKind::ALL {
        let active = selected == Some(kind);
        let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
        response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, active, kind.label()));
        widgets::paint_row(ui.painter(), rect, active, response.hovered());
        icons::paint(ui.painter(), egui::Rect::from_center_size(pos2(rect.left()+16.0, rect.center().y), vec2(16.0,16.0)), kind.vicon(), if active {t.accent} else {t.text_dim});
        ui.painter().text(pos2(rect.left()+32.0,rect.center().y), Align2::LEFT_CENTER, kind.label(), fonts::medium(12.5), t.text);
        ui.painter().text(pos2(rect.right()-10.0,rect.center().y), Align2::RIGHT_CENTER, kind.shortcut(), fonts::medium(11.5), t.text_faint);
        widgets::focus_ring(ui, &response, 5);
        if response.clicked() { chosen=Some(kind); ui.close(); }
    }
    chosen
}

/// Repairs only the terminal transport, including when a CLI's internal hang
/// cannot be inferred from its output. Never launches or restarts the process.
fn terminal_recovery_menu_item(ui:&mut egui::Ui,conn:&mut conn::Conn,session:Option<SessionId>)->bool {
    let Some(session)=session.filter(|sid|conn.infos.get(sid).is_none_or(|info|info.exited.is_none())) else {return false;};
    if widgets::button_with(ui,Some(Icon::Refresh),kiln_common::i18n::tr("연결 다시 복구"),ButtonKind::Ghost,true).clicked() {
        conn.recover_terminal(session);
        ui.close();
        return true;
    }
    false
}

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
    pub running: bool,
    pub icon: Icon,
}

/// A constant-length arc rotates in a fixed slot: neither text metrics nor
/// animation phase can resize the title. Repaint only while it is visible.
pub(super) fn paint_running(ui: &egui::Ui, center: egui::Pos2, color: Color32) {
    let rect = egui::Rect::from_center_size(center, vec2(14.0, 14.0));
    if !ui.is_rect_visible(rect) { return; }
    let phase = ui.input(|i| (i.time / 1.2).fract()) as f32 * std::f32::consts::TAU;
    let radius = 5.5;
    ui.painter().circle_stroke(center, radius, Stroke::new(1.7, color.gamma_multiply(0.18)));
    let points = (0..=32).map(|i| {
        let angle = phase + i as f32 / 32.0 * std::f32::consts::TAU * 0.72;
        center + vec2(angle.cos(), angle.sin()) * radius
    }).collect();
    ui.painter().add(egui::Shape::line(points, Stroke::new(1.7, color)));
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
}

pub(super) fn agent_icon(agent: Option<kiln_accounts::Tool>) -> Icon {
    match agent { Some(kiln_accounts::Tool::Claude) => Icon::Claude, Some(kiln_accounts::Tool::Codex) => Icon::Codex, None => Icon::Terminal }
}

pub(super) fn agent_ink(icon: Icon, theme: &Theme) -> Color32 {
    match icon { Icon::Claude => theme.orange, Icon::Codex => theme.blue, _ => theme.text_dim }
}

/// Keep the brand legible inside a separate fixed-size activity ring.
pub(super) fn paint_agent_progress(ui: &egui::Ui, center: egui::Pos2, color: Color32) {
    let rect = egui::Rect::from_center_size(center, vec2(24.0, 24.0));
    if !ui.is_rect_visible(rect) { return; }
    let phase = ui.input(|i| (i.time / 1.2).fract()) as f32 * std::f32::consts::TAU;
    let points = (0..=32).map(|i| {
        let angle = phase + i as f32 / 32.0 * std::f32::consts::TAU * 0.72;
        center + vec2(angle.cos(), angle.sin()) * 11.5
    }).collect();
    ui.painter().add(egui::Shape::line(points, Stroke::new(1.0, color)));
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
}

pub(super) fn title_activity(title: &str, activity: kiln_proto::AgentActivity, exited: Option<i32>) -> kiln_proto::AgentActivity {
    if let Some(code) = exited {
        return if code == 0 { kiln_proto::AgentActivity::Done } else { kiln_proto::AgentActivity::Failed };
    }
    if activity == kiln_proto::AgentActivity::Unknown && action_required_title(title).is_some() {
        kiln_proto::AgentActivity::Waiting
    } else if activity == kiln_proto::AgentActivity::Unknown && terminal_title_marker(title).is_some_and(|(running, _)| running) {
        kiln_proto::AgentActivity::Running
    } else { activity }
}

/// OSC titles may begin with an agent's animated activity token. Activity already
/// has a fixed-size icon in Kiln; don't let fallback glyph advances resize tabs.
/// Only a standalone leading token is removed, never characters in the title.
/// https://github.com/openai/codex/blob/main/codex-rs/tui/src/chatwidget/status_surfaces.rs
pub(super) fn stable_terminal_title(title: &str) -> &str {
    action_required_title(title).or_else(|| terminal_title_marker(title).map(|(_, title)| title)).unwrap_or_else(|| title.trim())
}

fn action_required_title(title: &str) -> Option<&str> {
    let title=title.trim();
    for prefix in ["[ ! ] Action Required", "[ . ] Action Required"] {
        if let Some(rest)=title.strip_prefix(prefix) {
            if rest.is_empty() { return Some(""); }
            if let Some(rest)=rest.strip_prefix(" | ") { return Some(rest.trim()); }
        }
    }
    None
}

/// Codex emits braille frames. Claude Code 2.1.289 emits ◐/◑ while
/// animating and ✳ otherwise (verified in its installed title renderer).
/// Idle alone is not completion: Claude also stops animating for dialogs.
fn terminal_title_marker(title: &str) -> Option<(bool, &str)> {
    let title = title.trim();
    let first = title.chars().next()?;
    let running = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏◐◑".contains(first);
    if !running && first != '✳' { return None; }
    let rest = &title[first.len_utf8()..];
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) { return None; }
    let rest = rest.trim_start();
    // Older/custom Codex title configurations separate activity with ` | `.
    Some((running, rest.strip_prefix("| ").unwrap_or(rest).trim_start()))
}

fn is_kiro_bridge(process: &str) -> bool {
    ["zsh (kiro-cli-term)", "bash (kiro-cli-term)", "sh (kiro-cli-term)", "fish (kiro-cli-term)"].contains(&process)
}

/// Prefer an identified foreground agent; wrappers often report only the shell,
/// so recognize the same standalone OSC tokens used for activity as a fallback.
pub(super) fn session_agent(info: &kiln_proto::SessionInfo) -> Option<kiln_accounts::Tool> {
    if info.exited.is_some() { return None; }
    let process = info.fg_process.as_deref()?;
    if let Some(agent) = super::rotation::tool_for(process) { return Some(agent); }
    // Legacy/unresolved Kiro bridges may still use the exact OSC fallback.
    // A confirmed shell/editor or a failed process lookup must clear old marks.
    if !is_kiro_bridge(process) { return None; }
    if action_required_title(&info.title).is_some() { return Some(kiln_accounts::Tool::Codex); }
    terminal_title_marker(&info.title)?;
    match info.title.trim().chars().next()? {
        '◐' | '◑' | '✳' => Some(kiln_accounts::Tool::Claude),
        _ => Some(kiln_accounts::Tool::Codex),
    }
}

/// Confirmed non-agent jobs must not animate an agent's leftover OSC title.
pub(super) fn session_activity(info: &kiln_proto::SessionInfo, activity: kiln_proto::AgentActivity) -> kiln_proto::AgentActivity {
    let title = if info.fg_process.as_deref().is_some_and(|process| super::rotation::tool_for(process).is_some() || is_kiro_bridge(process)) { &info.title } else { "" };
    title_activity(title, activity, info.exited)
}

/// A shell command is a running task only with a current process lookup and
/// no agent identity. An idle agent's launch command can remain open for hours.
pub(super) fn task_activity(info: &kiln_proto::SessionInfo, telemetry: Option<&kiln_proto::SessionTelemetry>, agent: Option<kiln_accounts::Tool>) -> kiln_proto::AgentActivity {
    let activity=session_activity(info,telemetry.map(|t|t.activity).unwrap_or_default());
    if activity==kiln_proto::AgentActivity::Unknown && !info.attention && agent.is_none()
        && info.fg_process.as_deref().is_some_and(|p|!is_kiro_bridge(p) && super::rotation::tool_for(p).is_none())
        && telemetry.and_then(|t|t.commands.last()).is_some_and(|c|c.finished_unix.is_none() && !c.command.is_empty()) {
        kiln_proto::AgentActivity::Running
    } else { activity }
}

pub(super) fn running_color(agent: Option<kiln_accounts::Tool>, theme: &Theme) -> Color32 {
    match agent {
        Some(kiln_accounts::Tool::Claude) => theme.orange,
        Some(kiln_accounts::Tool::Codex) => theme.blue,
        None => theme.text_dim,
    }
}

/// 터미널 제목 중 보여줄 만한 것만 고른다. 셸 기본 제목(`user@host:경로`)이나 경로만 있는 제목은 버린다.
pub(crate) fn meaningful_title(title: &str, proc_name: &str, cwd: Option<&str>) -> Option<String> {
    let t = stable_terminal_title(title);
    if t.is_empty() || t == proc_name {
        return None;
    }
    let first = t.split_whitespace().next().unwrap_or("");
    if shells().contains(&first) && t.strip_prefix(first).is_some_and(|suffix|suffix.trim().starts_with('(')) { return None; }
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


/// Disambiguate only colliding visible titles, including truncation collisions.
fn distinct_tab_titles(labels: &[String]) -> Vec<String> {
    let visible: Vec<String> = labels.iter().enumerate().map(|(i,label)| {
        if label.is_empty() {kiln_common::trf!("작업 {}",i+1)}
        else if label.chars().count()>28 {format!("{}…",label.chars().take(27).collect::<String>())}
        else {label.clone()}
    }).collect();
    visible.iter().enumerate().map(|(i,label)| {
        if visible.iter().filter(|other|*other==label).count()>1 {
            let ordinal=visible[..=i].iter().filter(|other|*other==label).count();
            format!("{label} · {ordinal}")
        } else {label.clone()}
    }).collect()
}

fn card_title_galley(ui: &egui::Ui, title: &str, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job=egui::text::LayoutJob::simple_singleline(title.to_owned(),fonts::semibold(12.5),color);
    job.wrap.max_width=width.max(0.0);
    job.wrap.max_rows=1;job.wrap.break_anywhere=true;job.wrap.overflow_character=Some('…');
    ui.fonts_mut(|fonts|fonts.layout_job(job))
}

/// Shared by the titlebar and geometry regression tests.
fn allocate_tab_title(ui: &mut egui::Ui, title: &str, fg: Color32, max_width: f32, icon_width: f32, close_width: f32)
    -> (egui::Response, std::sync::Arc<egui::Galley>, egui::Pos2)
{
    let mut job=egui::text::LayoutJob::simple_singleline(title.to_owned(),fonts::medium(12.5),fg);
    job.wrap.max_width=(max_width-24.0-icon_width-close_width).max(24.0);
    job.wrap.max_rows=1;job.wrap.break_anywhere=true;job.wrap.overflow_character=Some('…');
    let galley=ui.fonts_mut(|fonts|fonts.layout_job(job));
    let (rect,response)=ui.allocate_exact_size(vec2((galley.size().x+24.0+icon_width+close_width).max(80.0),30.0),Sense::click());
    let origin=pos2(rect.left()+12.0+icon_width,rect.center().y-galley.size().y/2.0);
    (response,galley,origin)
}

#[cfg(test)]
mod tab_tests {
    use super::{distinct_tab_titles, meaningful_title, stable_terminal_title};

    #[test]
    fn healthy_terminal_menu_can_repair_only_its_existing_connection() {
        use egui_kittest::{Harness,kittest::Queryable};
        use kiln_proto::{SessionInfo,TerminalState};
        let mut conn=super::conn::Conn::offline(egui::Context::default());
        conn.infos.insert(7,SessionInfo {id:7,fg_process:Some("codex".into()),..Default::default()});
        conn.screens.insert(7,super::conn::Screen::default());
        let mut installed=false;
        let mut h=Harness::builder().with_size([350.0,180.0]).build_ui_state(move|ui,s:&mut(super::conn::Conn,u8)| {
            if !installed {kiln_common::fonts::install(ui.ctx());installed=true;return;}
            let menu=kiln_common::widgets::icon_button(ui,kiln_common::icons::Icon::More,26.0,false,"패널 작업");
            egui::Popup::menu(&menu).show(|ui| {
                if super::terminal_recovery_menu_item(ui,&mut s.0,Some(7)) {s.1+=1;}
            });
        },(conn,0));
        h.run_steps(3);
        h.get_by_label("패널 작업").click();h.run_steps(2);
        h.get_by_label("연결 다시 복구").click();h.run_steps(2);
        assert_eq!(h.state().1,1);
        assert_eq!(h.state().0.terminal_health[&7].state,TerminalState::Recovering);
        assert_eq!(h.state().0.infos[&7].fg_process.as_deref(),Some("codex"));
        assert!(h.state().0.screens.contains_key(&7),"manual repair retains the existing canvas");
        assert!(h.query_by_label("연결 다시 복구").is_none(),"repair closes the menu");
        h.state_mut().0.infos.get_mut(&7).unwrap().exited=Some(0);
        h.get_by_label("패널 작업").click();h.run_steps(2);
        assert!(h.query_by_label("연결 다시 복구").is_none(),"completed sessions need no repair action");
    }

    #[test]
    fn running_indicator_rotates_without_changing_layout() {
        use egui_kittest::Harness;
        let mut installed=false;
        let mut h=Harness::builder().with_size([840.0,140.0]).build_ui(move |ui| {
            if !installed {kiln_common::fonts::install(ui.ctx()); installed=true; return;}
            let before=ui.cursor();
            super::paint_running(ui,egui::pos2(24.0,30.0),egui::Color32::from_rgb(126,177,255));
            assert_eq!(ui.cursor(),before,"animation must not allocate or move title layout");
            ui.painter().text(egui::pos2(42.0,30.0),egui::Align2::LEFT_CENTER,"애니 실시간 업데이트 방식 조사",kiln_common::fonts::semibold(12.5),egui::Color32::WHITE);
        });
        h.set_pixels_per_point(2.0);
        h.run_steps(3);
        let mut frames=Vec::new();
        for (i,time) in [1.0,1.3,1.6].into_iter().enumerate() {
            h.input_mut().time=Some(time);
            h.step();
            let path=h.output().shapes.iter().find_map(|shape| match &shape.shape {egui::Shape::Path(path)=>Some(path),_=>None}).unwrap();
            assert_eq!(path.points.len(),33);
            assert!(path.points.iter().all(|p|egui::Rect::from_center_size(egui::pos2(24.0,30.0),egui::vec2(14.0,14.0)).contains(*p)));
            frames.push(path.points.clone());
            h.render().unwrap().save(format!("/tmp/kiln-running-{i}.png")).unwrap();
        }
        assert_ne!(frames[0],frames[1]);
        assert_ne!(frames[1],frames[2]);
    }

    #[test]
    fn non_agent_or_failed_lookup_clears_stale_title_activity_and_brand() {
        use kiln_proto::{SessionInfo, AgentActivity};
        for process in [None, Some("bash"), Some("vim"), Some("other (kiro-cli-term)")] {
            for title in ["⠋ Old task", "✳ Old task", "[ ! ] Action Required | Old task"] {
                let info = SessionInfo { fg_process: process.map(String::from), title: title.into(), ..Default::default() };
                assert_eq!(super::session_agent(&info), None);
                assert_eq!(super::session_activity(&info, AgentActivity::Unknown), AgentActivity::Unknown);
                // Explicit integration signals remain authoritative.
                assert_eq!(super::session_activity(&info, AgentActivity::Failed), AgentActivity::Failed);
            }
        }
    }

    #[test]
    fn agent_colors_follow_process_or_standalone_title_markers() {
        use kiln_accounts::Tool::{Claude,Codex};
        use kiln_proto::SessionInfo;
        for (title,process,expected) in [
            ("◐ Work", "zsh (kiro-cli-term)", Some(Claude)),
            ("◑ Work", "zsh (kiro-cli-term)", Some(Claude)),
            ("✳ Work", "zsh (kiro-cli-term)", Some(Claude)),
            ("✳ Work", "zsh", None),
            ("⠼ stale", "sh", None),
            ("✳ stale", "vim", None),
            ("⠋ stale", "sleep", None),
            ("⠼ Work", "zsh (kiro-cli-term)", Some(Codex)),
            ("⠼ Work", "claude", Some(Claude)),
            ("Work", "codex", Some(Codex)),
            ("Work", "claude", Some(Claude)),
            ("[ ! ] Action Required | Work", "zsh (kiro-cli-term)", Some(Codex)),
            ("[ . ] Action Required | Work", "zsh (kiro-cli-term)", Some(Codex)),
            ("[ ! ] Action Requiredness", "zsh", None),
            ("◐project", "zsh", None),
            ("Work ◑", "zsh", None),
            ("Work on claude integration", "zsh", None),
        ] {
            let info=SessionInfo{title:title.into(),fg_process:Some(process.into()),..Default::default()};
            assert_eq!(super::session_agent(&info),expected,"{title} / {process}");
        }
        for theme in super::Theme::ALL {
            assert_eq!(super::running_color(Some(Claude),&theme),theme.orange);
            assert_eq!(super::running_color(Some(Codex),&theme),theme.blue);
            assert_eq!(super::running_color(None,&theme),theme.text_dim);
            assert_ne!(theme.orange,theme.blue);
        }
    }

    #[test]
    fn animated_terminal_title_frames_keep_identical_tab_text() {
        let title = "애니 실시간 업데이트 방식 조사 | personal";
        let expected = meaningful_title(title, "codex", None).unwrap();
        for frame in "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏◐◑✳".chars() {
            for separator in [" ", "  ", " | "] {
                let actual = meaningful_title(&format!("{frame}{separator}{title}"), "codex", None).unwrap();
                assert_eq!(actual, expected);
                assert_eq!(distinct_tab_titles(&[actual]), distinct_tab_titles(&[expected.clone()]));
            }
        }
        assert_eq!(meaningful_title("⠼", "codex", None), None);
    }

    #[test]
    fn spinner_activity_fallback_preserves_explicit_agent_states() {
        use kiln_proto::AgentActivity::*;
        assert_eq!(super::title_activity("⠼ 작업",Unknown,None),Running);
        assert_eq!(super::title_activity("작업",Unknown,None),Unknown);
        for title in ["◐ Claude task", "◑ Claude task"] {
            assert_eq!(super::title_activity(title, Unknown, None), Running);
            for state in [Waiting, Failed, Done] { assert_eq!(super::title_activity(title,state,None),state); }
            assert_eq!(super::title_activity(title,Unknown,Some(0)),Done);
            assert_eq!(super::title_activity(title,Running,Some(1)),Failed);
        }
        for title in ["✳ Claude task", "◐project", "project ◑", "plain title"] {
            assert_eq!(super::title_activity(title, Unknown, None), Unknown);
        }
        assert_eq!(super::title_activity("⠼ 작업",Unknown,Some(0)),Done);
        assert_eq!(super::title_activity("⠼ 작업",Running,Some(0)),Done);
        assert_eq!(super::title_activity("⠼ 작업",Running,Some(1)),Failed);
        for state in [Waiting,Failed,Done,Running] {
            assert_eq!(super::title_activity("⠼ 작업",state,None),state);
        }
    }

    #[test]
    fn animated_titles_preserve_rendered_tab_and_glyph_geometry() {
        use egui_kittest::Harness;
        #[derive(Default)]
        struct State {
            title: String,
            geometry: Option<(egui::Rect, egui::Pos2, egui::Vec2, Vec<(char, f32)>, egui::Vec2, Vec<(char, f32)>)>,
        }
        for max_width in [110.0, 320.0] {
            for text in ["빌드", "애니 실시간 업데이트 방식 조사와 한글 글꼴 확인 | personal"] {
                let mut initialized=false;
                let mut h=Harness::builder().with_size([500.0,100.0]).build_ui_state(move |ui,state: &mut State| {
                    if !initialized { kiln_common::fonts::install(ui.ctx()); initialized=true; return; }
                    let label=meaningful_title(&state.title,"codex",None).unwrap();
                    let labels=distinct_tab_titles(&[label.clone()]);
                    let (response,galley,origin)=super::allocate_tab_title(ui,&labels[0],egui::Color32::WHITE,max_width,22.0,24.0);
                    assert_eq!(galley.rows.len(),1);
                    assert!(response.rect.width() <= max_width+0.5);
                    let glyphs=galley.rows.iter().flat_map(|row|row.glyphs.iter().map(|glyph|(glyph.chr,glyph.pos.x))).collect();
                    let card=super::card_title_galley(ui,&label,egui::Color32::WHITE,max_width-70.0);
                    assert_eq!(card.rows.len(),1);
                    assert!(card.size().x <= max_width-70.0+0.5);
                    let card_glyphs=card.rows.iter().flat_map(|row|row.glyphs.iter().map(|glyph|(glyph.chr,glyph.pos.x))).collect();
                    state.geometry=Some((response.rect,origin,galley.size(),glyphs,card.size(),card_glyphs));
                    ui.painter().galley(origin,galley,egui::Color32::WHITE);
                    ui.painter().galley(egui::pos2(44.0,50.0),card,egui::Color32::WHITE);
                }, State{title:format!("⠋ {text}"),..Default::default()});
                h.run_steps(3);
                let expected=h.state().geometry.clone().unwrap();
                for frame in "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏◐◑✳".chars() {
                    h.state_mut().title=format!("{frame} {text}");
                    h.step();
                    assert_eq!(h.state().geometry.as_ref(),Some(&expected),"{frame} at {max_width}px: {text}");
                }
            }
        }
    }

    #[test]
    fn title_cleanup_preserves_braille_content_and_user_tab_names() {
        for title in ["⠼braille", "점자 ⠹ 문서", "⠼⠹", "일반  작업 | personal"] {
            assert_eq!(stable_terminal_title(title), title);
        }
        assert_eq!(stable_terminal_title("[ ! ] Action Required | Work"),"Work");
        assert_eq!(stable_terminal_title("[ . ] Action Required | Work"),"Work");
        assert_eq!(stable_terminal_title("[ ! ] Action Required"),"");
        // Explicit page titles bypass meaningful_title and must remain unchanged.
        assert_eq!(distinct_tab_titles(&["⠼ 내 작업".into()]), ["⠼ 내 작업"]);
    }

    #[test]
    fn duplicate_and_truncated_tabs_remain_distinct() {
        let titles=distinct_tab_titles(&["personal · 터미널".into(),"Build".into(),"personal · 터미널".into()]);
        assert_eq!(titles,["personal · 터미널 · 1","Build","personal · 터미널 · 2"]);
        let prefix="a".repeat(28);
        let titles=distinct_tab_titles(&[format!("{prefix} one"),format!("{prefix} two")]);
        assert_ne!(titles[0],titles[1]);
    }
}

impl KilnApp {
    pub(crate) fn card_info(&self, pid: PaneId) -> CardInfo {
        let t = self.theme;
        let Some(pane) = self.panes.get(&pid) else {
            return CardInfo { name: String::new(), detail: String::new(), right: String::new(), dot: t.text_faint, pulse: false, running: false, icon: Icon::Terminal };
        };
        match &pane.kind {
            PaneKind::Term { session, .. } => match session.and_then(|s| self.conn.infos.get(&s)) {
                Some(i) => {
                    let raw_process = i.fg_process.clone().unwrap_or_else(|| kiln_common::i18n::tr("셸").into());
                    let base = raw_process.split_whitespace().next().unwrap_or(&raw_process);
                    let proc_name = if shells().contains(&base) {base.to_owned()} else {raw_process};
                    let is_shell = shells().contains(&proc_name.as_str()) || proc_name == kiln_common::i18n::tr("셸");
                    let activity=task_activity(i,session.and_then(|sid|self.conn.telemetry.get(&sid)),session.and_then(|sid|self.conn.session_agent(sid)));
                    let (dot,pulse,status)=match (i.exited,activity) {
                        (Some(0),_) => (t.text_dim,false,kiln_common::i18n::tr("종료됨")),
                        (Some(_),_) => (t.red,false,kiln_common::i18n::tr("오류 종료")),
                        (_,kiln_proto::AgentActivity::Waiting)=>(t.orange,true,kiln_common::i18n::tr("입력 필요")),
                        (_,kiln_proto::AgentActivity::Running) if !i.attention => (running_color(session.and_then(|sid|self.conn.session_agent(sid)),&t),true,kiln_common::i18n::tr("실행 중")),
                        (_,kiln_proto::AgentActivity::Done)=>(t.green,false,kiln_common::i18n::tr("완료")),
                        (_,kiln_proto::AgentActivity::Failed)=>(t.red,false,kiln_common::i18n::tr("실패")),
                        _ if i.attention=>(t.orange,true,kiln_common::i18n::tr("확인 필요")),
                        _ if is_shell=>(t.text_faint,false,""),
                        _=>(t.text_faint,false,""),
                    };
                    let name = meaningful_title(&i.title, &proc_name, i.cwd.as_deref()).unwrap_or(proc_name);
                    let detail = status.into();
                    let right = i.cwd.as_deref().map(|c| short_path(Path::new(c))).unwrap_or_default();
                    let running = activity == kiln_proto::AgentActivity::Running && i.exited.is_none() && !i.attention;
                    let icon = agent_icon(session.and_then(|sid| self.conn.session_agent(sid)));
                    CardInfo { name, detail, right, dot, pulse, running, icon }
                }
                None => CardInfo { name: if self.terminal_launch_error(pid).is_some(){kiln_common::i18n::tr("시작 요청 보관됨")}else{kiln_common::i18n::tr("시작하는 중…")}.into(), detail: String::new(), right: String::new(), dot: if self.terminal_launch_error(pid).is_some(){t.orange}else{t.text_faint}, pulse: false, running: false, icon: Icon::Terminal },
            },
            PaneKind::Tool(tool) => {
                let dirty = tool.is_dirty();
                let key = tool.key();
                let icon = if key.starts_with("db") {
                    Icon::Database
                } else if key.starts_with("pr:") {
                    Icon::PullRequest
                } else if key.starts_with("git-history") || key.starts_with("diff") || key.starts_with("commit") || key.starts_with("range:") || key.starts_with("issue:") {
                    Icon::Branch
                } else {
                    Icon::File
                };
                CardInfo {
                    name: tool.title(),
                    detail: if dirty { kiln_common::i18n::tr("저장 안 됨").into() } else { String::new() },
                    right: tool.status_text().unwrap_or_default(),
                    dot: if dirty { t.yellow } else { t.accent },
                    pulse: false,
                    running: false,
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
        let main = names.iter().find(|n| !shells().contains(&n.as_str()) && n.as_str() != kiln_common::i18n::tr("셸")).cloned().unwrap_or_else(|| names.first().cloned().unwrap_or_default());
        let main=if shells().contains(&main.as_str()) || main==kiln_common::i18n::tr("셸") {
            let cwd=panes.iter().find_map(|id|self.panes.get(id).and_then(|p|p.session().and_then(|sid|self.conn.infos.get(&sid)).and_then(|i|i.cwd.as_deref()).or(p.cwd.as_deref())));
            let folder=cwd.and_then(|cwd|Path::new(cwd).file_name()).map(|n|n.to_string_lossy().into_owned());
            folder.map(|name|kiln_common::trf!("{name} · 터미널")).unwrap_or_else(||kiln_common::i18n::tr("터미널").into())
        }else{main};
        main
    }

    fn page_attention(&self, page: &Page) -> bool {
        page.root.panes().iter().any(|p| self.panes.get(p).and_then(|x| x.session()).and_then(|s| self.conn.infos.get(&s)).is_some_and(|i| i.attention))
    }

    /// Aggregate only reported activity; a running shell is not an agent in progress.
    fn page_activity(&self, page: &Page) -> Option<(Icon, Color32, &'static str, Option<kiln_accounts::Tool>)> {
        use kiln_proto::AgentActivity;
        let mut best: Option<(u8, Icon, Color32, &'static str, Option<kiln_accounts::Tool>)> = None;
        for id in page.root.panes() {
            let Some(session)=self.panes.get(&id).and_then(|pane|pane.session()) else {continue;};
            let info=self.conn.infos.get(&session);
            let activity=self.conn.telemetry.get(&session).map(|state|state.activity).unwrap_or_default();
            let activity=info.map(|info|session_activity(info,activity)).unwrap_or(activity);
            let state=match activity {
                AgentActivity::Failed => Some((5,Icon::Warning,self.theme.red,kiln_common::i18n::tr("실패"))),
                AgentActivity::Waiting => Some((4,Icon::Bell,self.theme.orange,kiln_common::i18n::tr("입력 필요"))),
                _ if info.is_some_and(|info|info.attention) => Some((3,Icon::Bell,self.theme.orange,kiln_common::i18n::tr("확인 필요"))),
                AgentActivity::Running => Some((2,Icon::Play,running_color(self.conn.session_agent(session),&self.theme),kiln_common::i18n::tr("실행 중"))),
                AgentActivity::Done => Some((1,Icon::Check,self.theme.green,kiln_common::i18n::tr("완료"))),
                _ => None,
            };
            if let Some(state)=state {if best.as_ref().is_none_or(|old|state.0>old.0 || (state.0==old.0 && id==page.focused)){best=Some((state.0,state.1,state.2,state.3,self.conn.session_agent(session)));}}
        }
        best.map(|(_,icon,color,label,agent)|(icon,color,label,agent)).or_else(|| {
            let agent=self.panes.get(&page.focused).and_then(Pane::session).and_then(|session|self.conn.session_agent(session))?;
            Some((Icon::Terminal,agent_ink(agent_icon(Some(agent)),&self.theme),kiln_common::i18n::tr("세션 열림"),Some(agent)))
        })
    }

    // ------------------------------------------------------------------ 상단 바

    pub(super) fn ui_topbar(&mut self, root: &mut egui::Ui) {
        let t = self.theme;
        let canvas = widgets::canvas_color(&t);
        egui::Panel::top("topbar").exact_size(TOPBAR_H).frame(Frame::new().fill(canvas)).show(root, |ui| {
            let full = ui.max_rect();
            let bar = egui::Rect::from_min_size(full.min, vec2(full.width(), TOPBAR_H));
            let bg = ui.interact(bar, ui.id().with("drag"), Sense::click_and_drag());
            if bg.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if bg.double_clicked() {
                let max = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
            }
            let cy = bar.center().y;
            let left = bar.left() + if is_mac() && !ui.ctx().input(|i| i.viewport().fullscreen.unwrap_or(false)) { 80.0 } else { 8.0 };
            let mut lui = ui.new_child(UiBuilder::new().max_rect(egui::Rect::from_min_size(pos2(left, cy - 15.0), vec2(30.0, 30.0))));
            if widgets::icon_button(&mut lui, Icon::Sidebar, 30.0, self.sidebar_open, &kiln_common::trf!("워크스페이스 사이드바 ({})",self.keymap.label("sidebar","⌘B"))).clicked() {
                self.actions.push(Action::ToggleSidebar);
            }
            let tool_label = kiln_common::i18n::tr("도구");
            let tool_width = ui.painter().layout_no_wrap(tool_label.into(), fonts::medium(12.5), t.text).size().x + 40.0;
            let tools_width = 12.0 + 3.0 * 30.0 + 3.0 * 4.0 + tool_width;
            let right = egui::Rect::from_min_max(pos2(bar.right() - tools_width, cy - 15.0), pos2(bar.right() - 12.0, cy + 15.0));
            let mut rui = ui.new_child(UiBuilder::new().max_rect(right).layout(egui::Layout::right_to_left(egui::Align::Center)));
            rui.spacing_mut().item_spacing.x = 4.0;
            if widgets::icon_button(&mut rui, Icon::Gear, 30.0, self.settings_ui.open, kiln_common::i18n::tr("설정 (⌘,)")).clicked() {
                self.actions.push(Action::OpenSettings);
            }
            let unread = self.notifications.unread_count();
            let bell = widgets::icon_button(&mut rui, Icon::Bell, 30.0, self.notifications.open, kiln_common::i18n::tr("알림 센터"));
            if unread > 0 {
                let c = pos2(bell.rect.right() - 7.0, bell.rect.top() + 7.0);
                rui.painter().circle_filled(c, 6.0, t.orange);
                rui.painter().text(c, Align2::CENTER_CENTER, if unread > 9 { "9+".into() } else { unread.to_string() }, fonts::semibold(9.0), Color32::BLACK);
            }
            if bell.clicked() { self.actions.push(Action::ToggleNotifications); }
            if widgets::icon_button(&mut rui, Icon::Search, 30.0, self.palette.is_open(), &kiln_common::trf!("검색 및 명령 실행 ({})",self.keymap.label("palette","⌘K"))).clicked() {
                self.actions.push(Action::OpenPalette);
            }
            let selected = self.workspaces[self.active].sheet.filter(|_| !self.workspaces[self.active].tools.is_agent_task_open());
            let menu = widgets::button_with(&mut rui, Some(Icon::Inspector), tool_label, if selected.is_some() {ButtonKind::Secondary} else {ButtonKind::Ghost}, true);
            menu.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected.is_some(), tool_label));
            egui::Popup::menu(&menu).show(|ui| {
                if let Some(kind) = tool_menu(ui, selected) { self.actions.push(Action::OpenSheet(kind)); }
            });
            // One titlebar: project identity belongs to the sidebar; pages belong here.
            let lane = egui::Rect::from_min_max(pos2(left + 40.0, full.top() + 5.0), pos2(right.left() - 10.0, full.bottom() - 5.0));
            let mut pui = ui.new_child(UiBuilder::new().max_rect(lane).layout(egui::Layout::left_to_right(egui::Align::Center)));
            pui.set_clip_rect(lane.intersect(ui.clip_rect()));
            pui.spacing_mut().item_spacing.x = 6.0;
            let create=widgets::button_with(&mut pui,Some(Icon::Plus),kiln_common::i18n::tr("새 작업"),ButtonKind::Secondary,true);
            egui::Popup::menu(&create).show(|ui| {
                if widgets::button_with(ui,Some(Icon::Codex),"Codex",ButtonKind::Ghost,true).clicked(){self.actions.push(Action::DirectAgent { tool: kiln_accounts::Tool::Codex, cwd: None });ui.close();}
                if widgets::button_with(ui,Some(Icon::Claude),"Claude Code",ButtonKind::Ghost,true).clicked(){self.actions.push(Action::DirectAgent { tool: kiln_accounts::Tool::Claude, cwd: None });ui.close();}
                ui.separator();
                if widgets::button_with(ui,Some(Icon::Sparkle),kiln_common::i18n::tr("에이전트 요청"),ButtonKind::Ghost,true).clicked(){self.actions.push(Action::NewAgentTask);ui.close();}
                if widgets::button_with(ui,Some(Icon::Terminal),kiln_common::i18n::tr("새 터미널"),ButtonKind::Ghost,true).on_hover_text(self.keymap.label("new_task","⌘T")).clicked(){self.actions.push(Action::NewPage);ui.close();}
                ui.separator();
                ui.menu_button(kiln_common::i18n::tr("폴더에서 열기"), |ui| {
                    if widgets::button_with(ui,Some(Icon::Codex),"Codex",ButtonKind::Ghost,true).clicked(){self.actions.push(Action::OpenAgentFolder(kiln_accounts::Tool::Codex));ui.close();}
                    if widgets::button_with(ui,Some(Icon::Claude),"Claude Code",ButtonKind::Ghost,true).clicked(){self.actions.push(Action::OpenAgentFolder(kiln_accounts::Tool::Claude));ui.close();}
                    if widgets::button_with(ui,Some(Icon::Terminal),kiln_common::i18n::tr("터미널"),ButtonKind::Ghost,true).clicked(){self.actions.push(Action::OpenFolder);ui.close();}
                });
            });
            let tab_max=(lane.width()-create.rect.width()-6.0).clamp(80.0,360.0);
            let active = self.workspaces[self.active].active_page;
            let labels: Vec<_> = self.workspaces[self.active].pages.iter().map(|p| (self.page_label(p), self.page_attention(p))).collect();
            let activities:Vec<_>=self.workspaces[self.active].pages.iter().map(|p|self.page_activity(p)).collect();
            let tab_titles = distinct_tab_titles(&labels.iter().map(|(label,_)|label.clone()).collect::<Vec<_>>());
            let selected_id = ui.id().with(("visible-page", self.workspaces[self.active].id));
            let changed = ui.ctx().data_mut(|d| {
                let visible = (active, full.width().round() as u32);
                let old = d.get_temp::<(usize, u32)>(selected_id);
                d.insert_temp(selected_id, visible);
                old != Some(visible)
            });
            egui::ScrollArea::horizontal().id_salt("pages").auto_shrink([false, false]).show(&mut pui, |ui| {
                ui.horizontal(|ui| {
                    for (i, (label, attention)) in labels.iter().enumerate() {
                        let label = if label.is_empty() { kiln_common::trf!("작업 {}", i + 1) } else { label.clone() };
                        let title = &tab_titles[i];
                        let activity=activities[i];
                        let fg=if *attention {t.orange}else if i==active {t.text}else{t.text_dim};
                        let icon_width=if activity.is_some(){22.0}else{0.0};
                        let close_width=if i==active {24.0}else{0.0};
                        let (response,galley,title_origin)=allocate_tab_title(ui,title,fg,tab_max,icon_width,close_width);
                        let rect=response.rect;
                        let accessible_title=activity.map(|(_,_,status,agent)|match agent {
                            Some(tool)=>format!("{title} · {} · {status}",tool.display_name()),
                            None=>format!("{title} · {status}"),
                        }).unwrap_or_else(||title.clone());
                        response.widget_info(||egui::WidgetInfo::selected(egui::WidgetType::Button,true,i==active,&accessible_title));
                        let tab_bg=if i==active{t.bg_panel}else if response.hovered(){t.bg_hover}else{canvas};
                        ui.painter().rect_filled(rect,5,tab_bg);
                        if let Some((icon,color,_,agent))=activity {
                            let center=pos2(rect.left()+17.0,rect.center().y);
                            if let Some(agent)=agent {
                                if icon==Icon::Play {paint_agent_progress(ui,center,color);}
                                let mark=agent_icon(Some(agent));
                                icons::paint(ui.painter(),egui::Rect::from_center_size(center,vec2(18.0,18.0)),mark,agent_ink(mark,&t));
                                if icon!=Icon::Play && icon!=Icon::Terminal {
                                    let badge=center+vec2(8.0,8.0);
                                    ui.painter().circle_filled(badge,3.5,tab_bg);
                                    ui.painter().circle_filled(badge,2.5,color);
                                }
                            } else if icon==Icon::Play {paint_running(ui,center,color);}
                            else {icons::paint(ui.painter(),egui::Rect::from_center_size(center,vec2(14.0,14.0)),icon,color);}
                        }
                        ui.painter().galley(title_origin,galley,fg);
                        widgets::focus_ring(ui,&response,5);
                        if i == active {ui.painter().line_segment([response.rect.left_bottom(),response.rect.right_bottom()],Stroke::new(2.0,t.accent));}
                        if i == active && changed { response.scroll_to_me_animation(Some(egui::Align::Center), egui::style::ScrollAnimation::none()); }
                        if response.clicked() { self.actions.push(Action::SelectPage(i)); }
                        if response.middle_clicked() { self.actions.push(Action::ClosePage(i, false)); }
                        if i==active {
                            let close_rect=egui::Rect::from_center_size(pos2(rect.right()-13.0,rect.center().y),vec2(22.0,22.0));
                            let mut close_ui=ui.new_child(UiBuilder::new().max_rect(close_rect));
                            if widgets::icon_button(&mut close_ui,Icon::Close,22.0,false,kiln_common::i18n::tr("작업 탭 닫기")).clicked(){self.actions.push(Action::ClosePage(i,false));}
                        }
                        if response.double_clicked() { self.rename_page = Some((i, label.clone())); }
                        response.on_hover_text(kiln_common::trf!("{label}{}\n작업 {} · 우클릭으로 작업 메뉴", activity.map(|(_,_,status,agent)|format!("\n{}{status}",agent.map(|tool|format!("{} · ",tool.display_name())).unwrap_or_default())).unwrap_or_default(), i + 1)).context_menu(|ui| {
                            if self.workspaces[self.active].pages[i].agent_request.is_some() {
                                if ui.button(kiln_common::i18n::tr("요청 보기")).clicked(){self.actions.push(Action::ShowAgentRequest(i));ui.close();}
                                if ui.button(kiln_common::i18n::tr("요청 복사")).clicked(){self.actions.push(Action::CopyAgentRequest(i));ui.close();}
                                ui.separator();
                            }
                            if ui.button(kiln_common::i18n::tr("작업 이름 바꾸기")).clicked() { self.rename_page = Some((i, label.clone())); ui.close(); }
                            if ui.button(kiln_common::i18n::tr("작업 탭 닫기")).clicked() {
                                self.actions.push(Action::ClosePage(i, false));
                                ui.close();
                            }
                        });
                    }
                });
            });
            ui.painter().line_segment([full.left_bottom(), full.right_bottom()], Stroke::new(1.0, t.border));
        });
    }

    // ------------------------------------------------------------------ 스페이스 레일

    pub(super) fn ui_spaces(&mut self, root: &mut egui::Ui) {
        let t = self.theme;
        let narrow = root.available_width() < 900.0;
        let wide = self.sidebar_open;
        let (width, rail_min, rail_max) = self.sidebar_dimensions(root.available_width());

        let panel = egui::Panel::left(if wide { "projects-expanded" } else { "projects-collapsed" })
            .default_size(width).min_size(if wide {rail_min} else {48.0})
            .max_size(if wide {rail_max} else {48.0}).resizable(wide)
            .frame(Frame::new().fill(widgets::canvas_color(&t)).inner_margin(Margin::same(8)))
            .show(root, |ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 6.0);
                ui.horizontal(|ui| {
                    if wide {ui.label(RichText::new(kiln_common::i18n::tr("워크스페이스")).font(fonts::semibold(12.0)).color(t.text_dim));}
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::icon_button(ui,Icon::Plus,26.0,false,kiln_common::i18n::tr("새 워크스페이스")).clicked(){self.actions.push(Action::NewWorkspace(None));}
                        if wide && widgets::icon_button(ui,Icon::History,26.0,false,&kiln_common::trf!("작업 검색 ({})",self.keymap.label("recent","⌘J"))).clicked(){self.actions.push(Action::OpenRecent);}
                    });
                });
                let list_height = ui.available_height().max(60.0);
                let scroll_active = ui.data_mut(|d|{
                    let key=egui::Id::new("project-list-visible-state");
                    let current=(self.active,list_height.round() as i32,wide);
                    let changed=d.get_temp::<(usize,i32,bool)>(key)!=Some(current);d.insert_temp(key,current);changed
                });
                egui::ScrollArea::vertical().id_salt("workspace-list").max_height(list_height).min_scrolled_height(list_height).auto_shrink([false,false]).show(ui,|ui|{
                    for (i,depth) in self.workspace_order(){
                        ui.push_id(i,|ui|{
                            if depth>0 && wide {ui.horizontal(|ui|{ui.add_space(12.0);ui.vertical(|ui|{self.space_row(ui,i,wide,scroll_active);});});}
                            else {self.space_row(ui,i,wide,scroll_active);}
                        });
                    }
                    if wide {self.detached_sessions(ui);}
                });

            });
        if wide && !narrow {self.settings.sidebar_width=panel.response.rect.width();}
    }

    fn workspace_order(&self)->Vec<(usize,usize)> {
        let parent:Vec<Option<usize>>=self.workspaces.iter().enumerate().map(|(i,ws)|{
            let identity=ws.tools.worktree_identity()?;
            if !identity.linked {return None;}
            self.workspaces.iter().enumerate().filter(|(j,other)|*j!=i && !other.tools.worktree_identity().is_some_and(|x|x.linked) && identity.main_root.starts_with(other.tools.canonical_root()))
                .max_by_key(|(_,other)|other.root.components().count()).map(|(j,_)|j)
        }).collect();
        let mut order=Vec::new();
        for i in 0..self.workspaces.len(){if parent[i].is_none(){order.push((i,0));for (j,p) in parent.iter().enumerate(){if *p==Some(i){order.push((j,1));}}}}
        order
    }

    fn space_row(&mut self, ui: &mut egui::Ui, i: usize, wide: bool, scroll_active: bool) {
        let t = self.theme;
        let active = i == self.active;
        let tasks = self.workspace_tasks(i);
        let ws = &self.workspaces[i];
        let sessions: Vec<SessionId> = ws.all_panes().iter().filter_map(|p| self.panes.get(p).and_then(|x| x.session())).collect();
        let infos: Vec<&kiln_proto::SessionInfo> = sessions.iter().filter_map(|s| self.conn.infos.get(s)).collect();
        let attention = infos.iter().filter(|x| x.attention).count();
        let running:Vec<_> = tasks.iter().filter(|task|task.phase==super::workspace_activity::TaskPhase::Running).map(|task|task.title.as_str()).collect();
        let note = infos.iter().filter(|x| x.attention).find_map(|x| x.last_notification.clone());
        let name = ws.name.clone();
        let root_path = ws.root.clone();
        let task_context = self.projects.task_context(&root_path);
        let h = if wide { if tasks.len()>3 {70.0}else{52.0} } else { 44.0 };
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, ui.is_enabled(), active, &name));
        widgets::focus_ring(ui, &resp, 5);
        let fill = if active {
            t.bg_elevated
        } else if resp.hovered() {
            widgets::lerp_color(widgets::canvas_color(&t), t.bg_elevated, 0.55)
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, CornerRadius::same(5), fill);
        if active {
            ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, t.border), StrokeKind::Inside);
        }
        let av = egui::Rect::from_center_size(pos2(rect.center().x,rect.center().y),vec2(30.0,30.0));
        if !wide { widgets::avatar(ui, av, &name, active); }
        if (attention > 0 || !running.is_empty()) && !wide {
            let c = pos2(av.right() - 1.0, av.top() + 1.0);
            ui.painter().circle_filled(c, 6.5, widgets::canvas_color(&t));
            widgets::status_dot(ui, c, if attention>0{t.orange}else{t.blue}, attention>0);
        }
        if wide {
            let mut text_rect = rect.shrink2(vec2(10.0, 6.0));
            if attention>0 || !running.is_empty() {text_rect.max.x-=12.0;}
            let mut row = ui.new_child(UiBuilder::new().max_rect(text_rect));
            row.set_clip_rect(text_rect.intersect(ui.clip_rect()));
            row.spacing_mut().item_spacing.y = 2.0;
            row.add(egui::Label::new(RichText::new(&name).font(fonts::semibold(13.0)).color(if active { t.text } else { t.text_dim })).truncate());
            let repo = self.workspaces[i].tools.summary();
            let identity = self.workspaces[i].tools.worktree_identity();
            let location = if let Some(identity)=identity {
                let mut text=format!("{} · {}",if identity.linked {kiln_common::i18n::tr("워크트리")}else{kiln_common::i18n::tr("메인")},identity.branch.as_deref().unwrap_or("detached"));
                if let Some(summary)=repo.as_ref().filter(|summary|summary.dirty>0){text+=&kiln_common::trf!(" · {} 변경",summary.dirty);}
                text
            } else {repo.map(|s| {
                let mut text = s.branch;
                if s.dirty > 0 { text += &kiln_common::trf!(" · {} 변경", s.dirty); }

                if let Some((n, state)) = s.pr { text += &format!(" · #{n} {state}"); }
                text
            }).unwrap_or_else(|| short_path(&root_path))};
            row.add(egui::Label::new(RichText::new(location).size(12.0).color(t.text_dim)).truncate());
            if tasks.len()>3 {
                let summary=super::workspace_activity::summary(&tasks);
                row.add(egui::Label::new(RichText::new(&summary).size(11.5).color(t.text_dim)).truncate()).on_hover_text(summary);
            }
            if attention > 0 || !running.is_empty() {
                let badge = pos2(rect.right()-10.0,rect.top()+12.0);
                widgets::status_dot(ui,badge,if attention>0{t.orange}else{t.blue},attention>0);
            }

        }
        if active && scroll_active {resp.scroll_to_me_animation(Some(egui::Align::Center),egui::style::ScrollAnimation::none());}
        let mut tooltip=format!("{}\n{}", name, short_path(&root_path));
        if !running.is_empty(){tooltip.push_str(&kiln_common::trf!("\n실행 중: {}",running.join(", ")));}
        if let Some(identity)=ws.tools.worktree_identity(){tooltip.push_str(&kiln_common::trf!("\n메인 저장소: {}",identity.main_root.display()));}
        if let Some(note)=note {tooltip.push_str(&format!("\n{note}"));}
        if let Some((_,memo,status))=task_context { if !memo.is_empty(){tooltip.push_str(&format!("\n{status} · {memo}"));} }
        let resp = resp.on_hover_text(tooltip);
        if resp.clicked() {
            self.actions.push(Action::SelectWorkspace(i));
        }
        if resp.double_clicked() && wide {
            self.workspaces[i].renaming = Some(name.clone());
        }
        resp.context_menu(|ui| {
            if ui.button(kiln_common::i18n::tr("새 워크스페이스")).clicked() {
                self.actions.push(Action::NewWorkspaceAt(root_path.clone())); ui.close();
            }
            if ui.button(kiln_common::i18n::tr("이름 바꾸기")).clicked() {
                self.workspaces[i].renaming = Some(name.clone());
                ui.close();
            }
            if ui.button(kiln_common::i18n::tr("Finder 에서 열기")).clicked() {
                let _ = open::that_detached(&root_path);
                ui.close();
            }
            ui.separator();
            ui.menu_button(kiln_common::i18n::tr("도구"),|ui|{
                if let Some(kind) = tool_menu(ui, self.workspaces[i].sheet) {
                    self.actions.push(Action::SelectWorkspace(i));
                    self.actions.push(Action::OpenSheet(kind));
                }
                ui.separator();
                for (label,action) in [(kiln_common::i18n::tr("빠른 터미널"),Action::ToggleQuickTerminal),(kiln_common::i18n::tr("저장 명령"),Action::OpenLaunchers),(kiln_common::i18n::tr("작업 복구 센터"),Action::OpenRecovery),(kiln_common::i18n::tr("프로젝트 관리"),Action::OpenProjects)] {
                    if ui.button(label).clicked(){self.actions.push(Action::SelectWorkspace(i));self.actions.push(action);ui.close();}
                }
            });
            ui.menu_button(kiln_common::i18n::tr("패널 배치"),|ui|{
                let mut automatic=!self.workspaces[i].page().manual_split;
                if ui.checkbox(&mut automatic,kiln_common::i18n::tr("좁아지면 선택한 패널만 표시")).changed(){self.workspaces[i].page_mut().manual_split=!automatic;}
                for (label,action) in [(kiln_common::i18n::tr("격자로 균형 배치"),Action::ArrangeGrid),(kiln_common::i18n::tr("좌우로 균등 배치"),Action::Arrange(layout::Dir::Horizontal)),(kiln_common::i18n::tr("위아래로 균등 배치"),Action::Arrange(layout::Dir::Vertical)),(kiln_common::i18n::tr("현재 패널에 집중 / 복원"),Action::ToggleZoom(None))] {
                    if ui.button(label).clicked(){self.actions.push(Action::SelectWorkspace(i));self.actions.push(action);ui.close();}
                }
            });
            ui.separator();
            if ui.button(RichText::new(kiln_common::i18n::tr("워크스페이스 닫기")).color(t.red)).clicked() {
                self.actions.push(Action::CloseWorkspace(i));
                ui.close();
            }
        });
        if wide {
            let selected = if active { Some(self.workspaces[i].page().focused) } else { None };
            if let Some(pane) = super::workspace_activity::task_rows(ui, &tasks, selected, &t) {
                self.actions.push(Action::RevealPane(pane));
            }
            ui.add_space(4.0);
        }
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
            ui.label(RichText::new(kiln_common::trf!("분리된 세션 {}", orphans.len())).font(fonts::semibold(11.5)).color(t.text_faint));
        });
        ui.add_space(2.0);
        for o in orphans {
            let label = o.fg_process.clone().unwrap_or_else(|| kiln_common::i18n::tr("셸").into());
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
            let resp = resp.on_hover_text(kiln_common::i18n::tr("눌러서 현재 프로젝트로 가져오기"));
            if resp.clicked() {
                self.actions.push(Action::AttachSession(o.id));
            }
            resp.context_menu(|ui| {
                if ui.button(kiln_common::i18n::tr("붙이기")).clicked() {
                    self.actions.push(Action::AttachSession(o.id));
                    ui.close();
                }
                if ui.button(RichText::new(kiln_common::i18n::tr("세션 종료")).color(t.red)).clicked() {
                    self.confirm = Some(Confirm { skip_running_confirmation: None,
                        title: kiln_common::i18n::tr("세션을 종료할까요?").into(),
                        body: kiln_common::i18n::tr("이 터미널에서 실행 중인 명령과 프로세스가 종료됩니다. 이 작업은 되돌릴 수 없습니다.").into(),
                        ok: kiln_common::i18n::tr("세션 종료").into(),
                        action: Action::KillSession(o.id),
                    });
                    ui.close();
                }
            });
        }
    }

    // ------------------------------------------------------------------ 카드 캔버스

    pub(super) fn ui_canvas(&mut self, root: &mut egui::Ui) {
        let t = self.theme;
        let canvas = widgets::canvas_color(&t);
        egui::CentralPanel::default().frame(Frame::new().fill(canvas).inner_margin(Margin { left: 0, right: 0, top: 0, bottom: 0 })).show(root, |ui| {
            let mut area = ui.max_rect();
            if area.width() < 80.0 { return; }
            if !self.conn.is_connected() {
                let banner=egui::Rect::from_min_size(area.min,vec2(area.width(),28.0));
                ui.painter().rect_filled(banner,0,t.bg_panel);
                ui.painter().text(banner.left_center()+vec2(12.0,0.0),Align2::LEFT_CENTER,kiln_common::i18n::tr("세션에 다시 연결하는 중…"),fonts::regular(12.0),t.orange);
                area.min.y+=28.0;
            }
            let layout_enabled=ui.memory(|m|m.allows_interaction(ui.layer_id())) && self.confirm.is_none() && self.workspace_dialog.is_none() && !self.palette.is_open() && !self.settings_ui.open && !self.notifications.open && self.workspaces[self.active].page().zoomed.is_none();
            self.prepare_pane_layout(ui,area,layout_enabled);
            let ws_idx = self.active;
            let (root_node, focused, zoomed) = {
                let p = self.workspaces[ws_idx].page();
                (p.root.clone(), p.focused, p.zoomed)
            };
            let mut rects = Vec::new();
            match zoomed {
                Some(z) if root_node.panes().contains(&z) => rects.push((z, area)),
                _ => root_node.layout_with_gap(area, self.settings.card_gap, &mut rects),
            }
            // Preserve split ratios, but switch to a readable focus view when panes collapse.
            let manual_split=self.workspaces[ws_idx].page().manual_split;
            let adaptive = !manual_split && zoomed.is_none() && rects.len() > 1 && rects.iter().any(|(id, r)| r.width() < if self.panes.get(id).and_then(Pane::tool).is_some(){360.0}else{240.0} || r.height() < 150.0);
            if adaptive {
                let switcher = egui::Rect::from_min_size(area.min, vec2(area.width(), 32.0));
                let mut nav = ui.new_child(UiBuilder::new().max_rect(switcher).layout(egui::Layout::left_to_right(egui::Align::Center)));
                nav.set_clip_rect(switcher.intersect(ui.clip_rect()));
                nav.spacing_mut().item_spacing.x = 4.0;
                nav.add_space(8.0);
                if area.width()>260.0 && widgets::icon_button(&mut nav, Icon::Grid, 26.0, false, kiln_common::i18n::tr("격자로 배치")).on_hover_text(kiln_common::i18n::tr("열린 패널을 두 열로 정리합니다")).clicked(){self.actions.push(Action::ArrangeGrid);}
                if widgets::icon_button(&mut nav, Icon::SplitRight, 26.0, false, kiln_common::i18n::tr("분할로 복원")).on_hover_text(kiln_common::i18n::tr("이 탭의 패널을 분할한 채 유지합니다. 워크스페이스를 우클릭한 뒤 패널 배치에서 ‘좁아지면 선택한 패널만 표시’를 다시 켤 수 있습니다.")).clicked(){self.workspaces[ws_idx].page_mut().manual_split=true;}
                let selected_name = self.card_info(focused).name;
                let selector_width = (nav.available_width() - 8.0).max(1.0);
                let selector = egui::ComboBox::from_id_salt("adaptive-pane-switcher").selected_text(&selected_name).width(selector_width).truncate().show_ui(&mut nav, |ui| {
                    for (index, pid) in root_node.panes().iter().enumerate() {
                        if ui.selectable_label(*pid == focused, format!("{} · {}", index + 1, self.card_info(*pid).name)).clicked() {
                            self.actions.push(Action::FocusPane(*pid));
                        }
                    }
                });
                selector.response.on_hover_text(format!("{}\n{}", selected_name, kiln_common::i18n::tr("좁아지면 선택한 패널만 표시")));
                area.min.y += 32.0;
                rects.clear();
                rects.push((focused, area));
            }
            let multi = rects.len() > 1;
            let focus_req = self.focus_terminal && ui.memory(|m| m.allows_interaction(ui.layer_id())) && self.confirm.is_none() && self.workspace_dialog.is_none() && !self.palette.is_open() && !self.settings_ui.open && !self.notifications.open && self.workspaces.iter().all(|w| w.renaming.is_none());
            let mut focus_consumed = false;
            let mut new_focus = None;
            let settings = terminal::TermSettings { font_size: self.settings.font_size, option_as_meta: self.settings.option_as_meta, line_height: self.settings.line_height, copy_on_select: self.settings.copy_on_select, cursor_blink: self.settings.cursor_blink, close_shortcut: Some(self.keymap.resolve("close_panel", KeyboardShortcut::new(if is_mac() { Modifiers::MAC_CMD } else { Modifiers::CTRL | Modifiers::SHIFT }, Key::W))) };
            for (pid, rect) in &rects {
                let is_focused = *pid == focused;
                let info = self.card_info(*pid);
                let session = self.panes.get(pid).and_then(|p| p.session());
                let attention = session.and_then(|s| self.conn.infos.get(&s)).is_some_and(|i| i.attention);
                let cwd = session.and_then(|s| self.conn.infos.get(&s)).and_then(|i| i.cwd.clone());
                ui.painter().rect_filled(*rect, CornerRadius::ZERO, t.bg);
                let header = egui::Rect::from_min_size(rect.min, vec2(rect.width(), HEADER_H));
                let body = egui::Rect::from_min_max(pos2(rect.left() + 1.0, header.bottom() + 1.0), pos2(rect.right() - 1.0, rect.bottom() - 5.0));
                let hresp = ui.interact(header, ui.id().with(("hdr", *pid)), Sense::click_and_drag());
                if hresp.drag_started() && layout_enabled && !adaptive { self.start_pane_move(*pid,area); }
                self.card_header(ui, *pid, header, &info, is_focused, multi, zoomed.is_some());
                if hresp.clicked() && !is_focused {
                    new_focus = Some(*pid);
                }
                hresp.clone().on_hover_text(format!("{}\n{}\n{}",info.name,info.detail,info.right)).context_menu(|ui| {
                    self.terminal_header_menu(ui,*pid);
                    if ui.button(kiln_common::i18n::tr("오른쪽으로 나누기")).clicked() { self.actions.push(Action::SplitPane(*pid, layout::Dir::Horizontal)); ui.close(); }
                    if ui.button(kiln_common::i18n::tr("아래로 나누기")).clicked() { self.actions.push(Action::SplitPane(*pid, layout::Dir::Vertical)); ui.close(); }
                    if ui.button(kiln_common::i18n::tr("크게 보기 / 복원")).clicked() { self.actions.push(Action::ToggleZoom(Some(*pid))); ui.close(); }
                    ui.separator();
                    if ui.button(kiln_common::i18n::tr("패널 닫기")).clicked() { self.actions.push(Action::ClosePane(*pid, false)); ui.close(); }
                });
                if hresp.double_clicked() {
                    self.actions.push(Action::ToggleZoom(Some(*pid)));
                }
                let launch_error=self.terminal_launch_error(*pid).map(str::to_owned);
                let mut child = ui.new_child(UiBuilder::new().max_rect(body).id_salt(("card", *pid)));
                child.set_clip_rect(body);
                match self.panes.get_mut(pid).map(|p| &mut p.kind) {
                    Some(PaneKind::Term { view: Some(_), session: Some(_), .. }) if self.quick.open && self.quick.pane==Some(*pid) => {
                        child.vertical_centered(|ui|{
                            ui.add_space(24.0);ui.label(kiln_common::i18n::tr("빠른 터미널 창에서 열려 있습니다"));
                            ui.label(kiln_common::i18n::tr("동일한 세션을 별도 창에서 사용 중입니다."));
                            if ui.button(kiln_common::i18n::tr("이 작업 화면으로 가져오기")).clicked(){self.actions.push(Action::ReturnQuickTerminal);}
                        });
                    }
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
                        if let Some((command,cwd))=out.command_to_run { self.actions.push(Action::RunSavedCommand(launchers::SelectedCommand{name:kiln_common::i18n::tr("명령 다시 실행").into(),command,cwd:cwd.map(PathBuf::from)})); }
                        if out.restart {
                            self.actions.push(Action::RestartPane(*pid));
                        }
                        if out.clicked && let Some(s) = session { self.acknowledge_session(s); }
                    }
                    Some(PaneKind::Tool(tool)) => {
                        if focus_req && is_focused {
                            tool.request_focus();
                            focus_consumed = true;
                        }
                        let acts = tool.ui(&mut child);
                        if !is_focused && child.ui_contains_pointer() && ui.input(|i| i.pointer.any_pressed()) {
                            new_focus = Some(*pid);
                        }
                        self.actions.extend(acts);
                    }
                    _ => {
                        if let Some(error)=launch_error {
                            egui::ScrollArea::vertical().show(&mut child,|ui|{
                                ui.add_space((body.height()*0.22).min(80.0));
                                ui.vertical_centered(|ui|{
                                    ui.set_max_width(body.width().min(420.0));
                                    ui.label(RichText::new(kiln_common::i18n::tr("시작 요청을 확인해 주세요")).font(fonts::semibold(14.0)).color(t.text));
                                    ui.add(egui::Label::new(RichText::new(error).size(12.5).color(t.text_dim)).wrap());
                                    ui.add_space(8.0);
                                    if widgets::button(ui,kiln_common::i18n::tr("시작 요청 확인"),ButtonKind::Secondary).clicked(){self.actions.push(Action::OpenRecovery);}
                                });
                            });
                        } else {
                            let msg = if self.conn.is_connected() { kiln_common::i18n::tr("셸을 시작하는 중…") } else { kiln_common::i18n::tr("데몬에 연결하는 중…") };
                            child.painter().text(body.center(), Align2::CENTER_CENTER, msg, fonts::regular(13.0), t.text_faint);
                        }
                    }
                }
                let stroke = if attention {
                    Stroke::new(1.5, t.orange)
                } else if is_focused && multi {
                    Stroke::new(1.5, t.accent_soft(190))
                } else {
                    Stroke::new(1.0, t.border)
                };
                ui.painter().rect_stroke(*rect, CornerRadius::ZERO, stroke, StrokeKind::Inside);
            }
            if focus_consumed {
                self.focus_terminal = false;
            }

            self.interact_pane_layout(ui,area,layout_enabled && zoomed.is_none() && !adaptive);
            let page = self.workspaces[ws_idx].page_mut();
            page.rects = rects;
            if let Some(f) = new_focus {
                page.focused = f;
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn card_header(&mut self, ui: &mut egui::Ui, pid: PaneId, rect: egui::Rect, info: &CardInfo, focused: bool, multi: bool, zoomed: bool) {
        let t = self.theme;
        let cy = rect.center().y;
        ui.painter().rect_filled(rect, CornerRadius::ZERO, if focused { t.bg_panel } else { widgets::canvas_color(&t) });
        ui.painter().line_segment([pos2(rect.left() + 1.0, rect.bottom()), pos2(rect.right() - 1.0, rect.bottom())], Stroke::new(1.0, t.border));

        let show_identity = rect.width() >= 160.0;
        let branded = matches!(info.icon, Icon::Claude | Icon::Codex);
        let icon_rect = egui::Rect::from_center_size(pos2(rect.left() + 30.0, cy), vec2(18.0, 18.0));
        if show_identity {
        if info.running && branded { paint_agent_progress(ui, icon_rect.center(), agent_ink(info.icon, &t)); }
        else if info.running { paint_running(ui, pos2(rect.left() + 12.0, cy), info.dot); }
        else { widgets::status_dot(ui, pos2(rect.left() + 12.0, cy), info.dot, info.pulse); }
        let custom = match self.panes.get(&pid).map(|p| &p.kind) {
            Some(PaneKind::Tool(tool)) => tool.paint_icon(ui, icon_rect),
            _ => false,
        };
        if !custom {
            icons::paint(ui.painter(), icon_rect, info.icon, if branded { agent_ink(info.icon, &t) } else if focused { t.text_dim } else { t.text_faint });
        }
        if branded {
            let brand = if info.icon == Icon::Claude { "Claude Code" } else { "Codex" };
            let label = format!("{} · {brand}{}", info.name, if info.detail.is_empty() { String::new() } else { format!(" · {}", info.detail) });
            let response = ui.interact(icon_rect.expand(3.0), ui.id().with(("agent-identity", pid)), Sense::hover());
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &label));
            response.on_hover_text(label);
        }
        }
        let mut x = rect.left() + if show_identity {47.0}else{8.0};
        let is_terminal = matches!(self.panes.get(&pid).map(|p|&p.kind),Some(PaneKind::Term {..}));
        // Preserve the three frequent actions at narrow widths. Secondary actions
        // move into More before the title or button hit targets overlap.
        let show_splits = rect.width() >= 100.0;
        let show_close = rect.width() >= 68.0;
        let show_more = !show_splits || rect.width() >= 220.0;
        let show_zoom = rect.width() >= 280.0;
        let show_history = is_terminal && rect.width() >= 360.0;
        let button_count = usize::from(show_close) + 2*usize::from(show_splits) + usize::from(show_more) + usize::from(show_zoom) + usize::from(show_history);
        let buttons_w = button_count as f32 * 28.0 + 8.0;
        let right_limit = rect.right() - 10.0 - buttons_w;
        let clip = egui::Rect::from_min_max(pos2(x, rect.top()), pos2(right_limit.max(x), rect.bottom()));
        let p = ui.painter().with_clip_rect(clip);
        let g = card_title_galley(ui, &info.name, if focused || !multi { t.text } else { t.text_dim }, (right_limit-x).max(0.0));
        let name_w = g.size().x;
        p.galley(pos2(x, cy - g.size().y / 2.0), g, if focused || !multi { t.text } else { t.text_dim });
        x += name_w + 10.0;
        if !info.detail.is_empty() {
            let g = ui.painter().layout_no_wrap(info.detail.clone(), fonts::regular(12.0), t.text_dim);
            let w = g.size().x;
            p.galley(pos2(x, cy - g.size().y / 2.0), g, t.text_dim);
            let _ = w;
        }
        // Paths and integration diagnostics belong to the header tooltip, not every pane.
        let br = egui::Rect::from_min_max(pos2(rect.right()-buttons_w,cy-13.0),pos2(rect.right()-4.0,cy+13.0));
        let mut bui=ui.new_child(UiBuilder::new().id_salt(("pane-actions",pid)).max_rect(br).layout(egui::Layout::right_to_left(egui::Align::Center)));
        bui.set_clip_rect(rect);
        bui.spacing_mut().item_spacing.x=2.0;
        if show_close {
        let close = widgets::icon_button(&mut bui,Icon::Close,26.0,false,kiln_common::i18n::tr("패널 닫기"));
        if close.hovered() { icons::paint(bui.painter(),egui::Rect::from_center_size(close.rect.center(),vec2(26.0*0.56,26.0*0.56)),Icon::Close,t.red); }
        if close.clicked() { self.actions.push(Action::ClosePane(pid,false)); }
        bui.add_space(4.0);
        }
        if show_splits {
        if widgets::icon_button(&mut bui,Icon::SplitDown,26.0,false,kiln_common::i18n::tr("아래로 나누기")).clicked(){self.actions.push(Action::SplitPane(pid,layout::Dir::Vertical));}
        if widgets::icon_button(&mut bui,Icon::SplitRight,26.0,false,kiln_common::i18n::tr("오른쪽으로 나누기")).clicked(){self.actions.push(Action::SplitPane(pid,layout::Dir::Horizontal));}
        }
        if show_more {
            let menu=widgets::icon_button(&mut bui,Icon::More,26.0,false,kiln_common::i18n::tr("패널 작업"));
            egui::Popup::menu(&menu).show(|ui| {
                self.terminal_header_menu(ui,pid);
                if !show_splits {
                    if ui.button(kiln_common::i18n::tr("오른쪽으로 나누기")).clicked(){self.actions.push(Action::SplitPane(pid,layout::Dir::Horizontal));ui.close();}
                    if ui.button(kiln_common::i18n::tr("아래로 나누기")).clicked(){self.actions.push(Action::SplitPane(pid,layout::Dir::Vertical));ui.close();}
                }
                if ui.button(kiln_common::i18n::tr("크게 보기 / 복원")).clicked(){self.actions.push(Action::ToggleZoom(Some(pid)));ui.close();}
                if !show_close && ui.button(kiln_common::i18n::tr("패널 닫기")).clicked(){self.actions.push(Action::ClosePane(pid,false));ui.close();}
            });
        }
        if show_zoom && widgets::icon_button(&mut bui,if zoomed {Icon::Restore}else{Icon::Maximize},26.0,false,&kiln_common::trf!("크게 보기 / 복원 ({})",self.keymap.label("focus_panel","⇧⌘↩"))).clicked(){self.actions.push(Action::ToggleZoom(Some(pid)));}
        if show_history {
            if let Some(Pane{kind:PaneKind::Term{view:Some(view),..},..})=self.panes.get_mut(&pid) {
                if widgets::icon_button(&mut bui,Icon::History,26.0,view.inspector_open(),kiln_common::i18n::tr("명령 기록")).clicked(){view.open_history();}
            }
        }
    }

    fn terminal_header_menu(&mut self,ui:&mut egui::Ui,pid:PaneId){
        if let Some(Pane{kind:PaneKind::Term{view:Some(view),..},..})=self.panes.get_mut(&pid) {
            if ui.button(kiln_common::i18n::tr("명령 기록")).clicked(){view.open_history();ui.close();}
            if ui.button(kiln_common::i18n::tr("셸·에이전트 연동")).clicked(){view.open_integration_help();ui.close();}
        }
        let session=self.panes.get(&pid).and_then(Pane::session);
        if terminal_recovery_menu_item(ui,&mut self.conn,session) {self.actions.push(Action::FocusPane(pid));}
        if session.is_some() {ui.separator();}
    }

    // ------------------------------------------------------------------ 도구 시트

    pub(super) fn ui_sheet(&mut self, root: &mut egui::Ui) {
        let t = self.theme;
        let ws_idx = self.active;
        let Some(kind) = self.workspaces[ws_idx].sheet else { return };
        let mut acts = Vec::new();
        let agent_task = kind == tools::ToolKind::Git && self.workspaces[ws_idx].tools.is_agent_task_open();
        let inspector = !agent_task && super::is_inspector(kind);
        let workspace_name = self.workspaces[ws_idx].name.clone();
        let workspace_path = self.workspaces[ws_idx].root.display().to_string();
        // Dock beside the editor instead of obscuring the file or terminal being worked on.
        let exclusive = root.available_width() < 700.0;
        // Inspection must not displace the primary workspace, including when an
        // older installation persisted an oversized dock on a large display.
        let max_width = (root.available_width() * 0.45).clamp(300.0, 560.0);
        let panel = if exclusive {
            egui::Panel::right("tools-compact").exact_size(root.available_width()).resizable(false)
        } else {
            egui::Panel::right("tools-dock").default_size(340.0).min_size(300.0).max_size(max_width).resizable(true)
        };
        panel
            .frame(Frame::new().fill(t.bg_panel).inner_margin(Margin::same(10)))
            .show(root, |panel_ui| {
                // The resize handle owns the dock width. Child content may wrap or
                // scroll, but must never feed a larger minimum back into PanelState.
                let (bounds, _) = panel_ui.allocate_exact_size(panel_ui.available_size(), Sense::hover());
                let mut content = panel_ui.new_child(UiBuilder::new().max_rect(bounds));
                content.set_clip_rect(bounds);
                let ui = &mut content;
                ui.horizontal(|ui| {
                    if exclusive && widgets::icon_button(ui, Icon::Undo, 28.0, false, kiln_common::i18n::tr("작업으로 돌아가기 (Esc)")).clicked() {
                        acts.push(Action::CloseSheet);
                    }
                    if !inspector {
                        let (icon, _) = ui.allocate_exact_size(vec2(18.0, 30.0), Sense::hover());
                        icons::paint(ui.painter(), egui::Rect::from_center_size(icon.center(), vec2(16.0, 16.0)), if agent_task { Icon::Sparkle } else { kind.vicon() }, t.accent);
                    }
                    let title = if inspector { workspace_name.as_str() } else if agent_task { kiln_common::i18n::tr("새 에이전트 작업") } else { kind.label() };
                    let close_width = if exclusive { 0.0 } else { 28.0 + ui.spacing().item_spacing.x };
                    let title_width = (ui.available_width() - close_width).max(40.0);
                    ui.allocate_ui_with_layout(vec2(title_width,30.0),egui::Layout::left_to_right(egui::Align::Center),|ui|{
                        ui.set_min_size(vec2(title_width,30.0));
                        ui.add(egui::Label::new(RichText::new(title).font(fonts::semibold(14.0)).color(t.text)).truncate())
                            .on_hover_text(if inspector {workspace_path.as_str()}else{title});
                    });
                    if !exclusive && widgets::icon_button(ui, Icon::Close, 28.0, false, kiln_common::i18n::tr("도구 닫기")).clicked() {
                        acts.push(Action::CloseSheet);
                    }
                });
                if inspector {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let compact_tabs = ui.available_width() < 320.0;
                        for (target, label) in [(tools::ToolKind::Explorer, kiln_common::i18n::tr("파일")), (tools::ToolKind::Git, kiln_common::i18n::tr("변경")), (tools::ToolKind::PullRequests, "GitHub"), (tools::ToolKind::Database, kiln_common::i18n::tr("데이터베이스"))] {
                            let selected = kind == target || target == tools::ToolKind::Explorer && kind == tools::ToolKind::Search;
                            let response = if compact_tabs || target == tools::ToolKind::Database {
                                widgets::icon_button(ui, target.vicon(), 28.0, false, label)
                            } else {
                                widgets::button_with(ui, if target == tools::ToolKind::PullRequests { Some(Icon::GitHub) } else { None }, label, ButtonKind::Ghost, true)
                            };
                            response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, label));
                            if selected {
                                ui.painter().line_segment([response.rect.left_bottom(), response.rect.right_bottom()], Stroke::new(2.0, t.accent));
                            }
                            if response.on_hover_text(target.shortcut()).clicked() && kind != target {
                                acts.push(Action::OpenSheet(target));
                            }
                        }
                        if matches!(kind, tools::ToolKind::Explorer | tools::ToolKind::Search) {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if kind == tools::ToolKind::Explorer {
                                    let menu = widgets::icon_button(ui, Icon::More, 28.0, false, kiln_common::i18n::tr("파일 작업"));
                                    egui::Popup::menu(&menu).show(|ui| {
                                        acts.extend(self.workspaces[ws_idx].tools.file_menu_ui(ui));
                                    });
                                }
                                if widgets::icon_button(ui, Icon::Search, 28.0, kind == tools::ToolKind::Search, kiln_common::i18n::tr("파일 내용 검색 (⇧⌘F)")).clicked() {
                                    acts.push(Action::OpenSheet(if kind == tools::ToolKind::Search { tools::ToolKind::Explorer } else { tools::ToolKind::Search }));
                                }
                            });
                        }
                    });
                }
                widgets::divider(ui);
                ui.add_space(8.0);
                let body = ui.available_rect_before_wrap();
                let mut child = ui.new_child(UiBuilder::new().max_rect(body).id_salt(("workspace-tools", self.workspaces[ws_idx].id)));
                child.set_clip_rect(body);
                acts.extend(self.workspaces[ws_idx].tools.panel_ui(&mut child, kind));
            });
        if exclusive && self.confirm.is_none() && self.agent_request_view.is_none() && self.workspace_dialog.is_none() && !self.launchers.is_open() && !self.palette.is_open() && !self.settings_ui.open && !self.notifications.open
            && !egui::Popup::is_any_open(root.ctx())
            && root.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            acts.push(Action::CloseSheet);
        }
        if (exclusive || self.workspaces[ws_idx].tools.is_agent_task_open()) && acts.iter().any(|a| matches!(a, Action::OpenTab(_) | Action::OpenHistory | Action::NewTermAt(_) | Action::RunInTerminal(_) | Action::RunInTerminalAt{..})) {
            acts.push(Action::CloseSheet);
        }
        self.actions.extend(acts);
    }

    // ------------------------------------------------------------------ 오버레이

    pub(super) fn ui_overlays(&mut self, ctx: &egui::Context) {
        let t = self.theme;
        self.ui_workspace_dialog(ctx);
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

        if self.launchers.is_open() {
            let cwd = self.workspaces[self.active].root.clone();
            if let Some(command) = self.launchers.ui(ctx, &cwd) {
                self.actions.push(Action::RunSavedCommand(command));
            }
            if !self.launchers.is_open() { self.focus_terminal = true; }
        }

        if self.settings_ui.open {
            let info = settings::AboutInfo { daemon_pid: self.conn.daemon_pid, daemon_build: self.conn.daemon_build.clone(), connected: self.conn.is_connected() };
            let acts = self.settings_ui.ui(ctx, &mut self.settings, &info, &self.rotator.mgr, &mut self.keymap);
            self.actions.extend(acts);
            if !self.settings_ui.open {
                self.focus_terminal = true;
            }
        }

        if let Some(index) = self.workspaces.iter().position(|workspace| workspace.renaming.is_some()) {
            let mut save = false;
            let mut cancel = false;
            let frame = Frame::new().fill(t.bg_panel).stroke(Stroke::new(1.0, t.border_strong)).corner_radius(12).inner_margin(Margin::same(20));
            let response = egui::Modal::new(egui::Id::new("rename-workspace")).frame(frame).show(ctx, |ui| {
                ui.set_width(340.0f32.min(ctx.content_rect().width() - 64.0));
                ui.label(RichText::new(kiln_common::i18n::tr("워크스페이스 이름 바꾸기")).font(fonts::semibold(17.0)));
                ui.add_space(12.0);
                let buffer = self.workspaces[index].renaming.as_mut().unwrap();
                let input = ui.add(egui::TextEdit::singleline(buffer).desired_width(f32::INFINITY));
                let focus_id = input.id.with("initial-focus");
                if !ui.data(|d| d.get_temp::<bool>(focus_id).unwrap_or(false)) {
                    input.request_focus();
                    ui.data_mut(|d| d.insert_temp(focus_id, true));
                }
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    let valid = !buffer.trim().is_empty();
                    let enter = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
                    ui.add_enabled_ui(valid, |ui| {
                        save = widgets::button(ui, kiln_common::i18n::tr("이름 저장"), ButtonKind::Primary).clicked();
                    });
                    save |= valid && enter;
                    if enter && !valid {input.request_focus();}
                    cancel = widgets::button(ui, kiln_common::i18n::tr("취소"), ButtonKind::Secondary).clicked();
                });
                focus_id
            });
            let closed = cancel || response.should_close();
            if save || closed { ctx.data_mut(|d| d.remove::<bool>(response.inner)); }
            if save {
                if let Some(name) = self.workspaces[index].renaming.take() { self.actions.push(Action::RenameWorkspace(index, name.trim().into())); }
            } else if closed { self.workspaces[index].renaming = None; }
        }

        let mut close_confirm = false;
        let mut preserve_quit = false;
        let mut ok = false;
        if let Some(c) = &mut self.confirm {
            let frame = Frame::new().fill(t.bg_elevated).stroke(Stroke::new(1.0, t.border_strong)).corner_radius(CornerRadius::same(14)).shadow(t.shadow()).inner_margin(Margin::same(22));
            egui::Modal::new(egui::Id::new("confirm")).frame(frame).backdrop_color(Color32::from_black_alpha(110)).show(ctx, |ui| {
                ui.set_width(390.0f32.min((ctx.content_rect().width()-56.0).max(220.0)));
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
                    ui.painter().rect_filled(r, CornerRadius::same(10), Color32::from_rgba_unmultiplied(t.red.r(), t.red.g(), t.red.b(), 36));
                    icons::paint(ui.painter(), r.shrink(9.0), Icon::Warning, t.red);
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        ui.label(RichText::new(&c.title).font(fonts::semibold(15.5)).color(t.text));

                    });
                });
                ui.add_space(10.0);
                egui::ScrollArea::vertical().id_salt("confirm-details").max_height((ctx.content_rect().height() - if c.skip_running_confirmation.is_some(){260.0}else{190.0}).clamp(48.0, 280.0)).show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(&c.body).size(13.0).color(t.text_dim)).wrap());
                });
                ui.add_space(16.0);
                if let Some(skip) = &mut c.skip_running_confirmation {
                    ui.checkbox(skip,kiln_common::i18n::tr("다시 묻지 않기"))
                        .on_hover_text(kiln_common::i18n::tr("실행 중인 패널·탭·워크스페이스의 종료 확인을 생략합니다. 설정에서 다시 켤 수 있습니다."));
                    ui.add(egui::Label::new(RichText::new(kiln_common::i18n::tr("패널·탭·워크스페이스에 적용됩니다. 설정에서 다시 켤 수 있습니다.")).size(11.5).color(t.text_dim)).wrap());
                    ui.add_space(12.0);
                }
                if matches!(c.action, Action::QuitConfirmed) && !self.launchers.has_unsaved_edits() && !self.projects.has_unsaved_edits() && !self.keymap.has_unsaved_edits() {
                    if widgets::button(ui,kiln_common::i18n::tr("작성 내용 남기고 종료"),ButtonKind::Primary).clicked(){preserve_quit=true;}
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::button(ui, &c.ok, ButtonKind::Danger).clicked() {
                        ok = true;
                    }
                    if widgets::button(ui, kiln_common::i18n::tr("취소"), ButtonKind::Secondary).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        close_confirm = true;
                    }
                });
            });
        }
        if preserve_quit { self.confirm=None; self.actions.push(Action::QuitPreservingDrafts); }
        if ok {
            if let Some(c) = self.confirm.take() {
                if c.skip_running_confirmation == Some(true) {
                    self.settings.confirm_close_running = false;
                    self.save_if_changed(true);
                }
                self.actions.push(c.action);
            }
        }
        if close_confirm {
            #[cfg(feature = "updater-test")]
            crate::updater_fixture_event("confirmation-cancel-click");
            let returning_from_quit = self.confirm.as_ref().is_some_and(|c| matches!(c.action, Action::QuitConfirmed));
            if returning_from_quit { ctx.data_mut(|data| data.insert_temp(egui::Id::new("quit-restore-focus"), true)); }
            self.confirm = None;
            #[cfg(target_os = "macos")]
            macos::reply_to_termination(false);
            self.focus_terminal = true;
        }

        self.ui_agent_request(ctx);
        if !self.notifications.open && !self.palette.is_open() && !self.settings_ui.open && self.confirm.is_none() && self.agent_request_view.is_none() {
            self.ui_toasts(ctx);
        }
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
        let mut pressed = None;
        egui::Area::new(egui::Id::new("toasts")).anchor(Align2::RIGHT_BOTTOM, vec2(-20.0, -20.0)).order(egui::Order::Tooltip).show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            let n = self.toasts.len();
            for idx in (n.saturating_sub(if ctx.content_rect().height() < 700.0 { 1 } else { 2 })..n).rev() {
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
                    .inner_margin(Margin { left: 12, right: 12, top: 8, bottom: 8 })
                    .show(ui, |ui| {
                        let compact=ctx.content_rect().height()<600.0 || ctx.content_rect().width()<900.0;
                        ui.set_width(if compact {250.0f32.min(ctx.content_rect().width()-60.0)}else{300.0});
                        if compact && toast.button.is_none() {
                            ui.horizontal(|ui|{
                                let title=format!("{} · {}",toast.title,toast.body);
                                let available=(ui.available_width()-30.0).max(80.0);
                                let response=ui.add_sized([available,24.0],egui::Button::new(RichText::new(title).size(12.0)).frame(false).truncate()).on_hover_text(kiln_common::trf!("{}\n{}\n클릭하여 열기",toast.title,toast.body));
                                if response.clicked(){if let Some(session)=toast.session {reveal=Some((idx,session));}else{self.actions.push(Action::ToggleNotifications);dismiss=Some(idx);}}
                                if widgets::icon_button(ui,Icon::Close,22.0,false,kiln_common::i18n::tr("알림 닫기")).clicked(){dismiss=Some(idx);}
                            });
                            return;
                        }
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(18.0, 24.0), Sense::hover());
                            icons::paint(ui.painter(), r.shrink(2.0), icon, accent);
                            let title_width = (ui.available_width() - 30.0).max(80.0);
                            let title = if ctx.content_rect().height() < 600.0 && !toast.body.is_empty() {
                                format!("{} · {}", toast.title, toast.body)
                            } else { toast.title.clone() };
                            ui.allocate_ui_with_layout(vec2(title_width, 24.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                ui.add(egui::Label::new(RichText::new(title).font(fonts::semibold(13.0)).color(t.text)).truncate()).on_hover_text(&toast.body);
                            });
                            if widgets::icon_button(ui, Icon::Close, 22.0, false, kiln_common::i18n::tr("알림 닫기")).clicked() { dismiss = Some(idx); }
                        });
                        if !toast.body.is_empty() && ctx.content_rect().height() >= 600.0 {
                            ui.add(egui::Label::new(RichText::new(&toast.body).size(12.0).color(t.text_dim)).truncate()).on_hover_text(&toast.body);
                        }
                        ui.horizontal(|ui| {
                            if let Some((label, _)) = &toast.button {
                                if widgets::button(ui, label, ButtonKind::Ghost).clicked() { pressed = Some(idx); }
                            } else if toast.session.is_some() && widgets::button(ui, kiln_common::i18n::tr("세션으로 이동"), ButtonKind::Ghost).clicked() {
                                reveal = toast.session.map(|session| (idx, session));
                            }
                        });
                    });
                let _ = resp;

            }
        });
        if let Some(idx) = pressed {
            let toast = self.toasts.remove(idx);
            if let Some((_, a)) = toast.button {
                self.actions.push(a);
            }
        } else if let Some((idx, s)) = reveal {
            self.toasts.remove(idx);
            self.reveal_session(s);
        } else if let Some(idx) = dismiss {
            self.toasts.remove(idx);
        }
    }

    fn palette_items(&self) -> Vec<palette::Item<Action>> {
        use palette::Group;
        let mut items: Vec<palette::Item<Action>> = Vec::new();
        let mut add = |group: Group, icon: Icon, label: String, hint: &str, a: Action| {
            let (key,project)=match &a {Action::RevealPane(id)=>(format!("pane:{id}"),self.workspaces.iter().find(|w|w.all_panes().contains(id)).map(|w|format!("{} · {}",w.name,short_path(&w.root))).unwrap_or_default()),_=>(format!("{:?}:{label}",group),String::new())};
            let binding=match &a{Action::OpenRecent=>Some("recent"),Action::OpenLaunchers=>Some("launchers"),Action::NewWorkspace(None)=>Some("projects"),Action::QuickOpen=>Some("files"),Action::ToggleSidebar=>Some("sidebar"),Action::OpenPalette=>Some("palette"),Action::NewPage=>Some("new_task"),Action::Split(layout::Dir::Horizontal)=>Some("split_right"),Action::Split(layout::Dir::Vertical)=>Some("split_down"),Action::CloseActive=>Some("close_panel"),Action::ToggleZoom(_)=>Some("focus_panel"),Action::Equalize=>Some("equalize"),Action::NextPage(1)=>Some("next_task"),Action::NextPage(-1)=>Some("previous_task"),Action::JumpUnread=>Some("unread"),_=>None};
            let hint=binding.map(|id|self.keymap.label(id,hint)).unwrap_or_else(||hint.into());
            let (label, detail) = label.split_once('\n').map(|(title,context)|(title.to_owned(),context.to_owned())).unwrap_or((label,String::new()));
            items.push(palette::Item { key, project, group, icon, label, hint, detail, action: a });
        };
        let mut open_tasks = Vec::new();
        for ws in &self.workspaces {
            for (page_index,page) in ws.pages.iter().enumerate() {
                for (pane_index,id) in page.root.panes().into_iter().enumerate() {
                    let Some(pane) = self.panes.get(&id) else { continue; };
                    let info = self.card_info(id);
                    let (kind, path) = match &pane.kind {
                        PaneKind::Term { .. } => (kiln_common::i18n::tr("터미널"), pane.cwd.clone().unwrap_or_else(|| info.right.clone())),
                        PaneKind::Tool(tool) => {
                            let key = tool.key();
                            let kind = if key.starts_with("db") { kiln_common::i18n::tr("데이터베이스") } else if key.starts_with("git-history") || key.starts_with("diff") || key.starts_with("commit") || key.starts_with("range:") || key.starts_with("issue:") || key.starts_with("history") || key.starts_with("pr:") { "Git" } else { kiln_common::i18n::tr("편집기") };
                            (kind, tool.path().map(|p| short_path(p)).unwrap_or_default())
                        }
                    };
                    let detail = if path.is_empty() { info.detail } else { path };

                    let command=pane.session().and_then(|sid|self.conn.telemetry.get(&sid)).and_then(|telemetry|telemetry.commands.last());
                    // Agent launch commands stay open while awaiting input. Show
                    // the actual task title rather than calling that launch busy.
                    let title=if let Some(command)=command.filter(|command|!command.command.is_empty() && (command.finished_unix.is_some() || !matches!(info.icon,Icon::Codex|Icon::Claude))) {
                        format!("{} · {}",if command.finished_unix.is_some(){kiln_common::i18n::tr("최근 명령")}else{kiln_common::i18n::tr("실행 중")},command.command)
                    }else if let Some(name)=page.title.as_ref().filter(|name|*name!=&info.name) {format!("{name} · {}",info.name)}else{info.name.clone()};
                    let title=if pane.session().is_some(){kiln_common::trf!("터미널 {}.{} · {}",page_index+1,pane_index+1,title)}else{title};
                    let label=kiln_common::trf!("{title}\n{} · 작업 {} / 패널 {} · {}",ws.name,page_index+1,pane_index+1,detail);
                    let running = info.running;
                    open_tasks.push((!running,self.recent_panes.iter().position(|p| *p == id).unwrap_or(usize::MAX), info.icon, label, kind, id));
                }
            }
        }
        open_tasks.sort_by_key(|item| (item.0,item.1));
        for (_, _, icon, label, kind, id) in open_tasks {
            add(Group::Sessions, icon, label, kind, Action::RevealPane(id));
        }
        add(Group::Commands,Icon::Folder,kiln_common::i18n::tr("폴더 열기").into(),"",Action::OpenFolder);
        add(Group::Commands,Icon::Folder,kiln_common::i18n::tr("파일 살펴보기").into(),"",Action::OpenSheet(tools::ToolKind::Explorer));
        add(Group::Commands,Icon::Sparkle,kiln_common::i18n::tr("새 에이전트 작업").into(),"",Action::NewAgentTask);
        add(Group::Commands, Icon::Folder, kiln_common::i18n::tr("새 워크스페이스").into(), "⌘N", Action::NewWorkspace(None));
        add(Group::Commands, Icon::Folder, kiln_common::i18n::tr("프로젝트 관리").into(), "", Action::OpenProjects);
        add(Group::Commands, Icon::History, kiln_common::i18n::tr("작업 복구 센터").into(), "", Action::OpenRecovery);
        add(Group::Commands, Icon::Terminal, kiln_common::i18n::tr("빠른 터미널").into(), "Ctrl+`", Action::ToggleQuickTerminal);
        add(Group::Commands, Icon::History, kiln_common::i18n::tr("최근 작업 전환").into(), "⌘J", Action::OpenRecent);
        add(Group::Commands, Icon::Terminal, kiln_common::i18n::tr("저장 명령 실행…").into(), "⇧⌘J", Action::OpenLaunchers);
        add(Group::Commands, Icon::Plus, kiln_common::i18n::tr("새 터미널 탭").into(), "⌘T", Action::NewPage);
        add(Group::Commands, Icon::Codex, kiln_common::i18n::tr("Codex로 새 작업").into(), "", Action::DirectAgent {tool:kiln_accounts::Tool::Codex,cwd:None});
        add(Group::Commands, Icon::Claude, kiln_common::i18n::tr("Claude Code로 새 작업").into(), "", Action::DirectAgent {tool:kiln_accounts::Tool::Claude,cwd:None});
        add(Group::Commands, Icon::SplitRight, kiln_common::i18n::tr("오른쪽으로 나누기").into(), "⌘D", Action::Split(layout::Dir::Horizontal));
        add(Group::Commands, Icon::SplitDown, kiln_common::i18n::tr("아래로 나누기").into(), "⇧⌘D", Action::Split(layout::Dir::Vertical));
        add(Group::Commands, Icon::History, kiln_common::i18n::tr("Git 로그 (히스토리)").into(), "⇧⌘L", Action::OpenHistory);
        add(Group::Commands, Icon::Maximize, kiln_common::i18n::tr("패널 크게 보기 전환").into(), "⇧⌘↩", Action::ToggleZoom(None));
        add(Group::Commands, Icon::Maximize, kiln_common::i18n::tr("앱 전체 화면 켜기 / 끄기").into(), if cfg!(target_os="macos"){"⌃⌘F"}else{"F11"}, Action::ToggleFullscreen);
        add(Group::Commands, Icon::Maximize, if self.workspaces[self.active].page().manual_split {kiln_common::i18n::tr("자동 집중 보기 켜기")}else{kiln_common::i18n::tr("자동 집중 보기 끄기")}.into(), "", Action::ToggleAutoFocus);
        add(Group::Commands, Icon::Command, kiln_common::i18n::tr("격자로 균형 배치").into(), "", Action::ArrangeGrid);
        add(Group::Commands, Icon::Command, kiln_common::i18n::tr("패널 크기 균등하게").into(), "⌥⌘=", Action::Equalize);
        add(Group::Commands, Icon::Close, kiln_common::i18n::tr("패널 닫기").into(), "⌘W", Action::CloseActive);
        add(Group::Commands, Icon::File, kiln_common::i18n::tr("파일 빠르게 열기").into(), "⌘P", Action::QuickOpen);
        add(Group::Commands, Icon::Search, kiln_common::i18n::tr("터미널·에디터에서 찾기").into(), "⌘F", Action::FindInFocused);

        add(Group::Commands, Icon::Sidebar, kiln_common::i18n::tr("워크스페이스 목록 접기/펴기").into(), "⌘B", Action::ToggleSidebar);
        add(Group::Commands, Icon::Bell, kiln_common::i18n::tr("알림 센터 열기").into(), "", Action::ToggleNotifications);
        add(Group::Commands, Icon::Bell, kiln_common::i18n::tr("읽지 않은 알림으로 이동").into(), "⇧⌘U", Action::JumpUnread);
        for k in tools::ToolKind::ALL {
            add(Group::Tools, k.vicon(), k.label().into(), k.shortcut(), Action::OpenSheet(k));
        }
        for (i, w) in self.workspaces.iter().enumerate() {
            add(Group::Spaces, Icon::Folder, w.name.clone(), &format!("⌘{}", i + 1), Action::SelectWorkspace(i));
        }
        for th in Theme::ALL {
            add(Group::Settings, Icon::Sparkle, kiln_common::trf!("테마: {}", th.label), "", Action::SetTheme(th.name.into()));
        }
        add(Group::Settings, Icon::Gear, kiln_common::i18n::tr("설정 열기").into(), "⌘,", Action::OpenSettings);
        add(Group::Settings, Icon::Command, kiln_common::i18n::tr("글꼴 크게").into(), "⌘=", Action::FontDelta(1.0));
        add(Group::Settings, Icon::Command, kiln_common::i18n::tr("글꼴 작게").into(), "⌘-", Action::FontDelta(-1.0));
        add(Group::Settings, Icon::Sparkle, kiln_common::i18n::tr("데몬을 이 버전으로 교체").into(), "", Action::UpgradeDaemon);
        items
    }
}
