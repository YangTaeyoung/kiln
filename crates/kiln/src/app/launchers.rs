//! Local named commands, reviewed before explicit execution in a fresh terminal.
//! References: https://cmux.com/docs/custom-commands
//! https://docs.warp.dev/knowledge-and-collaboration/warp-drive/workflows
use std::path::{Path, PathBuf};
use egui::{Color32, CornerRadius, Frame, Key, Margin, RichText, Stroke};
use kiln_common::{fonts, Theme, widgets::{self, ButtonKind}};
use serde::{Deserialize, Serialize};

const MAX_COMMANDS: usize = 200;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedCommand {
    pub name: String,
    pub command: String,
    pub cwd: Option<PathBuf>,
}

#[derive(Clone, Default, PartialEq, Serialize)]
struct Draft { name: String, command: String, cwd: String }
impl From<&SelectedCommand> for Draft {
    fn from(value: &SelectedCommand) -> Self {
        Self { name: value.name.clone(), command: value.command.clone(), cwd: value.cwd.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default() }
    }
}
impl Draft {
    fn validate(&self) -> Result<SelectedCommand, String> {
        let name = self.name.trim();
        if name.is_empty() { return Err(kiln_common::i18n::tr("명령 이름을 입력하세요.").into()); }
        if name.chars().count() > 100 || name.chars().any(char::is_control) { return Err(kiln_common::i18n::tr("이름은 줄바꿈 없이 100자 이내로 입력하세요.").into()); }
        if self.command.trim().is_empty() { return Err(kiln_common::i18n::tr("실행할 명령을 입력하세요.").into()); }
        if self.command.len() > 16_384 { return Err(kiln_common::i18n::tr("명령은 16KB 이내로 입력하세요.").into()); }
        if self.command.chars().any(|c| c.is_control() && c != '\n' && c != '\t') { return Err(kiln_common::i18n::tr("명령에 지원하지 않는 제어 문자가 있습니다.").into()); }
        let raw = self.cwd.trim();
        let cwd = if raw.is_empty() { None } else {
            if raw.len() > 4096 || raw.chars().any(char::is_control) { return Err(kiln_common::i18n::tr("작업 폴더 경로를 확인하세요.").into()); }
            let path = if raw == "~" || raw.starts_with("~/") {
                let home = std::env::var_os("HOME").ok_or(kiln_common::i18n::tr("홈 폴더를 찾을 수 없습니다. 전체 경로를 입력하세요."))?;
                PathBuf::from(home).join(raw.strip_prefix("~/").unwrap_or(""))
            } else { PathBuf::from(raw) };
            if !path.is_absolute() { return Err(kiln_common::i18n::tr("작업 폴더는 전체 경로 또는 ~/로 입력하세요.").into()); }
            Some(path)
        };
        // Preserve command whitespace: trimming would change shell heredoc content.
        Ok(SelectedCommand { name: name.into(), command: self.command.clone(), cwd })
    }
}

#[derive(Clone)]
enum Screen {
    List,
    Preview(usize),
    Edit { index: Option<usize>, draft: Draft, original: Draft },
    Delete(usize),
    Discard { edit: Box<Screen>, close: bool },
}
enum Intent { None, Close, List, Preview(usize), Edit(Option<usize>), Save, Delete(usize), ConfirmDelete(usize), Run(usize), DiscardEdit(bool), ResumeEdit, Reload, Recover }

pub struct Launcher {
    open: bool,
    entries: Vec<SelectedCommand>,
    path: PathBuf,
    disk: Option<Vec<u8>>,
    load_failed: bool,
    error: Option<String>,
    notice: Option<String>,
    query: String,
    selected: usize,
    focus_search: bool,
    focus_primary: bool,
    focus_name: bool,
    screen: Screen,
}
impl Default for Launcher {
    fn default() -> Self { Self::load() }
}
impl Launcher {
    pub fn load() -> Self { Self::from_path(kiln_common::paths::config_file("commands.json")) }
    fn from_path(path: PathBuf) -> Self {
        let mut this = Self { open: false, entries: vec![], path, disk: None, load_failed: false, error: None, notice: None, query: String::new(), selected: 0, focus_search: false, focus_primary: false, focus_name: false, screen: Screen::List };
        this.reload(); this
    }
    fn reload(&mut self) {
        match read_commands(&self.path) {
            Ok((entries, disk)) => { self.entries = entries; self.disk = disk; self.load_failed = false; self.error = None; }
            Err(error) => { self.load_failed = true; self.disk = std::fs::metadata(&self.path).ok().filter(|m| m.len() <= MAX_FILE_BYTES).and_then(|_| std::fs::read(&self.path).ok()); self.error = Some(kiln_common::trf!("저장된 명령을 읽지 못했습니다. 원본은 유지됩니다. 다시 불러오거나 원본을 백업한 뒤 빈 목록으로 복구하세요. 파일: {} · {error}", self.path.display())); }
        }
    }
    fn recover_empty(&mut self) -> Result<PathBuf, String> {
        use std::io::Write;
        if !self.load_failed { return Err(kiln_common::i18n::tr("명령 목록을 정상적으로 읽었습니다. 복구할 필요가 없습니다.").into()); }
        let expected = self.disk.as_ref().ok_or(kiln_common::i18n::tr("원본을 읽지 못해 복구를 중단했습니다. 파일 크기와 읽기 권한을 확인한 뒤 다시 불러오세요."))?;
        let bytes = std::fs::read(&self.path).map_err(|e| kiln_common::trf!("원본을 읽지 못했습니다. 파일 권한을 확인하세요: {e}"))?;
        if &bytes != expected { return Err(kiln_common::i18n::tr("명령 파일이 변경되었습니다. 다시 불러온 뒤 복구하세요.").into()); }
        let mut backup = None;
        for n in 1..=1000 {
            let candidate = self.path.with_extension(format!("json.backup-{n}"));
            let mut options = std::fs::OpenOptions::new(); options.write(true).create_new(true);
            #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
            match options.open(&candidate) {
                Ok(mut file) => { file.write_all(&bytes).and_then(|_| file.sync_all()).map_err(|e| kiln_common::trf!("백업 저장 실패. 원본은 유지됩니다: {e}"))?; backup = Some(candidate); break; }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(kiln_common::trf!("백업 생성 실패. 원본은 유지됩니다: {e}")),
            }
        }
        let backup = backup.ok_or(kiln_common::i18n::tr("백업 파일 이름을 만들지 못했습니다. 원본은 유지됩니다."))?;
        if std::fs::read(&self.path).map_err(|e| e.to_string())? != bytes { return Err(kiln_common::i18n::tr("백업 중 원본이 변경되었습니다. 다시 불러온 뒤 복구하세요.").into()); }
        kiln_common::store::save_json(&self.path, &Vec::<SelectedCommand>::new()).map_err(|e| kiln_common::trf!("빈 목록 저장 실패. 백업: {} · {e}", backup.display()))?;
        self.reload();
        if self.load_failed { return Err(kiln_common::i18n::tr("복구한 명령 목록을 다시 읽지 못했습니다. 다시 불러오기를 눌러주세요.").into()); }
        self.screen = Screen::List; self.query.clear(); self.selected = 0;
        Ok(backup)
    }
    pub fn open(&mut self) {
        if self.open { return; }
        self.reload(); self.open = true; self.screen = Screen::List; self.query.clear(); self.selected = 0; self.focus_search = true;
    }
    pub fn is_open(&self) -> bool { self.open }
    pub fn has_unsaved_edits(&self) -> bool {
        fn dirty(screen: &Screen) -> bool {
            match screen {
                Screen::Edit { draft, original, .. } => draft != original,
                Screen::Discard { edit, .. } => dirty(edit),
                _ => false,
            }
        }
        dirty(&self.screen)
    }
    fn persist(&mut self, entries: Vec<SelectedCommand>) -> Result<(), String> {
        if self.load_failed { return Err(kiln_common::i18n::tr("명령 파일을 먼저 다시 불러오세요.").into()); }
        let (_, current) = read_commands(&self.path)?;
        if current != self.disk { return Err(kiln_common::i18n::tr("다른 창에서 명령 파일을 변경했습니다. 목록으로 돌아가 다시 불러온 뒤 편집하세요.").into()); }
        if entries.len() > MAX_COMMANDS { return Err(kiln_common::i18n::tr("저장할 수 있는 명령은 최대 200개입니다.").into()); }
        for entry in &entries { Draft::from(entry).validate()?; }
        let bytes = serde_json::to_vec_pretty(&entries).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_FILE_BYTES { return Err(kiln_common::i18n::tr("명령 파일이 4MB 제한을 초과했습니다. 명령 수나 내용을 줄여주세요.").into()); }
        kiln_common::store::save_json(&self.path, &entries).map_err(|e| kiln_common::trf!("저장하지 못했습니다. 입력한 내용은 유지됩니다. {e}"))?;
        self.disk = Some(bytes); self.entries = entries; Ok(())
    }
    pub fn ui(&mut self, ctx: &egui::Context, default_cwd: &Path) -> Option<SelectedCommand> {
        if !self.open { return None; }
        let t = Theme::current();
        let viewport = ctx.content_rect();
        let width = (viewport.width() - 48.0).clamp(240.0, 660.0);
        let error_budget = if self.error.is_some() || self.notice.is_some() { 66.0 } else { 0.0 };
        let body_height = (viewport.height() - 158.0 - error_budget).clamp(48.0, 440.0);
        let frame = Frame::new().fill(t.bg_elevated).stroke(Stroke::new(1.0, t.border_strong)).corner_radius(CornerRadius::same(12)).shadow(t.shadow()).inner_margin(Margin::same(16));
        let mut intent = Intent::None;
        let mut screen = self.screen.clone();
        let modal = egui::Modal::new(egui::Id::new("saved-command-launcher")).frame(frame).backdrop_color(Color32::from_black_alpha(if t.dark { 120 } else { 50 })).show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().scroll.floating = false;
            ui.spacing_mut().scroll.dormant_handle_opacity = 0.65;
            ui.horizontal(|ui| {
                let title = match screen { Screen::List => kiln_common::i18n::tr("저장 명령"), Screen::Preview(_) => kiln_common::i18n::tr("명령 실행"), Screen::Edit { index: Some(_), .. } => kiln_common::i18n::tr("명령 편집"), Screen::Edit { .. } => kiln_common::i18n::tr("새 명령"), Screen::Delete(_) => kiln_common::i18n::tr("명령 삭제"), Screen::Discard { .. } => kiln_common::i18n::tr("편집 취소") };
                ui.label(RichText::new(title).font(fonts::semibold(20.0)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::icon_button(ui, kiln_common::icons::Icon::Close, 28.0, false, kiln_common::i18n::tr("명령 창 닫기")).clicked() { intent = Intent::Close; }
                });
            });
            ui.add_space(10.0);
            egui::ScrollArea::vertical().id_salt("saved-command-content").max_height(body_height).auto_shrink([false, true]).show(ui, |ui| {
                match &mut screen {
                    Screen::List => {
                        let search = ui.add(egui::TextEdit::singleline(&mut self.query).hint_text(kiln_common::i18n::tr("이름 또는 명령 검색")).desired_width(f32::INFINITY));
                        if self.focus_search { search.request_focus(); self.focus_search = false; }
                        if search.changed() { self.selected = 0; }
                        ui.add_space(8.0);
                        let query = self.query.trim().to_lowercase();
                        let visible: Vec<_> = self.entries.iter().enumerate().filter(|(_, item)| query.is_empty() || item.name.to_lowercase().contains(&query) || item.command.to_lowercase().contains(&query)).collect();
                        self.selected = self.selected.min(visible.len().saturating_sub(1));
                        let down = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowDown));
                        let up = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowUp));
                        if !visible.is_empty() {
                            if down { self.selected = (self.selected + 1) % visible.len(); }
                            if up { self.selected = (self.selected + visible.len() - 1) % visible.len(); }
                            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)) { intent = Intent::Preview(visible[self.selected].0); }
                        }
                        if visible.is_empty() {
                            ui.add_space(12.0);
                            ui.label(RichText::new(if self.entries.is_empty() { kiln_common::i18n::tr("반복하는 작업을 한 번 저장해두세요.") } else { kiln_common::i18n::tr("일치하는 명령이 없습니다. 검색어를 바꿔보세요.") }).color(t.text_dim));
                            ui.add_space(12.0);
                        }
                        for (position, (index, item)) in visible.iter().enumerate() {
                            let label = format!("{}\n{}", item.name, item.command.lines().next().unwrap_or("").chars().take(90).collect::<String>());
                            let response = ui.add(egui::Button::new(RichText::new(label).font(fonts::regular(13.0))).selected(position == self.selected).min_size(egui::vec2(ui.available_width(), 48.0)).wrap());
                            if response.clicked() { intent = Intent::Preview(*index); }
                            if position == self.selected && (up || down) { response.scroll_to_me(Some(egui::Align::Center)); }
                        }
                    }
                    Screen::Edit { draft, .. } => {
                        let label = ui.label(kiln_common::i18n::tr("이름"));
                        let name = ui.add(egui::TextEdit::singleline(&mut draft.name).hint_text(kiln_common::i18n::tr("예: 개발 서버")).desired_width(f32::INFINITY)).labelled_by(label.id);
                        if self.focus_name { name.request_focus(); self.focus_name = false; }
                        let label = ui.label(kiln_common::i18n::tr("명령"));
                        ui.add(egui::TextEdit::multiline(&mut draft.command).code_editor().desired_rows(4).hint_text(kiln_common::i18n::tr("예: npm run dev")).desired_width(f32::INFINITY)).labelled_by(label.id);
                        let label = ui.label(kiln_common::i18n::tr("작업 폴더 · 선택 사항"));
                        ui.add(egui::TextEdit::singleline(&mut draft.cwd).hint_text(kiln_common::i18n::tr("비우면 실행 시 현재 프로젝트 폴더")).desired_width(f32::INFINITY)).labelled_by(label.id);
                        ui.add(egui::Label::new(RichText::new(kiln_common::i18n::tr("명령은 이 컴퓨터에 저장됩니다. 저장만으로 실행되지 않습니다.")).size(12.0).color(t.text_dim)).wrap());
                    }
                    Screen::Preview(index) => if let Some(item) = self.entries.get(*index) {
                        ui.label(RichText::new(&item.name).font(fonts::semibold(16.0)));
                        ui.label(RichText::new(kiln_common::i18n::tr("실행할 명령")).color(t.text_dim));
                        ui.add(egui::Label::new(RichText::new(&item.command).font(fonts::mono(13.0))).wrap().selectable(true));
                        ui.add_space(10.0);
                        ui.label(RichText::new(kiln_common::i18n::tr("작업 폴더")).color(t.text_dim));
                        ui.add(egui::Label::new(item.cwd.as_deref().unwrap_or(default_cwd).display().to_string()).wrap().selectable(true));
                        ui.add_space(8.0);
                        ui.label(RichText::new(kiln_common::i18n::tr("새 터미널에서 실행합니다.")).size(12.0).color(t.text_dim));
                    },
                    Screen::Delete(index) => if let Some(item) = self.entries.get(*index) {
                        ui.add(egui::Label::new(kiln_common::trf!("다음 저장 명령을 삭제할까요?\n{}", item.name)).wrap());
                        ui.label(RichText::new(kiln_common::i18n::tr("실행 중인 터미널에는 영향을 주지 않습니다.")).color(t.text_dim));
                    },
                    Screen::Discard { .. } => { ui.label(kiln_common::i18n::tr("저장하지 않은 편집 내용을 버릴까요?")); },
                }
            });
            if let Some(error) = self.error.as_ref().or(self.notice.as_ref()) {
                ui.add_space(8.0);
                egui::ScrollArea::vertical().id_salt("saved-command-error").max_height(50.0).show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(error).size(12.5).color(if self.error.is_some() {t.red} else {t.green})).wrap());
                });
            }
            ui.add_space(12.0);
            widgets::divider(ui);
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| match &screen {
                Screen::List => {
                    ui.add_enabled_ui(!self.load_failed && self.entries.len() < MAX_COMMANDS, |ui| { if widgets::button(ui, kiln_common::i18n::tr("새 명령"), ButtonKind::Primary).clicked() { intent = Intent::Edit(None); } });
                    if widgets::button(ui, kiln_common::i18n::tr("다시 불러오기"), ButtonKind::Ghost).clicked() { intent = Intent::Reload; }
                    if self.load_failed && widgets::button(ui, kiln_common::i18n::tr("원본 백업 후 빈 목록으로 복구"), ButtonKind::Secondary).clicked() { intent = Intent::Recover; }
                    ui.label(RichText::new(kiln_common::i18n::tr("↑↓ 선택 · Enter 미리보기")).size(11.5).color(t.text_dim));
                }
                Screen::Edit { draft, .. } => {
                    if self.error.is_some() && widgets::button(ui, kiln_common::i18n::tr("초안 내보내기…"), ButtonKind::Secondary).clicked() {
                        if let Some(path) = rfd::FileDialog::new().set_file_name("kiln-command-draft.json").save_file() {
                            match kiln_common::store::save_json(&path, draft) { Ok(()) => self.notice = Some(kiln_common::trf!("초안을 내보냈습니다: {}", path.display())), Err(error) => self.error = Some(kiln_common::trf!("초안 내보내기 실패. 입력은 유지됩니다: {error}")) }
                        }
                    }
                    if widgets::button(ui, kiln_common::i18n::tr("명령 저장"), ButtonKind::Primary).clicked() { intent = Intent::Save; }
                    if widgets::button(ui, kiln_common::i18n::tr("편집 취소"), ButtonKind::Secondary).clicked() { intent = Intent::List; }
                }
                Screen::Preview(index) => {
                    let run = widgets::button(ui, kiln_common::i18n::tr("새 터미널에서 실행"), ButtonKind::Primary);
                    if self.focus_primary { run.request_focus(); self.focus_primary = false; }
                    if run.clicked() { intent = Intent::Run(*index); }
                    if widgets::button(ui, kiln_common::i18n::tr("편집"), ButtonKind::Secondary).clicked() { intent = Intent::Edit(Some(*index)); }
                    if widgets::button(ui, kiln_common::i18n::tr("삭제"), ButtonKind::Ghost).clicked() { intent = Intent::Delete(*index); }
                    if widgets::button(ui, kiln_common::i18n::tr("목록"), ButtonKind::Ghost).clicked() { intent = Intent::List; }
                }
                Screen::Delete(index) => {
                    if widgets::button(ui, kiln_common::i18n::tr("명령 삭제"), ButtonKind::Danger).clicked() { intent = Intent::ConfirmDelete(*index); }
                    if widgets::button(ui, kiln_common::i18n::tr("유지"), ButtonKind::Secondary).clicked() { intent = Intent::Preview(*index); }
                }
                Screen::Discard { close, .. } => {
                    if widgets::button(ui, kiln_common::i18n::tr("편집 버리기"), ButtonKind::Danger).clicked() { intent = Intent::DiscardEdit(*close); }
                    if widgets::button(ui, kiln_common::i18n::tr("계속 편집"), ButtonKind::Secondary).clicked() { intent = Intent::ResumeEdit; }
                }
            });
        });
        self.screen = screen;
        if modal.should_close() && matches!(intent, Intent::None) { intent = Intent::Close; }
        self.apply_intent(intent, default_cwd)
    }
    fn apply_intent(&mut self, intent: Intent, default_cwd: &Path) -> Option<SelectedCommand> {
        match intent {
            Intent::None => {},
            Intent::Close | Intent::List => {
                let close = matches!(intent, Intent::Close);
                if let Screen::Edit { draft, original, .. } = &self.screen {
                    if draft != original { self.screen = Screen::Discard { edit: Box::new(self.screen.clone()), close }; return None; }
                }
                if matches!(self.screen, Screen::Discard { .. }) { self.apply_intent(Intent::ResumeEdit, default_cwd); return None; }
                self.screen = Screen::List; self.error = None; self.open = !close;
            }
            Intent::Preview(index) => { self.screen = Screen::Preview(index); self.error = None; self.focus_primary = true; }
            Intent::Edit(index) => {
                let draft = index.and_then(|i| self.entries.get(i)).map(Draft::from).unwrap_or_default();
                self.screen = Screen::Edit { index, original: draft.clone(), draft }; self.error = None; self.focus_name = true;
            }
            Intent::Save => if let Screen::Edit { index, draft, .. } = self.screen.clone() {
                let result = draft.validate().and_then(|entry| {
                    let mut entries = self.entries.clone();
                    if let Some(index) = index { entries[index] = entry; }
                    else if entries.len() < MAX_COMMANDS { entries.push(entry); }
                    else { return Err(kiln_common::i18n::tr("저장할 수 있는 명령은 최대 200개입니다.").into()); }
                    self.persist(entries)
                });
                match result { Ok(()) => { self.screen = Screen::List; self.query.clear(); self.error = None; self.focus_search = true; }, Err(error) => self.error = Some(error) }
            },
            Intent::Delete(index) => { self.screen = Screen::Delete(index); self.error = None; }
            Intent::ConfirmDelete(index) => {
                let mut entries = self.entries.clone();
                if index < entries.len() { entries.remove(index); }
                match self.persist(entries) { Ok(()) => { self.screen = Screen::List; self.error = None; }, Err(error) => self.error = Some(error) }
            }
            Intent::Run(index) => if let Some(item) = self.entries.get(index) {
                let cwd = item.cwd.as_deref().unwrap_or(default_cwd);
                if !cwd.is_dir() { self.error = Some(kiln_common::i18n::tr("작업 폴더를 찾을 수 없습니다. 명령을 편집해 경로를 수정하세요.").into()); }
                else { self.open = false; return Some(item.clone()); }
            },
            Intent::DiscardEdit(close) => { self.screen = Screen::List; self.error = None; self.open = !close; },
            Intent::ResumeEdit => if let Screen::Discard { edit, .. } = &self.screen { self.screen = *edit.clone(); },
            Intent::Reload => { self.reload(); self.selected = 0; }
            Intent::Recover => { match self.recover_empty() { Ok(path) => self.notice = Some(kiln_common::trf!("빈 명령 목록으로 복구했습니다. 원본 백업: {}", path.display())), Err(error) => self.error = Some(error) } }
        }
        None
    }
}

fn read_commands(path: &Path) -> Result<(Vec<SelectedCommand>, Option<Vec<u8>>), String> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((vec![], None)),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.len() > MAX_FILE_BYTES { return Err(kiln_common::i18n::tr("명령 파일이 4MB 제한을 초과했습니다.").into()); }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let entries: Vec<SelectedCommand> = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if entries.len() > MAX_COMMANDS { return Err(kiln_common::i18n::tr("명령 파일에는 최대 200개만 저장할 수 있습니다.").into()); }
    let entries = entries.iter().map(|entry| Draft::from(entry).validate()).collect::<Result<Vec<_>, _>>()?;
    Ok((entries, Some(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn corrupt_recovery_preserves_backups_rejects_external_changes_and_allows_execution() {
        let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("commands.json");
        std::fs::write(&path, "broken-json").unwrap();
        std::fs::write(path.with_extension("json.backup-1"), "existing backup").unwrap();
        let mut launcher = Launcher::from_path(path.clone());
        std::fs::write(&path, "external edit").unwrap();
        assert!(launcher.recover_empty().is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external edit");
        launcher.reload();
        let backup = launcher.recover_empty().unwrap();
        assert_eq!(std::fs::read_to_string(backup).unwrap(), "external edit");
        assert_eq!(std::fs::read_to_string(path.with_extension("json.backup-1")).unwrap(), "existing backup");
        assert!(launcher.entries.is_empty()); assert!(!launcher.load_failed);
        launcher.persist(vec![SelectedCommand{name:"test".into(),command:"echo ok".into(),cwd:None}]).unwrap();
        assert_eq!(launcher.apply_intent(Intent::Run(0),dir.path()).unwrap().command,"echo ok");
    }
    #[test]
    fn validates_fields_without_altering_shell_content() {
        let mut draft = Draft { name: "  테스트  ".into(), command: "cat <<'EOF'\n  content  \nEOF\n".into(), cwd: String::new() };
        let entry = draft.validate().unwrap();
        assert_eq!(entry.name, "테스트"); assert_eq!(entry.command, draft.command); assert_eq!(entry.cwd, None);
        draft.cwd = "relative".into(); assert!(draft.validate().is_err());
        draft.cwd.clear(); draft.command.push('\0'); assert!(draft.validate().is_err());
        draft.command = "  \n".into(); assert!(draft.validate().is_err());
        draft.command = "echo ok".into(); draft.name.clear(); assert!(draft.validate().is_err());
    }
    #[test]
    fn missing_corrupt_and_external_changes_never_overwrite_originals() {
        let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("commands.json");
        let mut launcher = Launcher::from_path(path.clone());
        assert!(!launcher.load_failed);
        let entry = SelectedCommand { name: "Test".into(), command: "echo ok".into(), cwd: None };
        launcher.persist(vec![entry.clone()]).unwrap();
        assert_eq!(Launcher::from_path(path.clone()).entries, vec![entry.clone()]);
        std::fs::write(&path, "broken-json").unwrap();
        assert!(launcher.persist(vec![]).is_err());
        let mut corrupt = Launcher::from_path(path.clone()); assert!(corrupt.load_failed);
        assert!(corrupt.persist(vec![entry]).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "broken-json");
    }
    #[test]
    fn saving_and_canceling_never_execute_and_delete_requires_confirmation() {
        let dir = tempfile::tempdir().unwrap(); let mut launcher = Launcher::from_path(dir.path().join("commands.json"));
        launcher.open(); launcher.apply_intent(Intent::Edit(None), dir.path());
        if let Screen::Edit { draft, .. } = &mut launcher.screen { draft.name = "Test".into(); draft.command = "echo ok".into(); }
        assert!(launcher.apply_intent(Intent::Save, dir.path()).is_none()); assert_eq!(launcher.entries.len(), 1);
        assert!(launcher.apply_intent(Intent::Delete(0), dir.path()).is_none()); assert_eq!(launcher.entries.len(), 1);
        launcher.apply_intent(Intent::Preview(0), dir.path()); assert_eq!(launcher.entries.len(), 1);
        assert_eq!(launcher.apply_intent(Intent::Run(0), dir.path()).unwrap().command, "echo ok");
        launcher.apply_intent(Intent::ConfirmDelete(0), dir.path()); assert!(launcher.entries.is_empty());
    }
    #[test]
    fn invalid_execution_directory_keeps_preview_open() {
        let dir = tempfile::tempdir().unwrap(); let mut launcher = Launcher::from_path(dir.path().join("commands.json"));
        launcher.persist(vec![SelectedCommand { name: "Test".into(), command: "echo ok".into(), cwd: Some(dir.path().join("missing")) }]).unwrap();
        launcher.open(); launcher.apply_intent(Intent::Preview(0), dir.path());
        assert!(launcher.apply_intent(Intent::Run(0), dir.path()).is_none()); assert!(launcher.is_open()); assert!(launcher.error.is_some());
    }
    #[test]
    fn keyboard_requires_preview_then_separate_execution_and_footer_fits_small_view() {
        use egui_kittest::{Harness, kittest::Queryable};
        let dir = tempfile::tempdir().unwrap();
        let mut launcher = Launcher::from_path(dir.path().join("commands.json"));
        launcher.persist(vec![SelectedCommand { name: "개발 서버".into(), command: "echo hello".into(), cwd: None }]).unwrap();
        launcher.open();
        let mut initialized = false;
        let mut h = Harness::builder().with_size([720.0 / 1.3, 440.0 / 1.3]).build_ui_state(|ui, state: &mut (Launcher, Option<SelectedCommand>)| {
            if !initialized { fonts::install(ui.ctx()); Theme::current().apply(ui.ctx()); initialized = true; return; }
            if let Some(command) = state.0.ui(ui.ctx(), dir.path()) { state.1 = Some(command); }
        }, (launcher, None));
        h.run_steps(3);
        h.key_press(Key::Enter); h.run_steps(3);
        assert!(matches!(h.state().0.screen, Screen::Preview(0)));
        assert!(h.state().1.is_none(), "selecting a result must never run it");
        assert!(h.ctx.content_rect().contains_rect(h.get_by_label("새 터미널에서 실행").rect()));
        h.render().unwrap().save("/tmp/kiln-launcher-preview-small.png").unwrap();
        h.key_press(Key::Enter); h.run_steps(2);
        assert_eq!(h.state().1.as_ref().unwrap().command, "echo hello");
    }

    #[test]
    fn edit_cancel_preserves_draft_and_save_failure_preserves_existing_commands() {
        use egui_kittest::{Harness, kittest::Queryable};
        let dir = tempfile::tempdir().unwrap();
        let mut launcher = Launcher::from_path(dir.path().join("commands.json"));
        launcher.open(); launcher.apply_intent(Intent::Edit(None), dir.path());
        if let Screen::Edit { draft, .. } = &mut launcher.screen { draft.name = "Test".into(); draft.command = "echo hi".into(); }
        let mut initialized = false;
        let mut h = Harness::builder().with_size([720.0 / 1.3, 440.0 / 1.3]).build_ui_state(|ui, state: &mut Launcher| {
            if !initialized { fonts::install(ui.ctx()); Theme::current().apply(ui.ctx()); initialized = true; return; }
            assert!(state.ui(ui.ctx(), dir.path()).is_none());
        }, launcher);
        h.run_steps(3);
        assert!(h.ctx.content_rect().contains_rect(h.get_by_label("명령 저장").rect()));
        h.render().unwrap().save("/tmp/kiln-launcher-edit-small.png").unwrap();
        h.key_press(Key::Escape); h.run_steps(3);
        assert!(matches!(h.state().screen, Screen::Discard { .. }));
        h.get_by_label("계속 편집").click(); h.run_steps(3);
        assert!(matches!(&h.state().screen, Screen::Edit { draft, .. } if draft.command == "echo hi"));
        // Simulate another app replacing the file while this draft is open.
        std::fs::write(dir.path().join("commands.json"), "[]").unwrap();
        h.get_by_label("명령 저장").click(); h.run_steps(3);
        assert!(h.state().error.is_some());
        assert!(matches!(&h.state().screen, Screen::Edit { draft, .. } if draft.command == "echo hi"));
        assert_eq!(std::fs::read_to_string(dir.path().join("commands.json")).unwrap(), "[]");
    }

    #[test]
    fn serialized_size_limit_preserves_previous_file() {
        let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("commands.json");
        let mut launcher = Launcher::from_path(path.clone());
        launcher.persist(vec![]).unwrap();
        let oversized = vec![SelectedCommand { name: "Large".into(), command: "\"".repeat(16_384), cwd: None }; MAX_COMMANDS];
        assert!(launcher.persist(oversized).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "[]");
        assert!(launcher.entries.is_empty());
    }

    #[test]
    fn unsaved_edits_remain_dirty_during_discard_confirmation() {
        let dir = tempfile::tempdir().unwrap();
        let mut launcher = Launcher::from_path(dir.path().join("commands.json"));
        launcher.open(); launcher.apply_intent(Intent::Edit(None), dir.path());
        assert!(!launcher.has_unsaved_edits());
        if let Screen::Edit { draft, .. } = &mut launcher.screen { draft.command = "echo edited".into(); }
        assert!(launcher.has_unsaved_edits());
        launcher.apply_intent(Intent::Close, dir.path());
        assert!(matches!(launcher.screen, Screen::Discard { .. }));
        assert!(launcher.has_unsaved_edits());
        launcher.apply_intent(Intent::ResumeEdit, dir.path());
        assert!(launcher.has_unsaved_edits());
        launcher.apply_intent(Intent::DiscardEdit(false), dir.path());
        assert!(!launcher.has_unsaved_edits());
    }

    #[test]
    fn validation_error_stays_visible_after_scrolling_form() {
        use egui_kittest::{Harness, kittest::Queryable};
        let dir = tempfile::tempdir().unwrap();
        let mut launcher = Launcher::from_path(dir.path().join("commands.json"));
        launcher.open(); launcher.apply_intent(Intent::Edit(None), dir.path());
        if let Screen::Edit { draft, .. } = &mut launcher.screen { draft.command = "echo example".into(); }
        let mut initialized = false;
        let mut h = Harness::builder().with_size([720.0 / 1.3, 440.0 / 1.3]).build_ui_state(|ui, state: &mut Launcher| {
            if !initialized { fonts::install(ui.ctx()); Theme::current().apply(ui.ctx()); initialized = true; return; }
            assert!(state.ui(ui.ctx(), dir.path()).is_none());
        }, launcher);
        h.run_steps(3);
        h.event(egui::Event::PointerMoved(egui::pos2(250.0, 170.0)));
        h.event(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -600.0), phase: egui::TouchPhase::Move, modifiers: egui::Modifiers::NONE });
        h.run_steps(3);
        h.get_by_label("명령 저장").click(); h.run_steps(3);
        assert!(h.ctx.content_rect().contains_rect(h.get_by_label("명령 이름을 입력하세요.").rect()));
        assert!(h.ctx.content_rect().contains_rect(h.get_by_label("명령 저장").rect()));
        assert!(h.state().has_unsaved_edits());
        h.render().unwrap().save("/tmp/kiln-launcher-error-small.png").unwrap();
    }

    #[test]
    fn empty_and_populated_lists_keep_actions_visible() {
        use egui_kittest::{Harness, kittest::Queryable};
        let dir = tempfile::tempdir().unwrap();
        for populated in [false, true] {
            let mut launcher = Launcher::from_path(dir.path().join(if populated { "populated.json" } else { "empty.json" }));
            if populated {
                launcher.persist([("개발 서버", "npm run dev"), ("전체 테스트", "cargo test --workspace"), ("변경 확인", "git diff --stat"), ("타입 검사", "npm run typecheck"), ("로그 보기", "tail -f app.log")]
                    .into_iter().map(|(name, command)| SelectedCommand { name: name.into(), command: command.into(), cwd: None }).collect()).unwrap();
            }
            launcher.open();
            let mut initialized = false;
            let mut h = Harness::builder().with_size([720.0 / 1.3, 440.0 / 1.3]).build_ui_state(|ui, state: &mut Launcher| {
                if !initialized { fonts::install(ui.ctx()); Theme::current().apply(ui.ctx()); initialized = true; return; }
                assert!(state.ui(ui.ctx(), dir.path()).is_none());
            }, launcher);
            h.run_steps(4);
            assert!(h.ctx.content_rect().contains_rect(h.get_by_label("새 명령").rect()));
            let path = if populated { "/tmp/kiln-launcher-list-small.png" } else { "/tmp/kiln-launcher-empty-small.png" };
            h.render().unwrap().save(path).unwrap();
        }
    }

}
