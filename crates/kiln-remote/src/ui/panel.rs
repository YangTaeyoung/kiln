use super::{icon, protocol, root_path};
use crate::{ConnectionProfile, RemoteEndpoint, Secrets};
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
    alias: String,
    password: String,
    access_key: String,
    secret_key: String,
    session_token: String,
    config_path: Option<PathBuf>,
    clear_secrets: bool,
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
    fn profile(&self) -> Result<ConnectionProfile, String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err(tr("연결 이름을 입력하세요").into());
        }
        let endpoint = match self.kind {
            0 => {
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
            _ => {
                if self.alias.trim().is_empty() {
                    return Err(tr("SSH 호스트를 입력하세요").into());
                }
                RemoteEndpoint::Sftp {
                    alias: self.alias.trim().into(),
                    config_path: self.config_path.clone(),
                    root: self.root.clone(),
                }
            }
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
            } => {
                f.bucket = bucket.clone();
                f.region = region.clone();
                f.endpoint = endpoint.clone().unwrap_or_default();
                f.path_style = *path_style;
                f.root = prefix.clone()
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
            } => {
                f.kind = 2;
                f.alias = alias.clone();
                f.config_path = config_path.clone();
                f.root = root.clone()
            }
        }
        f
    }
}
pub struct RemotePanel {
    manager: RemoteManager,
    form: Option<Form>,
    saving: Option<Task<Result<(), String>>>,
    imports: Option<Task<Result<Vec<crate::ssh_config::SshHost>, String>>>,
    hosts: Vec<crate::ssh_config::SshHost>,
    show_hosts: bool,
    error: Option<String>,
    remove: Option<ConnectionProfile>,
}
impl RemotePanel {
    pub fn new(manager: RemoteManager) -> Self {
        Self {
            manager,
            form: None,
            saving: None,
            imports: None,
            hosts: Vec::new(),
            show_hosts: false,
            error: None,
            remove: None,
        }
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
        if let Some(result) = self.imports.as_mut().and_then(Task::take) {
            self.imports = None;
            self.show_hosts = true;
            match result {
                Ok(hosts) => self.hosts = hosts,
                Err(e) => self.error = Some(e),
            }
        }
        ui.horizontal(|ui| {
            ui.strong(tr("연결"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon(ui, Icon::Plus, tr("연결 추가")) {
                    self.form = Some(Form::new())
                }
                if icon(ui, Icon::Terminal, tr("SSH Config 가져오기")) {
                    let path = std::env::var_os("HOME")
                        .map(PathBuf::from)
                        .unwrap_or_default()
                        .join(".ssh/config");
                    self.imports = Some(Task::spawn(ui.ctx(), move || {
                        crate::ssh_config::discover_hosts(&path).map_err(|e| e.to_string())
                    }));
                }
                ui.menu_button("…", |ui| {
                    if ui.button(tr("SSH Config 파일 선택")).clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_file() {
                            self.imports = Some(Task::spawn(ui.ctx(), move || {
                                crate::ssh_config::discover_hosts(&path).map_err(|e| e.to_string())
                            }));
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
        if self.imports.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("SSH 연결을 읽는 중…"));
            });
        }
        let profiles = self.manager.profiles();
        if profiles.is_empty() {
            ui.add_space(20.0);
            ui.label(tr("S3, FTP 또는 SSH 연결을 추가하세요"));
            if ui.button(tr("연결 추가")).clicked() {
                self.form = Some(Form::new());
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
                                        super::file_row(ui, &entry, false)
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
                                alias, config_path, ..
                            } = &p.endpoint
                            {
                                if icon(ui, Icon::Terminal, tr("SSH 터미널 열기")) {
                                    events.push(RemoteEvent::Ssh {
                                        alias: alias.clone(),
                                        config_path: config_path.clone(),
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
        if self.show_hosts {
            egui::Window::new(tr("SSH Config 연결"))
                .id(egui::Id::new("ssh-import"))
                .collapsible(false)
                .default_width(450.0)
                .max_width((ui.ctx().content_rect().width() - 32.0).max(180.0))
                .max_height((ui.ctx().content_rect().height() - 32.0).max(160.0))
                .show(ui.ctx(), |ui| {
                    if self.hosts.is_empty() {
                        ui.label(tr("추가할 SSH 호스트가 없습니다"));
                    }
                    egui::ScrollArea::vertical()
                        .max_height(360.0)
                        .show(ui, |ui| {
                            for host in &self.hosts {
                                ui.horizontal_wrapped(|ui| {
                                    ui.strong(&host.alias);
                                    ui.label(&host.hostname);
                                    if ui.button(tr("추가")).clicked() {
                                        let mut f = Form::new();
                                        f.kind = 2;
                                        f.name = host.alias.clone();
                                        f.alias = host.alias.clone();
                                        f.root = ".".into();
                                        f.config_path = Some(host.config_path.clone());
                                        self.form = Some(f);
                                        self.show_hosts = false;
                                    }
                                });
                            }
                        });
                    if ui.button(tr("닫기")).clicked() {
                        self.show_hosts = false;
                    }
                });
        }
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
        let busy = self.saving.is_some();
        let mut cancel = false;
        let mut save = false;
        egui::Modal::new(egui::Id::new("remote-connect-form")).show(ctx, |ui| {
            ui.set_width((ctx.content_rect().width() - 60.0).clamp(240.0, 450.0));
            ui.heading(tr("원격 연결"));
            egui::ScrollArea::vertical()
                .max_height((ctx.content_rect().height() - 200.0).max(130.0))
                .show(ui, |ui| {
                    ui.add_enabled_ui(!busy, |ui| {
                        field(ui, tr("연결 이름"), &mut f.name, false);
                        ui.horizontal_wrapped(|ui| {
                            for (kind, label) in [(0, "S3"), (1, "FTP / FTPS"), (2, "SFTP / SSH")] {
                                if ui.selectable_label(f.kind == kind, label).clicked() {
                                    f.kind = kind;
                                    f.root = if kind == 0 { String::new() } else { "/".into() };
                                }
                            }
                        });
                        match f.kind {
                            0 => {
                                field(ui, tr("버킷"), &mut f.bucket, false);
                                field(ui, tr("리전"), &mut f.region, false);
                                field(ui, tr("시작 경로"), &mut f.root, false);
                                field(ui, "Access Key ID", &mut f.access_key, false);
                                field(ui, "Secret Access Key", &mut f.secret_key, true);
                                egui::CollapsingHeader::new(tr("고급 연결 설정"))
                                    .id_salt("remote-s3-advanced")
                                    .default_open(!f.endpoint.is_empty() || f.path_style)
                                    .show(ui, |ui| {
                                        field(ui, tr("엔드포인트 (선택)"), &mut f.endpoint, false);
                                        field(ui, "Session Token", &mut f.session_token, true);
                                        ui.checkbox(&mut f.path_style, tr("경로 방식 주소 사용"));
                                    });
                                ui.label(
                                    RichText::new(tr(
                                        "키를 비워 두면 기본 AWS 자격증명을 사용합니다",
                                    ))
                                    .small(),
                                );
                            }
                            1 => {
                                field(ui, tr("호스트"), &mut f.host, false);
                                field(ui, tr("포트"), &mut f.port, false);
                                field(ui, tr("사용자 이름"), &mut f.user, false);
                                field(ui, tr("비밀번호"), &mut f.password, true);
                                field(ui, tr("시작 경로"), &mut f.root, false);
                                ui.checkbox(&mut f.tls, tr("TLS로 연결 (FTPS)"));
                            }
                            _ => {
                                field(ui, tr("SSH 호스트"), &mut f.alias, false);
                                field(ui, tr("시작 경로"), &mut f.root, false);
                                ui.label(
                                    RichText::new(tr("SSH Config와 SSH Agent의 인증을 사용합니다"))
                                        .small(),
                                );
                            }
                        }
                        if !f.id.is_empty() {
                            ui.checkbox(&mut f.clear_secrets, tr("저장된 자격증명 제거"));
                            ui.label(
                                RichText::new(tr("비밀번호와 키를 비우면 저장된 값을 유지합니다"))
                                    .small(),
                            );
                        }
                    });
                });
            if let Some(e) = &f.error {
                ui.colored_label(Theme::current().red, e);
            }
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(!busy, egui::Button::new(tr("취소")))
                    .clicked()
                {
                    cancel = true
                }
                if ui
                    .add_enabled(!busy, egui::Button::new(tr("연결 저장")))
                    .clicked()
                {
                    save = true
                }
                if busy {
                    ui.spinner();
                }
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
                    let clear = f.clear_secrets;
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
                        let merged = merge_secrets(new, old);
                        if matches!(profile.endpoint, RemoteEndpoint::S3 { .. })
                            && merged.access_key.is_some() != merged.secret_key.is_some()
                        {
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
fn field(ui: &mut Ui, label: &str, value: &mut String, password: bool) {
    ui.label(label);
    ui.add(
        egui::TextEdit::singleline(value)
            .password(password)
            .desired_width(f32::INFINITY)
            .hint_text(label),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};
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
                panel.ui(ui);
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
                            root: "/".into()
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
    use egui_kittest::Harness;
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
                        let mut panel = RemotePanel::new(manager.clone());
                        let mut f = Form::new();
                        f.kind = kind;
                        f.name = "Production assets".into();
                        panel.form = Some(f);
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
                        h.run();
                        h.render()
                            .unwrap()
                            .save(shots.join(format!(
                                "form-{}-{theme}-{width}-{kind}.png",
                                language.code()
                            )))
                            .unwrap();
                    }
                }
            }
        }
        kiln_common::i18n::set_language(kiln_common::i18n::Language::Korean);
        Theme::set_current("kiln-dark");
    }
}
