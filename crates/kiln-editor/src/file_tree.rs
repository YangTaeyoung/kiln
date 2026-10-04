//! 지연 로딩 파일 탐색기. .gitignore 존중, 파일 감시로 펼쳐진 폴더만 다시 읽는다.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use egui::{
    Align2, Color32, CursorIcon, Event, EventFilter, FontId, Id, Key, Rect, Response, ScrollArea,
    Sense, Stroke, Ui, pos2, vec2,
};
use kiln_common::Theme;
use notify::{RecursiveMode, Watcher};
use parking_lot::Mutex;

use crate::EditorEvent;
use crate::ui_kit::{self, Icon};

/// 앱이 지정하는 파일 장식(예: git 상태 색과 배지 글자).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decoration {
    pub color: Color32,
    pub badge: Option<char>,
}

/// 폴더의 한 항목.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    /// .gitignore 등으로 숨겨지는 항목(표시 옵션이 켜졌을 때만 목록에 나온다).
    pub ignored: bool,
}

/// 이름을 대소문자 무시·숫자 크기 순으로 비교한다.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut ai, mut bi) = (a.char_indices().peekable(), b.char_indices().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some((_, ca)), Some((_, cb))) if ca.is_ascii_digit() && cb.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(&(_, c)) = ai.peek().filter(|(_, c)| c.is_ascii_digit()) {
                    na.push(c);
                    ai.next();
                }
                let mut nb = String::new();
                while let Some(&(_, c)) = bi.peek().filter(|(_, c)| c.is_ascii_digit()) {
                    nb.push(c);
                    bi.next();
                }
                let ta = na.trim_start_matches('0');
                let tb = nb.trim_start_matches('0');
                let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some((_, ca)), Some((_, cb))) => {
                let o = ca.to_lowercase().cmp(cb.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                ai.next();
                bi.next();
            }
        }
    }
}

/// 폴더 하나를 읽는다. 폴더 먼저, 이름 순. `show_ignored` 가 거짓이면 무시 대상은 뺀다.
pub fn list_dir(dir: &Path, show_ignored: bool) -> std::io::Result<Vec<DirEntry>> {
    let mut visible: HashSet<OsString> = HashSet::new();
    let walker = ignore::WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .ignore(true)
        .parents(true)
        .follow_links(false)
        .build();
    for e in walker.flatten() {
        if e.depth() == 1 {
            visible.insert(e.file_name().to_owned());
        }
    }
    let mut out = Vec::new();
    for de in std::fs::read_dir(dir)? {
        let Ok(de) = de else { continue };
        let name_os = de.file_name();
        let name = name_os.to_string_lossy().into_owned();
        let ignored = name == ".git" || name == ".DS_Store" || !visible.contains(&name_os);
        if ignored && !show_ignored {
            continue;
        }
        let path = de.path();
        let is_dir = match de.file_type() {
            Ok(ft) if ft.is_symlink() => path.is_dir(),
            Ok(ft) => ft.is_dir(),
            Err(_) => false,
        };
        out.push(DirEntry { path, name, is_dir, ignored });
    }
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| natural_cmp(&a.name, &b.name)));
    Ok(out)
}

#[derive(Clone, Debug)]
enum RowKind {
    Entry(DirEntry),
    NewEntry { is_dir: bool },
}

#[derive(Clone, Debug)]
struct Row {
    depth: usize,
    kind: RowKind,
}

#[derive(Debug)]
enum Action {
    Click(PathBuf, bool),
    Select(PathBuf),
    NewEntry(PathBuf, bool),
    Rename(PathBuf),
    Delete(PathBuf),
    CopyPath(PathBuf),
    CopyRelative(PathBuf),
    Terminal(PathBuf),
    Refresh,
    CollapseAll,
    ToggleIgnored,
}

struct RenameState {
    path: PathBuf,
    text: String,
    focus: bool,
}

struct CreateState {
    parent: PathBuf,
    is_dir: bool,
    text: String,
    focus: bool,
}

struct FsWatch {
    watcher: notify::RecommendedWatcher,
    rx: Receiver<notify::Result<notify::Event>>,
    recursive: bool,
    watched: HashSet<PathBuf>,
}

const ROW_H: f32 = 28.0;
const HEADER_H: f32 = 36.0;
const INDENT: f32 = 14.0;
/// 감시 이벤트 디바운스(초, egui 시각 기준).
const DEBOUNCE_QUIET: f64 = 0.08;
const DEBOUNCE_MAX: f64 = 0.3;

/// 프로젝트 파일 트리 위젯.
pub struct FileTree {
    root: PathBuf,
    canon_root: PathBuf,
    id: Id,
    dirs: HashMap<PathBuf, Vec<DirEntry>>,
    expanded: HashSet<PathBuf>,
    rows: Vec<Row>,
    rows_dirty: bool,
    selected: Option<PathBuf>,
    focus_pending: bool,
    show_ignored: bool,
    decorations: HashMap<PathBuf, Decoration>,
    dir_decor: HashMap<PathBuf, Color32>,
    rename: Option<RenameState>,
    create: Option<CreateState>,
    confirm_delete: Option<PathBuf>,
    scroll_to_selected: bool,
    scroll_y: f32,
    view_h: f32,
    ctx: Arc<Mutex<Option<egui::Context>>>,
    watch: Option<FsWatch>,
    watch_failed: bool,
    pending: HashSet<PathBuf>,
    pending_first: Option<f64>,
    pending_last: Option<f64>,
    reload_all_pending: bool,
    error: Option<(String, Instant)>,
    use_trash: bool,
    /// 마지막으로 파일 감시로 다시 읽은 폴더들(테스트·진단용).
    last_reloaded: Vec<PathBuf>,
    pending_open: Option<PathBuf>,
    pending_rename: Option<(PathBuf, PathBuf)>,
}

impl FileTree {
    pub fn new(root: PathBuf) -> Self {
        crate::syntax::prewarm();
        let canon_root = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
        let mut t = Self {
            id: Id::new(("kiln-file-tree", root.clone())),
            root,
            canon_root,
            dirs: HashMap::new(),
            expanded: HashSet::new(),
            rows: Vec::new(),
            rows_dirty: true,
            selected: None,
            focus_pending: false,
            show_ignored: false,
            decorations: HashMap::new(),
            dir_decor: HashMap::new(),
            rename: None,
            create: None,
            confirm_delete: None,
            scroll_to_selected: false,
            scroll_y: 0.0,
            view_h: 0.0,
            ctx: Arc::new(Mutex::new(None)),
            watch: None,
            watch_failed: false,
            pending: HashSet::new(),
            pending_first: None,
            pending_last: None,
            reload_all_pending: false,
            error: None,
            use_trash: true,
            last_reloaded: Vec::new(),
            pending_open: None,
            pending_rename: None,
        };
        let root = t.root.clone();
        t.load_dir(&root);
        t.expanded.insert(root);
        t
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Focus the visible tree once when its host explicitly opens the file view.
    pub fn request_focus(&mut self) {
        self.focus_pending = true;
    }

    /// 루트를 바꾸고 모든 상태를 초기화한다.
    pub fn set_root(&mut self, root: PathBuf) {
        let decorations = std::mem::take(&mut self.decorations);
        let show_ignored = self.show_ignored;
        let use_trash = self.use_trash;
        let ctx = self.ctx.lock().clone();
        *self = Self::new(root);
        self.show_ignored = show_ignored;
        self.use_trash = use_trash;
        *self.ctx.lock() = ctx;
        if show_ignored {
            self.refresh();
        }
        self.set_decorations(decorations);
    }

    /// 읽어 둔 모든 폴더를 다시 읽는다(펼침 상태 유지).
    pub fn refresh(&mut self) {
        let loaded: Vec<PathBuf> = self.dirs.keys().cloned().collect();
        for d in loaded {
            self.reload_dir(&d);
        }
        self.rows_dirty = true;
    }

    /// 장식 목록을 바꾼다. 장식된 항목의 상위 폴더에도 색 점이 표시된다.
    pub fn set_decorations(&mut self, decorations: HashMap<PathBuf, Decoration>) {
        let mut dir_decor = HashMap::new();
        let mut sorted: Vec<(&PathBuf, &Decoration)> = decorations.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(b.0));
        for (p, d) in sorted {
            let mut cur = p.parent();
            while let Some(dir) = cur {
                if !dir.starts_with(&self.root) || dir == self.root {
                    break;
                }
                dir_decor.entry(dir.to_path_buf()).or_insert(d.color);
                cur = dir.parent();
            }
        }
        self.decorations = decorations;
        self.dir_decor = dir_decor;
    }

    /// 경로의 상위 폴더를 모두 펼치고 선택한 뒤 보이도록 스크롤한다.
    pub fn reveal(&mut self, path: &Path) {
        let Ok(rel) = path.strip_prefix(&self.root) else { return };
        let mut cur = self.root.clone();
        let comps: Vec<_> = rel.components().collect();
        for c in comps.iter().take(comps.len().saturating_sub(1)) {
            cur.push(c);
            if !self.dirs.contains_key(&cur) {
                self.load_dir(&cur);
            }
            self.expanded.insert(cur.clone());
        }
        self.selected = Some(path.to_path_buf());
        self.scroll_to_selected = true;
        self.rows_dirty = true;
    }

    /// 선택된 경로.
    pub fn selected(&self) -> Option<&Path> {
        self.selected.as_deref()
    }

    /// 무시된(.gitignore) 항목 표시 여부.
    pub fn set_show_ignored(&mut self, show: bool) {
        if self.show_ignored != show {
            self.show_ignored = show;
            self.refresh();
        }
    }

    pub fn show_ignored(&self) -> bool {
        self.show_ignored
    }

    /// 삭제를 휴지통 대신 영구 삭제로 할지 정한다. 기본값은 휴지통.
    pub fn set_use_trash(&mut self, use_trash: bool) {
        self.use_trash = use_trash;
    }

    /// 폴더가 펼쳐져 있는지.
    pub fn is_expanded(&self, dir: &Path) -> bool {
        self.expanded.contains(dir)
    }

    /// 폴더를 펼치거나 접는다.
    pub fn set_expanded(&mut self, dir: &Path, expanded: bool) {
        if expanded {
            if !self.dirs.contains_key(dir) {
                self.load_dir(dir);
            }
            self.expanded.insert(dir.to_path_buf());
        } else {
            self.expanded.remove(dir);
        }
        self.rows_dirty = true;
    }

    /// 현재 화면에 보이는 항목 경로들(위에서 아래 순).
    pub fn visible_paths(&mut self) -> Vec<PathBuf> {
        self.rebuild_rows();
        self.rows
            .iter()
            .filter_map(|r| match &r.kind {
                RowKind::Entry(e) => Some(e.path.clone()),
                RowKind::NewEntry { .. } => None,
            })
            .collect()
    }

    /// 읽어 둔 폴더 목록.
    pub fn loaded_dirs(&self) -> Vec<PathBuf> {
        let mut v: Vec<_> = self.dirs.keys().cloned().collect();
        v.sort();
        v
    }

    /// 마지막 파일 감시 반영에서 다시 읽은 폴더들.
    pub fn last_reloaded(&self) -> &[PathBuf] {
        &self.last_reloaded
    }

    fn load_dir(&mut self, dir: &Path) {
        match list_dir(dir, self.show_ignored) {
            Ok(entries) => {
                self.dirs.insert(dir.to_path_buf(), entries);
                self.watch_dir(dir);
            }
            Err(e) => {
                self.dirs.insert(dir.to_path_buf(), Vec::new());
                self.set_error(format!("읽을 수 없습니다: {} · {e}", dir.display()));
            }
        }
        self.rows_dirty = true;
    }

    fn reload_dir(&mut self, dir: &Path) {
        match list_dir(dir, self.show_ignored) {
            Ok(entries) => {
                // 사라진 하위 폴더의 캐시를 버린다.
                let alive: HashSet<&Path> = entries.iter().filter(|e| e.is_dir).map(|e| e.path.as_path()).collect();
                let gone: Vec<PathBuf> = self
                    .dirs
                    .get(dir)
                    .map(|old| {
                        old.iter().filter(|e| e.is_dir && !alive.contains(e.path.as_path())).map(|e| e.path.clone()).collect()
                    })
                    .unwrap_or_default();
                for g in gone {
                    self.forget_subtree(&g);
                }
                self.dirs.insert(dir.to_path_buf(), entries);
            }
            Err(_) => self.forget_subtree(dir),
        }
        self.rows_dirty = true;
    }

    fn forget_subtree(&mut self, dir: &Path) {
        let keys: Vec<PathBuf> = self.dirs.keys().filter(|k| k.starts_with(dir)).cloned().collect();
        for k in keys {
            self.dirs.remove(&k);
            self.expanded.remove(&k);
            self.unwatch_dir(&k);
        }
        if self.selected.as_deref().is_some_and(|s| s.starts_with(dir)) {
            self.selected = None;
        }
    }

    fn set_error(&mut self, msg: String) {
        self.error = Some((msg, Instant::now()));
    }

    // ---- 파일 감시 ----

    fn ensure_watcher(&mut self, ctx: &egui::Context) {
        {
            let mut c = self.ctx.lock();
            if c.is_none() {
                *c = Some(ctx.clone());
            }
        }
        if self.watch.is_some() || self.watch_failed {
            return;
        }
        let (tx, rx) = channel();
        let shared = self.ctx.clone();
        let handler = move |res: notify::Result<notify::Event>| {
            let _ = tx.send(res);
            if let Some(c) = shared.lock().as_ref() {
                c.request_repaint();
            }
        };
        match notify::recommended_watcher(handler) {
            Ok(watcher) => {
                let recursive = !cfg!(target_os = "linux");
                let mut w = FsWatch { watcher, rx, recursive, watched: HashSet::new() };
                if recursive {
                    if w.watcher.watch(&self.root, RecursiveMode::Recursive).is_ok() {
                        w.watched.insert(self.root.clone());
                    }
                } else {
                    for d in self.dirs.keys() {
                        if w.watcher.watch(d, RecursiveMode::NonRecursive).is_ok() {
                            w.watched.insert(d.clone());
                        }
                    }
                }
                self.watch = Some(w);
            }
            Err(e) => {
                self.watch_failed = true;
                log::warn!("file watcher unavailable: {e}");
            }
        }
    }

    fn watch_dir(&mut self, dir: &Path) {
        if let Some(w) = &mut self.watch
            && !w.recursive
            && !w.watched.contains(dir)
            && w.watcher.watch(dir, RecursiveMode::NonRecursive).is_ok()
        {
            w.watched.insert(dir.to_path_buf());
        }
    }

    fn unwatch_dir(&mut self, dir: &Path) {
        if let Some(w) = &mut self.watch
            && !w.recursive
            && w.watched.remove(dir)
        {
            let _ = w.watcher.unwatch(dir);
        }
    }

    fn map_event_path(&self, p: &Path) -> PathBuf {
        match p.strip_prefix(&self.canon_root) {
            Ok(rel) if self.canon_root != self.root => self.root.join(rel),
            _ => p.to_path_buf(),
        }
    }

    /// 감시 이벤트를 모아 두었다가 조용해지면 영향받은 읽어 둔 폴더만 다시 읽는다.
    fn pump_watcher(&mut self, ctx: &egui::Context) {
        let mut got = Vec::new();
        if let Some(w) = &self.watch {
            while let Ok(res) = w.rx.try_recv() {
                if let Ok(ev) = res {
                    got.push(ev);
                }
            }
        }
        let now = ctx.input(|i| i.time);
        for ev in got {
            if matches!(ev.kind, notify::EventKind::Access(_)) {
                continue;
            }
            for p in &ev.paths {
                self.note_changed_path(&self.map_event_path(p), now);
            }
        }
        if self.pending.is_empty() && !self.reload_all_pending {
            return;
        }
        let first = self.pending_first.unwrap_or(now);
        let last = self.pending_last.unwrap_or(now);
        if now - last >= DEBOUNCE_QUIET || now - first >= DEBOUNCE_MAX {
            self.apply_pending();
        } else {
            let wait = (DEBOUNCE_QUIET - (now - last)).min(DEBOUNCE_MAX - (now - first));
            ctx.request_repaint_after(Duration::from_secs_f64(wait.max(0.0)));
        }
    }

    /// 경로 변경을 기록한다. 읽어 둔 폴더와 관계없는 경로는 무시한다.
    fn note_changed_path(&mut self, p: &Path, now: f64) {
        if !p.starts_with(&self.root) {
            return;
        }
        if let Ok(rel) = p.strip_prefix(&self.root)
            && rel.components().any(|c| c.as_os_str() == ".git")
        {
            return;
        }
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let mut hit = false;
        if name == ".gitignore" || name == ".ignore" {
            self.reload_all_pending = true;
            hit = true;
        }
        if let Some(parent) = p.parent()
            && self.dirs.contains_key(parent)
        {
            self.pending.insert(parent.to_path_buf());
            hit = true;
        }
        if self.dirs.contains_key(p) {
            self.pending.insert(p.to_path_buf());
            hit = true;
        }
        if hit {
            self.pending_first.get_or_insert(now);
            self.pending_last = Some(now);
        }
    }

    fn apply_pending(&mut self) {
        let mut dirs: Vec<PathBuf> = if self.reload_all_pending {
            self.dirs.keys().cloned().collect()
        } else {
            self.pending.drain().collect()
        };
        self.pending.clear();
        self.reload_all_pending = false;
        self.pending_first = None;
        self.pending_last = None;
        dirs.sort();
        for d in &dirs {
            if self.dirs.contains_key(d) {
                self.reload_dir(d);
            }
        }
        self.last_reloaded = dirs;
    }

    /// 쌓인 감시 이벤트를 즉시 반영한다(대기 시간 무시).
    pub fn flush_fs_events(&mut self) {
        let now = 0.0;
        let mut got = Vec::new();
        if let Some(w) = &self.watch {
            while let Ok(Ok(ev)) = w.rx.try_recv() {
                got.push(ev);
            }
        }
        for ev in got {
            for p in &ev.paths {
                self.note_changed_path(&self.map_event_path(p), now);
            }
        }
        if !self.pending.is_empty() || self.reload_all_pending {
            self.apply_pending();
        }
    }

    // ---- 행 ----

    fn rebuild_rows(&mut self) {
        if !self.rows_dirty {
            return;
        }
        self.rows_dirty = false;
        let mut rows = Vec::new();
        let root = self.root.clone();
        self.push_children(&root, 0, &mut rows);
        self.rows = rows;
    }

    fn push_children(&self, dir: &Path, depth: usize, rows: &mut Vec<Row>) {
        if let Some(c) = &self.create
            && c.parent == dir
        {
            rows.push(Row { depth, kind: RowKind::NewEntry { is_dir: c.is_dir } });
        }
        let Some(children) = self.dirs.get(dir) else { return };
        for e in children {
            rows.push(Row { depth, kind: RowKind::Entry(e.clone()) });
            if e.is_dir && self.expanded.contains(&e.path) {
                self.push_children(&e.path, depth + 1, rows);
            }
        }
    }

    fn row_index(&self, path: &Path) -> Option<usize> {
        self.rows.iter().position(|r| matches!(&r.kind, RowKind::Entry(e) if e.path == path))
    }

    fn entry_at(&self, i: usize) -> Option<&DirEntry> {
        match &self.rows.get(i)?.kind {
            RowKind::Entry(e) => Some(e),
            RowKind::NewEntry { .. } => None,
        }
    }

    fn toggle_dir(&mut self, dir: &Path) {
        let open = !self.expanded.contains(dir);
        self.set_expanded(dir, open);
    }

    // ---- UI ----

    /// 트리를 그리고 사건 목록을 돌려준다.
    pub fn ui(&mut self, ui: &mut Ui) -> Vec<EditorEvent> {
        self.ui_with_header(ui, true)
    }

    /// Embedded inspectors already own the workspace title and file actions.
    pub fn ui_embedded(&mut self, ui: &mut Ui) -> Vec<EditorEvent> {
        self.ui_with_header(ui, false)
    }

    /// Root/selection actions for a host inspector's file menu.
    pub fn menu_ui(&mut self, ui: &mut Ui) -> Vec<EditorEvent> {
        let mut actions = Vec::new();
        let mut events = Vec::new();
        let target = self.selected_dir();
        menu_item(ui, "새 파일…", || actions.push(Action::NewEntry(target.clone(), false)));
        menu_item(ui, "새 폴더…", || actions.push(Action::NewEntry(target.clone(), true)));
        ui.separator();
        menu_item(ui, "새로 고침", || actions.push(Action::Refresh));
        menu_item(ui, "폴더 모두 접기", || actions.push(Action::CollapseAll));
        let ignored_label = if self.show_ignored { "무시된 파일 숨기기" } else { "무시된 파일 표시" };
        menu_item(ui, ignored_label, || actions.push(Action::ToggleIgnored));
        self.apply_actions(ui, actions, &mut events);
        events
    }

    fn ui_with_header(&mut self, ui: &mut Ui, show_header: bool) -> Vec<EditorEvent> {
        let t = Theme::current();
        let mut events = Vec::new();
        let mut actions: Vec<Action> = Vec::new();
        self.ensure_watcher(ui.ctx());
        self.pump_watcher(ui.ctx());
        self.rebuild_rows();

        let full = ui.available_rect_before_wrap();
        ui.painter().rect_filled(full, 0.0, t.bg_panel);
        let header = Rect::from_min_size(full.min, vec2(full.width(), if show_header { HEADER_H } else { 0.0 }));
        let list_rect = Rect::from_min_max(pos2(full.left(), header.bottom()), full.max);

        // 목록 배경: 포커스 보관과 빈 곳 클릭/우클릭.
        let bg = ui.interact(list_rect, self.id, Sense::click());
        if bg.clicked() {
            ui.memory_mut(|m| m.request_focus(self.id));
        }
        let root = self.root.clone();
        bg.context_menu(|ui| {
            menu_item(ui, "새 파일…", || actions.push(Action::NewEntry(root.clone(), false)));
            menu_item(ui, "새 폴더…", || actions.push(Action::NewEntry(root.clone(), true)));
            ui.separator();
            menu_item(ui, "새로 고침", || actions.push(Action::Refresh));
            menu_item(ui, "터미널에서 열기", || actions.push(Action::Terminal(root.clone())));
        });

        if show_header { self.header_ui(ui, header, &mut actions); }

        if self.focus_pending && !egui::Popup::is_any_open(ui.ctx()) {
            self.focus_pending = false;
            if let Some(create) = &mut self.create {
                create.focus = true;
            } else if let Some(rename) = &mut self.rename {
                rename.focus = true;
            } else {
                ui.memory_mut(|m| m.request_focus(self.id));
            }
        }
        let focused = ui.memory(|m| m.has_focus(self.id));
        if focused {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(self.id, EventFilter { tab: false, horizontal_arrows: true, vertical_arrows: true, escape: false })
            });
            if self.rename.is_none() && self.create.is_none() && self.confirm_delete.is_none() {
                self.handle_keys(ui, &mut actions, &mut events);
            }
        }

        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(list_rect).id_salt(self.id.with("list")));
        child.set_clip_rect(list_rect.intersect(ui.clip_rect()));
        child.spacing_mut().item_spacing.y = 0.0;
        let mut sa = ScrollArea::vertical().id_salt(self.id.with("scroll")).auto_shrink([false, false]);
        if self.scroll_to_selected {
            self.scroll_to_selected = false;
            if let Some(i) = self.selected.as_deref().and_then(|p| self.row_index(p)) {
                let y = i as f32 * ROW_H;
                let vh = if self.view_h > 0.0 { self.view_h } else { list_rect.height() };
                if y < self.scroll_y {
                    sa = sa.vertical_scroll_offset(y);
                } else if y + ROW_H > self.scroll_y + vh {
                    sa = sa.vertical_scroll_offset(y + ROW_H - vh);
                }
            }
        }
        let n = self.rows.len();
        let out = sa.show_rows(&mut child, ROW_H, n, |ui, range| {
            for i in range {
                self.row_ui(ui, i, focused, &mut actions);
            }
        });
        self.scroll_y = out.state.offset.y;
        self.view_h = out.inner_rect.height();

        if let Some((msg, at)) = &self.error {
            if at.elapsed() < Duration::from_secs(5) {
                let p = ui.painter();
                let galley = p.layout(msg.clone(), FontId::proportional(12.5), t.text, full.width() - 52.0);
                let h = (galley.size().y + 16.0).max(36.0);
                let r = Rect::from_min_max(pos2(full.left() + 8.0, full.bottom() - 8.0 - h), pos2(full.right() - 8.0, full.bottom() - 8.0));
                p.add(t.shadow().as_shape(r, 8));
                p.rect_filled(r, 8.0, t.bg_elevated);
                p.rect_filled(r, 8.0, kiln_common::widgets::tint(t.red, if t.dark { 0.14 } else { 0.08 }));
                p.rect_stroke(r, 8.0, Stroke::new(1.0, kiln_common::widgets::tint(t.red, 0.45)), egui::StrokeKind::Inside);
                ui_kit::paint_icon(p, Rect::from_center_size(pos2(r.left() + 17.0, r.center().y), vec2(14.0, 14.0)), Icon::Warning, t.red);
                p.galley(pos2(r.left() + 32.0, r.center().y - galley.size().y / 2.0), galley, t.text);
                ui.ctx().request_repaint_after(Duration::from_millis(500));
            } else {
                self.error = None;
            }
        }

        self.delete_modal(ui, &mut events);
        self.apply_actions(ui, actions, &mut events);
        events
    }

    fn header_ui(&mut self, ui: &mut Ui, rect: Rect, actions: &mut Vec<Action>) {
        let t = Theme::current();
        let name = self.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.root.display().to_string());
        let p = ui.painter();
        p.text(pos2(rect.left() + 14.0, rect.center().y), Align2::LEFT_CENTER, name, kiln_common::fonts::semibold(13.0), t.text);
        let hovered = ui.rect_contains_pointer(ui.max_rect()) || ui.memory(|m| m.has_focus(self.id));
        let btns = Rect::from_min_max(pos2(rect.right() - 5.0 * 25.0 - 8.0, rect.top() + 5.0), pos2(rect.right() - 8.0, rect.bottom() - 5.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(btns).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            if !hovered {
                ui.set_invisible();
            }
            let target = self.selected_dir();
            if ui_kit::icon_button(ui, Icon::NewFile, "새 파일…").clicked() {
                actions.push(Action::NewEntry(target.clone(), false));
            }
            if ui_kit::icon_button(ui, Icon::NewFolder, "새 폴더…").clicked() {
                actions.push(Action::NewEntry(target, true));
            }
            if ui_kit::icon_button(ui, Icon::Refresh, "탐색기 새로 고침").clicked() {
                actions.push(Action::Refresh);
            }
            if ui_kit::icon_button(ui, Icon::CollapseAll, "폴더 모두 접기").clicked() {
                actions.push(Action::CollapseAll);
            }
            let (icon, tip) = if self.show_ignored { (Icon::Eye, "무시된 파일 숨기기") } else { (Icon::EyeOff, "무시된 파일 표시") };
            if ui_kit::icon_toggle(ui, icon, tip, self.show_ignored, true).clicked() {
                actions.push(Action::ToggleIgnored);
            }
        });
    }

    /// 새 항목을 만들 폴더: 선택이 폴더면 그 폴더, 파일이면 그 부모, 없으면 루트.
    fn selected_dir(&self) -> PathBuf {
        match &self.selected {
            Some(p) if self.dirs.contains_key(p) || p.is_dir() => p.clone(),
            Some(p) => p.parent().map(Path::to_path_buf).unwrap_or_else(|| self.root.clone()),
            None => self.root.clone(),
        }
    }

    fn row_ui(&mut self, ui: &mut Ui, i: usize, focused: bool, actions: &mut Vec<Action>) {
        let t = Theme::current();
        let row = self.rows[i].clone();
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
        let indent_x = rect.left() + 10.0 + row.depth as f32 * INDENT;
        let p = ui.painter();
        // 들여쓰기 안내선
        for d in 0..row.depth {
            let x = (rect.left() + 10.0 + d as f32 * INDENT + 7.0).round() + 0.5;
            p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0, t.border));
        }
        match row.kind {
            RowKind::NewEntry { is_dir } => {
                let icon_rect = Rect::from_center_size(pos2(indent_x + 25.0, rect.center().y), vec2(16.0, 16.0));
                if is_dir {
                    ui_kit::paint_icon(p, icon_rect, Icon::Folder, t.text_dim);
                } else {
                    let name = self.create.as_ref().map(|c| c.text.clone()).unwrap_or_default();
                    ui_kit::paint_file_badge(p, icon_rect, &name);
                }
                let edit_rect = Rect::from_min_max(pos2(indent_x + 36.0, rect.top() + 2.0), pos2(rect.right() - 8.0, rect.bottom() - 2.0));
                self.inline_edit(ui, edit_rect, true);
            }
            RowKind::Entry(e) => {
                let selected = self.selected.as_deref() == Some(e.path.as_path());
                ui_kit::paint_row_bg(p, rect, selected, focused, resp.hovered());
                let chevron_rect = Rect::from_center_size(pos2(indent_x + 7.0, rect.center().y), vec2(12.0, 12.0));
                let icon_rect = Rect::from_center_size(pos2(indent_x + 25.0, rect.center().y), vec2(16.0, 16.0));
                let expanded = e.is_dir && self.expanded.contains(&e.path);
                let dim = if e.ignored { 0.55 } else { 1.0 };
                if e.is_dir {
                    let chev = if expanded { Icon::ChevronDown } else { Icon::ChevronRight };
                    ui_kit::paint_icon(p, chevron_rect, chev, t.text_faint.gamma_multiply(dim));
                    let folder_color = kiln_common::widgets::lerp_color(t.text_dim, t.accent, 0.35).gamma_multiply(dim);
                    ui_kit::paint_icon(p, icon_rect, if expanded { Icon::FolderOpen } else { Icon::Folder }, folder_color);
                } else {
                    ui_kit::paint_file_badge(p, icon_rect, &e.name);
                }
                let deco = self.decorations.get(&e.path).copied();
                let dir_dot = if e.is_dir { self.dir_decor.get(&e.path).copied() } else { None };
                let mut color = deco.map(|d| d.color).or(dir_dot.map(|c| c.gamma_multiply(0.85))).unwrap_or(t.text);
                if e.ignored {
                    color = t.text_faint;
                }
                let renaming = self.rename.as_ref().is_some_and(|r| r.path == e.path);
                let text_left = indent_x + 38.0;
                if renaming {
                    let edit_rect = Rect::from_min_max(pos2(text_left - 6.0, rect.top() + 2.0), pos2(rect.right() - 8.0, rect.bottom() - 2.0));
                    self.inline_edit(ui, edit_rect, false);
                } else {
                    let right_reserved = if deco.and_then(|d| d.badge).is_some() || dir_dot.is_some() { 30.0 } else { 10.0 };
                    let clip = Rect::from_min_max(pos2(text_left, rect.top()), pos2(rect.right() - right_reserved, rect.bottom()));
                    let pc = ui.painter().with_clip_rect(clip.intersect(ui.clip_rect()));
                    pc.text(pos2(text_left, rect.center().y), Align2::LEFT_CENTER, &e.name, FontId::proportional(13.5), color);
                    let p = ui.painter();
                    if let Some(b) = deco.and_then(|d| d.badge) {
                        let c = deco.map(|d| d.color).unwrap_or(t.text_dim);
                        let br = Rect::from_center_size(pos2(rect.right() - 20.0, rect.center().y), vec2(18.0, 17.0));
                        p.rect_filled(br, 5.0, kiln_common::widgets::tint(c, if t.dark { 0.16 } else { 0.12 }));
                        p.text(br.center(), Align2::CENTER_CENTER, b, kiln_common::fonts::semibold(11.0), c);
                    } else if let Some(c) = dir_dot {
                        p.circle_filled(pos2(rect.right() - 20.0, rect.center().y), 3.0, c.gamma_multiply(0.85));
                    }
                }
                let label = e.name.clone();
                resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &label));
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(CursorIcon::Default);
                }
                if resp.clicked() {
                    ui.memory_mut(|m| m.request_focus(self.id));
                    actions.push(Action::Click(e.path.clone(), e.is_dir));
                }
                if resp.secondary_clicked() {
                    actions.push(Action::Select(e.path.clone()));
                }
                self.row_menu(&resp, &e, actions);
            }
        }
    }

    fn row_menu(&self, resp: &Response, e: &DirEntry, actions: &mut Vec<Action>) {
        let target_dir = if e.is_dir { e.path.clone() } else { e.path.parent().map(Path::to_path_buf).unwrap_or_else(|| self.root.clone()) };
        resp.context_menu(|ui| {
            ui.set_min_width(190.0);
            if !e.is_dir {
                menu_item(ui, "열기", || actions.push(Action::Click(e.path.clone(), false)));
                ui.separator();
            }
            menu_item(ui, "새 파일…", || actions.push(Action::NewEntry(target_dir.clone(), false)));
            menu_item(ui, "새 폴더…", || actions.push(Action::NewEntry(target_dir.clone(), true)));
            ui.separator();
            menu_item(ui, "경로 복사", || actions.push(Action::CopyPath(e.path.clone())));
            menu_item(ui, "상대 경로 복사", || actions.push(Action::CopyRelative(e.path.clone())));
            ui.separator();
            menu_item(ui, "터미널에서 열기", || actions.push(Action::Terminal(target_dir.clone())));
            ui.separator();
            menu_item(ui, "이름 바꾸기…", || actions.push(Action::Rename(e.path.clone())));
            menu_item(ui, "삭제", || actions.push(Action::Delete(e.path.clone())));
        });
    }

    /// 이름 바꾸기/새 항목 입력칸. `creating` 이면 새 항목 입력.
    fn inline_edit(&mut self, ui: &mut Ui, rect: Rect, creating: bool) {
        let t = Theme::current();
        let id = self.id.with(if creating { "create" } else { "rename" });
        let (text, focus_req) = if creating {
            let Some(c) = self.create.as_mut() else { return };
            (&mut c.text, &mut c.focus)
        } else {
            let Some(r) = self.rename.as_mut() else { return };
            (&mut r.text, &mut r.focus)
        };
        let mut commit = false;
        let mut cancel = false;
        let first_focus = *focus_req;
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            egui::Frame::new()
                .fill(t.bg_input)
                .stroke(Stroke::new(1.0, t.accent))
                .corner_radius(6)
                .inner_margin(egui::Margin { left: 6, right: 4, top: 1, bottom: 0 })
                .show(ui, |ui| {
                    let out = egui::TextEdit::singleline(text)
                        .id(id)
                        .frame(egui::Frame::NONE)
                        .font(FontId::proportional(13.5))
                        .desired_width(rect.width() - 12.0)
                        .margin(vec2(0.0, 2.0))
                        .return_key(None)
                        .show(ui);
                    if first_focus {
                        out.response.request_focus();
                        let stem = match text.rfind('.') {
                            Some(i) if i > 0 && !creating => text[..i].chars().count(),
                            _ => text.chars().count(),
                        };
                        let mut st = out.state.clone();
                        st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                            egui::text::CCursor::new(0),
                            egui::text::CCursor::new(stem),
                        )));
                        st.store(ui.ctx(), id);
                    }
                    let (enter, esc) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
                    if out.response.has_focus() && enter {
                        commit = true;
                    } else if esc {
                        cancel = true;
                    } else if out.response.lost_focus() {
                        commit = true;
                    }
                });
        });
        *focus_req = false;
        if cancel {
            if creating {
                self.create = None;
            } else {
                self.rename = None;
            }
            self.rows_dirty = true;
            ui.memory_mut(|m| m.request_focus(self.id));
        } else if commit {
            ui.memory_mut(|m| m.request_focus(self.id));
            if creating {
                self.commit_create(ui);
            } else {
                self.commit_rename(ui);
            }
        }
    }

    fn commit_create(&mut self, ui: &Ui) {
        let Some(c) = self.create.take() else { return };
        self.rows_dirty = true;
        let name = c.text.trim();
        if name.is_empty() {
            return;
        }
        if let Err(msg) = validate_name(name) {
            self.set_error(msg);
            return;
        }
        let path = c.parent.join(name);
        let res = if c.is_dir {
            std::fs::create_dir_all(&path)
        } else {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map(|_| ())
        };
        match res {
            Ok(()) => {
                self.reload_dir(&c.parent);
                self.reveal(&path);
                if !c.is_dir {
                    self.pending_open = Some(path);
                }
                ui.ctx().request_repaint();
            }
            Err(e) => self.set_error(format!("만들 수 없습니다: {name} · {e}")),
        }
    }

    fn commit_rename(&mut self, ui: &Ui) {
        let Some(r) = self.rename.take() else { return };
        self.rows_dirty = true;
        let name = r.text.trim();
        let old_name = r.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name.is_empty() || name == old_name {
            return;
        }
        if let Err(msg) = validate_name(name) {
            self.set_error(msg);
            return;
        }
        let to = r.path.with_file_name(name);
        if to.exists() && !name.eq_ignore_ascii_case(&old_name) {
            self.set_error(format!("같은 이름이 이미 있습니다: {name}"));
            return;
        }
        match std::fs::rename(&r.path, &to) {
            Ok(()) => {
                if let Some(parent) = r.path.parent() {
                    self.forget_subtree(&r.path);
                    self.reload_dir(parent);
                }
                self.selected = Some(to.clone());
                self.scroll_to_selected = true;
                self.pending_rename = Some((r.path.clone(), to));
                ui.ctx().request_repaint();
            }
            Err(e) => self.set_error(format!("이름 바꾸기 실패: {e}")),
        }
    }

    fn delete_modal(&mut self, ui: &mut Ui, events: &mut Vec<EditorEvent>) {
        let Some(path) = self.confirm_delete.clone() else { return };
        let t = Theme::current();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let is_dir = path.is_dir();
        let mut confirm = false;
        let mut cancel = false;
        let modal = egui::Modal::new(self.id.with("delete"))
            .backdrop_color(ui_kit::backdrop())
            .frame(ui_kit::modal_frame())
            .show(ui.ctx(), |ui| {
                ui.set_width((ui.ctx().content_rect().width() - 72.0).clamp(200.0, 360.0));
                let what = if is_dir { "폴더를" } else { "파일을" };
                let verb = if self.use_trash { "휴지통으로 이동할까요" } else { "영구 삭제할까요" };
                ui.add(egui::Label::new(egui::RichText::new(format!("다음 {what} {verb}?\n{name}")).font(kiln_common::fonts::semibold(15.0)).color(t.text)).wrap());
                ui.add_space(6.0);
                let sub = if self.use_trash {
                    if is_dir { "폴더와 그 안의 내용은 휴지통에서 복원할 수 있습니다." } else { "휴지통에서 복원할 수 있습니다." }
                } else {
                    "이 작업은 되돌릴 수 없습니다."
                };
                ui.label(egui::RichText::new(sub).size(13.0).color(t.text_dim));
                ui.add_space(18.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if self.use_trash { "휴지통으로 이동" } else { "삭제" };
                    if ui_kit::danger_button(ui, label).clicked() {
                        confirm = true;
                    }
                    if ui_kit::flat_button(ui, "취소", false).clicked() {
                        cancel = true;
                    }
                });
                if ui.input(|i| i.key_pressed(Key::Enter)) {
                    confirm = true;
                }
            });
        if modal.should_close() {
            cancel = true;
        }
        if confirm {
            self.confirm_delete = None;
            let res = if self.use_trash {
                trash::delete(&path).map_err(|e| e.to_string())
            } else if is_dir {
                std::fs::remove_dir_all(&path).map_err(|e| e.to_string())
            } else {
                std::fs::remove_file(&path).map_err(|e| e.to_string())
            };
            match res {
                Ok(()) => {
                    if let Some(parent) = path.parent() {
                        self.forget_subtree(&path);
                        self.reload_dir(parent);
                    }
                    if self.selected.as_deref() == Some(path.as_path()) {
                        self.selected = path.parent().filter(|p| *p != self.root).map(Path::to_path_buf);
                    }
                    events.push(EditorEvent::FileDeleted(path));
                }
                Err(e) => self.set_error(format!("삭제 실패: {e}")),
            }
            ui.memory_mut(|m| m.request_focus(self.id));
        } else if cancel {
            self.confirm_delete = None;
            ui.memory_mut(|m| m.request_focus(self.id));
        }
    }

    fn handle_keys(&mut self, ui: &mut Ui, actions: &mut Vec<Action>, events: &mut Vec<EditorEvent>) {
        let is_mac = ui.ctx().os() == egui::os::OperatingSystem::Mac;
        let evs = ui.input(|i| i.events.clone());
        let mut consumed = Vec::new();
        for (idx, ev) in evs.iter().enumerate() {
            let Event::Key { key, pressed: true, modifiers: m, .. } = ev else { continue };
            let cur = self.selected.as_deref().and_then(|p| self.row_index(p));
            let n = self.rows.len();
            let select = |i: usize, acts: &mut Vec<Action>, this: &Self| {
                if let Some(e) = this.entry_at(i) {
                    acts.push(Action::Select(e.path.clone()));
                }
            };
            let handled = match key {
                Key::ArrowDown => {
                    let next = cur.map_or(0, |c| (c + 1).min(n.saturating_sub(1)));
                    select(next, actions, self);
                    true
                }
                Key::ArrowUp => {
                    let prev = cur.map_or(0, |c| c.saturating_sub(1));
                    select(prev, actions, self);
                    true
                }
                Key::Home => {
                    select(0, actions, self);
                    true
                }
                Key::End => {
                    select(n.saturating_sub(1), actions, self);
                    true
                }
                Key::ArrowRight => {
                    if let Some(e) = cur.and_then(|c| self.entry_at(c)).cloned()
                        && e.is_dir
                    {
                        if self.expanded.contains(&e.path) {
                            select(cur.unwrap_or(0) + 1, actions, self);
                        } else {
                            self.set_expanded(&e.path, true);
                        }
                    }
                    true
                }
                Key::ArrowLeft => {
                    if let Some(e) = cur.and_then(|c| self.entry_at(c)).cloned() {
                        if e.is_dir && self.expanded.contains(&e.path) {
                            self.set_expanded(&e.path, false);
                        } else if let Some(parent) = e.path.parent()
                            && parent != self.root
                        {
                            actions.push(Action::Select(parent.to_path_buf()));
                        }
                    }
                    true
                }
                Key::Enter | Key::Space => {
                    if let Some(e) = cur.and_then(|c| self.entry_at(c)).cloned() {
                        if e.is_dir {
                            self.toggle_dir(&e.path);
                        } else {
                            events.push(EditorEvent::OpenFile(e.path.clone()));
                        }
                    }
                    true
                }
                Key::F2 => {
                    if let Some(p) = self.selected.clone() {
                        actions.push(Action::Rename(p));
                    }
                    true
                }
                Key::Delete => {
                    if let Some(p) = self.selected.clone() {
                        actions.push(Action::Delete(p));
                    }
                    true
                }
                Key::Backspace if m.command || (is_mac && m.mac_cmd) => {
                    if let Some(p) = self.selected.clone() {
                        actions.push(Action::Delete(p));
                    }
                    true
                }
                _ => false,
            };
            if handled {
                consumed.push(idx);
            }
        }
        if !consumed.is_empty() {
            ui.input_mut(|i| {
                let mut k = 0usize;
                i.events.retain(|_| {
                    let keep = !consumed.contains(&k);
                    k += 1;
                    keep
                });
            });
        }
    }

    fn apply_actions(&mut self, ui: &mut Ui, actions: Vec<Action>, events: &mut Vec<EditorEvent>) {
        for a in actions {
            match a {
                Action::Click(p, is_dir) => {
                    self.selected = Some(p.clone());
                    if is_dir {
                        self.toggle_dir(&p);
                    } else {
                        events.push(EditorEvent::OpenFile(p));
                    }
                }
                Action::Select(p) => {
                    self.selected = Some(p);
                    self.scroll_to_selected = true;
                }
                Action::NewEntry(dir, is_dir) => {
                    self.rename = None;
                    if dir != self.root {
                        self.set_expanded(&dir, true);
                    }
                    self.create = Some(CreateState { parent: dir, is_dir, text: String::new(), focus: true });
                    self.rows_dirty = true;
                }
                Action::Rename(p) => {
                    self.create = None;
                    let text = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.selected = Some(p.clone());
                    self.rename = Some(RenameState { path: p, text, focus: true });
                }
                Action::Delete(p) => self.confirm_delete = Some(p),
                Action::CopyPath(p) => ui.ctx().copy_text(p.display().to_string()),
                Action::CopyRelative(p) => {
                    let rel = p.strip_prefix(&self.root).unwrap_or(&p);
                    ui.ctx().copy_text(rel.to_string_lossy().replace('\\', "/"));
                }
                Action::Terminal(p) => events.push(EditorEvent::RevealInTerminal(p)),
                Action::Refresh => self.refresh(),
                Action::CollapseAll => {
                    self.expanded.retain(|p| *p == self.root);
                    self.rows_dirty = true;
                }
                Action::ToggleIgnored => {
                    let v = !self.show_ignored;
                    self.set_show_ignored(v);
                }
            }
        }
        if let Some(p) = self.pending_open.take() {
            events.push(EditorEvent::OpenFile(p));
        }
        if let Some((from, to)) = self.pending_rename.take() {
            events.push(EditorEvent::FileRenamed { from, to });
        }
        self.rebuild_rows();
    }
}

fn menu_item(ui: &mut Ui, label: &str, f: impl FnOnce()) {
    if ui.button(label).clicked() {
        f();
        ui.close();
    }
}

fn validate_name(name: &str) -> Result<(), String> {
    if name == "." || name == ".." {
        return Err("잘못된 이름".into());
    }
    if name.contains('\\') || name.contains('\0') || (cfg!(windows) && name.contains(['<', '>', ':', '"', '|', '?', '*'])) {
        return Err(format!("“{name}”에 사용할 수 없는 문자가 있습니다"));
    }
    if name.starts_with('/') {
        return Err("이름은 /로 시작할 수 없습니다".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree_fixture() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        std::fs::create_dir_all(r.join("src/nested")).unwrap();
        std::fs::create_dir_all(r.join("target/debug")).unwrap();
        std::fs::create_dir_all(r.join("Docs")).unwrap();
        std::fs::write(r.join(".gitignore"), "/target\n*.log\n").unwrap();
        std::fs::write(r.join("README.md"), "hi").unwrap();
        std::fs::write(r.join("app.log"), "x").unwrap();
        std::fs::write(r.join("b.rs"), "").unwrap();
        std::fs::write(r.join("a10.rs"), "").unwrap();
        std::fs::write(r.join("a2.rs"), "").unwrap();
        std::fs::write(r.join(".env"), "").unwrap();
        std::fs::write(r.join("src/main.rs"), "").unwrap();
        std::fs::write(r.join("src/debug.log"), "").unwrap();
        std::fs::write(r.join("src/nested/x.txt"), "").unwrap();
        d
    }

    fn names(v: &[DirEntry]) -> Vec<&str> {
        v.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn list_dir_respects_gitignore_and_sorts_dirs_first_naturally() {
        let d = tree_fixture();
        let v = list_dir(d.path(), false).unwrap();
        assert_eq!(names(&v), ["Docs", "src", ".env", ".gitignore", "a2.rs", "a10.rs", "b.rs", "README.md"]);
        let sub = list_dir(&d.path().join("src"), false).unwrap();
        assert_eq!(names(&sub), ["nested", "main.rs"]);
    }

    #[test]
    fn list_dir_can_show_ignored_entries_marked() {
        let d = tree_fixture();
        let v = list_dir(d.path(), true).unwrap();
        let ignored: Vec<_> = v.iter().filter(|e| e.ignored).map(|e| e.name.as_str()).collect();
        assert_eq!(ignored, ["target", "app.log"]);
    }

    #[test]
    fn natural_sort_orders_numbers_by_value() {
        let mut v = vec!["file10", "File2", "file1", "a"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["a", "file1", "File2", "file10"]);
    }

    #[test]
    fn lazy_loading_and_reveal_expand_ancestors_only() {
        let d = tree_fixture();
        let mut t = FileTree::new(d.path().to_path_buf());
        assert_eq!(t.loaded_dirs(), vec![d.path().to_path_buf()]);
        t.reveal(&d.path().join("src/nested/x.txt"));
        assert!(t.is_expanded(&d.path().join("src/nested")));
        assert_eq!(t.loaded_dirs().len(), 3);
        let vis = t.visible_paths();
        assert!(vis.contains(&d.path().join("src/nested/x.txt")));
        assert!(!vis.iter().any(|p| p.starts_with(d.path().join("target"))));
        assert_eq!(t.selected(), Some(d.path().join("src/nested/x.txt").as_path()));
    }

    #[test]
    fn change_events_reload_only_affected_loaded_dirs() {
        let d = tree_fixture();
        let mut t = FileTree::new(d.path().to_path_buf());
        t.set_expanded(&d.path().join("src"), true);
        let now = 0.0;
        // 읽지 않은 폴더(target/debug)와 .git 내부 변경은 무시된다.
        t.note_changed_path(&d.path().join("target/debug/out.o"), now);
        t.note_changed_path(&d.path().join(".git/index"), now);
        assert!(t.pending.is_empty());
        std::fs::write(d.path().join("src/new.rs"), "").unwrap();
        t.note_changed_path(&d.path().join("src/new.rs"), now);
        t.apply_pending();
        assert_eq!(t.last_reloaded(), &[d.path().join("src")]);
        assert!(t.visible_paths().contains(&d.path().join("src/new.rs")));
    }

    #[test]
    fn removing_a_loaded_dir_forgets_its_subtree() {
        let d = tree_fixture();
        let mut t = FileTree::new(d.path().to_path_buf());
        t.reveal(&d.path().join("src/nested/x.txt"));
        std::fs::remove_dir_all(d.path().join("src")).unwrap();
        let now = 0.0;
        t.note_changed_path(&d.path().join("src"), now);
        t.apply_pending();
        assert_eq!(t.loaded_dirs(), vec![d.path().to_path_buf()]);
        assert!(t.selected().is_none());
    }

    #[test]
    fn decorations_propagate_to_ancestor_dirs() {
        let d = tree_fixture();
        let mut t = FileTree::new(d.path().to_path_buf());
        let mut m = HashMap::new();
        m.insert(d.path().join("src/nested/x.txt"), Decoration { color: Color32::RED, badge: Some('M') });
        t.set_decorations(m);
        assert_eq!(t.dir_decor.get(&d.path().join("src")), Some(&Color32::RED));
        assert_eq!(t.dir_decor.get(&d.path().join("src/nested")), Some(&Color32::RED));
        assert!(!t.dir_decor.contains_key(d.path()));
    }
}
