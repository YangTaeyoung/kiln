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
use crate::repo::{self, Branch, RepoOp, StashEntry};
use crate::status::{Status, StatusEntry, decoration_char};
use crate::util::{now_unix, relative_time};

const POLL_INTERVAL: Duration = Duration::from_secs(10);
const MIN_REFRESH_GAP: Duration = Duration::from_millis(350);
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
}

fn load_snapshot(root: &Path) -> GitResult<Snapshot> {
    let top_raw = git(root, Mode::Read, &["rev-parse", "--show-toplevel", "--show-prefix"])?;
    let mut lines = top_raw.lines();
    let top = PathBuf::from(lines.next().unwrap_or("").trim());
    let prefix = lines.next().unwrap_or("").trim().to_string();
    let status = repo::status(&top)?;
    let op = repo::in_progress_op(&top);
    let stashes = if status.stash_count > 0 { repo::stash_list(&top).unwrap_or_default() } else { Vec::new() };
    Ok(Snapshot { top, prefix, status, op, stashes })
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
    history: Option<super::history_view::HistoryView>,
    message: String,
    submitted_message: Option<String>,
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
            history: None,
            message: String::new(),
            submitted_message: None,
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
        if let Some(history)=&mut self.history {history.refresh();}
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
            || self.history.as_mut().is_some_and(|h|h.is_busy())
    }

    /// 상대 시간 계산 기준 시각을 고정한다.
    #[doc(hidden)]
    pub fn set_now(&mut self, ts: i64) {
        self.now_override = Some(ts);
        if let Some(history)=&mut self.history {history.set_now(ts);}
    }

    /// 커밋 메시지 입력값.
    pub fn commit_draft(&self) -> &str { &self.message }

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
                if let Some(history)=&mut self.history {history.refresh();}
                match (&done.result, done.kind) {
                    (Ok(out), JobKind::Sync) => {
                        let msg = out.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").to_string();
                        self.banner = Some((BannerKind::Success, kiln_common::trf!("{} 완료", done.label), Some(msg)));
                    }
                    (Ok(_), JobKind::Commit) => {
                        if self.submitted_message.take().as_deref() == Some(self.message.as_str()) { self.message.clear(); }
                        self.banner = None;
                    }
                    (Ok(_), _) => {}
                    (Err(e), _) => {
                        self.banner = Some((BannerKind::Error, kiln_common::trf!("{} 실패", done.label), Some(e.to_string())));
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
            self.load = Some(Task::spawn(ctx, move || load_snapshot(&root)));
        } else if self.dirty && !gap_ok {
            ctx.request_repaint_after(MIN_REFRESH_GAP);
        }
        ctx.request_repaint_after(POLL_INTERVAL);
    }

    fn mutation_busy(&self)->bool {
        self.worker.as_ref().is_some_and(|w|w.is_busy())
            || self.history.as_ref().is_some_and(|h|h.blocks_external_mutation())
    }

    fn submit(&mut self, kind: JobKind, label: &str, f: impl FnOnce(&Path) -> GitResult<String> + Send + 'static) {
        if self.mutation_busy() {return;}
        let top = self.top();
        if let Some(w) = &self.worker {
            if kind == JobKind::Commit { self.submitted_message=Some(self.message.clone()); }
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
        if self.snap.is_some() && self.history.is_none() {self.history=Some(super::history_view::HistoryView::new(self.root.clone()));}
        if let Some(history)=&mut self.history {history.pump(ui.ctx());}
        let t = theme();

        egui::Frame::new().fill(t.bg_panel).inner_margin(Margin::ZERO).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.spacing_mut().item_spacing = vec2(6.0, 4.0);

            if self.snap.is_none() {
                self.ui_unavailable(ui);
                return;
            }
            egui::Frame::new().inner_margin(Margin { left: 12, right: 12, top: 10, bottom: 10 }).show(ui, |ui| {
                self.ui_header(ui);
                self.ui_banners(ui);
                self.ui_commit_box(ui);
            });
            let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
            ui.painter().rect_filled(line.shrink2(vec2(12.0, 0.0)), 0.0, t.border);
            ui.add_space(4.0);
            egui::ScrollArea::vertical().id_salt("git_panel_scroll").auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                self.ui_sections(ui, &mut events);
            });
        });

        self.ui_confirm(ui.ctx());
        if let Some(history)=&mut self.history {
            for event in history.finish_embedded(ui.ctx(),self.worker.as_ref().is_some_and(|w|w.is_busy())) {
                if matches!(event,crate::history::HistoryEvent::Toast(_)) {self.dirty=true;}
                events.push(GitEvent::History(event));
            }
        }
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
                    ui.label(dim(kiln_common::i18n::tr("저장소 읽는 중…")));
                });
            }
            Some(GitError::NotARepo) => {
                empty_state_icon(ui, Icon::Branch, kiln_common::i18n::tr("Git 저장소 없음"), kiln_common::i18n::tr("이 폴더는 Git으로 추적되고 있지 않습니다."));
                let root = self.root.clone();
                ui.vertical_centered(|ui| {
                    if primary_button(ui, kiln_common::i18n::tr("저장소 초기화"), None).clicked() {
                        let _ = git(&root, Mode::Write, &["init"]);
                        self.refresh();
                    }
                });
            }
            Some(GitError::GitMissing) => {
                empty_state_icon(ui, Icon::Warning, kiln_common::i18n::tr("Git을 찾을 수 없음"), kiln_common::i18n::tr("Git을 설치하고 PATH에 있는지 확인하세요."));
            }
            Some(e) => {
                let msg = e.to_string();
                egui::Frame::new().inner_margin(Margin::same(10)).show(ui, |ui| {
                    banner(ui, BannerKind::Error, kiln_common::i18n::tr("저장소를 읽을 수 없습니다"), Some(&msg), false);
                    ui.add_space(6.0);
                    if tool_button(ui, Some(Icon::Refresh), kiln_common::i18n::tr("다시 시도")).clicked() {
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
        let busy = self.mutation_busy();

        // 브랜치 선택 버튼
        let label = br.display_name();
        let detached = br.is_detached();
        let resp = {
            let w = ui.available_width();
            let (rect, resp) = ui.allocate_exact_size(vec2(w, 34.0), Sense::click());
            let hovered = resp.hovered();
            ui.painter().rect(
                rect,
                CornerRadius::same(8),
                if hovered { t.bg_hover } else { t.bg_elevated },
                Stroke::new(1.0, if hovered { t.border_strong } else { t.border }),
                egui::StrokeKind::Inside,
            );
            let p = ui.painter();
            let chip = Rect::from_center_size(rect.left_center() + vec2(18.0, 0.0), vec2(22.0, 22.0));
            let ic = if detached { t.orange } else { t.accent };
            p.rect_filled(chip, CornerRadius::same(6), alpha(ic, if t.dark { 0.16 } else { 0.12 }));
            paint_icon(p, chip.shrink(4.0), Icon::Branch, ic);
            let name_rect = p.text(
                rect.left_center() + vec2(36.0, 0.0),
                Align2::LEFT_CENTER,
                &label,
                kiln_common::fonts::semibold(13.5),
                t.text,
            );
            if let Some(up) = &br.upstream {
                p.text(
                    pos2(name_rect.right() + 8.0, rect.center().y),
                    Align2::LEFT_CENTER,
                    up.to_string(),
                    FontId::proportional(12.0),
                    t.text_faint,
                );
            } else if detached {
                p.text(pos2(name_rect.right() + 8.0, rect.center().y), Align2::LEFT_CENTER, kiln_common::i18n::tr("분리된 HEAD"), FontId::proportional(12.0), t.orange);
            }
            paint_icon(p, Rect::from_center_size(rect.right_center() - vec2(14.0, 0.0), vec2(12.0, 12.0)), Icon::ChevronDown, t.text_dim);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, kiln_common::trf!("브랜치 {label}")));
            resp.on_hover_text(kiln_common::i18n::tr("브랜치 전환"))
        };
        self.ui_branch_popup(ui, &resp);

        ui.add_space(8.0);
        // 동기화 버튼
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.add_enabled_ui(!busy, |ui| {
                if tool_button(ui, Some(Icon::Refresh), kiln_common::i18n::tr("Fetch")).on_hover_text("git fetch --all --prune").clicked() {
                    self.submit(JobKind::Sync, kiln_common::i18n::tr("Fetch"), repo::fetch);
                }
                let pull_label = if br.behind > 0 { kiln_common::trf!("Pull {}", br.behind) } else { kiln_common::i18n::tr("Pull").into() };
                if tool_button(ui, Some(Icon::ArrowDown), &pull_label).on_hover_text("git pull").clicked() {
                    self.submit(JobKind::Sync, kiln_common::i18n::tr("Pull"), repo::pull);
                }
                let push_label = if br.ahead > 0 {
                    kiln_common::trf!("Push {}", br.ahead)
                } else {
                    kiln_common::i18n::tr("Push").into()
                };
                if tool_button(ui, Some(Icon::ArrowUp), &push_label)
                    .on_hover_text(if br.upstream.is_none() { kiln_common::i18n::tr("원격 브랜치에 Push하고 추적 브랜치로 연결") } else { "git push" })
                    .clicked()
                {
                    self.submit(JobKind::Sync, kiln_common::i18n::tr("Push"), repo::push);
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
                    .stroke(Stroke::new(1.0, t.border_strong))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::same(6))
                    .shadow(t.shadow()),
            )
            .show(|ui| {
                ui.set_width(width - 12.0);
                let focused = ui.memory(|m| m.focused()).is_some();
                let te = egui::TextEdit::singleline(&mut self.picker.filter)
                    .hint_text(kiln_common::i18n::tr("브랜치 필터 또는 새로 만들기…"))
                    .desired_width(f32::INFINITY)
                    .frame(kiln_common::widgets::input_frame(focused, false));
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
                    let resp = branch_row(ui, &kiln_common::trf!("새 브랜치 \"{filter}\" 만들기"), None, false, t.accent);
                    if resp.clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))) {
                        action = Some(BranchAction::Create(filter.clone()));
                    }
                }
                if self.picker.load.is_some() && self.picker.list.is_empty() {
                    ui.horizontal(|ui| {
                        spinner(ui, 12.0);
                        ui.label(dim(kiln_common::i18n::tr("브랜치 불러오는 중…")));
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
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            ui.label(RichText::new(if remote { kiln_common::i18n::tr("원격") } else { kiln_common::i18n::tr("로컬") }).font(kiln_common::fonts::semibold(11.5)).color(t.text_faint));
                        });
                        ui.add_space(2.0);
                        for b in items {
                            let color = if b.current { t.accent } else { t.text };
                            let detail = if !b.track.is_empty() { Some(b.track.as_str()) } else { None };
                            let resp = branch_row(ui, &b.name, detail, b.current, color);
                            if !remote && !b.current && resp.hovered() {
                                let r = Rect::from_center_size(resp.rect.right_center() - vec2(16.0, 0.0), vec2(22.0, 22.0));
                                if icon_button_at(ui, r, resp.id.with("del"), Icon::Trash, kiln_common::i18n::tr("브랜치 삭제")).clicked() {
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
                    self.submit(JobKind::Checkout, kiln_common::i18n::tr("브랜치 만들기"), move |p| repo::create_branch(p, &name));
                }
                BranchAction::Checkout(b) => {
                    self.submit(JobKind::Checkout, kiln_common::i18n::tr("체크아웃"), move |p| repo::checkout(p, &b));
                }
                BranchAction::Delete(name) => {
                    self.confirm = Some(Confirm::DeleteBranch { name, force: false });
                }
            }
        }
    }

    fn ui_banners(&mut self, ui: &mut Ui) {
        let Some(snap) = &self.snap else { return };
        if let Some(history)=&mut self.history {
            ui.add_enabled_ui(!self.worker.as_ref().is_some_and(|w|w.is_busy()),|ui|history.ui_op_banner(ui,true));
        }
        if let Some(op) = snap.op && self.history.is_none() {
            ui.add_space(6.0);
            let conflicts = snap.status.conflicted_count();
            let title = if conflicts > 0 {
                kiln_common::trf!("{} 진행 중 — 충돌 {conflicts}개를 해결하고 커밋하세요", op.label())
            } else {
                kiln_common::trf!("{} 진행 중 — 모든 충돌을 해결했습니다. 커밋할 수 있습니다", op.label())
            };
            let mut abort = false;
            egui::Frame::new()
                .fill(alpha(theme().orange, if theme().dark { 0.10 } else { 0.07 }))
                .stroke(Stroke::new(1.0, alpha(theme().orange, 0.35)))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        icon_label(ui, Icon::Warning, theme().orange, 16.0);
                        ui.add(egui::Label::new(RichText::new(&title).font(kiln_common::fonts::medium(12.5)).color(theme().text)).wrap());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if tool_button(ui, None, kiln_common::i18n::tr("중단")).clicked() {
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
        let busy = self.mutation_busy();
        ui.add_space(10.0);

        let te_id = Id::new(("kiln_git_commit_msg", &self.root));
        let focused = ui.memory(|m| m.has_focus(te_id));
        let submit = focused && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));

        let te = egui::TextEdit::multiline(&mut self.message)
            .id(te_id)
            .hint_text(kiln_common::i18n::tr("커밋 메시지"))
            .desired_rows(3)
            .desired_width(f32::INFINITY)
            .font(FontId::proportional(13.0))
            .frame(kiln_common::widgets::input_frame(focused, false).inner_margin(Margin::symmetric(10, 8)));
        let te_resp = ui.add(te).on_hover_text(kiln_common::trf!("{}로 커밋", shortcut_label(ui.ctx())));

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
            let tip = if subject_len > SUBJECT_MAX { kiln_common::i18n::tr("제목 줄이 72자를 넘습니다") } else { kiln_common::i18n::tr("제목 줄 길이") };
            let r = ui.painter().text(
                te_resp.rect.right_bottom() - vec2(8.0, 5.0),
                Align2::RIGHT_BOTTOM,
                format!("{subject_len}/{SUBJECT_MAX}"),
                FontId::monospace(10.5),
                c,
            );
            ui.interact(r, te_id.with("len"), Sense::hover()).on_hover_text(tip);
        }
        ui.add_space(8.0);

        let can_commit = !busy && conflicts == 0 && (staged > 0 || merging) && !self.message.trim().is_empty();
        let can_amend = !busy && has_head && conflicts == 0;
        let row_w = ui.available_width();
        ui.allocate_ui_with_layout(vec2(row_w, 30.0), Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let push_clicked = ui
                .add_enabled_ui(can_commit, |ui| secondary_button(ui, kiln_common::i18n::tr("커밋 후 Push")))
                .inner
                .on_hover_text(kiln_common::i18n::tr("커밋한 뒤 upstream 브랜치로 Push"))
                .clicked();
            let amend_clicked = ui
                .add_enabled_ui(can_amend, |ui| secondary_button(ui, kiln_common::i18n::tr("커밋 수정")))
                .inner
                .on_hover_text(kiln_common::i18n::tr("마지막 커밋 수정 (입력란이 비어 있으면 기존 메시지 유지)"))
                .clicked();
            let w = ui.available_width().max(60.0);
            let label = if staged > 0 { kiln_common::trf!("커밋 ({staged})") } else { kiln_common::i18n::tr("커밋").into() };
            let r = ui.add_enabled_ui(can_commit, |ui| primary_button(ui, &label, Some(w))).inner;
            let hint = if conflicts > 0 {
                kiln_common::i18n::tr("먼저 병합 충돌을 해결하세요")
            } else if staged == 0 && !merging {
                kiln_common::i18n::tr("커밋할 변경 사항을 스테이징하세요")
            } else if self.message.trim().is_empty() {
                kiln_common::i18n::tr("커밋 메시지를 입력하세요")
            } else {
                kiln_common::i18n::tr("스테이징된 변경 사항 커밋")
            };
            let clicked = r.on_hover_text(hint).on_disabled_hover_text(hint).clicked();
            if (clicked || submit) && can_commit {
                let msg = self.message.clone();
                self.submit(JobKind::Commit, kiln_common::i18n::tr("커밋"), move |p| repo::commit(p, &msg, false));
            }
            if amend_clicked {
                let msg = self.message.clone();
                self.submit(JobKind::Commit, kiln_common::i18n::tr("커밋 수정"), move |p| repo::commit(p, &msg, true));
            }
            if push_clicked {
                let msg = self.message.clone();
                self.submit(JobKind::Commit, kiln_common::i18n::tr("커밋 후 Push"), move |p| {
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
        let busy = self.mutation_busy();
        let t = theme();

        if !conflicts.is_empty() {
            let mut open = self.open[&Section::Conflicts];
            section_header(ui, &mut open, kiln_common::i18n::tr("병합 충돌"), Some(conflicts.len()), Some(t.orange), |_| {});
            self.open.insert(Section::Conflicts, open);
            if open {
                for e in &conflicts {
                    self.file_row(ui, e, RowKind::Conflict, busy, events);
                }
            }
        }

        if !staged.is_empty() {
            let mut open = self.open[&Section::Staged];
            let mut unstage_all = false;
            section_header(ui, &mut open, kiln_common::i18n::tr("스테이징된 변경 사항"), Some(staged.len()), None, |ui| {
                if !staged.is_empty() && icon_button(ui, Icon::Minus, kiln_common::i18n::tr("모두 스테이징 취소")).clicked() {
                    unstage_all = true;
                }
            });
            self.open.insert(Section::Staged, open);
            if unstage_all {
                self.mutate(kiln_common::i18n::tr("모두 스테이징 취소"), repo::unstage_all);
            }
            if open {
                for e in &staged {
                    self.file_row(ui, e, RowKind::Staged, busy, events);
                }
            }
        }

        if !changes.is_empty() {
            let mut open = self.open[&Section::Changes];
            let mut stage_all = false;
            let mut discard_all = false;
            section_header(ui, &mut open, kiln_common::i18n::tr("변경 사항"), Some(changes.len()), None, |ui| {
                if !changes.is_empty() {
                    if icon_button(ui, Icon::Plus, kiln_common::i18n::tr("모든 변경 사항 스테이징")).clicked() {
                        stage_all = true;
                    }
                    if icon_button(ui, Icon::Discard, kiln_common::i18n::tr("스테이징 전 변경 버리기")).clicked() {
                        discard_all = true;
                    }
                }
            });
            self.open.insert(Section::Changes, open);
            if stage_all {
                let paths: Vec<String> = changes.iter().map(|e| e.path.clone()).collect();
                self.mutate(kiln_common::i18n::tr("모두 스테이징"), move |p| repo::stage(p, &paths));
            }
            if discard_all {
                self.confirm = Some(Confirm::DiscardAll);
            }
            if open {
                for e in &changes {
                    self.file_row(ui, e, RowKind::Changed, busy, events);
                }
            }
        }

        if !untracked.is_empty() {
            let mut open = self.open[&Section::Untracked];
            let mut stage_u = false;
            section_header(ui, &mut open, kiln_common::i18n::tr("새 파일"), Some(untracked.len()), None, |ui| {
                if icon_button(ui, Icon::Plus, kiln_common::i18n::tr("추적되지 않은 파일 모두 스테이징")).clicked() {
                    stage_u = true;
                }
            });
            self.open.insert(Section::Untracked, open);
            if stage_u {
                let paths: Vec<String> = untracked.iter().map(|e| e.path.clone()).collect();
                self.mutate(kiln_common::i18n::tr("추적되지 않은 파일 스테이징"), move |p| repo::stage(p, &paths));
            }
            if open {
                for e in &untracked {
                    self.file_row(ui, e, RowKind::Untracked, busy, events);
                }
            }
        }

        let dirty_tree = !staged.is_empty() || !changes.is_empty() || !untracked.is_empty();
        if !dirty_tree && conflicts.is_empty() {
            hint_row(ui, kiln_common::i18n::tr("변경 사항 없음"));
        }

        // Existing stashes retain their list; creating the first stash needs only an action.
        let mut open = self.open[&Section::Stashes];
        let mut new_stash = false;
        let can_stash = dirty_tree && conflicts.is_empty() && !busy;
        if !snap.stashes.is_empty() {
            section_header(ui, &mut open, kiln_common::i18n::tr("스태시"), Some(snap.stashes.len()), None, |ui| {
                if can_stash && icon_button(ui, Icon::Plus, kiln_common::i18n::tr("변경 사항 스태시…")).clicked() {
                    new_stash = true;
                }
            });
        } else if dirty_tree && self.stash_input.is_none() {
            egui::Frame::new().inner_margin(Margin { left: 12, right: 12, top: 0, bottom: 0 }).show(ui, |ui| {
                new_stash = ui.add_enabled_ui(can_stash, |ui| tool_button(ui, Some(Icon::Stash), kiln_common::i18n::tr("스태시 만들기"))).inner.clicked();
            });
        }
        if new_stash {
            open = true;
            self.stash_input = Some(String::new());
        }
        self.open.insert(Section::Stashes, open);
        if open && (!snap.stashes.is_empty() || self.stash_input.is_some()) {
            self.ui_stashes(ui, &snap.stashes);
        }

        // 커밋 로그
        let mut open = self.open[&Section::Commits];
        section_header(ui, &mut open, kiln_common::i18n::tr("커밋 기록"), None, None, |ui| {
            if icon_button(ui, Icon::Open, kiln_common::i18n::tr("커밋 기록 크게 보기"))
                .on_hover_text(kiln_common::i18n::tr("같은 기록을 넓은 탭에서 보기"))
                .clicked()
            {
                events.push(GitEvent::OpenHistory);
            }
        });
        self.open.insert(Section::Commits, open);
        if open {
            let root=self.root.clone();
            let history=self.history.get_or_insert_with(||super::history_view::HistoryView::new(root));
            if let Some(now)=self.now_override {history.set_now(now);}
            let busy=self.worker.as_ref().is_some_and(|w|w.is_busy());
            ui.add_enabled_ui(!busy,|ui|history.ui_embedded(ui));
        }
        ui.add_space(12.0);
    }

    fn ui_stashes(&mut self, ui: &mut Ui, stashes: &[StashEntry]) {
        let t = theme();
        if let Some(mut msg) = self.stash_input.take() {
            let mut keep = true;
            egui::Frame::new().inner_margin(Margin { left: 12, right: 12, top: 4, bottom: 6 }).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut msg)
                            .hint_text(kiln_common::i18n::tr("스태시 메시지 (선택 사항)"))
                            .desired_width(ui.available_width() - 110.0)
                            .frame(input_frame()),
                    );
                    if !r.has_focus() && msg.is_empty() {
                        r.request_focus();
                    }
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    if tool_button(ui, None, kiln_common::i18n::tr("스태시")).clicked() || enter {
                        let m = msg.clone();
                        self.submit(JobKind::Mutate, kiln_common::i18n::tr("스태시"), move |p| repo::stash_push(p, &m));
                        keep = false;
                    }
                    if icon_button(ui, Icon::Close, kiln_common::i18n::tr("취소")).clicked() {
                        keep = false;
                    }
                });
            });
            if keep {
                self.stash_input = Some(msg);
            }
        }
        if stashes.is_empty() {
            return;
        }
        let now = self.now();
        for s in stashes {
            let w = ui.available_width();
            let (rect, resp) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::click());
            let hovered = resp.hovered() || ui.rect_contains_pointer(rect);
            row_bg(ui, rect, false, hovered);
            let p = ui.painter();
            paint_icon(p, Rect::from_center_size(rect.left_center() + vec2(24.0, 0.0), vec2(14.0, 14.0)), Icon::Stash, t.text_faint);
            let msg = s.message.split_once(": ").map(|(_, m)| m).unwrap_or(&s.message);
            let right_reserved = if hovered { 80.0 } else { 70.0 };
            let job = one_line_job(
                &[(msg, 12.5, t.text), (&format!("  {}", relative_time(s.date, now)), 11.5, t.text_faint)],
                w - 34.0 - right_reserved,
            );
            let g = p.layout_job(job);
            p.galley(pos2(rect.left() + 34.0, rect.center().y - g.size().y / 2.0), g, t.text);
            p.text(rect.right_center() - vec2(14.0, 0.0), Align2::RIGHT_CENTER, &s.reference, FontId::monospace(11.0), t.text_faint);
            if hovered {
                let mut x = rect.right() - 10.0;
                let mut btn = |ui: &mut Ui, icon: Icon, tip: &str| {
                    let r = Rect::from_center_size(pos2(x - 11.0, rect.center().y), vec2(22.0, 22.0));
                    x -= 24.0;
                    icon_button_at(ui, r, resp.id.with(tip), icon, tip).clicked()
                };
                ui.painter().rect_filled(
                    Rect::from_min_max(pos2(rect.right() - 84.0, rect.top() + 1.0), rect.max - vec2(4.0, 1.0)),
                    CornerRadius::same(6),
                    t.bg_hover,
                );
                let idx = s.index;
                if btn(ui, Icon::Trash, kiln_common::i18n::tr("스태시 삭제")) {
                    self.confirm = Some(Confirm::DropStash { index: idx, message: msg.to_string() });
                }
                if btn(ui, Icon::Pop, kiln_common::i18n::tr("스태시 팝 (적용 후 삭제)")) {
                    self.submit(JobKind::Mutate, kiln_common::i18n::tr("스태시 팝"), move |p| repo::stash_pop(p, idx));
                }
                if btn(ui, Icon::Apply, kiln_common::i18n::tr("스태시 적용")) {
                    self.submit(JobKind::Mutate, kiln_common::i18n::tr("스태시 적용"), move |p| repo::stash_apply(p, idx));
                }
            }
        }
    }

    fn file_row(&mut self, ui: &mut Ui, e: &StatusEntry, kind: RowKind, busy: bool, events: &mut Vec<GitEvent>) {
        let t = theme();
        let w = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::click());
        let label = format!("{}{}", if kind == RowKind::Staged { kiln_common::i18n::tr("스테이징됨: ") } else { "" }, e.path);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
        if !ui.is_rect_visible(rect) {
            return;
        }
        let staged = kind == RowKind::Staged;
        let is_sel = self.selected.as_ref().is_some_and(|(p, s)| p == &e.path && *s == staged);
        let hovered = resp.hovered() || ui.rect_contains_pointer(rect);
        row_bg(ui, rect, is_sel, hovered);
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
        let right_pad = if hovered { 100.0 } else { 34.0 };
        let mut parts: Vec<(&str, f32, Color32)> = Vec::new();
        let name_color = if letter == 'D' { t.text_dim } else { t.text };
        parts.push((&name, 13.0, name_color));
        let dir_s;
        if let Some(orig) = &e.orig_path {
            dir_s = kiln_common::trf!("  {orig}에서");
            parts.push((&dir_s, 11.5, t.text_faint));
        } else if !dir.is_empty() {
            dir_s = format!("  {dir}");
            parts.push((&dir_s, 11.5, t.text_faint));
        }
        let sub_s;
        if e.submodule {
            sub_s = kiln_common::i18n::tr("  서브모듈").to_string();
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
        p.galley(pos2(rect.left() + 24.0, rect.center().y - g.size().y / 2.0), g, t.text);
        let chip = Rect::from_center_size(rect.right_center() - vec2(20.0, 0.0), vec2(20.0, 18.0));
        p.rect_filled(chip, CornerRadius::same(5), alpha(color, if t.dark { 0.14 } else { 0.11 }));
        p.text(chip.center(), Align2::CENTER_CENTER, letter, kiln_common::fonts::semibold(11.0), color);

        let abs = self.abs_path(&e.path);
        let mut handled = false;
        if hovered {
            let mut x = rect.right() - 34.0;
            let row_id = resp.id;
            let mut btn = |ui: &mut Ui, icon: Icon, tip: &str| -> bool {
                let r = Rect::from_center_size(pos2(x - 11.0, rect.center().y), vec2(22.0, 22.0));
                x -= 24.0;
                ui.add_enabled_ui(!busy, |ui| icon_button_at(ui, r, row_id.with(tip), icon, tip)).inner.clicked()
            };
            let paths = vec![e.path.clone()];
            match kind {
                RowKind::Staged => {
                    if btn(ui, Icon::Minus, kiln_common::i18n::tr("스테이징 취소")) {
                        handled = true;
                        self.mutate(kiln_common::i18n::tr("스테이징 취소"), move |p| repo::unstage(p, &paths));
                    }
                }
                RowKind::Changed => {
                    if btn(ui, Icon::Plus, kiln_common::i18n::tr("스테이징")) {
                        handled = true;
                        let ps = paths.clone();
                        self.mutate(kiln_common::i18n::tr("스테이징"), move |p| repo::stage(p, &ps));
                    }
                    if btn(ui, Icon::Discard, kiln_common::i18n::tr("변경 버리기")) {
                        handled = true;
                        self.confirm = Some(Confirm::Discard { paths: paths.clone(), untracked: false });
                    }
                }
                RowKind::Untracked => {
                    if btn(ui, Icon::Plus, kiln_common::i18n::tr("스테이징")) {
                        handled = true;
                        let ps = paths.clone();
                        self.mutate(kiln_common::i18n::tr("스테이징"), move |p| repo::stage(p, &ps));
                    }
                    if btn(ui, Icon::Trash, kiln_common::i18n::tr("파일 삭제")) {
                        handled = true;
                        self.confirm = Some(Confirm::Discard { paths: paths.clone(), untracked: true });
                    }
                }
                RowKind::Conflict => {
                    if btn(ui, Icon::Check, kiln_common::i18n::tr("해결됨으로 표시 (스테이징)")) {
                        handled = true;
                        let ps = paths.clone();
                        self.mutate(kiln_common::i18n::tr("해결됨으로 표시"), move |p| repo::stage(p, &ps));
                    }
                }
            }
            if btn(ui, Icon::Open, kiln_common::i18n::tr("파일 열기")) {
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
            if ui.button(kiln_common::i18n::tr("파일 열기")).clicked() {
                events.push(GitEvent::OpenFile(abs.clone()));
                ui.close();
            }
            if kind != RowKind::Conflict && kind != RowKind::Untracked && ui.button(kiln_common::i18n::tr("변경 사항 열기")).clicked() {
                events.push(GitEvent::OpenDiff { path: abs.clone(), staged });
                ui.close();
            }
            ui.separator();
            match kind {
                RowKind::Conflict => {
                    if ui.button(kiln_common::i18n::tr("현재 변경 수락 (ours)")).clicked() {
                        let p2 = path_owned.clone();
                        self.mutate(kiln_common::i18n::tr("현재 변경 수락"), move |p| repo::resolve_conflict(p, &p2, true));
                        ui.close();
                    }
                    if ui.button(kiln_common::i18n::tr("수신 변경 수락 (theirs)")).clicked() {
                        let p2 = path_owned.clone();
                        self.mutate(kiln_common::i18n::tr("수신 변경 수락"), move |p| repo::resolve_conflict(p, &p2, false));
                        ui.close();
                    }
                    if ui.button(kiln_common::i18n::tr("해결됨으로 표시")).clicked() {
                        let ps = vec![path_owned.clone()];
                        self.mutate(kiln_common::i18n::tr("해결됨으로 표시"), move |p| repo::stage(p, &ps));
                        ui.close();
                    }
                }
                RowKind::Staged => {
                    if ui.button(kiln_common::i18n::tr("스테이징 취소")).clicked() {
                        let ps = vec![path_owned.clone()];
                        self.mutate(kiln_common::i18n::tr("스테이징 취소"), move |p| repo::unstage(p, &ps));
                        ui.close();
                    }
                }
                RowKind::Changed | RowKind::Untracked => {
                    if ui.button(kiln_common::i18n::tr("스테이징")).clicked() {
                        let ps = vec![path_owned.clone()];
                        self.mutate(kiln_common::i18n::tr("스테이징"), move |p| repo::stage(p, &ps));
                        ui.close();
                    }
                    if ui.button(if kind == RowKind::Untracked { kiln_common::i18n::tr("파일 삭제…") } else { kiln_common::i18n::tr("변경 사항 취소…") }).clicked() {
                        self.confirm =
                            Some(Confirm::Discard { paths: vec![path_owned.clone()], untracked: kind == RowKind::Untracked });
                        ui.close();
                    }
                }
            }
            ui.separator();
            if ui.button(kiln_common::i18n::tr("경로 복사")).clicked() {
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
                kiln_common::i18n::tr("선택한 파일의 변경을 버릴까요?").to_string(),
                kiln_common::trf!("{}의 변경 사항이 사라집니다. 이 작업은 되돌릴 수 없습니다.", paths.join(", ")),
                kiln_common::i18n::tr("변경 버리기"),
                true,
            ),
            Confirm::Discard { paths, untracked: true } => (
                kiln_common::i18n::tr("추적되지 않은 파일을 삭제할까요?").to_string(),
                kiln_common::trf!("다음 파일을 영구적으로 삭제합니다: {}", paths.join(", ")),
                kiln_common::i18n::tr("삭제"),
                true,
            ),
            Confirm::DiscardAll => (
                kiln_common::i18n::tr("스테이징 전 변경을 버릴까요?").to_string(),
                kiln_common::i18n::tr("추적 중인 파일의 스테이징되지 않은 변경 사항이 모두 사라집니다. 추적되지 않은 파일은 유지됩니다.").to_string(),
                kiln_common::i18n::tr("변경 버리기"),
                true,
            ),
            Confirm::DeleteBranch { name, .. } => (
                kiln_common::trf!("\"{name}\" 브랜치를 삭제할까요?"),
                kiln_common::i18n::tr("로컬 브랜치가 삭제됩니다. 강제로 삭제하면 다른 곳에 병합되지 않은 커밋을 잃을 수 있습니다.").to_string(),
                kiln_common::i18n::tr("브랜치 삭제"),
                true,
            ),
            Confirm::DropStash { message, .. } => (
                kiln_common::i18n::tr("스태시를 삭제할까요?").to_string(),
                kiln_common::trf!("다음 스태시를 영구적으로 삭제합니다: \"{message}\""),
                kiln_common::i18n::tr("삭제"),
                true,
            ),
            Confirm::AbortOp(op) => (
                kiln_common::trf!("{} 작업을 중단할까요?", op.label()),
                kiln_common::i18n::tr("저장소가 작업 시작 전 상태로 돌아갑니다.").to_string(),
                kiln_common::i18n::tr("중단"),
                true,
            ),
        };
        let mut force = matches!(c, Confirm::DeleteBranch { force: true, .. });
        let is_branch = matches!(c, Confirm::DeleteBranch { .. });
        let r = confirm_modal(ctx, id, &title, &msg, ok, danger, |ui| {
            if is_branch {
                ui.add_space(8.0);
                checkbox_row(ui, &mut force, kiln_common::i18n::tr("병합되지 않았어도 강제 삭제 (-D)"));
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
                            self.mutate(kiln_common::i18n::tr("삭제"), move |p| repo::clean_untracked(p, &paths));
                        } else {
                            self.mutate(kiln_common::i18n::tr("변경 버리기"), move |p| repo::discard(p, &paths));
                        }
                    }
                    Confirm::DiscardAll => {
                        self.mutate(kiln_common::i18n::tr("변경 버리기"), |p| repo::discard(p, &[".".to_string()]));
                    }
                    Confirm::DeleteBranch { name, .. } => {
                        self.submit(JobKind::Mutate, kiln_common::i18n::tr("브랜치 삭제"), move |p| repo::delete_branch(p, &name, force));
                    }
                    Confirm::DropStash { index, .. } => {
                        self.submit(JobKind::Mutate, kiln_common::i18n::tr("스태시 삭제"), move |p| repo::stash_drop(p, index));
                    }
                    Confirm::AbortOp(op) => {
                        self.submit(JobKind::Mutate, kiln_common::i18n::tr("중단"), move |p| repo::abort_op(p, op));
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

/// 목록 행 배경(좌우를 조금 들인 둥근 사각형).
fn row_bg(ui: &Ui, rect: Rect, selected: bool, hovered: bool) {
    kiln_common::widgets::paint_row(ui.painter(), rect.shrink2(vec2(6.0, 1.0)), selected, hovered);
}

fn hint_row(ui: &mut Ui, text: &str) {
    let w = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text));
    ui.painter().text(rect.left_center() + vec2(24.0, 0.0), Align2::LEFT_CENTER, text, FontId::proportional(12.5), theme().text_faint);
}

fn branch_row(ui: &mut Ui, name: &str, detail: Option<&str>, current: bool, color: Color32) -> egui::Response {
    let t = theme();
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
    kiln_common::widgets::paint_row(ui.painter(), rect, current, resp.hovered());
    let p = ui.painter();
    if current {
        paint_icon(p, Rect::from_center_size(rect.left_center() + vec2(12.0, 0.0), vec2(12.0, 12.0)), Icon::Check, t.accent);
    }
    let mut parts: Vec<(&str, f32, Color32)> = vec![(name, 13.0, color)];
    let d;
    if let Some(x) = detail {
        d = format!("  {x}");
        parts.push((&d, 11.0, t.text_faint));
    }
    let g = p.layout_job(one_line_job(&parts, w - 50.0));
    p.galley(pos2(rect.left() + 26.0, rect.center().y - g.size().y / 2.0), g, t.text);
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

/// 커밋 단축키 표기(macOS 는 ⌘Enter, 그 외 Ctrl+Enter).
fn shortcut_label(ctx: &egui::Context) -> &'static str {
    if ctx.os() == egui::os::OperatingSystem::Mac { "⌘Enter" } else { "Ctrl+Enter" }
}

#[cfg(test)]
mod pending_commit_draft_tests {
    use super::*;
    #[test]
    fn successful_delayed_commit_preserves_newer_message() {
        for edit_while_sending in [false,true] {
            let ctx=egui::Context::default();let mut panel=GitPanel::new(PathBuf::from("."));
            panel.worker=Some(Worker::new(&ctx));panel.last_load=Some(Instant::now());panel.dirty=false;
            panel.message="submitted message".into();
            let (tx,rx)=std::sync::mpsc::channel();
            panel.submit(JobKind::Commit,"mock commit",move |_|{rx.recv().unwrap();Ok(String::new())});
            if edit_while_sending {panel.message="next commit message".into();}
            tx.send(()).unwrap();
            let deadline=Instant::now()+Duration::from_secs(2);
            while panel.submitted_message.is_some() && Instant::now()<deadline {panel.pump(&ctx);std::thread::sleep(Duration::from_millis(1));}
            assert!(panel.submitted_message.is_none());
            assert_eq!(panel.message,if edit_while_sending {"next commit message"}else{""});
        }
    }
}

#[cfg(test)]
mod concise_sections_tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};
    fn panel(status: Status) -> GitPanel {
        let mut panel = GitPanel::new(PathBuf::from("/workspace"));
        panel.snap = Some(Snapshot { top: panel.root.clone(), prefix: String::new(), status,
            op: None, stashes: vec![] });
        panel
    }
    #[test]
    fn untracked_files_are_not_reported_as_a_clean_tree_and_stash_remains_available() {
        let mut h = Harness::builder().with_size([350., 600.]).build_ui_state(
            |ui, panel: &mut GitPanel| {
                kiln_common::fonts::install(ui.ctx());
                if ui.ctx().fonts(|f| f.families().contains(&egui::FontFamily::Name(kiln_common::fonts::SEMIBOLD.into()))) {
                    panel.ui_sections(ui, &mut vec![]);
                }
            },
            panel(crate::status::parse_status_v2(b"? src/lib.rs\0")));
        h.run_steps(3);
        h.get_by_label("새 파일");
        assert!(h.query_by_label("변경 사항 없음").is_none());
        assert!(h.query_by_label("스테이징된 변경 사항").is_none());
        assert!(h.query_by_label("변경 사항").is_none());
        assert!(h.query_by_label("스태시").is_none());
        h.get_by_label("스태시 만들기").click();
        h.run_steps(2);
        assert!(h.state().stash_input.is_some());
        h.get_by_label("취소").click();
        h.run_steps(2);
        assert!(h.state().stash_input.is_none());
    }
    #[test]
    fn clean_tree_has_one_summary_without_empty_categories() {
        let mut h = Harness::builder().with_size([350., 600.]).build_ui_state(
            |ui, panel: &mut GitPanel| {
                kiln_common::fonts::install(ui.ctx());
                if ui.ctx().fonts(|f| f.families().contains(&egui::FontFamily::Name(kiln_common::fonts::SEMIBOLD.into()))) {
                    panel.ui_sections(ui, &mut vec![]);
                }
            }, panel(Status::default()));
        h.run_steps(3);
        h.get_by_label("변경 사항 없음");
        for label in ["스테이징된 변경 사항", "변경 사항", "새 파일", "스태시", "스태시 만들기"] {
            assert!(h.query_by_label(label).is_none(), "unexpected empty category: {label}");
        }
    }
}
