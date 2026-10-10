use super::{RemoteManager, icon, protocol};
use crate::{
    ConnectionProfile, JobHandle, Operation, RemoteEndpoint, RemoteEntry, RemoteResult, Secrets,
};
use egui::{RichText, Ui};
use kiln_common::{Task, Theme, i18n::tr, icons::Icon};
use kiln_editor::Editor;
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RemoteDraft {
    pub remote_path: String,
    pub local_path: PathBuf,
    pub text: String,
    pub baseline: String,
    pub original: Vec<u8>,
}
struct Editing {
    path: String,
    local: PathBuf,
    editor: Editor,
    baseline: String,
    original: Vec<u8>,
}
enum Purpose {
    Listing { path: String, append: bool },
    Edit { entry: RemoteEntry, local: PathBuf },
    Transfer,
    CheckSave { local: PathBuf, text: String },
    Save { text: String },
}
struct Active {
    job: JobHandle,
    purpose: Purpose,
    label: String,
    cancelling: bool,
}
#[derive(Clone)]
enum Dialog {
    Delete(RemoteEntry),
    Rename(RemoteEntry, String),
    Folder(String),
    Overwrite { local: PathBuf, path: String },
}
pub struct RemoteBrowser {
    manager: RemoteManager,
    profile: ConnectionProfile,
    path: String,
    path_input: String,
    entries: Vec<RemoteEntry>,
    cursor: Option<String>,
    selected: Option<String>,
    loading_secrets: Option<Task<Result<Secrets, String>>>,
    secrets: Option<Secrets>,
    pending: VecDeque<(Operation, Purpose, String)>,
    uploads: VecDeque<PathBuf>,
    active: Option<Active>,
    error: Option<String>,
    notice: Option<String>,
    dialog: Option<Dialog>,
    editing: Option<Editing>,
    connected: bool,
    filter: String,
    navigation_only: bool,
    file: Option<RemoteEntry>,
    open_requests: Vec<RemoteEntry>,
    modal_id: egui::Id,
    interrupted: bool,
}
impl RemoteBrowser {
    pub fn new(manager: RemoteManager, profile: ConnectionProfile, path: String) -> Self {
        Self {
            manager,
            profile,
            path_input: path.clone(),
            path,
            entries: Vec::new(),
            cursor: None,
            selected: None,
            loading_secrets: None,
            secrets: None,
            pending: VecDeque::new(),
            uploads: VecDeque::new(),
            active: None,
            error: None,
            notice: None,
            dialog: None,
            editing: None,
            connected: false,
            filter: String::new(),
            navigation_only: false,
            file: None,
            open_requests: Vec::new(),
            modal_id: egui::Id::new("remote-file-action"),
            interrupted: false,
        }
    }
    pub fn title(&self) -> String {
        self.file.as_ref().map(|e| e.name.clone()).unwrap_or_else(|| self.profile.name.clone())
    }
    pub fn navigation(mut self) -> Self { self.navigation_only = true; self }
    pub fn file_entry(&self) -> Option<&RemoteEntry> { self.file.as_ref() }
    pub fn open_file(mut self, entry: RemoteEntry) -> Self { self.file = Some(entry); self }
    pub fn take_open_requests(&mut self) -> Vec<RemoteEntry> { std::mem::take(&mut self.open_requests) }
    fn activate_file(&mut self, entry: RemoteEntry) {
        if self.navigation_only { self.open_requests.push(entry); } else { self.edit(entry); }
    }
    pub fn connected(&self) -> bool { self.connected }
    pub fn update_name(&mut self, name: &str) { self.profile.name = name.into(); }
    fn endpoint_valid(&self) -> bool {
        self.manager.get(&self.profile.id).is_some_and(|p| p.endpoint == self.profile.endpoint)
    }
    pub fn connection(&self) -> &str {
        &self.profile.id
    }
    pub fn profile(&self) -> &ConnectionProfile {
        &self.profile
    }
    pub fn pending_operation(&self) -> Option<String> {
        self.active
            .as_ref()
            .filter(|a| !matches!(a.purpose, Purpose::Listing { .. }))
            .map(|a| a.label.clone())
            .or_else(|| {
                self.pending
                    .iter()
                    .find(|(_, purpose, _)| !matches!(purpose, Purpose::Listing { .. }))
                    .map(|(_, _, label)| label.clone())
            })
            .or_else(|| (!self.uploads.is_empty()).then(|| tr("업로드 중…").into()))
    }
    pub fn recovery_pending_operation(&self) -> Option<String> {
        self.pending_operation().or_else(|| self.interrupted.then(|| tr("이전 원격 전송의 결과를 새로고침해 확인하세요. 작업을 자동으로 재실행하지 않았습니다.").into()))
    }
    pub fn restore_pending_notice(&mut self) {
        self.interrupted = true;
        self.notice=Some(tr("이전 원격 전송의 결과를 새로고침해 확인하세요. 작업을 자동으로 재실행하지 않았습니다.").into());
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn is_dirty(&self) -> bool {
        self.editing
            .as_ref()
            .is_some_and(|e| e.editor.text() != e.baseline)
    }
    pub fn draft(&self) -> Option<RemoteDraft> {
        self.editing
            .as_ref()
            .filter(|_| self.is_dirty())
            .map(|e| RemoteDraft {
                remote_path: e.path.clone(),
                local_path: e.local.clone(),
                text: e.editor.text(),
                baseline: e.baseline.clone(),
                original: e.original.clone(),
            })
    }
    pub fn restore(&mut self, draft: &RemoteDraft) {
        let mut editor = Editor::open(&draft.local_path)
            .unwrap_or_else(|_| Editor::from_text(draft.local_path.clone(), &draft.baseline));
        editor.select_all();
        editor.insert_text(&draft.text);
        self.editing = Some(Editing {
            path: draft.remote_path.clone(),
            local: draft.local_path.clone(),
            editor,
            baseline: draft.baseline.clone(),
            original: draft.original.clone(),
        });
        self.notice =
            Some(tr("원격 편집 내용을 복원했습니다. 원격 저장은 실행되지 않았습니다.").into());
    }
    pub fn connect(&mut self, ctx: &egui::Context) {
        if self.loading_secrets.is_some() || self.active.is_some() {
            return;
        }
        // A recovered editor must never write to an endpoint changed under the same ID.
        if self.file.is_some() && !self.endpoint_valid() {
            self.error = Some(tr("연결 정보가 변경되었습니다. 연결을 다시 선택하세요.").into());
            return;
        }
        let store = self.manager.store();
        let id = self.profile.id.clone();
        self.loading_secrets = Some(Task::spawn(ctx, move || {
            crate::load_secrets(store.as_ref(), &id).map_err(|e| e.to_string())
        }));
        if let Some(entry) = self.file.clone() {
            if self.editing.is_none() { self.edit(entry); }
            return;
        }
        self.queue(
            Operation::List {
                path: self.path_input.clone(),
                cursor: None,
            },
            Purpose::Listing {
                path: self.path_input.clone(),
                append: false,
            },
            tr("연결하는 중…"),
        );
    }
    fn reconnect(&mut self, ctx: &egui::Context) {
        if self.busy() || self.editing.is_some() {
            return;
        }
        let Some(profile) = self.manager.get(&self.profile.id) else {
            return;
        };
        self.path = super::root_path(&profile);
        self.path_input = self.path.clone();
        self.profile = profile;
        self.secrets = None;
        self.connected = false;
        self.error = None;
        self.notice = None;
        self.connect(ctx);
    }
    fn queue(&mut self, op: Operation, purpose: Purpose, label: &str) {
        self.pending.push_back((op, purpose, label.into()));
    }
    pub fn tick(&mut self, ctx: &egui::Context) {
        if let Some(result) = self.loading_secrets.as_mut().and_then(Task::take) {
            self.loading_secrets = None;
            match result {
                Ok(s) => self.secrets = Some(s),
                Err(e) => {
                    self.error = Some(e);
                    self.pending.clear();
                }
            }
        }
        if let Some(result) = self.active.as_ref().and_then(|a| a.job.try_recv()) {
            let active = self.active.take().unwrap();
            match result {
                Err(e) => {
                    cleanup_purpose(&active.purpose);
                    self.error = Some(if active.cancelling {
                        tr("전송을 중단했습니다. 원격 결과를 새로고침해 확인하세요.").into()
                    } else {
                        e.to_string()
                    });
                    self.pending.clear();
                    self.uploads.clear();
                }
                Ok(result) => {
                    self.error = None;
                    match (active.purpose, result) {
                        (Purpose::Listing { path, append }, RemoteResult::Listed(page)) => {
                            if !append {
                                self.entries.clear();
                                self.selected = None;
                            }
                            for entry in page.entries {
                                if !self.entries.iter().any(|e| e.path == entry.path) {
                                    self.entries.push(entry)
                                }
                            }
                            self.entries.sort_by(|a, b| {
                                b.is_dir
                                    .cmp(&a.is_dir)
                                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                            });
                            self.cursor = page.next_cursor;
                            self.path = path;
                            self.path_input = self.path.clone();
                            self.connected = true;
                            if self.interrupted { self.interrupted = false; self.notice = None; }
                        }
                        (Purpose::Edit { entry, local }, RemoteResult::Done) => {
                            self.interrupted = false; self.notice = None;
                            if std::fs::metadata(&local)
                                .map(|m| m.len() > 8 * 1024 * 1024)
                                .unwrap_or(true)
                            {
                                cleanup_cache(&local);
                                self.error =
                                    Some(tr("8 MB보다 큰 파일은 다운로드해서 편집하세요").into());
                            } else {
                                match Editor::open(&local) {
                                    Ok(editor) => {
                                        let baseline = editor.text();
                                        let original = std::fs::read(&local).unwrap_or_default();
                                        if let Some(previous) = self.editing.take() {
                                            cleanup_cache(&previous.local);
                                        }
                                        self.editing = Some(Editing {
                                            path: entry.path,
                                            local,
                                            editor,
                                            baseline,
                                            original,
                                        });
                                    }
                                    Err(e) => {
                                        cleanup_cache(&local);
                                        self.error = Some(e.to_string());
                                    }
                                }
                            }
                        }
                        (Purpose::CheckSave { local, text }, RemoteResult::Done) => {
                            let current = std::fs::read(&local);
                            cleanup_cache(&local);
                            if current.as_ref().ok() != self.editing.as_ref().map(|e| &e.original) {
                                self.error=Some(tr("원격 파일이 변경되었습니다. 편집 내용을 로컬에 보관하고 다시 열어 주세요.").into());
                            } else if let Some(e) = &self.editing {
                                self.queue(
                                    Operation::Upload {
                                        local: e.local.clone(),
                                        path: e.path.clone(),
                                        overwrite: true,
                                    },
                                    Purpose::Save { text },
                                    tr("원격 파일 저장 중…"),
                                );
                            }
                        }
                        (Purpose::Save { text }, RemoteResult::Done) => {
                            self.interrupted = false;
                            if let Some(e) = &mut self.editing {
                                e.baseline = text;
                                e.original = std::fs::read(&e.local).unwrap_or_default();
                            }
                            self.notice = Some(tr("원격 파일을 저장했습니다").into());
                            self.refresh();
                        }
                        (Purpose::Transfer, RemoteResult::Done) => {
                            self.notice = Some(tr("전송 완료").into());
                            self.refresh();
                        }
                        _ => self.error = Some(tr("예상하지 못한 원격 응답입니다").into()),
                    }
                }
            }
        }
        if self.active.is_none()
            && self.loading_secrets.is_none()
            && self.pending.is_empty()
            && self.dialog.is_none()
            && self.editing.is_none()
            && let Some(local) = self.uploads.pop_front()
        {
            self.upload(local);
        }
        if self.active.is_none()
            && self.loading_secrets.is_none()
            && let Some(secrets) = &self.secrets
            && let Some((op, purpose, label)) = self.pending.pop_front()
        {
            if self.file.is_some() && !self.endpoint_valid() {
                cleanup_purpose(&purpose);
                for (_, purpose, _) in self.pending.drain(..) { cleanup_purpose(&purpose); }
                self.error = Some(tr("연결 정보가 변경되었습니다. 연결을 다시 선택하세요.").into());
            } else { self.active = Some(Active {
                job: crate::spawn(self.profile.clone(), secrets.clone(), op),
                purpose,
                label,
                cancelling: false,
            }); }
        }
        if self.active.is_some()
            || self.loading_secrets.is_some()
            || (!self.uploads.is_empty() && self.dialog.is_none())
        {
            ctx.request_repaint_after(Duration::from_millis(80));
        }
    }
    fn refresh(&mut self) {
        if self.file.is_some() { return; }
        if !self
            .pending
            .iter()
            .any(|(op, _, _)| matches!(op, Operation::List { .. }))
        {
            self.queue(
                Operation::List {
                    path: self.path.clone(),
                    cursor: None,
                },
                Purpose::Listing {
                    path: self.path.clone(),
                    append: false,
                },
                tr("새로고침 중…"),
            );
        }
    }
    fn navigate(&mut self, path: String) {
        self.queue(
            Operation::List {
                path: path.clone(),
                cursor: None,
            },
            Purpose::Listing {
                path,
                append: false,
            },
            tr("파일 목록을 읽는 중…"),
        );
    }
    pub fn busy(&self) -> bool {
        self.active.is_some()
            || self.loading_secrets.is_some()
            || !self.pending.is_empty()
            || !self.uploads.is_empty()
    }
    fn upload(&mut self, local: PathBuf) {
        if !local.is_file() {
            self.uploads.clear();
            self.error = Some(tr("일반 파일만 업로드할 수 있습니다").into());
            return;
        }
        if matches!(self.profile.endpoint,RemoteEndpoint::ObjectStorage {provider:crate::ObjectProvider::Cloudflare,authentication:crate::ObjectAuthentication::Cli {..},..})
            && std::fs::metadata(&local).is_ok_and(|meta|meta.len()>crate::R2_API_MAX_UPLOAD_BYTES) {
            self.uploads.clear();
            self.error=Some(tr("R2 CLI 인증은 파일당 300 MB까지 업로드할 수 있습니다. 더 큰 파일은 S3 키 인증을 사용하세요.").into());
            return;
        }
        let Some(name) = local.file_name().and_then(|n| n.to_str()) else {
            self.uploads.clear();
            self.error = Some(tr("일반 파일만 업로드할 수 있습니다").into());
            return;
        };
        let path = join(&self.path, name);
        if self.entries.iter().any(|e| e.path == path) {
            self.dialog = Some(Dialog::Overwrite { local, path });
        } else {
            self.queue(
                Operation::Upload {
                    local,
                    path,
                    overwrite: false,
                },
                Purpose::Transfer,
                tr("업로드 중…"),
            );
        }
    }
    fn edit(&mut self, entry: RemoteEntry) {
        if entry.is_dir {
            return;
        }
        if entry.size > 8 * 1024 * 1024 {
            self.error = Some(tr("8 MB보다 큰 파일은 다운로드해서 편집하세요").into());
            return;
        }
        if self.is_dirty() {
            self.error = Some(tr("작성 중인 파일을 먼저 원격 저장하거나 로컬에 보관하세요").into());
            return;
        }
        match cache_path(&self.profile.id, &entry.name) {
            Ok(local) => self.queue(
                Operation::Download {
                    path: entry.path.clone(),
                    local: local.clone(),
                },
                Purpose::Edit { entry, local },
                tr("편집할 파일을 받는 중…"),
            ),
            Err(e) => self.error = Some(e.to_string()),
        }
    }
    pub fn ui(&mut self, ui: &mut Ui) {
        self.tick(ui.ctx());
        self.modal_id = ui.id().with("remote-file-action");
        let theme = Theme::current();
        if !self.navigation_only { ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
            kiln_common::icons::paint(
                ui.painter(),
                rect,
                super::provider_icon(&self.profile),
                theme.text_dim,
            );
            ui.strong(&self.profile.name);
            ui.label(
                RichText::new(protocol(&self.profile))
                    .small()
                    .color(theme.text_dim),
            );
            ui.menu_button("…", |ui| {
                if ui
                    .add_enabled(
                        !self.busy()
                            && self.editing.is_none()
                            && self.manager.get(&self.profile.id).is_some(),
                        egui::Button::new(tr("재연결")),
                    )
                    .clicked()
                {
                    self.reconnect(ui.ctx());
                    ui.close();
                }
            });
        }); }
        if self.file.is_none() { ui.horizontal(|ui| {
            ui.add_enabled_ui(
                self.connected
                    && !self.busy()
                    && self.editing.is_none()
                    && parent_path(&self.profile, &self.path).is_some(),
                |ui| {
                    if icon(ui, Icon::ArrowUp, tr("상위 폴더")) {
                        if let Some(path) = parent_path(&self.profile, &self.path) {
                            self.navigate(path);
                        }
                    }
                },
            );
            let response = ui.add_sized(
                [(ui.available_width() - 36.0).max(80.0), 28.0],
                egui::TextEdit::singleline(&mut self.path_input).hint_text(tr("원격 경로")),
            );
            if response.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                && !self.busy()
                && self.editing.is_none()
            {
                if self.secrets.is_none() {
                    self.connect(ui.ctx());
                } else {
                    self.navigate(self.path_input.clone());
                }
            }
            ui.add_enabled_ui(!self.busy(), |ui| {
                if icon(ui, Icon::Refresh, tr("새로고침")) {
                    if self.secrets.is_none() {
                        self.connect(ui.ctx());
                    } else {
                        self.refresh();
                    }
                }
            });
        }); }
        if self.file.is_some() && !self.endpoint_valid() {
            ui.colored_label(theme.red, tr("연결 정보가 변경되었습니다. 연결을 다시 선택하세요."));
        }
        if let Some(e) = &self.error {
            ui.colored_label(theme.red, tr("원격 작업에 실패했습니다"));
            ui.collapsing(tr("오류 상세"), |ui| {
                ui.label(e);
            });
        }
        if let Some(notice) = &self.notice {
            ui.label(RichText::new(notice).small().color(theme.text_dim));
        }
        if let Some(active) = &mut self.active {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(&active.label);
                let p = active.job.progress();
                if p.bytes > 0 {
                    ui.label(bytes(p.bytes));
                }
                if ui
                    .add_enabled(!active.cancelling, egui::Button::new(tr("중단")))
                    .clicked()
                {
                    active.job.cancel();
                    active.cancelling = true;
                    self.uploads.clear();
                }
            });
        } else if self.loading_secrets.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("자격증명 확인 중…"));
            });
        }
        if self.editing.is_some() {
            if self.secrets.is_none() && !self.busy() && ui.button(tr("연결")).clicked() {
                self.connect(ui.ctx());
            }
            self.editor_ui(ui);
            self.dialog_ui(ui.ctx());
            return;
        }
        if (self.file.is_some() || !self.connected) && !self.busy() {
            if ui.button(tr("연결")).clicked() {
                self.connect(ui.ctx())
            }
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(!self.busy(), |ui| {
                let upload_label=if matches!(self.profile.endpoint,RemoteEndpoint::ObjectStorage {provider:crate::ObjectProvider::Cloudflare,authentication:crate::ObjectAuthentication::Cli {..},..}) {tr("파일 업로드 · 최대 300 MB")}else{tr("파일 업로드")};
                if icon(ui, Icon::Upload, upload_label) {
                    if let Some(paths) = rfd::FileDialog::new().pick_files() {
                        if !paths.is_empty() {
                            self.uploads.extend(paths);
                            ui.ctx().request_repaint();
                        }
                    }
                }
                if icon(ui, Icon::Plus, tr("폴더 만들기")) {
                    self.dialog = Some(Dialog::Folder(String::new()))
                }
                if let Some(entry) = self
                    .entries
                    .iter()
                    .find(|e| Some(&e.path) == self.selected.as_ref())
                    .cloned()
                {
                    if !entry.is_dir && icon(ui, Icon::Pencil, tr("원격 파일 편집")) {
                        self.activate_file(entry.clone())
                    }
                    if !entry.is_dir && icon(ui, Icon::Download, tr("다운로드")) {
                        if let Some(local) = rfd::FileDialog::new()
                            .set_file_name(&entry.name)
                            .save_file()
                        {
                            self.queue(
                                Operation::Download {
                                    path: entry.path.clone(),
                                    local,
                                },
                                Purpose::Transfer,
                                tr("다운로드 중…"),
                            );
                        }
                    }
                    if icon(ui, Icon::Trash, tr("삭제")) {
                        self.dialog = Some(Dialog::Delete(entry));
                    }
                }
            });
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.filter)
                .desired_width(ui.available_width())
                .hint_text(tr("파일 필터")),
        );
        ui.separator();
        let mut activate = None;
        let busy = self.busy();
        let filter = self.filter.to_lowercase();
        let visible = self
            .entries
            .iter()
            .filter(|entry| entry.name.to_lowercase().contains(&filter))
            .cloned()
            .collect::<Vec<_>>();
        if visible.is_empty() && !busy {
            ui.label(if self.filter.is_empty() {
                tr("폴더가 비어 있습니다")
            } else {
                tr("일치하는 파일이 없습니다")
            });
        }
        egui::ScrollArea::vertical()
            .id_salt("remote-files")
            .max_height(
                (ui.available_height() - if self.cursor.is_some() { 38.0 } else { 0.0 }).max(32.0),
            )
            .auto_shrink([false, false])
            .show_rows(ui, 32.0, visible.len(), |ui, range| {
                for entry in &visible[range] {
                    ui.push_id(&entry.path, |ui| {
                        ui.add_enabled_ui(!busy, |ui| {
                            let row = super::file_row(
                                ui,
                                entry,
                                self.selected.as_deref() == Some(&entry.path),
                            );
                            if row.clicked() {
                                self.selected = Some(entry.path.clone())
                            }
                            if row.double_clicked() || (self.navigation_only && !entry.is_dir && row.clicked()) {
                                activate = Some(entry.clone())
                            }
                            row.clone().on_hover_text(format!(
                                "{}\n{}",
                                entry.path,
                                entry.modified.as_deref().unwrap_or("")
                            ));
                            row.context_menu(|ui| {
                                if entry.is_dir && ui.button(tr("폴더 열기")).clicked() {
                                    activate = Some(entry.clone());
                                    ui.close();
                                }
                                if !entry.is_dir && ui.button(tr("원격 파일 편집")).clicked()
                                {
                                    activate = Some(entry.clone());
                                    ui.close()
                                }
                                if !entry.is_dir && ui.button(tr("다운로드")).clicked() {
                                    if let Some(local) = rfd::FileDialog::new()
                                        .set_file_name(&entry.name)
                                        .save_file()
                                    {
                                        self.queue(
                                            Operation::Download {
                                                path: entry.path.clone(),
                                                local,
                                            },
                                            Purpose::Transfer,
                                            tr("다운로드 중…"),
                                        );
                                    }
                                    ui.close()
                                }
                                if can_rename(&self.profile, entry)
                                    && ui.button(tr("이름 변경")).clicked()
                                {
                                    self.dialog =
                                        Some(Dialog::Rename(entry.clone(), entry.name.clone()));
                                    ui.close()
                                }
                                if ui.button(tr("삭제")).clicked() {
                                    self.dialog = Some(Dialog::Delete(entry.clone()));
                                    ui.close()
                                }
                            });
                        });
                    });
                }
            });
        if let Some(cursor) = self.cursor.clone() {
            if ui
                .add_enabled(!busy, egui::Button::new(tr("더 불러오기")))
                .clicked()
            {
                self.queue(
                    Operation::List {
                        path: self.path.clone(),
                        cursor: Some(cursor),
                    },
                    Purpose::Listing {
                        path: self.path.clone(),
                        append: true,
                    },
                    tr("파일 목록을 읽는 중…"),
                );
            }
        }
        if let Some(entry) = activate {
            if entry.is_dir {
                self.navigate(entry.path)
            } else {
                self.activate_file(entry)
            }
        }
        if ui.rect_contains_pointer(ui.max_rect()) && !self.busy() {
            let paths = ui.input(|i| {
                i.raw
                    .dropped_files
                    .iter()
                    .map(|f| f.path().to_path_buf())
                    .collect::<Vec<_>>()
            });
            if !paths.is_empty() {
                self.uploads.extend(paths);
                ui.ctx().request_repaint();
            }
        }
        self.dialog_ui(ui.ctx());
    }
    fn editor_ui(&mut self, ui: &mut Ui) {
        let busy = self.busy();
        let dirty = self.is_dirty();
        let mut save = false;
        let mut close = false;
        let mut rescue = false;
        ui.horizontal_wrapped(|ui| {
            if self.file.is_none() && icon(ui, Icon::ArrowUp, tr("파일 목록으로 돌아가기")) {
                if dirty {
                    self.error =
                        Some(tr("작성 중인 파일을 먼저 원격 저장하거나 로컬에 보관하세요").into())
                } else {
                    close = true
                }
            }
            if let Some(e) = &self.editing {
                ui.label(RichText::new(&e.path).strong());
            }
            if ui
                .add_enabled(
                    dirty && !busy && self.secrets.is_some() && (self.file.is_none() || self.endpoint_valid()),
                    egui::Button::new(tr("원격 저장")),
                )
                .clicked()
            {
                save = true
            }
            if ui.button(tr("로컬에 보관")).clicked() {
                rescue = true
            }
        });
        if rescue
            && let Some(e) = &self.editing
            && let Some(path) = rfd::FileDialog::new()
                .set_file_name(e.local.file_name().unwrap_or_default().to_string_lossy())
                .save_file()
        {
            match e.editor.save_copy(&path) {
                Ok(()) => {
                    self.notice = Some(tr("편집 내용을 로컬에 보관했습니다").into());
                    close = self.file.is_none();
                }
                Err(error) => self.error = Some(error.to_string()),
            }
        }
        if save && let Some(e) = &mut self.editing {
            match e.editor.save() {
                Ok(()) => {
                    let text = e.editor.text();
                    let path = e.path.clone();
                    match cache_path(&self.profile.id, "remote-check") {
                        Ok(local) => self.queue(
                            Operation::Download {
                                path,
                                local: local.clone(),
                            },
                            Purpose::CheckSave { local, text },
                            tr("원격 변경을 확인하는 중…"),
                        ),
                        Err(error) => self.error = Some(error.to_string()),
                    }
                }
                Err(error) => self.error = Some(error.to_string()),
            }
        }
        if close {
            if let Some(e) = self.editing.take() {
                cleanup_cache(&e.local);
            }
            return;
        }
        if let Some(e) = &mut self.editing {
            ui.add_enabled_ui(!busy, |ui| e.editor.ui(ui));
        }
    }
    fn dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.clone() else {
            return;
        };
        let mut cancel = false;
        let mut accept = false;
        egui::Modal::new(self.modal_id).show(ctx, |ui| {
            ui.set_width((ctx.content_rect().width() - 60.0).clamp(220.0, 420.0));
            match &mut dialog {
                Dialog::Delete(entry) => {
                    ui.heading(tr("원격 파일을 삭제할까요?"));
                    ui.label(&entry.name);
                    if entry.is_dir {
                        ui.label(tr("비어 있는 폴더만 삭제할 수 있습니다"));
                    }
                }
                Dialog::Rename(entry, name) => {
                    ui.heading(tr("이름 변경"));
                    ui.label(&entry.name);
                    ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
                }
                Dialog::Folder(name) => {
                    ui.heading(tr("폴더 만들기"));
                    ui.add(
                        egui::TextEdit::singleline(name)
                            .hint_text(tr("폴더 이름"))
                            .desired_width(f32::INFINITY),
                    );
                }
                Dialog::Overwrite { path, .. } => {
                    ui.heading(tr("기존 파일을 덮어쓸까요?"));
                    ui.label(path.as_str());
                }
            }
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if ui.button(tr("취소")).clicked() {
                    cancel = true
                }
                let valid = match &dialog {
                    Dialog::Rename(_, name) | Dialog::Folder(name) => valid_name(name),
                    _ => true,
                };
                if ui
                    .add_enabled(valid, egui::Button::new(tr("확인")))
                    .clicked()
                {
                    accept = true
                }
            });
        });
        self.dialog = Some(dialog.clone());
        if cancel {
            self.dialog = None
        }
        if accept {
            self.dialog = None;
            match dialog {
                Dialog::Delete(entry) => self.queue(
                    Operation::Delete {
                        path: entry.path,
                        is_dir: entry.is_dir,
                    },
                    Purpose::Transfer,
                    tr("삭제 중…"),
                ),
                Dialog::Rename(entry, name) => self.queue(
                    Operation::Rename {
                        from: entry.path,
                        to: join(&self.path, name.trim()),
                        overwrite: false,
                    },
                    Purpose::Transfer,
                    tr("이름 변경 중…"),
                ),
                Dialog::Folder(name) => self.queue(
                    Operation::CreateDir {
                        path: join(&self.path, name.trim()),
                    },
                    Purpose::Transfer,
                    tr("폴더 만드는 중…"),
                ),
                Dialog::Overwrite { local, path } => self.queue(
                    Operation::Upload {
                        local,
                        path,
                        overwrite: true,
                    },
                    Purpose::Transfer,
                    tr("업로드 중…"),
                ),
            }
        }
    }
}
impl Drop for RemoteBrowser {
    fn drop(&mut self) {
        if !self.is_dirty()
            && let Some(editing) = &self.editing
        {
            cleanup_cache(&editing.local);
        }
        for (_, purpose, _) in &self.pending {
            cleanup_purpose(purpose);
        }
        if let Some(active) = self.active.take() {
            active.job.cancel();
            if matches!(
                active.purpose,
                Purpose::Edit { .. } | Purpose::CheckSave { .. }
            ) {
                // A cancelled worker may still hold its local destination. Only clean
                // it after the worker reports completion, never underneath the transfer.
                std::thread::spawn(move || {
                    for _ in 0..600 {
                        if active.job.try_recv().is_some() {
                            cleanup_purpose(&active.purpose);
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                });
            }
        }
    }
}
fn can_rename(profile: &ConnectionProfile, entry: &RemoteEntry) -> bool {
    !entry.is_dir || !matches!(profile.endpoint, RemoteEndpoint::S3 { .. } | RemoteEndpoint::ObjectStorage { .. })
}
fn parent_path(profile: &ConnectionProfile, path: &str) -> Option<String> {
    let normalized = |value: &str| value.trim_end_matches('/').to_owned();
    if normalized(path) == normalized(&super::root_path(profile)) || matches!(path, "" | "." | "/")
    {
        return None;
    }
    let parent = parent(path);
    Some(
        if parent.is_empty() && !matches!(profile.endpoint, RemoteEndpoint::S3 { .. } | RemoteEndpoint::ObjectStorage { .. }) {
            ".".into()
        } else {
            parent
        },
    )
}
fn cleanup_purpose(purpose: &Purpose) {
    if let Purpose::Edit { local, .. } | Purpose::CheckSave { local, .. } = purpose {
        cleanup_cache(local);
    }
}
fn cleanup_cache(path: &Path) {
    cleanup_cache_in(&kiln_common::paths::config_file("remote-cache"), path);
}
fn cleanup_cache_in(root: &Path, path: &Path) {
    // Restored drafts are input, not authority to delete arbitrary local files.
    // Clean only a direct file in an actual, private edit-* cache directory.
    let Some(dir) = path.parent() else { return };
    if dir.parent() != Some(root)
        || !dir
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("edit-"))
    {
        return;
    }
    let Ok(metadata) = std::fs::symlink_metadata(dir) else {
        return;
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return;
    }
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_dir(dir); // Never recursively delete unknown contents.
}
fn valid_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0', '\n', '\r'])
}
fn join(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.into()
    } else {
        format!("{}/{}", path.trim_end_matches('/'), name)
    }
}
fn parent(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit_once('/')
        .map(|(p, _)| if p.is_empty() { "/".into() } else { p.into() })
        .unwrap_or_default()
}
fn bytes(n: u64) -> String {
    if n >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", n as f64 / 1024f64.powi(3))
    } else if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / 1024f64.powi(2))
    } else if n >= 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}
fn cache_path(id: &str, name: &str) -> std::io::Result<PathBuf> {
    let dir = kiln_common::paths::config_file("remote-cache");
    std::fs::create_dir_all(&dir)?;
    let temp = tempfile::Builder::new().prefix("edit-").tempdir_in(dir)?;
    let dir = temp.keep();
    let _ = id;
    let basename = Path::new(name).file_name().unwrap_or_default();
    Ok(dir.join(basename))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(endpoint: RemoteEndpoint) -> ConnectionProfile {
        ConnectionProfile {
            id: "fixture".into(),
            name: "Fixture".into(),
            endpoint,
        }
    }
    #[test]
    fn file_connect_downloads_exact_target_and_changed_endpoint_blocks_dispatch() {
        let dir = tempfile::tempdir().unwrap(); let ctx = egui::Context::default();
        let manager = RemoteManager::with_store(dir.path().join("remote.json"), std::sync::Arc::new(kiln_accounts::MemoryStore::new()));
        let profile = fixture(RemoteEndpoint::Sftp { alias: "fixture".into(), config_path: None, root: ".".into(), options: Default::default() });
        manager.save(profile.clone(), Secrets::default()).unwrap();
        let entry = RemoteEntry { name: "notes.txt".into(), path: "docs/notes.txt".into(), is_dir: false, size: 0, modified: None };
        let mut browser = RemoteBrowser::new(manager.clone(), profile.clone(), "docs".into()).open_file(entry);
        browser.connect(&ctx);
        assert_eq!(browser.pending.len(), 1);
        assert!(matches!(&browser.pending[0].0, Operation::Download {path,..} if path == "docs/notes.txt"));
        let mut changed = profile.clone(); if let RemoteEndpoint::Sftp {alias,..} = &mut changed.endpoint { *alias = "different".into(); }
        manager.save(changed, Secrets::default()).unwrap();
        browser.secrets = Some(Secrets::default()); browser.loading_secrets = None;
        browser.tick(&ctx);
        assert!(browser.active.is_none()); assert!(browser.pending.is_empty()); assert!(browser.error.is_some());
        // A saved draft is still available locally after the connection is removed.
        let draft = RemoteDraft {remote_path: "docs/notes.txt".into(), local_path: dir.path().join("notes.txt"), text: "pending".into(), baseline: "original".into(), original: b"original".to_vec()};
        browser.restore(&draft); manager.remove(&profile.id).unwrap(); browser.connect(&ctx);
        assert_eq!(browser.draft(), Some(draft)); assert!(browser.pending.is_empty());
    }
    #[test]
    fn configured_roots_do_not_offer_escape_or_unsupported_folder_rename() {
        let s3 = fixture(RemoteEndpoint::S3 {
            bucket: "fixture".into(),
            region: "us-east-1".into(),
            endpoint: None,
            path_style: false,
            prefix: "assets/".into(),
            aws_profile: None,
            aws_auth: None,
        });
        assert_eq!(parent_path(&s3, "assets"), None);
        assert_eq!(parent_path(&s3, "assets/folder/"), Some("assets".into()));
        let sftp = fixture(RemoteEndpoint::Sftp {
            alias: "fixture".into(),
            config_path: None,
            root: ".".into(),
            options: Default::default(),
        });
        assert_eq!(parent_path(&sftp, "."), None);
        assert_eq!(parent_path(&sftp, "folder"), Some(".".into()));
        let folder = RemoteEntry {
            name: "folder".into(),
            path: "folder/".into(),
            is_dir: true,
            size: 0,
            modified: None,
        };
        assert!(!can_rename(&s3, &folder));
        assert!(can_rename(&sftp, &folder));
    }
    #[test]
    fn native_r2_size_limit_is_reported_before_any_upload_is_queued() {
        let dir=tempfile::tempdir().unwrap();
        let manager=RemoteManager::with_store(dir.path().join("connections.json"),std::sync::Arc::new(kiln_accounts::MemoryStore::new()));
        let profile=fixture(RemoteEndpoint::ObjectStorage {provider:crate::ObjectProvider::Cloudflare,bucket:"fixture-bucket".into(),prefix:String::new(),authentication:crate::ObjectAuthentication::Cli{profile:"studio".into(),config_path:None},region:None,namespace:None,account_id:Some("0123456789abcdef0123456789abcdef".into())});
        let mut browser=RemoteBrowser::new(manager,profile,String::new());
        let path=dir.path().join("large.bin");let file=std::fs::File::create(&path).unwrap();file.set_len(crate::R2_API_MAX_UPLOAD_BYTES+1).unwrap();
        browser.upload(path.clone());assert!(browser.pending.is_empty());assert!(browser.error.as_ref().is_some_and(|e|e.contains("300 MB")));
        if let RemoteEndpoint::ObjectStorage{authentication,..}=&mut browser.profile.endpoint {*authentication=crate::ObjectAuthentication::S3{region:"auto".into(),endpoint:"https://example.r2.cloudflarestorage.com".into(),path_style:true,aws_profile:None,aws_auth:Some(crate::S3Authentication::Manual)};}
        browser.error=None;browser.upload(path);assert_eq!(browser.pending.len(),1,"S3-compatible credentials use their own multipart backend");
    }
    #[test]
    fn multi_file_upload_keeps_later_files_while_waiting_for_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let manager = RemoteManager::with_store(
            dir.path().join("connections.json"),
            std::sync::Arc::new(kiln_accounts::MemoryStore::new()),
        );
        let profile = fixture(RemoteEndpoint::Sftp {
            alias: "fixture".into(),
            config_path: None,
            root: ".".into(),
            options: Default::default(),
        });
        let mut browser = RemoteBrowser::new(manager, profile, ".".into());
        let first = dir.path().join("first.txt");
        let second = dir.path().join("second.txt");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        browser.entries.push(RemoteEntry {
            name: "first.txt".into(),
            path: "./first.txt".into(),
            is_dir: false,
            size: 0,
            modified: None,
        });
        browser.uploads.extend([first, second.clone()]);
        browser.tick(&egui::Context::default());
        assert!(matches!(browser.dialog, Some(Dialog::Overwrite { .. })));
        assert_eq!(browser.uploads.len(), 1);
        assert!(browser.pending_operation().is_some());
        browser.dialog = None; // Cancel this overwrite, retain the remaining file.
        browser.tick(&egui::Context::default());
        assert!(browser.uploads.is_empty());
        assert!(
            matches!(&browser.pending[0].0, Operation::Upload { local, path, overwrite: false } if local == &second && path == "./second.txt")
        );
        assert!(browser.active.is_none()); // No credentials or remote worker in this fixture.
    }
    #[test]
    fn cache_cleanup_removes_only_owned_files_and_never_recurses() {
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join("remote-cache");
        let owned = cache.join("edit-fixture");
        std::fs::create_dir_all(&owned).unwrap();
        let file = owned.join("config.json");
        let keep = owned.join("keep.json");
        std::fs::write(&file, b"clean").unwrap();
        std::fs::write(&keep, b"retained").unwrap();
        cleanup_cache_in(&cache, &file);
        assert!(!file.exists());
        assert!(keep.exists());
        assert!(owned.exists());
        let outside = root.path().join("user.txt");
        std::fs::write(&outside, b"user").unwrap();
        cleanup_cache_in(&cache, &outside);
        assert!(outside.exists());
        cleanup_cache_in(&cache, &keep);
        assert!(!owned.exists());
    }
    #[test]
    fn names_and_paths_do_not_inject_remote_commands() {
        assert!(!valid_name("../x"));
        assert!(!valid_name("x\ny"));
        assert!(valid_name("안녕 'file'.txt"));
        assert_eq!(join("a/b/", "c.txt"), "a/b/c.txt");
        assert_eq!(parent("/a/b"), "/a");
        assert_eq!(parent("/a"), "/");
    }
}

#[cfg(test)]
mod visual_tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};
    #[test]
    fn remote_file_browser_localized_compact_and_wide() {
        let dir = tempfile::tempdir().unwrap();
        let manager = RemoteManager::with_store(
            dir.path().join("connections.json"),
            std::sync::Arc::new(kiln_accounts::MemoryStore::new()),
        );
        let profile = ConnectionProfile {
            id: "synthetic".into(),
            name: "Production assets".into(),
            endpoint: crate::RemoteEndpoint::S3 {
                bucket: "synthetic-assets".into(),
                region: "ap-northeast-2".into(),
                endpoint: None,
                path_style: false,
                prefix: String::new(),
                aws_profile: None,
                aws_auth: None,
            },
        };
        let shots = PathBuf::from("/tmp/kiln-remote-captures");
        std::fs::create_dir_all(&shots).unwrap();
        for language in kiln_common::i18n::Language::ALL {
            kiln_common::i18n::set_language(language);
            for theme in ["kiln-dark", "kiln-light"] {
                Theme::set_current(theme);
                for width in [420.0, 980.0] {
                    let mut browser = RemoteBrowser::new(
                        manager.clone(),
                        profile.clone(),
                        "releases/2026".into(),
                    );
                    browser.connected = true;
                    browser.entries = vec![
                        RemoteEntry {
                            name: "archives".into(),
                            path: "releases/2026/archives".into(),
                            is_dir: true,
                            size: 0,
                            modified: None,
                        },
                        RemoteEntry {
                            name: "very-long-한국어-日本語-document-name.json".into(),
                            path: "releases/2026/config.json".into(),
                            is_dir: false,
                            size: 18739,
                            modified: Some("2026-10-06".into()),
                        },
                        RemoteEntry {
                            name: "README.md".into(),
                            path: "releases/2026/README.md".into(),
                            is_dir: false,
                            size: 430,
                            modified: None,
                        },
                    ];
                    browser.selected = Some("releases/2026/config.json".into());
                    let mut h = Harness::builder()
                        .with_size([width, 660.0])
                        .with_pixels_per_point(1.3)
                        .wgpu()
                        .build_ui_state(
                            |ui, b: &mut RemoteBrowser| {
                                Theme::current().apply(ui.ctx());
                                if super::super::test_fonts(ui.ctx()) {
                                    b.ui(ui);
                                }
                            },
                            browser,
                        );
                    h.run();
                    assert!(h.query_by_label(tr("파일 업로드")).is_some());
                    h.render()
                        .unwrap()
                        .save(shots.join(format!("files-{}-{theme}-{width}.png", language.code())))
                        .unwrap();
                }
            }
        }
        kiln_common::i18n::set_language(kiln_common::i18n::Language::Korean);
        Theme::set_current("kiln-dark");
    }
    #[test]
    fn restored_remote_draft_never_uploads_and_remains_visible() {
        let dir = tempfile::tempdir().unwrap();
        let manager = RemoteManager::with_store(
            dir.path().join("connections.json"),
            std::sync::Arc::new(kiln_accounts::MemoryStore::new()),
        );
        let profile = ConnectionProfile {
            id: "restore".into(),
            name: "Restore fixture".into(),
            endpoint: crate::RemoteEndpoint::Sftp {
                alias: "localhost".into(),
                config_path: None,
                root: ".".into(),
                options: Default::default(),
            },
        };
        let mut browser = RemoteBrowser::new(manager, profile, ".".into());
        let draft = RemoteDraft {
            remote_path: "config.rs".into(),
            local_path: dir.path().join("config.rs"),
            text: "let pending = true;".into(),
            baseline: "let pending = false;".into(),
            original: b"let pending = false;".to_vec(),
        };
        browser.restore(&draft);
        assert!(browser.is_dirty());
        assert!(browser.pending.is_empty());
        assert!(browser.active.is_none());
        let mut h = Harness::builder()
            .with_size([800.0, 600.0])
            .build_ui_state(|ui, b: &mut RemoteBrowser| b.ui(ui), browser);
        h.run();
        assert!(h.query_by_label(tr("로컬에 보관")).is_some());
        assert_eq!(h.state().draft().unwrap().text, draft.text);
        assert!(h.state().secrets.is_none());
    }
}
