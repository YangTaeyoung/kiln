use super::{icon, protocol, root_path};
use crate::{
    ConnectionProfile, RemoteEndpoint, Secrets,
    aws_profiles::{self, Authentication, AwsProfile},
};
use egui::{RichText, Ui};
use kiln_accounts::CredentialStore;
use kiln_common::{Task, Theme, i18n::tr, icons::Icon};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub struct RemoteManager {
    inner: Arc<Inner>,
}
struct Inner {
    profiles: Mutex<Vec<ConnectionProfile>>,
    path: PathBuf,
    pub store: Arc<dyn CredentialStore>,
    load_error: Option<String>,
}
impl RemoteManager {
    pub fn load() -> Self {
        static MANAGERS: std::sync::OnceLock<
            Mutex<std::collections::HashMap<PathBuf, std::sync::Weak<Inner>>>,
        > = std::sync::OnceLock::new();
        let path = kiln_common::paths::config_file("remote-connections.json");
        let mut managers = MANAGERS.get_or_init(Default::default).lock().unwrap();
        if let Some(inner) = managers.get(&path).and_then(std::sync::Weak::upgrade) {
            return Self { inner };
        }
        let manager = Self::with_store(path.clone(), kiln_accounts::Env::real().store);
        managers.insert(path, Arc::downgrade(&manager.inner));
        manager
    }
    pub fn with_store(path: PathBuf, store: Arc<dyn CredentialStore>) -> Self {
        let loaded = match std::fs::read(&path) {
            Ok(data) => serde_json::from_slice(&data).map_err(|e| e.to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.to_string()),
        };
        let (profiles, load_error) = match loaded {
            Ok(p) => (p, None),
            Err(e) => (Vec::new(), Some(e)),
        };
        Self {
            inner: Arc::new(Inner {
                profiles: Mutex::new(profiles),
                path,
                store,
                load_error,
            }),
        }
    }
    pub fn profiles(&self) -> Vec<ConnectionProfile> {
        self.inner.profiles.lock().unwrap().clone()
    }
    pub fn get(&self, id: &str) -> Option<ConnectionProfile> {
        self.profiles().into_iter().find(|p| p.id == id)
    }
    pub fn store(&self) -> Arc<dyn CredentialStore> {
        self.inner.store.clone()
    }
    pub fn load_error(&self) -> Option<&str> {
        self.inner.load_error.as_deref()
    }
    pub fn save(&self, profile: ConnectionProfile, secrets: Secrets) -> Result<(), String> {
        if let Some(e) = self.load_error() {
            return Err(e.to_owned());
        }
        profile.validate().map_err(|e| e.to_string())?;
        let mut current = self.inner.profiles.lock().unwrap();
        let mut next = current.clone();
        if let Some(old) = next.iter_mut().find(|p| p.id == profile.id) {
            *old = profile.clone()
        } else {
            next.push(profile.clone())
        }
        let previous = crate::load_secrets(self.inner.store.as_ref(), &profile.id)
            .map_err(|e| e.to_string())?;
        crate::save_secrets(self.inner.store.as_ref(), &profile.id, &secrets)
            .map_err(|e| e.to_string())?;
        let result = (|| {
            let parent = self.inner.path.parent().ok_or("Invalid connection path")?;
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            let data = serde_json::to_vec_pretty(&next).map_err(|e| e.to_string())?;
            kiln_common::safe_file::write(&self.inner.path, &data).map_err(|e| e.to_string())
        })();
        if let Err(e) = result {
            let _ = crate::save_secrets(self.inner.store.as_ref(), &profile.id, &previous);
            return Err(e);
        }
        *current = next;
        Ok(())
    }
    pub fn remove(&self, id: &str) -> Result<(), String> {
        if let Some(e) = self.load_error() {
            return Err(e.to_owned());
        }
        let mut current = self.inner.profiles.lock().unwrap();
        let mut next = current.clone();
        next.retain(|p| p.id != id);
        let previous =
            crate::load_secrets(self.inner.store.as_ref(), id).map_err(|e| e.to_string())?;
        self.inner
            .store
            .delete("dev.kiln.remote", id)
            .map_err(|e| e.to_string())?;
        if let Err(error) = kiln_common::safe_file::write(
            &self.inner.path,
            &serde_json::to_vec_pretty(&next).map_err(|e| e.to_string())?,
        ) {
            let _ = crate::save_secrets(self.inner.store.as_ref(), id, &previous);
            return Err(error.to_string());
        }
        *current = next;
        Ok(())
    }
}
#[derive(Debug)]
pub enum RemoteEvent {
    Open {
        connection: String,
        path: String,
    },
    Ssh {
        alias: String,
        config_path: Option<PathBuf>,
        options: crate::ssh_config::SshOptions,
    },
}
#[derive(Default)]
struct Form {
    id: String,
    name: String,
    kind: usize,
    host: String,
    port: String,
    user: String,
    root: String,
    tls: bool,
    bucket: String,
    region: String,
    endpoint: String,
    path_style: bool,
    ssh: super::ssh_form::SshForm,
    password: String,
    access_key: String,
    secret_key: String,
    session_token: String,
    clear_secrets: bool,
    aws_mode: usize,
    aws_profile: String,
    aws_profiles: Vec<AwsProfile>,
    aws_scan: Option<Task<Result<(Vec<AwsProfile>, bool, bool), String>>>,
    aws_scanned: bool,
    aws_cli_available: bool,
    aws_error: Option<String>,
    auth_touched: bool,
    region_touched: bool,
    legacy_auth: bool,

    error: Option<String>,
}
impl Form {
    fn new() -> Self {
        Self {
            region: "us-east-1".into(),
            root: "/".into(),
            port: "21".into(),
            tls: true,
            ..Default::default()
        }
    }
    fn apply_scan(&mut self, profiles: Vec<AwsProfile>, cli: bool, has_saved: bool) {
        self.aws_profiles = profiles;
        self.aws_cli_available = cli;
        if self.auth_touched {
            return;
        }
        if !self.id.is_empty() {
            if self.legacy_auth {
                self.aws_mode = if has_saved { 2 } else { 1 };
            }
            return;
        }
        if self.aws_profile.is_empty() {
            if let Some(profile) = self
                .aws_profiles
                .iter()
                .find(|p| {
                    p.name == "default"
                        && (p.authentication == Authentication::Static
                            || (p.authentication == Authentication::External && cli))
                })
                .or_else(|| {
                    self.aws_profiles.iter().find(|p| {
                        p.authentication == Authentication::Static
                            || (p.authentication == Authentication::External && cli)
                    })
                })
            {
                self.aws_mode = 0;
                self.aws_profile = profile.name.clone();
                if !self.region_touched
                    && let Some(region) = &profile.region
                {
                    self.region = region.clone();
                }
            } else {
                self.aws_mode = 1;
            }
        }
    }
    fn profile(&self) -> Result<ConnectionProfile, String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err(tr("연결 이름을 입력하세요").into());
        }
        let endpoint = match self.kind {
            0 => {
                if self.aws_mode == 0 && self.aws_profile.is_empty() {
                    return Err(tr("AWS 프로필을 선택하세요").into());
                }
                if self.aws_mode == 0 {
                    let selected = self
                        .aws_profiles
                        .iter()
                        .find(|p| p.name == self.aws_profile)
                        .ok_or_else(|| tr("선택한 AWS 프로필을 찾을 수 없습니다").to_owned())?;
                    if selected.authentication == Authentication::Missing {
                        return Err(tr("이 AWS 프로필에 인증 정보가 없습니다").into());
                    }
                    if selected.authentication == Authentication::External
                        && !self.aws_cli_available
                    {
                        return Err(tr("이 AWS 프로필은 AWS CLI v2가 필요합니다").into());
                    }
                }
                if self.bucket.trim().is_empty() {
                    return Err(tr("버킷 이름을 입력하세요").into());
                }
                RemoteEndpoint::S3 {
                    bucket: self.bucket.trim().into(),
                    region: self.region.trim().into(),
                    endpoint: (!self.endpoint.trim().is_empty())
                        .then(|| self.endpoint.trim().into()),
                    path_style: self.path_style,
                    prefix: self.root.trim_start_matches('/').into(),
                    aws_profile: (self.aws_mode == 0).then(|| self.aws_profile.clone()),
                    aws_auth: if self.aws_mode == 0 {
                        None
                    } else {
                        Some(if self.aws_mode == 1 {
                            crate::S3Authentication::Default
                        } else {
                            crate::S3Authentication::Manual
                        })
                    },
                }
            }
            1 => {
                if self.host.trim().is_empty() || self.user.trim().is_empty() {
                    return Err(tr("호스트와 사용자 이름을 입력하세요").into());
                }
                RemoteEndpoint::Ftp {
                    host: self.host.trim().into(),
                    port: self
                        .port
                        .parse()
                        .map_err(|_| tr("유효한 포트를 입력하세요").to_owned())?,
                    username: self.user.trim().into(),
                    tls: self.tls,
                    root: self.root.clone(),
                }
            }
            _ => self.ssh.endpoint(&self.root)?,
        };
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = if self.id.is_empty() {
            format!(
                "remote-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos(),
                SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            )
        } else {
            self.id.clone()
        };
        Ok(ConnectionProfile {
            id,
            name: name.into(),
            endpoint,
        })
    }
    fn from_profile(p: &ConnectionProfile) -> Self {
        let mut f = Self::new();
        f.id = p.id.clone();
        f.name = p.name.clone();
        match &p.endpoint {
            RemoteEndpoint::S3 {
                bucket,
                region,
                endpoint,
                path_style,
                prefix,
                aws_profile,
                aws_auth,
            } => {
                f.bucket = bucket.clone();
                f.region = region.clone();
                f.endpoint = endpoint.clone().unwrap_or_default();
                f.path_style = *path_style;
                f.root = prefix.clone();
                f.aws_profile = aws_profile.clone().unwrap_or_default();
                f.aws_mode = if aws_profile.is_some() {
                    0
                } else if *aws_auth == Some(crate::S3Authentication::Default) {
                    1
                } else {
                    2
                };
                f.legacy_auth = aws_profile.is_none() && aws_auth.is_none();
                f.region_touched = true;
            }
            RemoteEndpoint::Ftp {
                host,
                port,
                username,
                tls,
                root,
            } => {
                f.kind = 1;
                f.host = host.clone();
                f.port = port.to_string();
                f.user = username.clone();
                f.tls = *tls;
                f.root = root.clone()
            }
            RemoteEndpoint::Sftp {
                alias,
                config_path,
                root,
                options,
            } => {
                f.kind = 2;
                f.ssh = super::ssh_form::SshForm::from_connection(alias, config_path, options);
                f.root = root.clone()
            }
        }
        f
    }
}
pub struct RemotePanel {
    manager: RemoteManager,
    ssh_config_path: Option<PathBuf>,
    form: Option<Form>,
    saving: Option<Task<Result<(), String>>>,
    error: Option<String>,
    remove: Option<ConnectionProfile>,
}
impl RemotePanel {
    pub fn new(manager: RemoteManager) -> Self {
        Self {
            manager,
            ssh_config_path: None,
            form: None,
            saving: None,
            error: None,
            remove: None,
        }
    }
    /// Use a chosen config as the discovery source for new connections.
    /// Existing connections retain their own saved source and options.
    pub fn with_ssh_config_path(mut self, path: PathBuf) -> Self {
        self.ssh_config_path = Some(path);
        self
    }
    fn new_form(&self) -> Form {
        let mut form = Form::new();
        if let Some(path) = &self.ssh_config_path {
            form.ssh = super::ssh_form::SshForm::with_config(path.clone());
        }
        form
    }
    pub fn ui(&mut self, ui: &mut Ui) -> Vec<RemoteEvent> {
        let mut events = Vec::new();
        let theme = Theme::current();
        if let Some(result) = self.saving.as_mut().and_then(Task::take) {
            self.saving = None;
            match result {
                Ok(()) => self.form = None,
                Err(e) => {
                    if let Some(f) = &mut self.form {
                        f.error = Some(e)
                    } else {
                        self.error = Some(e)
                    }
                }
            }
        }
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon(ui, Icon::Plus, tr("연결 추가")) {
                    self.form = Some(self.new_form())
                }
                if icon(ui, Icon::Terminal, tr("SSH Config 가져오기")) {
                    let mut f = self.new_form();
                    f.kind = 2;
                    f.root = ".".into();
                    self.form = Some(f);
                }
                ui.menu_button("…", |ui| {
                    if ui.button(tr("SSH Config 파일 선택")).clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_file() {
                            let mut f = Form::new();
                            f.kind = 2;
                            f.root = ".".into();
                            f.ssh = super::ssh_form::SshForm::with_config(path);
                            self.form = Some(f);
                        }
                        ui.close();
                    }
                });
            });
        });
        if let Some(e) = self.manager.load_error() {
            ui.colored_label(theme.red, tr("연결 목록을 읽을 수 없습니다"));
            ui.label(e);
        }
        if let Some(e) = &self.error {
            ui.colored_label(theme.red, tr("원격 작업에 실패했습니다"));
            ui.collapsing(tr("오류 상세"), |ui| {
                ui.label(e);
            });
        }
        let profiles = self.manager.profiles();
        if profiles.is_empty() {
            ui.add_space(20.0);
            ui.label(tr("S3, FTP 또는 SSH 연결을 추가하세요"));
            if ui.button(tr("연결 추가")).clicked() {
                self.form = Some(self.new_form());
            }
        }
        egui::ScrollArea::vertical()
            .id_salt("remote-connections")
            .show(ui, |ui| {
                for p in profiles {
                    ui.push_id(&p.id, |ui| {
                        ui.horizontal(|ui| {
                            let entry = crate::RemoteEntry {
                                name: p.name.clone(),
                                path: p.id.clone(),
                                is_dir: true,
                                size: 0,
                                modified: None,
                            };
                            let row = ui
                                .allocate_ui_with_layout(
                                    egui::vec2((ui.available_width() - 68.0).max(50.0), 30.0),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        ui.set_min_width((ui.available_width()).max(50.0));
                                        super::row_with_icon(
                                            ui,
                                            &entry,
                                            false,
                                            super::provider_icon(&p),
                                        )
                                    },
                                )
                                .inner;
                            if row.clicked() {
                                events.push(RemoteEvent::Open {
                                    connection: p.id.clone(),
                                    path: root_path(&p),
                                })
                            }
                            row.on_hover_text(format!("{} · {}", protocol(&p), p.name));
                            if let RemoteEndpoint::Sftp {
                                alias, config_path, options, ..
                            } = &p.endpoint
                            {
                                if icon(ui, Icon::Terminal, tr("SSH 터미널 열기")) {
                                    events.push(RemoteEvent::Ssh {
                                        alias: alias.clone(),
                                        config_path: config_path.clone(),
                                        options: options.clone(),
                                    })
                                }
                            }
                            ui.menu_button("…", |ui| {
                                if ui.button(tr("연결 편집")).clicked() {
                                    self.form = Some(Form::from_profile(&p));
                                    ui.close()
                                }
                                if ui.button(tr("연결 삭제")).clicked() {
                                    self.remove = Some(p.clone());
                                    ui.close()
                                }
                            });
                        });
                        ui.label(RichText::new(protocol(&p)).small().color(theme.text_dim));
                    });
                }
            });
        if let Some(p) = self.remove.clone() {
            egui::Modal::new(egui::Id::new("remote-remove")).show(ui.ctx(), |ui| {
                ui.strong(tr("연결을 삭제할까요?"));
                ui.label(&p.name);
                ui.label(tr("원격 파일은 삭제되지 않습니다"));
                ui.horizontal(|ui| {
                    if ui.button(tr("취소")).clicked() {
                        self.remove = None
                    }
                    if ui.button(tr("삭제")).clicked() {
                        let manager = self.manager.clone();
                        let id = p.id.clone();
                        self.saving = Some(Task::spawn(ui.ctx(), move || manager.remove(&id)));
                        self.remove = None
                    }
                });
            });
        }
        self.form_ui(ui.ctx());
        events
    }
    fn form_ui(&mut self, ctx: &egui::Context) {
        let Some(f) = &mut self.form else { return };
        if f.kind == 0 && !f.aws_scanned {
            f.aws_scanned = true;
            if cfg!(test) {
                if f.aws_profiles.is_empty() && f.aws_profile.is_empty() {
                    f.aws_mode = 1;
                }
            } else {
                let files = aws_profiles::AwsFiles::current();
                let store = self.manager.store();
                let id = f.id.clone();
                f.aws_scan = Some(Task::spawn(ctx, move || {
                    let profiles = aws_profiles::discover(&files).map_err(|e| e.to_string())?;
                    let has_saved = !id.is_empty()
                        && crate::load_secrets(store.as_ref(), &id)
                            .map_err(|e| e.to_string())?
                            .access_key
                            .is_some();
                    Ok((profiles, aws_profiles::cli_path().is_some(), has_saved))
                }));
            }
        }
        if let Some(result) = f.aws_scan.as_mut().and_then(Task::take) {
            f.aws_scan = None;
            match result {
                Ok((profiles, cli, has_saved)) => {
                    f.apply_scan(profiles, cli, has_saved);
                }
                Err(error) => f.aws_error = Some(error),
            }
        }
        if f.aws_scan.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(40));
        }
        let busy = self.saving.is_some();
        let mut cancel = false;
        let mut save = false;
        egui::Modal::new(egui::Id::new("remote-connect-form")).show(ctx, |ui| {
            ui.set_width((ctx.content_rect().width() - 60.0).clamp(240.0, 450.0));
            ui.heading(if f.id.is_empty() { tr("연결 추가") } else { tr("연결 편집") });
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    if kiln_common::widgets::segmented_with_icons(ui, &mut f.kind, &[(0, Icon::S3, "S3"), (1, Icon::Server, "FTP"), (2, Icon::Terminal, "SFTP")]) {
                        f.root = String::new();
                    }
                });
            });
            ui.add_space(10.0);
            egui::ScrollArea::vertical()
                .max_height((ctx.content_rect().height() - 205.0).max(120.0))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.add_enabled_ui(!busy, |ui| {
                        field(ui, tr("연결 이름"), &mut f.name, false);
                        ui.add_space(8.0);
                        match f.kind {
                            0 => {
                                field(ui, tr("버킷"), &mut f.bucket, false);
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(tr("리전")).small().color(Theme::current().text_dim));
                                    f.region_touched |= ui.add(egui::TextEdit::singleline(&mut f.region).desired_width((ui.available_width()).min(210.0))).changed();
                                });
                                ui.add_space(8.0);
                                ui.separator();
                                ui.add_space(6.0);
                                ui.label(RichText::new(tr("인증 방식")).color(Theme::current().text_dim).small());
                                egui::ComboBox::from_id_salt("remote-auth-mode")
                                    .selected_text(tr(["저장된 AWS 프로필", "기본 AWS 인증", "직접 키 입력"][f.aws_mode]))
                                    .width(ui.available_width())
                                    .show_ui(ui, |ui| {
                                        for (mode, label) in [(0, "저장된 AWS 프로필"), (1, "기본 AWS 인증"), (2, "직접 키 입력")] {
                                            if ui.selectable_value(&mut f.aws_mode, mode, tr(label)).changed() { f.auth_touched = true; f.error = None; }
                                        }
                                    });
                                if f.aws_mode == 1 {
                                    ui.response().on_hover_text(tr("환경 변수와 기본 AWS 설정의 인증을 사용합니다"));
                                }
                                ui.add_space(4.0);
                                match f.aws_mode {
                                    0 => {
                                        if f.aws_scan.is_some() {
                                            ui.horizontal(|ui| { ui.spinner(); ui.label(tr("AWS 프로필 읽는 중…")); });
                                        } else {
                                            let selected = if f.aws_profile.is_empty() { tr("AWS 프로필을 선택하세요").to_owned() } else { f.aws_profile.clone() };
                                            let mut picked = None;
                                            egui::ComboBox::from_id_salt("remote-aws-profile").selected_text(selected).width(ui.available_width()).show_ui(ui, |ui| {
                                                ui.set_max_width((ui.ctx().content_rect().width()-56.0).min(380.0));
                                                for profile in &f.aws_profiles {
                                                    let usable = profile.authentication == Authentication::Static || (profile.authentication == Authentication::External && f.aws_cli_available);
                                                    let reason = if profile.authentication == Authentication::Missing { tr("이 AWS 프로필에 인증 정보가 없습니다") } else { tr("이 AWS 프로필은 AWS CLI v2가 필요합니다") };
                                                    let choice = ui.add_enabled(usable, egui::Button::selectable(f.aws_profile == profile.name, &profile.name).truncate()).on_hover_text(&profile.name).on_disabled_hover_text(reason);
                                                    if choice.clicked() { picked = Some(profile.clone()); }
                                                }
                                            });
                                            if let Some(profile) = picked { f.aws_profile = profile.name; if !f.region_touched && let Some(region) = profile.region { f.region = region; } }
                                            if f.aws_profiles.is_empty() { ui.label(RichText::new(tr("저장된 AWS 프로필이 없습니다")).small().color(Theme::current().text_dim)); }
                                            if let Some(profile) = f.aws_profiles.iter().find(|p| p.name == f.aws_profile) {
                                                let message = match profile.authentication {
                                                    Authentication::Missing => Some(tr("이 AWS 프로필에 인증 정보가 없습니다")),
                                                    Authentication::External if !f.aws_cli_available => Some(tr("이 AWS 프로필은 AWS CLI v2가 필요합니다")),
                                                    _ => None,
                                                };
                                                if let Some(message) = message { ui.colored_label(Theme::current().yellow, message); }
                                            }
                                        }
                                    }
                                    1 => {}
                                    _ => {
                                        field(ui, "Access Key ID", &mut f.access_key, false);
                                        field(ui, "Secret Access Key", &mut f.secret_key, true);
                                        let token = egui::CollapsingHeader::new(tr("임시 세션 토큰")).id_salt("remote-session-token").show(ui, |ui| { field(ui, "Session Token", &mut f.session_token, true) });
                                        if token.header_response.clicked() && let Some(response) = token.body_returned { response.scroll_to_me(Some(egui::Align::Max)); }
                                        if !f.id.is_empty() {
                                            ui.label(RichText::new(tr("비밀번호와 키를 비우면 저장된 값을 유지합니다")).small().color(Theme::current().text_dim));
                                        }
                                    }
                                }
                                if let Some(error) = &f.aws_error { ui.colored_label(Theme::current().red, error); }
                                ui.add_space(8.0);
                                let advanced = egui::CollapsingHeader::new(tr("고급 연결 설정")).id_salt("remote-s3-advanced").show(ui, |ui| {
                                    field(ui, tr("시작 경로"), &mut f.root, false);
                                    field(ui, tr("엔드포인트 (선택)"), &mut f.endpoint, false);
                                    ui.checkbox(&mut f.path_style, tr("경로 방식 주소 사용"));
                                });
                                if advanced.header_response.clicked() && let Some(body) = advanced.body_response { ui.scroll_to_rect(body.rect,Some(egui::Align::Max)); }
                            }
                            1 => {
                                field(ui, tr("호스트"), &mut f.host, false);
                                field(ui, tr("사용자 이름"), &mut f.user, false);
                                field(ui, tr("비밀번호"), &mut f.password, true);
                                ui.add_space(8.0);
                                egui::CollapsingHeader::new(tr("고급 연결 설정")).id_salt("remote-ftp-advanced").show(ui, |ui| {
                                    field(ui, tr("포트"), &mut f.port, false);
                                    field(ui, tr("시작 경로"), &mut f.root, false);
                                    ui.checkbox(&mut f.tls, tr("TLS로 연결 (FTPS)"));
                                });
                                if !f.id.is_empty() { ui.checkbox(&mut f.clear_secrets, tr("저장된 자격증명 제거")); }
                            }
                            _ => {
                                f.ssh.ui(ui, &mut f.name);
                                field(ui, tr("시작 경로"), &mut f.root, false);
                            }
                        }
                    });
                });
            if let Some(e) = &f.error {
                egui::ScrollArea::vertical().id_salt("remote-form-error").max_height(52.0).show(ui, |ui| { ui.colored_label(Theme::current().red, e); });
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    if kiln_common::widgets::button(ui, tr("취소"), kiln_common::widgets::ButtonKind::Ghost).clicked() { cancel = true; }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_enabled_ui(!busy && (f.kind != 0 || f.aws_scan.is_none()), |ui| {
                        if kiln_common::widgets::button(ui, tr("연결 저장"), kiln_common::widgets::ButtonKind::Primary).clicked() { save = true; }
                    });
                    if busy { ui.spinner(); }
                });
            });
        });
        if save {
            match f.profile() {
                Err(e) => f.error = Some(e),
                Ok(profile) => {
                    let mgr = self.manager.clone();
                    let new = Secrets {
                        password: (!f.password.is_empty()).then(|| f.password.clone()),
                        access_key: (!f.access_key.is_empty()).then(|| f.access_key.clone()),
                        secret_key: (!f.secret_key.is_empty()).then(|| f.secret_key.clone()),
                        session_token: (!f.session_token.is_empty())
                            .then(|| f.session_token.clone()),
                    };
                    let profile_auth = f.kind == 0 && f.aws_mode != 2;
                    let manual = f.kind == 0 && f.aws_mode == 2;
                    let clear = f.clear_secrets || profile_auth;
                    let new = if profile_auth {
                        Secrets::default()
                    } else {
                        new
                    };
                    self.saving = Some(Task::spawn(ctx, move || {
                        let previous = mgr.get(&profile.id);
                        let same_kind = previous.as_ref().is_some_and(|p| {
                            std::mem::discriminant(&p.endpoint)
                                == std::mem::discriminant(&profile.endpoint)
                        });
                        let old = if clear || !same_kind {
                            Secrets::default()
                        } else {
                            crate::load_secrets(mgr.store().as_ref(), &profile.id)
                                .map_err(|e| e.to_string())?
                        };
                        if manual && new.access_key.is_some() != new.secret_key.is_some() {
                            return Err(
                                tr("Access Key ID 및 Secret Access Key를 함께 입력하세요").into()
                            );
                        }
                        let merged = merge_secrets(new, old);
                        if manual && (merged.access_key.is_none() || merged.secret_key.is_none()) {
                            return Err(
                                tr("Access Key ID 및 Secret Access Key를 함께 입력하세요").into()
                            );
                        }
                        mgr.save(profile, merged)
                    }));
                }
            }
        }
        if cancel {
            self.form = None;
        }
    }
}
fn merge_secrets(new: Secrets, old: Secrets) -> Secrets {
    let replaces_s3_keys = new.access_key.is_some() || new.secret_key.is_some();
    Secrets {
        password: new.password.or(old.password),
        access_key: new.access_key.or(old.access_key),
        secret_key: new.secret_key.or(old.secret_key),
        // A temporary session token belongs to its original key pair. Never
        // silently carry it across an explicit key replacement.
        session_token: new.session_token.or(if replaces_s3_keys {
            None
        } else {
            old.session_token
        }),
    }
}
pub(super) fn field(ui: &mut Ui, label: &str, value: &mut String, password: bool) -> egui::Response {
    ui.label(
        RichText::new(label)
            .small()
            .color(Theme::current().text_dim),
    );
    ui.add(
        egui::TextEdit::singleline(value)
            .password(password)
            .desired_width(f32::INFINITY),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};
    pub(super) fn aws_fixture() -> Vec<AwsProfile> {
        vec![
            AwsProfile {
                name: "development".into(),
                region: Some("ap-northeast-2".into()),
                authentication: Authentication::Static,
            },
            AwsProfile {
                name: "production".into(),
                region: Some("eu-west-1".into()),
                authentication: Authentication::External,
            },
        ]
    }
    #[test]
    fn scan_recommendation_preserves_auth_choices_and_edited_regions() {
        let mut fresh = Form::new();
        fresh.apply_scan(aws_fixture(), true, false);
        assert_eq!(fresh.aws_mode, 0);
        assert_eq!(fresh.aws_profile, "development");
        assert_eq!(fresh.region, "ap-northeast-2");
        let mut edited = Form::new();
        edited.region = "us-west-2".into();
        edited.region_touched = true;
        edited.apply_scan(aws_fixture(), true, false);
        assert_eq!(edited.region, "us-west-2");
        let mut existing = Form::from_profile(&ConnectionProfile {
            id: "existing".into(),
            name: "Default".into(),
            endpoint: RemoteEndpoint::S3 {
                bucket: "fixture".into(),
                region: "eu-central-1".into(),
                endpoint: None,
                path_style: false,
                prefix: String::new(),
                aws_profile: None,
                aws_auth: Some(crate::S3Authentication::Default),
            },
        });
        existing.apply_scan(aws_fixture(), true, false);
        assert_eq!(existing.aws_mode, 1);
        assert_eq!(existing.region, "eu-central-1");
        assert!(existing.aws_profile.is_empty());
        let mut manual = Form::new();
        manual.aws_mode = 2;
        manual.auth_touched = true;
        manual.apply_scan(aws_fixture(), true, false);
        assert_eq!(manual.aws_mode, 2);
        let mut missing = Form::new();
        missing.name = "Missing".into();
        missing.bucket = "fixture".into();
        missing.aws_profile = "deleted".into();
        missing.apply_scan(aws_fixture(), true, false);
        assert!(missing.profile().is_err());
    }
    #[test]
    fn named_profile_and_manual_auth_have_distinct_actual_ui_fields() {
        let dir = tempfile::tempdir().unwrap();
        let manager = RemoteManager::with_store(
            dir.path().join("connections.json"),
            Arc::new(kiln_accounts::MemoryStore::new()),
        );
        let mut panel = RemotePanel::new(manager);
        let mut form = Form::new();
        form.name = "Fixture".into();
        form.bucket = "fixture".into();
        form.aws_scanned = true;
        form.apply_scan(aws_fixture(), true, false);
        panel.form = Some(form);
        let mut h = Harness::builder().with_size([420.0, 700.0]).build_ui_state(
            |ui, p: &mut RemotePanel| {
                if super::super::test_fonts(ui.ctx()) {
                    p.ui(ui);
                }
            },
            panel,
        );
        h.run();
        assert!(h.query_by_value("development").is_some());
        assert!(h.query_by_label("Access Key ID").is_none());
        h.state_mut().form.as_mut().unwrap().region = "us-west-2".into();
        h.state_mut().form.as_mut().unwrap().region_touched = true;
        h.get_by_value("development").click();
        h.run();
        h.get_by_label("production").click();
        h.run();
        assert_eq!(h.state().form.as_ref().unwrap().aws_profile, "production");
        assert_eq!(h.state().form.as_ref().unwrap().region, "us-west-2");
        h.get_by_label(tr("연결 저장")).click();
        for _ in 0..200 {
            h.step();
            if h.state().form.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(h.state().form.is_none());
        let profile = h.state().manager.profiles()[0].clone();
        assert!(
            matches!(&profile.endpoint, RemoteEndpoint::S3 {aws_profile:Some(p),region,..} if p == "production" && region == "us-west-2")
        );
        let mut form = Form::from_profile(&profile);
        form.aws_scanned = true;
        form.apply_scan(aws_fixture(), true, false);
        h.state_mut().form = Some(form);
        h.run();
        assert!(h.query_by_value("production").is_some());
        h.get_by_label(tr("취소")).click();
        h.run();
        assert!(h.state().form.is_none());
        assert_eq!(h.state().manager.get(&profile.id).unwrap(), profile);
        let mut form = Form::from_profile(&profile);
        form.aws_scanned = true;
        form.apply_scan(aws_fixture(), true, false);
        h.state_mut().form = Some(form);
        h.run();
        h.get_by_value(tr("저장된 AWS 프로필")).click();
        h.run();
        h.get_by_label(tr("직접 키 입력")).click();
        h.run();
        assert_eq!(h.state().form.as_ref().unwrap().aws_mode, 2);
        assert!(h.query_by_label("Access Key ID").is_some());
        h.get_by_value(tr("직접 키 입력")).click();
        h.run();
        h.get_by_label(tr("기본 AWS 인증")).click();
        h.run();
        assert_eq!(h.state().form.as_ref().unwrap().aws_mode, 1);
        assert!(h.query_by_label("Access Key ID").is_none());
        h.get_by_label(tr("고급 연결 설정")).click();
        h.run();
        assert!(h.query_by_label(tr("엔드포인트 (선택)")).is_some());
        h.get_by_label(tr("고급 연결 설정")).click();
        h.run();
        assert!(h.query_by_label(tr("엔드포인트 (선택)")).is_none());
    }
    #[test]
    fn manual_half_key_edit_preserves_store_and_default_switch_persists() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(kiln_accounts::MemoryStore::new());
        let manager = RemoteManager::with_store(dir.path().join("connections.json"), store.clone());
        let profile = ConnectionProfile {
            id: "manual".into(),
            name: "Fixture".into(),
            endpoint: RemoteEndpoint::S3 {
                bucket: "fixture".into(),
                region: "us-east-1".into(),
                endpoint: None,
                path_style: false,
                prefix: String::new(),
                aws_profile: None,
                aws_auth: Some(crate::S3Authentication::Manual),
            },
        };
        manager
            .save(
                profile.clone(),
                Secrets {
                    access_key: Some("old-access".into()),
                    secret_key: Some("old-secret".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let mut panel = RemotePanel::new(manager.clone());
        let mut form = Form::from_profile(&profile);
        form.aws_scanned = true;
        form.apply_scan(aws_fixture(), true, true);
        form.access_key = "new-half-access".into();
        panel.form = Some(form);
        let mut h = Harness::builder().with_size([420.0, 700.0]).build_ui_state(
            |ui, p: &mut RemotePanel| {
                if super::super::test_fonts(ui.ctx()) {
                    p.ui(ui);
                }
            },
            panel,
        );
        h.run();
        h.get_by_label(tr("연결 저장")).click();
        for _ in 0..200 {
            h.step();
            if h.state().saving.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(h.state().form.as_ref().unwrap().error.is_some());
        assert_eq!(
            crate::load_secrets(store.as_ref(), "manual")
                .unwrap()
                .access_key
                .as_deref(),
            Some("old-access")
        );
        h.run();
        h.get_by_value(tr("직접 키 입력")).click();
        h.run();
        h.get_by_label(tr("기본 AWS 인증")).click();
        h.run();
        h.get_by_label(tr("연결 저장")).click();
        for _ in 0..200 {
            h.step();
            if h.state().form.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(h.state().form.is_none());
        assert!(
            crate::load_secrets(store.as_ref(), "manual")
                .unwrap()
                .access_key
                .is_none()
        );
        let saved = manager.get("manual").unwrap();
        let mut reopened = Form::from_profile(&saved);
        reopened.apply_scan(aws_fixture(), true, false);
        assert_eq!(reopened.aws_mode, 1);
        assert!(reopened.aws_profile.is_empty());
    }
    #[test]
    fn legacy_s3_metadata_derives_auth_without_changing_identity() {
        let profile:ConnectionProfile=serde_json::from_str(r#"{"id":"legacy","name":"Legacy","endpoint":{"S3":{"bucket":"fixture","region":"eu-west-1","endpoint":null,"path_style":false,"prefix":"assets/"}}}"#).unwrap();
        let mut with_keys = Form::from_profile(&profile);
        with_keys.apply_scan(aws_fixture(), true, true);
        assert_eq!(with_keys.aws_mode, 2);
        assert_eq!(with_keys.region, "eu-west-1");
        assert!(with_keys.aws_profile.is_empty());
        let mut without_keys = Form::from_profile(&profile);
        without_keys.apply_scan(aws_fixture(), true, false);
        assert_eq!(without_keys.aws_mode, 1);
        assert!(without_keys.aws_profile.is_empty());
    }
    #[test]
    fn connection_form_keeps_errors_and_stores_only_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(kiln_accounts::MemoryStore::new());
        let manager = RemoteManager::with_store(dir.path().join("connections.json"), store.clone());
        let mut panel = RemotePanel::new(manager.clone());
        let mut form = Form::new();
        form.kind = 1;
        form.name = "Test FTP".into();
        form.host = "localhost".into();
        form.user = "fixture".into();
        form.password = "private-fixture".into();
        panel.form = Some(form);
        let mut h = Harness::builder().with_size([620.0, 740.0]).build_ui_state(
            |ui, panel: &mut RemotePanel| {
                if super::super::test_fonts(ui.ctx()) {
                    panel.ui(ui);
                }
            },
            panel,
        );
        h.run();
        h.get_by_label(tr("연결 저장")).click();
        for _ in 0..200 {
            h.step();
            if h.state().form.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(h.state().form.is_none());
        assert_eq!(manager.profiles().len(), 1);
        let data = std::fs::read_to_string(dir.path().join("connections.json")).unwrap();
        assert!(!data.contains("private-fixture"));
        let id = manager.profiles()[0].id.clone();
        assert_eq!(
            crate::load_secrets(store.as_ref(), &id)
                .unwrap()
                .password
                .as_deref(),
            Some("private-fixture")
        );
        let mut form = Form::new();
        form.kind = 1;
        form.name = "Missing host".into();
        h.state_mut().form = Some(form);
        h.run();
        h.get_by_label(tr("연결 저장")).click();
        h.run();
        assert!(h.state().form.as_ref().unwrap().error.is_some());
        assert_eq!(manager.profiles().len(), 1);
    }
    #[test]
    fn failed_metadata_removal_restores_credentials_and_connection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        let store = Arc::new(kiln_accounts::MemoryStore::new());
        let manager = RemoteManager::with_store(path.clone(), store.clone());
        manager
            .save(
                ConnectionProfile {
                    id: "rollback".into(),
                    name: "Rollback".into(),
                    endpoint: RemoteEndpoint::Ftp {
                        host: "localhost".into(),
                        port: 21,
                        username: "fixture".into(),
                        tls: false,
                        root: "/".into(),
                    },
                },
                Secrets {
                    password: Some("fixture-only".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap(); // Force atomic metadata replacement to fail.
        assert!(manager.remove("rollback").is_err());
        assert_eq!(manager.profiles().len(), 1);
        assert_eq!(
            crate::load_secrets(store.as_ref(), "rollback")
                .unwrap()
                .password
                .as_deref(),
            Some("fixture-only")
        );
    }
    #[test]
    fn replacing_s3_keys_does_not_reuse_the_old_session_token() {
        let old = Secrets {
            access_key: Some("old-access".into()),
            secret_key: Some("old-secret".into()),
            session_token: Some("old-session".into()),
            ..Default::default()
        };
        let kept = merge_secrets(Secrets::default(), old.clone());
        assert_eq!(kept.session_token.as_deref(), Some("old-session"));
        let changed = merge_secrets(
            Secrets {
                access_key: Some("new-access".into()),
                secret_key: Some("new-secret".into()),
                ..Default::default()
            },
            old,
        );
        assert_eq!(changed.session_token, None);
        assert_eq!(changed.access_key.as_deref(), Some("new-access"));
    }
    #[test]
    fn damaged_metadata_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        std::fs::write(&path, b"broken").unwrap();
        let manager =
            RemoteManager::with_store(path.clone(), Arc::new(kiln_accounts::MemoryStore::new()));
        assert!(
            manager
                .save(
                    ConnectionProfile {
                        id: "test".into(),
                        name: "test".into(),
                        endpoint: RemoteEndpoint::Sftp {
                            alias: "localhost".into(),
                            config_path: None,
                            root: "/".into(),
                            options: Default::default(),
                        }
                    },
                    Secrets::default()
                )
                .is_err()
        );
        assert_eq!(std::fs::read(path).unwrap(), b"broken");
    }
}

#[cfg(test)]
mod visual_tests {
    use super::*;
    use egui_kittest::{
        Harness,
        kittest::{NodeT, Queryable},
    };
    #[test]
    fn provider_connections_and_selector_are_identifiable() {
        let dir = tempfile::tempdir().unwrap();
        let manager = RemoteManager::with_store(
            dir.path().join("connections.json"),
            Arc::new(kiln_accounts::MemoryStore::new()),
        );
        let mut s3 = Form::new();
        s3.name = "Production assets".into();
        s3.bucket = "fixture-assets".into();
        s3.aws_mode = 1;
        manager
            .save(s3.profile().unwrap(), Secrets::default())
            .unwrap();
        let mut ftp = Form::new();
        ftp.kind = 1;
        ftp.name = "Build server".into();
        ftp.host = "127.0.0.1".into();
        ftp.user = "fixture".into();
        manager
            .save(ftp.profile().unwrap(), Secrets::default())
            .unwrap();
        let mut ssh = Form::new();
        ssh.kind = 2;
        ssh.name = "Remote workspace".into();
        ssh.ssh = super::super::ssh_form::SshForm::from_connection("synthetic-host", &None, &Default::default());
        manager
            .save(ssh.profile().unwrap(), Secrets::default())
            .unwrap();
        let shots = PathBuf::from("/tmp/kiln-remote-captures");
        std::fs::create_dir_all(&shots).unwrap();
        for language in kiln_common::i18n::Language::ALL {
            kiln_common::i18n::set_language(language);
            for theme in ["kiln-dark", "kiln-light"] {
                Theme::set_current(theme);
                for width in [420.0, 980.0] {
                    let panel = RemotePanel::new(manager.clone());
                    let mut h = Harness::builder()
                        .with_size([width, 660.0])
                        .with_pixels_per_point(1.3)
                        .wgpu()
                        .build_ui_state(
                            |ui, panel: &mut RemotePanel| {
                                Theme::current().apply(ui.ctx());
                                if super::super::test_fonts(ui.ctx()) {
                                    panel.ui(ui);
                                }
                            },
                            panel,
                        );
                    h.run();
                    for name in ["Production assets", "Build server", "Remote workspace"] {
                        assert!(h.query_all_by_label(name).next().is_some());
                    }
                    assert!(h.query_by_label(tr("SSH 터미널 열기")).is_some());
                    h.render()
                        .unwrap()
                        .save(shots.join(format!(
                            "connections-{}-{theme}-{width}.png",
                            language.code()
                        )))
                        .unwrap();
                    h.get_by_label(tr("연결 추가")).click();
                    h.run();
                    h.query_all_by_label("FTP")
                        .find(|node| node.accesskit_node().toggled().is_some())
                        .expect("protocol radio")
                        .click();
                    h.run();
                    assert_eq!(h.state().form.as_ref().unwrap().kind, 1);
                    h.query_all_by_label("SFTP")
                        .find(|node| node.accesskit_node().toggled().is_some())
                        .expect("protocol radio")
                        .click();
                    h.run();
                    assert_eq!(h.state().form.as_ref().unwrap().kind, 2);
                    h.query_all_by_label("S3")
                        .find(|node| node.accesskit_node().toggled().is_some())
                        .expect("protocol radio")
                        .click();
                    h.run();
                    assert_eq!(h.state().form.as_ref().unwrap().kind, 0);
                    h.get_by_label(tr("취소")).click();
                    h.run();
                    assert!(h.state().form.is_none());
                }
            }
        }
        kiln_common::i18n::set_language(kiln_common::i18n::Language::Korean);
        Theme::set_current("kiln-dark");
    }

    #[test]
    fn connection_forms_fit_the_compact_viewport() {
        let dir = tempfile::tempdir().unwrap();
        let manager = RemoteManager::with_store(
            dir.path().join("connections.json"),
            Arc::new(kiln_accounts::MemoryStore::new()),
        );
        let shots = PathBuf::from("/tmp/kiln-remote-captures");
        std::fs::create_dir_all(&shots).unwrap();
        for language in kiln_common::i18n::Language::ALL {
            kiln_common::i18n::set_language(language);
            for theme in ["kiln-dark", "kiln-light"] {
                Theme::set_current(theme);
                for width in [420.0, 980.0] {
                    for kind in 0..3 {
                        for variant in 0..if kind == 0 { 10 } else { 1 } {
                            let mut panel = RemotePanel::new(manager.clone());
                            let mut f = Form::new();
                            f.kind = kind;
                            f.name = "Production assets".into();
                            f.aws_scanned = true;
                            f.apply_scan(tests::aws_fixture(), true, false);
                            if kind == 0 {
                                f.bucket = "production-assets".into();
                                if variant == 1 {
                                    f.aws_mode = 1;
                                }
                                if variant == 2 || variant == 3 {
                                    f.aws_mode = 2;
                                }
                                if variant == 4 {
                                    f.error =
                                        Some(tr("선택한 AWS 프로필을 찾을 수 없습니다").into());
                                    f.aws_profile = "unavailable-profile".into();
                                }
                                if variant == 6 {
                                    f.aws_profiles.clear();
                                    f.aws_profile.clear();
                                }
                                if variant == 8 {
                                    f.aws_mode = 2;
                                }
                                if variant == 9 {
                                    f.aws_profiles.push(AwsProfile {
                                        name: "restricted-account-with-a-long-profile-name".into(),
                                        region: None,
                                        authentication: Authentication::Missing,
                                    });
                                }
                                if variant == 5 {
                                    f.id = "edited-fixture".into();
                                    f.region_touched = true;
                                }
                            }
                            panel.form = Some(f);
                            let mut busy_release = None;
                            if kind == 0 && variant == 7 {
                                let (sender, receiver) = std::sync::mpsc::channel::<()>();
                                busy_release = Some(sender);
                                panel.saving =
                                    Some(Task::spawn(&egui::Context::default(), move || {
                                        let _ = receiver.recv();
                                        Ok(())
                                    }));
                            }
                            let mut h = Harness::builder()
                                .with_size([width, 700.0])
                                .with_pixels_per_point(1.3)
                                .wgpu()
                                .build_ui_state(
                                    |ui, p: &mut RemotePanel| {
                                        Theme::current().apply(ui.ctx());
                                        if super::super::test_fonts(ui.ctx()) {
                                            p.ui(ui);
                                        }
                                    },
                                    panel,
                                );
                            if kind == 0 && variant == 7 {
                                h.run_steps(4);
                                assert!(
                                    h.get_by_label(tr("연결 저장"))
                                        .accesskit_node()
                                        .is_disabled()
                                );
                            } else {
                                h.run();
                            }
                            if kind == 0 && (variant == 3 || variant == 8) {
                                let heading = if variant == 3 {
                                    tr("고급 연결 설정")
                                } else {
                                    tr("임시 세션 토큰")
                                };
                                h.get_by_label(heading).scroll_to_me();
                                h.run();
                                h.get_by_label(heading).click();
                                h.run();
                                let expected = if variant == 3 {
                                    tr("엔드포인트 (선택)")
                                } else {
                                    "Session Token"
                                };
                                assert!(h.query_by_label(expected).is_some());
                            }
                            if kind == 0 && variant == 9 {
                                h.get_by_value("development").click();
                                h.run();
                                h.get_by_label("restricted-account-with-a-long-profile-name")
                                    .hover();
                                h.run_steps(45);
                            }
                            h.render()
                                .unwrap()
                                .save(shots.join(format!(
                                    "form-{}-{theme}-{width}-{kind}-state{variant}.png",
                                    language.code()
                                )))
                                .unwrap();
                            drop(busy_release);
                        }
                    }
                }
            }
        }
        kiln_common::i18n::set_language(kiln_common::i18n::Language::Korean);
        Theme::set_current("kiln-dark");
    }
}
