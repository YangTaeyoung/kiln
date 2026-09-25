//! 소스 컨트롤 사이드 패널(GitPanel).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Id, Key, Layout, Margin, Modifiers, Rect, RichText, Sense, Stroke,
    TextFormat, Ui, pos2, text::LayoutJob, vec2,
};
use kiln_common::Task;

use super::widgets::*;
use super::worker::Worker;
use crate::GitEvent;
use crate::cmd::{GitError, GitResult, Mode, git};
use crate::graph::{GraphRow, compute_graph};
use crate::repo::{self, Branch, Commit, RepoOp, StashEntry};
use crate::status::{Status, StatusEntry, decoration_char};
use crate::util::{now_unix, relative_time};

const POLL_INTERVAL: Duration = Duration::from_secs(10);
const MIN_REFRESH_GAP: Duration = Duration::from_millis(350);
const LOG_PAGE: usize = 100;
const COMMIT_ROW_H: f32 = 38.0;
const LANE_W: f32 = 11.0;
const SUBJECT_HINT: usize = 50;
const SUBJECT_MAX: usize = 72;

/// 백그라운드에서 한 번에 읽어오는 저장소 상태.
#[derive(Clone)]
struct Snapshot {
    top: PathBuf,
    /// 패널 루트의 저장소 내 접두 경로(`sub/dir/` 또는 빈 문자열).
    prefix: String,
    status: Status,
    op: Option<RepoOp>,
    stashes: Vec<StashEntry>,
    log: Vec<Commit>,
    graph: Vec<GraphRow>,
}

fn load_snapshot(root: &Path, log_limit: usize) -> GitResult<Snapshot> {
    let top_raw = git(root, Mode::Read, &["rev-parse", "--show-toplevel", "--show-prefix"])?;
    let mut lines = top_raw.lines();
    let top = PathBuf::from(lines.next().unwrap_or("").trim());
    let prefix = lines.next().unwrap_or("").trim().to_string();
    let status = repo::status(&top)?;
    let op = repo::in_progress_op(&top);
    let stashes = if status.stash_count > 0 { repo::stash_list(&top).unwrap_or_default() } else { Vec::new() };
    let log = repo::log(&top, 0, log_limit).unwrap_or_default();
    let graph = compute_graph(&log);
    Ok(Snapshot { top, prefix, status, op, stashes, log, graph })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JobKind {
    Mutate,
    Sync,
    Commit,
    Checkout,
}

#[derive(Clone, Debug)]
enum Confirm {
    Discard { paths: Vec<String>, untracked: bool },
    DiscardAll,
    DeleteBranch { name: String, force: bool },
    DropStash { index: usize, message: String },
    AbortOp(RepoOp),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Section {
    Conflicts,
    Staged,
    Changes,
    Untracked,
    Stashes,
    Commits,
}

#[derive(Default)]
struct BranchPicker {
    load: Option<Task<GitResult<Vec<Branch>>>>,
    list: Vec<Branch>,
    error: Option<String>,
    filter: String,
    focus_filter: bool,
}

/// 소스 컨트롤 패널.
pub struct GitPanel {
    root: PathBuf,
    worker: Option<Worker<JobKind>>,
    load: Option<Task<GitResult<Snapshot>>>,
    snap: Option<Snapshot>,
    load_error: Option<GitError>,
    dirty: bool,
    last_load: Option<Instant>,
    log_limit: usize,
    message: String,
    banner: Option<(BannerKind, String, Option<String>)>,
    confirm: Option<Confirm>,
    open: HashMap<Section, bool>,
    picker: BranchPicker,
    stash_input: Option<String>,
    selected: Option<(String, bool)>,
    now_override: Option<i64>,
    decorations: HashMap<PathBuf, (Color32, char)>,
}

impl GitPanel {
    pub fn new(root: PathBuf) -> Self {
        let open = [
            (Section::Conflicts, true),
            (Section::Staged, true),
            (Section::Changes, true),
            (Section::Untracked, true),
            (Section::Stashes, false),
            (Section::Commits, true),
        ]
        .into_iter()
        .collect();
        Self {
            root,
            worker: None,
            load: None,
            snap: None,
            load_error: None,
            dirty: true,
            last_load: None,
            log_limit: LOG_PAGE,
            message: String::new(),
            banner: None,
            confirm: None,
            open,
            picker: BranchPicker::default(),
            stash_input: None,
            selected: None,
            now_override: None,
            decorations: HashMap::new(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 상태를 다시 읽도록 요청한다. 실제 로드는 다음 `ui()` 에서 시작되며 짧은 간격의 요청은 합쳐진다.
    pub fn refresh(&mut self) {
        self.dirty = true;
    }

    /// 파일 트리 장식: 절대 경로 → (색, 상태 문자). 상태 문자는 M/A/D/R/?/C(충돌).
    pub fn file_decorations(&self) -> HashMap<PathBuf, (Color32, char)> {
        self.decorations.clone()
    }

    /// 백그라운드 로드나 작업이 진행 중인지.
    pub fn is_busy(&mut self) -> bool {
        self.load.as_mut().is_some_and(|t| t.is_pending())
            || self.worker.as_ref().is_some_and(|w| w.is_busy())
            || self.dirty
    }

    /// 상대 시간 계산 기준 시각을 고정한다.
    #[doc(hidden)]
    pub fn set_now(&mut self, ts: i64) {
        self.now_override = Some(ts);
    }

    /// 커밋 메시지 입력값.
    pub fn commit_message_mut(&mut self) -> &mut String {
        &mut self.message
    }

    /// 마지막 로드 오류.
    pub fn error(&self) -> Option<&GitError> {
        self.load_error.as_ref()
    }

    /// 현재 상태(로드된 경우).
    pub fn status(&self) -> Option<&Status> {
        self.snap.as_ref().map(|s| &s.status)
    }

    fn now(&self) -> i64 {
        self.now_override.unwrap_or_else(now_unix)
    }

    fn top(&self) -> PathBuf {
        self.snap.as_ref().map(|s| s.top.clone()).unwrap_or_else(|| self.root.clone())
    }

    /// 저장소 상대 경로를 절대 경로로 바꾼다(패널 루트 기준 표기 우선).
    fn abs_path(&self, rel: &str) -> PathBuf {
        let rel = rel.trim_end_matches('/');
        match &self.snap {
            Some(s) => match rel.strip_prefix(s.prefix.as_str()) {
                Some(r) => self.root.join(r),
                None => s.top.join(rel),
            },
            None => self.root.join(rel),
        }
    }

    fn rebuild_decorations(&mut self) {
        let mut map = HashMap::new();
        if let Some(s) = &self.snap {
            for e in &s.status.entries {
                if e.kind == crate::status::EntryKind::Ignored {
                    continue;
                }
                let Some(r) = e.path.trim_end_matches('/').strip_prefix(s.prefix.as_str()) else { continue };
                let ch = decoration_char(e);
                map.insert(self.root.join(r), (status_color(ch), ch));
            }
        }
        self.decorations = map;
    }

    fn pump(&mut self, ctx: &egui::Context) {
        if self.worker.is_none() {
            self.worker = Some(Worker::new(ctx));
        }
        if let Some(w) = &self.worker {
            for done in w.drain() {
                self.dirty = true;
                match (&done.result, done.kind) {
                    (Ok(out), JobKind::Sync) => {
                        let msg = out.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").to_string();
                        self.banner = Some((BannerKind::Success, format!("{} 완료", done.label), Some(msg)));
                    }
                    (Ok(_), JobKind::Commit) => {
                        self.message.clear();
                        self.banner = None;
                    }
                    (Ok(_), _) => {}
                    (Err(e), _) => {
                        self.banner = Some((BannerKind::Error, format!("{} 실패", done.label), Some(e.to_string())));
                    }
                }
                if done.kind == JobKind::Checkout && done.result.is_ok() {
                    self.picker.list.clear();
                }
            }
        }
        if let Some(t) = &mut self.load
            && let Some(r) = t.take()
        {
            self.load = None;
            self.last_load = Some(Instant::now());
            match r {
                Ok(s) => {
                    self.snap = Some(s);
                    self.load_error = None;
                    self.rebuild_decorations();
                }
                Err(e) => {
                    self.load_error = Some(e);
                    self.snap = None;
                    self.decorations.clear();
                }
            }
        }
        let worker_busy = self.worker.as_ref().is_some_and(|w| w.is_busy());
        let due = self.last_load.is_none_or(|t| t.elapsed() >= POLL_INTERVAL);
        let gap_ok = self.last_load.is_none_or(|t| t.elapsed() >= MIN_REFRESH_GAP);
        if self.load.is_none() && !worker_busy && ((self.dirty && gap_ok) || due) {
            self.dirty = false;
            let root = self.root.clone();
            let limit = self.log_limit;
            self.load = Some(Task::spawn(ctx, move || load_snapshot(&root, limit)));
        } else if self.dirty && !gap_ok {
            ctx.request_repaint_after(MIN_REFRESH_GAP);
        }
        ctx.request_repaint_after(POLL_INTERVAL);
    }

    fn submit(&mut self, kind: JobKind, label: &str, f: impl FnOnce(&Path) -> GitResult<String> + Send + 'static) {
        let top = self.top();
        if let Some(w) = &self.worker {
            w.submit(kind, label, move || f(&top));
        }
    }

    fn mutate(&mut self, label: &str, f: impl FnOnce(&Path) -> GitResult<()> + Send + 'static) {
        self.submit(JobKind::Mutate, label, move |p| f(p).map(|_| String::new()));
    }

    /// 패널을 그린다.
    pub fn ui(&mut self, ui: &mut Ui) -> Vec<GitEvent> {
        let mut events = Vec::new();
        self.pump(ui.ctx());
        let t = theme();

        egui::Frame::new().fill(t.bg_panel).inner_margin(Margin::ZERO).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.spacing_mut().item_spacing = vec2(6.0, 4.0);

            if self.snap.is_none() {
                self.ui_unavailable(ui);
                return;
            }
            egui::Frame::new().inner_margin(Margin { left: 10, right: 10, top: 8, bottom: 6 }).show(ui, |ui| {
                self.ui_header(ui);
                self.ui_banners(ui);
                self.ui_commit_box(ui);
            });
            ui.add(egui::Separator::default().spacing(0.0));
            egui::ScrollArea::vertical().id_salt("git_panel_scroll").auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                self.ui_sections(ui, &mut events);
            });
        });

        self.ui_confirm(ui.ctx());
        events
    }

    fn ui_unavailable(&mut self, ui: &mut Ui) {
        let t = theme();
        ui.add_space(16.0);
        match &self.load_error {
            None => {
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    spinner(ui, 14.0);
                    ui.label(dim("저장소 읽는 중…"));
                });
            }
            Some(GitError::NotARepo) => {
                empty_state(ui, "Git 저장소 없음", "이 폴더는 Git으로 추적되고 있지 않습니다.");
                let root = self.root.clone();
                ui.vertical_centered(|ui| {
                    if primary_button(ui, "저장소 초기화", None).clicked() {
                        let _ = git(&root, Mode::Write, &["init"]);
                        self.refresh();
                    }
                });
            }
            Some(GitError::GitMissing) => {
                empty_state(ui, "Git을 찾을 수 없음", "Git을 설치하고 PATH에 있는지 확인하세요.");
            }
            Some(e) => {
                let msg = e.to_string();
                egui::Frame::new().inner_margin(Margin::same(10)).show(ui, |ui| {
                    banner(ui, BannerKind::Error, "저장소를 읽을 수 없습니다", Some(&msg), false);
                    ui.add_space(6.0);
                    if tool_button(ui, Some(Icon::Refresh), "다시 시도").clicked() {
                        self.refresh();
                    }
                });
            }
        }
        let _ = t;
    }

    fn ui_header(&mut self, ui: &mut Ui) {
        let t = theme();
        let Some(snap) = &self.snap else { return };
        let br = snap.status.branch.clone();
        let running = self.worker.as_ref().and_then(|w| w.running());
        let busy = self.worker.as_ref().is_some_and(|w| w.is_busy());

        // 브랜치 선택 버튼
        let label = br.display_name();
        let detached = br.is_detached();
        let resp = {
            let w = ui.available_width();
            let (rect, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
            let hovered = resp.hovered();
            ui.painter().rect(
                rect,
                CornerRadius::same(5),
                if hovered { t.bg_hover } else { t.bg_elevated },
                Stroke::new(1.0, t.border),
                egui::StrokeKind::Inside,
            );
            let p = ui.painter();
            paint_icon(p, Rect::from_center_size(rect.left_center() + vec2(16.0, 0.0), vec2(15.0, 15.0)), Icon::Branch, if detached { t.orange } else { t.accent });
            let name_rect = p.text(
                rect.left_center() + vec2(30.0, 0.0),
                Align2::LEFT_CENTER,
                &label,
                FontId::proportional(13.5),
                t.text,
            );
            if let Some(up) = &br.upstream {
                p.text(
                    pos2(name_rect.right() + 8.0, rect.center().y),
                    Align2::LEFT_CENTER,
                    up.to_string(),
                    FontId::proportional(11.5),
                    t.text_faint,
                );
            } else if detached {
                p.text(pos2(name_rect.right() + 8.0, rect.center().y), Align2::LEFT_CENTER, "분리된 HEAD", FontId::proportional(11.5), t.orange);
            }
            paint_icon(p, Rect::from_center_size(rect.right_center() - vec2(14.0, 0.0), vec2(12.0, 12.0)), Icon::ChevronDown, t.text_dim);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("브랜치 {label}")));
            resp.on_hover_text("브랜치 전환")
        };
        self.ui_branch_popup(ui, &resp);

        ui.add_space(4.0);
        // 동기화 버튼
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.add_enabled_ui(!busy, |ui| {
                if tool_button(ui, Some(Icon::Refresh), "Fetch").on_hover_text("git fetch --all --prune").clicked() {
                    self.submit(JobKind::Sync, "Fetch", repo::fetch);
                }
                let pull_label = if br.behind > 0 { format!("Pull {}", br.behind) } else { "Pull".into() };
                if tool_button(ui, Some(Icon::ArrowDown), &pull_label).on_hover_text("git pull").clicked() {
                    self.submit(JobKind::Sync, "Pull", repo::pull);
                }
                let push_label = if br.upstream.is_none() && !detached {
                    "게시".to_string()
                } else if br.ahead > 0 {
                    format!("Push {}", br.ahead)
                } else {
                    "Push".into()
                };
                if tool_button(ui, Some(Icon::ArrowUp), &push_label)
                    .on_hover_text(if br.upstream.is_none() { "Push 후 upstream 설정" } else { "git push" })
                    .clicked()
                {
                    self.submit(JobKind::Sync, "Push", repo::push);
                }
            });
            if let Some(r) = &running {
                spinner(ui, 12.0);
                ui.label(faint(format!("{r}…")));
            } else if self.load.is_some() {
                spinner(ui, 11.0);
            }
        });
    }

    fn ui_branch_popup(&mut self, ui: &mut Ui, anchor: &egui::Response) {
        let t = theme();
        let popup_id = anchor.id.with("branch_popup");
        if anchor.clicked() {
            egui::Popup::toggle_id(ui.ctx(), popup_id);
            if egui::Popup::is_id_open(ui.ctx(), popup_id) {
                self.picker.filter.clear();
                self.picker.focus_filter = true;
                let root = self.top();
                self.picker.load = Some(Task::spawn(ui.ctx(), move || repo::branches(&root)));
            }
        }
        if let Some(task) = &mut self.picker.load
            && let Some(r) = task.take()
        {
            self.picker.load = None;
            match r {
                Ok(l) => {
                    self.picker.list = l;
                    self.picker.error = None;
                }
                Err(e) => self.picker.error = Some(e.to_string()),
            }
        }
        let mut action: Option<BranchAction> = None;
        let width = anchor.rect.width().max(300.0);
        egui::Popup::new(popup_id, ui.ctx().clone(), anchor.rect, ui.layer_id())
            .open_memory(None)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .width(width)
            .frame(
                egui::Frame::new()
                    .fill(t.bg_elevated)
                    .stroke(Stroke::new(1.0, t.border))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::same(6))
                    .shadow(egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(120) }),
            )
            .show(|ui| {
                ui.set_width(width - 12.0);
                let te = egui::TextEdit::singleline(&mut self.picker.filter)
                    .hint_text("브랜치 필터 또는 새로 만들기…")
                    .desired_width(f32::INFINITY)
                    .frame(input_frame());
                let r = ui.add(te);
                if self.picker.focus_filter {
                    r.request_focus();
                    self.picker.focus_filter = false;
                }
                let filter = self.picker.filter.trim().to_string();
                let fl = filter.to_lowercase();
                let exists = self.picker.list.iter().any(|b| !b.remote && b.name == filter);
                ui.add_space(4.0);
                if !filter.is_empty() && !exists {
                    let resp = branch_row(ui, &format!("새 브랜치 \"{filter}\" 만들기"), None, false, t.accent);
                    if resp.clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))) {
                        action = Some(BranchAction::Create(filter.clone()));
                    }
                }
                if self.picker.load.is_some() && self.picker.list.is_empty() {
                    ui.horizontal(|ui| {
                        spinner(ui, 12.0);
                        ui.label(dim("브랜치 불러오는 중…"));
                    });
                }
                if let Some(e) = &self.picker.error {
                    ui.label(RichText::new(e).color(t.red).size(12.0));
                }
                egui::ScrollArea::vertical().max_height(320.0).auto_shrink([false, true]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for remote in [false, true] {
                        let items: Vec<&Branch> = self
                            .picker
                            .list
                            .iter()
                            .filter(|b| b.remote == remote && (fl.is_empty() || b.name.to_lowercase().contains(&fl)))
                            .take(if remote { 60 } else { 200 })
                            .collect();
                        if items.is_empty() {
                            continue;
                        }
                        ui.add_space(4.0);
                        ui.label(RichText::new(if remote { "원격" } else { "로컬" }).size(10.5).color(t.text_faint));
                        ui.add_space(2.0);
                        for b in items {
                            let color = if b.current { t.accent } else { t.text };
                            let detail = if !b.track.is_empty() { Some(b.track.as_str()) } else { None };
                            let resp = branch_row(ui, &b.name, detail, b.current, color);
                            if !remote && !b.current && resp.hovered() {
                                let r = Rect::from_center_size(resp.rect.right_center() - vec2(14.0, 0.0), vec2(20.0, 20.0));
                                if icon_button_at(ui, r, resp.id.with("del"), Icon::Trash, "브랜치 삭제").clicked() {
                                    action = Some(BranchAction::Delete(b.name.clone()));
                                    continue;
                                }
                            }
                            if resp.clicked() && !b.current {
                                action = Some(BranchAction::Checkout(b.clone()));
                            }
                        }
                    }
                });
            });
        if let Some(a) = action {
            egui::Popup::close_id(ui.ctx(), popup_id);
            match a {
                BranchAction::Create(name) => {
                    self.submit(JobKind::Checkout, "브랜치 만들기", move |p| repo::create_branch(p, &name));
                }
                BranchAction::Checkout(b) => {
                    self.submit(JobKind::Checkout, "체크아웃", move |p| repo::checkout(p, &b));
                }
                BranchAction::Delete(name) => {
                    self.confirm = Some(Confirm::DeleteBranch { name, force: false });
                }
            }
        }
    }

    fn ui_banners(&mut self, ui: &mut Ui) {
        let Some(snap) = &self.snap else { return };
        if let Some(op) = snap.op {
            ui.add_space(6.0);
            let conflicts = snap.status.conflicted_count();
            let title = if conflicts > 0 {
                format!("{} 진행 중 — 충돌 {conflicts}개를 해결하고 커밋하세요", op.label())
            } else {
                format!("{} 진행 중 — 모든 충돌을 해결했습니다. 커밋할 수 있습니다", op.label())
            };
            let mut abort = false;
            egui::Frame::new()
                .fill(alpha(theme().orange, 0.10))
                .stroke(Stroke::new(1.0, alpha(theme().orange, 0.5)))
                .corner_radius(CornerRadius::same(5))
                .inner_margin(Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("⚠").color(theme().orange));
                        ui.add(egui::Label::new(RichText::new(&title).size(12.5).color(theme().text)).wrap());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if tool_button(ui, None, "중단").clicked() {
                                abort = true;
                            }
                        });
                    });
                });
            if abort {
                self.confirm = Some(Confirm::AbortOp(op));
            }
        }
        if let Some((kind, title, detail)) = self.banner.clone() {
            ui.add_space(6.0);
            if banner(ui, kind, &title, detail.as_deref(), true) {
                self.banner = None;
            }
        }
    }

    fn ui_commit_box(&mut self, ui: &mut Ui) {
        let t = theme();
        let Some(snap) = &self.snap else { return };
        let staged = snap.status.staged().count();
        let has_head = snap.status.branch.oid.is_some();
        let merging = snap.op.is_some();
        let conflicts = snap.status.conflicted_count();
        let branch = snap.status.branch.display_name();
        let busy = self.worker.as_ref().is_some_and(|w| w.is_busy());
        ui.add_space(8.0);

        let te_id = Id::new(("kiln_git_commit_msg", &self.root));
        let focused = ui.memory(|m| m.has_focus(te_id));
        let submit = focused && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));

        let te = egui::TextEdit::multiline(&mut self.message)
            .id(te_id)
            .hint_text(format!("메시지 ({}로 \"{branch}\"에 커밋)", shortcut_label(ui.ctx())))
            .desired_rows(3)
            .desired_width(f32::INFINITY)
            .font(FontId::proportional(13.0))
            .frame(input_frame());
        let te_resp = ui.add(te);

        // 제목 길이 표시(입력창 오른쪽 아래)
        let subject_len = self.message.lines().next().map(|l| l.chars().count()).unwrap_or(0);
        if subject_len > 0 {
            let c = if subject_len > SUBJECT_MAX {
                t.red
            } else if subject_len > SUBJECT_HINT {
                t.yellow
            } else {
                t.text_faint
            };
            let tip = if subject_len > SUBJECT_MAX { "제목 줄이 72자를 넘습니다" } else { "제목 줄 길이" };
            let r = ui.painter().text(
                te_resp.rect.right_bottom() - vec2(8.0, 5.0),
                Align2::RIGHT_BOTTOM,
                format!("{subject_len}/{SUBJECT_MAX}"),
                FontId::monospace(10.5),
                c,
            );
            ui.interact(r, te_id.with("len"), Sense::hover()).on_hover_text(tip);
        }
        ui.add_space(4.0);

        let can_commit = !busy && conflicts == 0 && (staged > 0 || merging) && !self.message.trim().is_empty();
        let can_amend = !busy && has_head && conflicts == 0;
        let row_w = ui.available_width();
        ui.allocate_ui_with_layout(vec2(row_w, 28.0), Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let push_clicked = ui
                .add_enabled_ui(can_commit, |ui| secondary_button(ui, "커밋 및 Push"))
                .inner
                .on_hover_text("커밋한 뒤 upstream 브랜치로 Push")
                .clicked();
            let amend_clicked = ui
                .add_enabled_ui(can_amend, |ui| secondary_button(ui, "커밋 수정"))
                .inner
                .on_hover_text("마지막 커밋 수정 (입력란이 비어 있으면 기존 메시지 유지)")
                .clicked();
            let w = ui.available_width().max(60.0);
            let label = if staged > 0 { format!("커밋 ({staged})") } else { "커밋".into() };
            let r = ui.add_enabled_ui(can_commit, |ui| primary_button(ui, &label, Some(w))).inner;
            let hint = if conflicts > 0 {
                "먼저 병합 충돌을 해결하세요"
            } else if staged == 0 && !merging {
                "커밋할 변경 사항을 스테이징하세요"
            } else if self.message.trim().is_empty() {
                "커밋 메시지를 입력하세요"
            } else {
                "스테이징된 변경 사항 커밋"
            };
            let clicked = r.on_hover_text(hint).on_disabled_hover_text(hint).clicked();
            if (clicked || submit) && can_commit {
                let msg = self.message.clone();
                self.submit(JobKind::Commit, "커밋", move |p| repo::commit(p, &msg, false));
            }
            if amend_clicked {
                let msg = self.message.clone();
                self.submit(JobKind::Commit, "커밋 수정", move |p| repo::commit(p, &msg, true));
            }
            if push_clicked {
                let msg = self.message.clone();
                self.submit(JobKind::Commit, "커밋 및 Push", move |p| {
                    let out = repo::commit(p, &msg, false)?;
                    repo::push(p).map(|o| format!("{out}\n{o}"))
                });
            }
        });
    }

    fn ui_sections(&mut self, ui: &mut Ui, events: &mut Vec<GitEvent>) {
        let Some(snap) = self.snap.clone() else { return };
        let st = &snap.status;
        let conflicts: Vec<&StatusEntry> = st.conflicted().collect();
        let staged: Vec<&StatusEntry> = st.staged().collect();
        let changes: Vec<&StatusEntry> = st.unstaged().collect();
        let untracked: Vec<&StatusEntry> = st.untracked().collect();
        let busy = self.worker.as_ref().is_some_and(|w| w.is_busy());
        let t = theme();

        if !conflicts.is_empty() {
            let mut open = self.open[&Section::Conflicts];
            section_header(ui, &mut open, "병합 충돌", Some(conflicts.len()), Some(t.orange), |_| {});
            self.open.insert(Section::Conflicts, open);
            if open {
                for e in &conflicts {
                    self.file_row(ui, e, RowKind::Conflict, busy, events);
                }
            }
        }

        let mut open = self.open[&Section::Staged];
        let mut unstage_all = false;
        section_header(ui, &mut open, "스테이징된 변경 사항", Some(staged.len()), None, |ui| {
            if !staged.is_empty() && icon_button(ui, Icon::Minus, "모두 스테이징 취소").clicked() {
                unstage_all = true;
            }
        });
        self.open.insert(Section::Staged, open);
        if unstage_all {
            self.mutate("모두 스테이징 취소", repo::unstage_all);
        }
        if open {
            if staged.is_empty() {
                hint_row(ui, "스테이징된 변경 사항 없음");
            }
            for e in &staged {
                self.file_row(ui, e, RowKind::Staged, busy, events);
            }
        }

        let mut open = self.open[&Section::Changes];
        let mut stage_all = false;
        let mut discard_all = false;
        section_header(ui, &mut open, "변경 사항", Some(changes.len()), None, |ui| {
            if !changes.is_empty() {
                if icon_button(ui, Icon::Plus, "모든 변경 사항 스테이징").clicked() {
                    stage_all = true;
                }
                if icon_button(ui, Icon::Discard, "모든 변경 사항 취소").clicked() {
                    discard_all = true;
                }
            }
        });
        self.open.insert(Section::Changes, open);
        if stage_all {
            let paths: Vec<String> = changes.iter().map(|e| e.path.clone()).collect();
            self.mutate("모두 스테이징", move |p| repo::stage(p, &paths));
        }
        if discard_all {
            self.confirm = Some(Confirm::DiscardAll);
        }
        if open {
            if changes.is_empty() {
                hint_row(ui, "작업 트리가 깨끗합니다");
            }
            for e in &changes {
                self.file_row(ui, e, RowKind::Changed, busy, events);
            }
        }

        if !untracked.is_empty() {
            let mut open = self.open[&Section::Untracked];
            let mut stage_u = false;
            section_header(ui, &mut open, "추적되지 않음", Some(untracked.len()), None, |ui| {
                if icon_button(ui, Icon::Plus, "추적되지 않은 파일 모두 스테이징").clicked() {
                    stage_u = true;
                }
            });
            self.open.insert(Section::Untracked, open);
            if stage_u {
                let paths: Vec<String> = untracked.iter().map(|e| e.path.clone()).collect();
                self.mutate("추적되지 않은 파일 스테이징", move |p| repo::stage(p, &paths));
            }
            if open {
                for e in &untracked {
                    self.file_row(ui, e, RowKind::Untracked, busy, events);
                }
            }
        }

        // Stash
        let mut open = self.open[&Section::Stashes];
        let mut new_stash = false;
        let dirty_tree = !staged.is_empty() || !changes.is_empty() || !untracked.is_empty();
        section_header(ui, &mut open, "스태시", Some(snap.stashes.len()), None, |ui| {
            if dirty_tree && icon_button(ui, Icon::Plus, "변경 사항 스태시…").clicked() {
                new_stash = true;
            }
        });
        if new_stash {
            open = true;
            self.stash_input = Some(String::new());
        }
        self.open.insert(Section::Stashes, open);
        if open {
            self.ui_stashes(ui, &snap.stashes);
        }

        // 커밋 로그
        let mut open = self.open[&Section::Commits];
        section_header(ui, &mut open, "커밋 기록", None, None, |_| {});
        self.open.insert(Section::Commits, open);
        if open {
            self.ui_log(ui, &snap, events);
        }
        ui.add_space(12.0);
    }

    fn ui_stashes(&mut self, ui: &mut Ui, stashes: &[StashEntry]) {
        let t = theme();
        if let Some(mut msg) = self.stash_input.take() {
            let mut keep = true;
            egui::Frame::new().inner_margin(Margin { left: 18, right: 8, top: 4, bottom: 6 }).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut msg)
                            .hint_text("스태시 메시지 (선택 사항)")
                            .desired_width(ui.available_width() - 110.0)
                            .frame(input_frame()),
                    );
                    if !r.has_focus() && msg.is_empty() {
                        r.request_focus();
                    }
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    if tool_button(ui, None, "스태시").clicked() || enter {
                        let m = msg.clone();
                        self.submit(JobKind::Mutate, "스태시", move |p| repo::stash_push(p, &m));
                        keep = false;
                    }
                    if icon_button(ui, Icon::Close, "취소").clicked() {
                        keep = false;
                    }
                });
            });
            if keep {
                self.stash_input = Some(msg);
            }
        }
        if stashes.is_empty() {
            hint_row(ui, "스태시 없음");
            return;
        }
        let now = self.now();
        for s in stashes {
            let w = ui.available_width();
            let (rect, resp) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::click());
            let hovered = resp.hovered() || ui.rect_contains_pointer(rect);
            if hovered {
                ui.painter().rect_filled(rect, CornerRadius::ZERO, t.bg_hover);
            }
            let p = ui.painter();
            paint_icon(p, Rect::from_center_size(rect.left_center() + vec2(24.0, 0.0), vec2(13.0, 13.0)), Icon::Stash, t.text_faint);
            let msg = s.message.split_once(": ").map(|(_, m)| m).unwrap_or(&s.message);
            let right_reserved = if hovered { 80.0 } else { 70.0 };
            let job = one_line_job(
                &[(msg, 12.5, t.text), (&format!("  {}", relative_time(s.date, now)), 11.5, t.text_faint)],
                w - 34.0 - right_reserved,
            );
            let g = p.layout_job(job);
            p.galley(pos2(rect.left() + 34.0, rect.center().y - g.size().y / 2.0), g, t.text);
            p.text(rect.right_center() - vec2(8.0, 0.0), Align2::RIGHT_CENTER, &s.reference, FontId::monospace(10.5), t.text_faint);
            if hovered {
                let mut x = rect.right() - 8.0;
                let mut btn = |ui: &mut Ui, icon: Icon, tip: &str| {
                    let r = Rect::from_center_size(pos2(x - 10.0, rect.center().y), vec2(20.0, 20.0));
                    x -= 22.0;
                    icon_button_at(ui, r, resp.id.with(tip), icon, tip).clicked()
                };
                ui.painter().rect_filled(
                    Rect::from_min_max(pos2(rect.right() - 76.0, rect.top()), rect.max),
                    CornerRadius::ZERO,
                    t.bg_hover,
                );
                let idx = s.index;
                if btn(ui, Icon::Trash, "스태시 삭제") {
                    self.confirm = Some(Confirm::DropStash { index: idx, message: msg.to_string() });
                }
                if btn(ui, Icon::Pop, "스태시 팝 (적용 후 삭제)") {
                    self.submit(JobKind::Mutate, "스태시 팝", move |p| repo::stash_pop(p, idx));
                }
                if btn(ui, Icon::Apply, "스태시 적용") {
                    self.submit(JobKind::Mutate, "스태시 적용", move |p| repo::stash_apply(p, idx));
                }
            }
        }
    }

    fn ui_log(&mut self, ui: &mut Ui, snap: &Snapshot, events: &mut Vec<GitEvent>) {
        let t = theme();
        if snap.log.is_empty() {
            hint_row(ui, "아직 커밋이 없습니다");
            return;
        }
        let now = self.now();
        let lanes_max = snap.graph.iter().map(|g| g.width).max().unwrap_or(1).min(8);
        let graph_w = 12.0 + lanes_max as f32 * LANE_W;
        let colors = [t.accent, t.green, t.purple, t.orange, t.yellow, t.blue, t.red];
        for (i, c) in snap.log.iter().enumerate() {
            let w = ui.available_width();
            let (rect, resp) = ui.allocate_exact_size(vec2(w, COMMIT_ROW_H), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &c.subject));
            if !ui.is_rect_visible(rect) {
                continue;
            }
            if resp.hovered() {
                ui.painter().rect_filled(rect, CornerRadius::ZERO, t.bg_hover);
            }
            if resp.clicked() {
                events.push(GitEvent::OpenCommit(c.sha.clone()));
            }
            let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
            // 그래프
            if let Some(g) = snap.graph.get(i) {
                let lx = |lane: usize| rect.left() + 12.0 + lane.min(lanes_max) as f32 * LANE_W;
                let top = rect.top();
                let mid = rect.top() + 13.0;
                let bot = rect.bottom();
                let col = |lane: usize| colors[lane % colors.len()];
                for &(from, to) in &g.pass {
                    p.line_segment([pos2(lx(from), top), pos2(lx(to), bot)], Stroke::new(1.6, col(to)));
                }
                for &(from, _) in &g.top {
                    p.line_segment([pos2(lx(from), top), pos2(lx(g.col), mid)], Stroke::new(1.6, col(from)));
                }
                for &to in &g.bottom_from_commit {
                    let c2 = col(to);
                    if to == g.col {
                        p.line_segment([pos2(lx(g.col), mid), pos2(lx(to), bot)], Stroke::new(1.6, c2));
                    } else {
                        let pts = vec![pos2(lx(g.col), mid), pos2(lx(to), mid + 8.0), pos2(lx(to), bot)];
                        p.line(pts, Stroke::new(1.6, c2));
                    }
                }
                let cc = col(g.col);
                let center = pos2(lx(g.col), mid);
                if c.parents.len() > 1 {
                    p.circle(center, 4.0, t.bg_panel, Stroke::new(2.0, cc));
                } else {
                    p.circle(center, 4.0, cc, Stroke::new(1.5, t.bg_panel));
                }
            }
            // 제목 + refs
            let x0 = rect.left() + graph_w + 4.0;
            let mut x = x0;
            let y1 = rect.top() + 13.0;
            for r in &c.refs {
                let (label, fg) = ref_style(r);
                if label.is_empty() {
                    continue;
                }
                let g = p.layout_no_wrap(label.to_string(), FontId::proportional(10.5), fg);
                let br = Rect::from_min_size(pos2(x, y1 - 8.0), vec2(g.size().x + 10.0, 16.0));
                if br.right() > rect.right() - 60.0 {
                    break;
                }
                p.rect(br, CornerRadius::same(8), alpha(fg, 0.14), Stroke::new(1.0, alpha(fg, 0.45)), egui::StrokeKind::Inside);
                p.galley(br.center() - g.size() / 2.0, g, fg);
                x = br.right() + 4.0;
            }
            let job = one_line_job(&[(&c.subject, 12.5, t.text)], (rect.right() - 8.0 - x).max(20.0));
            let g = p.layout_job(job);
            p.galley(pos2(x, y1 - g.size().y / 2.0), g, t.text);
            let meta = format!("{} · {} · {}", c.author, relative_time(c.date, now), c.short());
            let job = one_line_job(&[(&meta, 11.0, t.text_faint)], rect.right() - 8.0 - x0);
            let g = p.layout_job(job);
            p.galley(pos2(x0, rect.top() + 21.0), g, t.text_faint);
            resp.on_hover_text(format!("{}\n{} <{}>", c.subject, c.author, c.email));
        }
        if snap.log.len() >= self.log_limit {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                if tool_button(ui, None, "커밋 더 불러오기").clicked() {
                    self.log_limit += LOG_PAGE;
                    self.refresh();
                }
            });
        }
    }

    fn file_row(&mut self, ui: &mut Ui, e: &StatusEntry, kind: RowKind, busy: bool, events: &mut Vec<GitEvent>) {
        let t = theme();
        let w = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::click());
        let label = format!("{}{}", if kind == RowKind::Staged { "스테이징됨: " } else { "" }, e.path);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
        if !ui.is_rect_visible(rect) {
            return;
        }
        let staged = kind == RowKind::Staged;
        let is_sel = self.selected.as_ref().is_some_and(|(p, s)| p == &e.path && *s == staged);
        let hovered = resp.hovered() || ui.rect_contains_pointer(rect);
        if is_sel {
            ui.painter().rect_filled(rect, CornerRadius::ZERO, t.bg_selected);
        } else if hovered {
            ui.painter().rect_filled(rect, CornerRadius::ZERO, t.bg_hover);
        }
        let letter = match kind {
            RowKind::Conflict => 'C',
            RowKind::Untracked => 'U',
            RowKind::Staged => {
                if e.index == 'R' { 'R' } else { e.index }
            }
            RowKind::Changed => e.worktree,
        };
        let color = match kind {
            RowKind::Untracked => t.green,
            _ => status_color(letter),
        };
        let path = e.path.trim_end_matches('/');
        let (dir, name) = match path.rfind('/') {
            Some(i) => (&path[..i], &path[i + 1..]),
            None => ("", path),
        };
        let name = if e.path.ends_with('/') { format!("{name}/") } else { name.to_string() };
        let p = ui.painter();
        let right_pad = if hovered { 90.0 } else { 26.0 };
        let mut parts: Vec<(&str, f32, Color32)> = Vec::new();
        let name_color = if letter == 'D' { t.text_dim } else { t.text };
        parts.push((&name, 13.0, name_color));
        let dir_s;
        if let Some(orig) = &e.orig_path {
            dir_s = format!("  {orig}에서");
            parts.push((&dir_s, 11.5, t.text_faint));
        } else if !dir.is_empty() {
            dir_s = format!("  {dir}");
            parts.push((&dir_s, 11.5, t.text_faint));
        }
        let sub_s;
        if e.submodule {
            sub_s = "  서브모듈".to_string();
            parts.push((&sub_s, 11.0, t.purple));
        }
        let conf_s;
        if kind == RowKind::Conflict {
            conf_s = format!("  {}", e.conflict_label());
            parts.push((&conf_s, 11.0, t.orange));
        }
        let mut job = one_line_job(&parts, w - 26.0 - right_pad);
        if letter == 'D'
            && let Some(s) = job.sections.first_mut() {
                s.format.strikethrough = Stroke::new(1.0, t.text_dim);
            }
        let g = p.layout_job(job);
        p.galley(pos2(rect.left() + 22.0, rect.center().y - g.size().y / 2.0), g, t.text);
        p.text(rect.right_center() - vec2(12.0, 0.0), Align2::CENTER_CENTER, letter, FontId::monospace(12.0), color);

        let abs = self.abs_path(&e.path);
        let mut handled = false;
        if hovered {
            let mut x = rect.right() - 24.0;
            let row_id = resp.id;
            let mut btn = |ui: &mut Ui, icon: Icon, tip: &str| -> bool {
                let r = Rect::from_center_size(pos2(x - 11.0, rect.center().y), vec2(20.0, 20.0));
                x -= 22.0;
                ui.add_enabled_ui(!busy, |ui| icon_button_at(ui, r, row_id.with(tip), icon, tip)).inner.clicked()
            };
            let paths = vec![e.path.clone()];
            match kind {
                RowKind::Staged => {
                    if btn(ui, Icon::Minus, "스테이징 취소") {
                        handled = true;
                        self.mutate("스테이징 취소", move |p| repo::unstage(p, &paths));
                    }
                }
                RowKind::Changed => {
                    if btn(ui, Icon::Plus, "스테이징") {
                        handled = true;
                        let ps = paths.clone();
                        self.mutate("스테이징", move |p| repo::stage(p, &ps));
                    }
                    if btn(ui, Icon::Discard, "변경 사항 취소") {
                        handled = true;
                        self.confirm = Some(Confirm::Discard { paths: paths.clone(), untracked: false });
                    }
                }
                RowKind::Untracked => {
                    if btn(ui, Icon::Plus, "스테이징") {
                        handled = true;
                        let ps = paths.clone();
                        self.mutate("스테이징", move |p| repo::stage(p, &ps));
                    }
                    if btn(ui, Icon::Trash, "파일 삭제") {
                        handled = true;
                        self.confirm = Some(Confirm::Discard { paths: paths.clone(), untracked: true });
                    }
                }
                RowKind::Conflict => {
                    if btn(ui, Icon::Check, "해결됨으로 표시 (스테이징)") {
                        handled = true;
                        let ps = paths.clone();
                        self.mutate("해결됨으로 표시", move |p| repo::stage(p, &ps));
                    }
                }
            }
            if btn(ui, Icon::Open, "파일 열기") {
                handled = true;
                events.push(GitEvent::OpenFile(abs.clone()));
            }
        }
        if !handled {
            if resp.double_clicked() {
                events.push(GitEvent::OpenFile(abs.clone()));
            } else if resp.clicked() {
                self.selected = Some((e.path.clone(), staged));
                if kind == RowKind::Conflict {
                    events.push(GitEvent::OpenFile(abs.clone()));
                } else {
                    events.push(GitEvent::OpenDiff { path: abs.clone(), staged });
                }
            }
        }
        let path_owned = e.path.clone();
        resp.context_menu(|ui| {
            if ui.button("파일 열기").clicked() {
                events.push(GitEvent::OpenFile(abs.clone()));
                ui.close();
            }
            if kind != RowKind::Conflict && kind != RowKind::Untracked && ui.button("변경 사항 열기").clicked() {
                events.push(GitEvent::OpenDiff { path: abs.clone(), staged });
                ui.close();
            }
            ui.separator();
            match kind {
                RowKind::Conflict => {
                    if ui.button("현재 변경 수락 (ours)").clicked() {
                        let p2 = path_owned.clone();
                        self.mutate("현재 변경 수락", move |p| repo::resolve_conflict(p, &p2, true));
                        ui.close();
                    }
                    if ui.button("수신 변경 수락 (theirs)").clicked() {
                        let p2 = path_owned.clone();
                        self.mutate("수신 변경 수락", move |p| repo::resolve_conflict(p, &p2, false));
                        ui.close();
                    }
                    if ui.button("해결됨으로 표시").clicked() {
                        let ps = vec![path_owned.clone()];
                        self.mutate("해결됨으로 표시", move |p| repo::stage(p, &ps));
                        ui.close();
                    }
                }
                RowKind::Staged => {
                    if ui.button("스테이징 취소").clicked() {
                        let ps = vec![path_owned.clone()];
                        self.mutate("스테이징 취소", move |p| repo::unstage(p, &ps));
                        ui.close();
                    }
                }
                RowKind::Changed | RowKind::Untracked => {
                    if ui.button("스테이징").clicked() {
                        let ps = vec![path_owned.clone()];
                        self.mutate("스테이징", move |p| repo::stage(p, &ps));
                        ui.close();
                    }
                    if ui.button(if kind == RowKind::Untracked { "파일 삭제…" } else { "변경 사항 취소…" }).clicked() {
                        self.confirm =
                            Some(Confirm::Discard { paths: vec![path_owned.clone()], untracked: kind == RowKind::Untracked });
                        ui.close();
                    }
                }
            }
            ui.separator();
            if ui.button("경로 복사").clicked() {
                ui.ctx().copy_text(path_owned.clone());
                ui.close();
            }
        });
    }

    fn ui_confirm(&mut self, ctx: &egui::Context) {
        let Some(c) = self.confirm.clone() else { return };
        let id = Id::new(("kiln_git_confirm", &self.root));
        let (title, msg, ok, danger) = match &c {
            Confirm::Discard { paths, untracked: false } => (
                "변경 사항을 취소할까요?".to_string(),
                format!("{}의 변경 사항이 사라집니다. 이 작업은 되돌릴 수 없습니다.", paths.join(", ")),
                "변경 사항 취소",
                true,
            ),
            Confirm::Discard { paths, untracked: true } => (
                "추적되지 않은 파일을 삭제할까요?".to_string(),
                format!("{}이(가) 영구적으로 삭제됩니다.", paths.join(", ")),
                "삭제",
                true,
            ),
            Confirm::DiscardAll => (
                "모든 변경 사항을 취소할까요?".to_string(),
                "추적 중인 파일의 스테이징되지 않은 변경 사항이 모두 사라집니다. 추적되지 않은 파일은 유지됩니다.".to_string(),
                "모두 취소",
                true,
            ),
            Confirm::DeleteBranch { name, .. } => (
                format!("\"{name}\" 브랜치를 삭제할까요?"),
                "로컬 브랜치가 삭제됩니다. 강제로 삭제하면 다른 곳에 병합되지 않은 커밋을 잃을 수 있습니다.".to_string(),
                "브랜치 삭제",
                true,
            ),
            Confirm::DropStash { message, .. } => (
                "스태시를 삭제할까요?".to_string(),
                format!("\"{message}\"이(가) 영구적으로 제거됩니다."),
                "삭제",
                true,
            ),
            Confirm::AbortOp(op) => (
                format!("{} 작업을 중단할까요?", op.label()),
                "저장소가 작업 시작 전 상태로 돌아갑니다.".to_string(),
                "중단",
                true,
            ),
        };
        let mut force = matches!(c, Confirm::DeleteBranch { force: true, .. });
        let is_branch = matches!(c, Confirm::DeleteBranch { .. });
        let r = confirm_modal(ctx, id, &title, &msg, ok, danger, |ui| {
            if is_branch {
                ui.add_space(8.0);
                checkbox_row(ui, &mut force, "병합되지 않았어도 강제 삭제 (-D)");
            }
        });
        if let Confirm::DeleteBranch { name, .. } = &c {
            self.confirm = Some(Confirm::DeleteBranch { name: name.clone(), force });
        }
        match r {
            Some(true) => {
                self.confirm = None;
                match c {
                    Confirm::Discard { paths, untracked } => {
                        if untracked {
                            self.mutate("삭제", move |p| repo::clean_untracked(p, &paths));
                        } else {
                            self.mutate("변경 사항 취소", move |p| repo::discard(p, &paths));
                        }
                    }
                    Confirm::DiscardAll => {
                        self.mutate("모두 취소", |p| repo::discard(p, &[".".to_string()]));
                    }
                    Confirm::DeleteBranch { name, .. } => {
                        self.submit(JobKind::Mutate, "브랜치 삭제", move |p| repo::delete_branch(p, &name, force));
                    }
                    Confirm::DropStash { index, .. } => {
                        self.submit(JobKind::Mutate, "스태시 삭제", move |p| repo::stash_drop(p, index));
                    }
                    Confirm::AbortOp(op) => {
                        self.submit(JobKind::Mutate, "중단", move |p| repo::abort_op(p, op));
                    }
                }
            }
            Some(false) => self.confirm = None,
            None => {}
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Conflict,
    Staged,
    Changed,
    Untracked,
}

enum BranchAction {
    Create(String),
    Checkout(Branch),
    Delete(String),
}

fn hint_row(ui: &mut Ui, text: &str) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::hover());
    ui.painter().text(rect.left_center() + vec2(22.0, 0.0), Align2::LEFT_CENTER, text, FontId::proportional(12.0), theme().text_faint);
}

fn branch_row(ui: &mut Ui, name: &str, detail: Option<&str>, current: bool, color: Color32) -> egui::Response {
    let t = theme();
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 24.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(4), t.bg_hover);
    }
    let p = ui.painter();
    if current {
        paint_icon(p, Rect::from_center_size(rect.left_center() + vec2(10.0, 0.0), vec2(12.0, 12.0)), Icon::Check, t.accent);
    }
    let mut parts: Vec<(&str, f32, Color32)> = vec![(name, 13.0, color)];
    let d;
    if let Some(x) = detail {
        d = format!("  {x}");
        parts.push((&d, 11.0, t.text_faint));
    }
    let g = p.layout_job(one_line_job(&parts, w - 50.0));
    p.galley(pos2(rect.left() + 22.0, rect.center().y - g.size().y / 2.0), g, t.text);
    resp
}

/// 여러 조각을 한 줄로 이어 붙이고 넘치면 `…` 로 자르는 레이아웃.
pub(crate) fn one_line_job(parts: &[(&str, f32, Color32)], max_width: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    for (text, size, color) in parts {
        job.append(text, 0.0, TextFormat { font_id: FontId::proportional(*size), color: *color, valign: Align::Center, ..Default::default() });
    }
    job.wrap.max_width = max_width.max(10.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    job
}

/// `%D` 장식 한 항목의 표시 문구와 색.
fn ref_style(r: &str) -> (String, Color32) {
    let t = theme();
    if let Some(b) = r.strip_prefix("HEAD -> ") {
        (b.to_string(), t.accent)
    } else if r == "HEAD" {
        ("HEAD".into(), t.orange)
    } else if let Some(tag) = r.strip_prefix("tag: ") {
        (tag.to_string(), t.yellow)
    } else if r.contains('/') {
        (r.to_string(), t.purple)
    } else {
        (r.to_string(), t.green)
    }
}

/// 커밋 단축키 표기(macOS 는 ⌘Enter, 그 외 Ctrl+Enter).
fn shortcut_label(ctx: &egui::Context) -> &'static str {
    if ctx.os() == egui::os::OperatingSystem::Mac { "⌘Enter" } else { "Ctrl+Enter" }
}
