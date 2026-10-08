use crate::{
    RemoteEndpoint,
    ssh_config::{self, SshHost, SshOptions},
};
use kiln_common::{Task, Theme, i18n::tr, icons::Icon};
use std::path::PathBuf;

#[derive(Clone, Default, PartialEq, Eq)]
struct Fields {
    host: String,
    user: String,
    port: String,
    identity: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn host(path: PathBuf) -> SshHost {
        SshHost {
            alias: "studio".into(),
            hostname: "10.23.4.5".into(),
            user: Some("deploy".into()),
            port: 2202,
            identity_files: vec![
                PathBuf::from("/fixture/키 with spaces"),
                PathBuf::from("/fixture/second"),
            ],
            config_path: path,
        }
    }
    fn options(endpoint: &RemoteEndpoint) -> &SshOptions {
        let RemoteEndpoint::Sftp { options, .. } = endpoint else {
            panic!()
        };
        options
    }

    #[test]
    fn config_selection_prefills_fields_without_freezing_config_values() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path().join("config"));
        let mut form = SshForm::default();
        let mut name = String::new();
        form.select(&host, &mut name);
        assert_eq!(name, "studio");
        assert_eq!(form.fields.host, "10.23.4.5");
        assert_eq!(form.fields.user, "deploy");
        assert_eq!(form.fields.port, "2202");
        assert_eq!(form.fields.identity, "/fixture/키 with spaces");
        let endpoint = form.endpoint("/work").unwrap();
        assert_eq!(options(&endpoint), &SshOptions::default());
        let RemoteEndpoint::Sftp {
            alias,
            config_path,
            root,
            ..
        } = endpoint
        else {
            panic!()
        };
        assert_eq!(alias, "studio");
        assert_eq!(config_path, Some(host.config_path));
        assert_eq!(root, "/work");
    }

    #[test]
    fn edits_are_scoped_and_same_host_selection_preserves_saved_options() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path().join("config"));
        let saved = SshOptions {
            user: Some("custom".into()),
            ..Default::default()
        };
        let mut form =
            SshForm::from_connection(&host.alias, &Some(host.config_path.clone()), &saved);
        form.apply_hosts(vec![host.clone()]);
        let mut name = "Edited name".into();
        form.select(&host, &mut name);
        assert_eq!(name, "Edited name");
        assert_eq!(options(&form.endpoint(".").unwrap()), &saved);
        form.fields.port = " 2300 ".into();
        let endpoint = form.endpoint(".").unwrap();
        assert_eq!(
            options(&endpoint),
            &SshOptions {
                user: Some("custom".into()),
                port: Some(2300),
                ..Default::default()
            }
        );
        form.fields.user.clear();
        assert_eq!(options(&form.endpoint(".").unwrap()).user, None);
    }

    #[test]
    fn late_discovery_preserves_typing_and_different_source_cannot_change_existing_target() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path().join("old-config"));
        let mut form = SshForm::from_connection(
            "studio",
            &Some(host.config_path.clone()),
            &Default::default(),
        );
        form.fields.user = "typed".into();
        form.apply_hosts(vec![host.clone()]);
        assert_eq!(form.fields.user, "typed");
        assert_eq!(form.fields.host, host.hostname);
        assert_eq!(
            options(&form.endpoint(".").unwrap()).user.as_deref(),
            Some("typed")
        );
        let mut other = host.clone();
        other.config_path = dir.path().join("other-config");
        other.hostname = "other.example.test".into();
        form.source = Some(other.config_path.clone());
        form.apply_hosts(vec![other.clone()]);
        assert_eq!(form.fields.host, host.hostname);
        let before = form.endpoint(".").unwrap();
        form.select(&other, &mut "Keep name".into());
        assert_eq!(form.fields.host, other.hostname);
        assert_ne!(form.endpoint(".").unwrap(), before);
        assert_eq!(
            options(&form.endpoint(".").unwrap()),
            &SshOptions::default()
        );
    }

    #[test]
    fn legacy_unmatched_alias_and_default_config_keep_original_connection_semantics() {
        let json = r#"{"Sftp":{"alias":"legacy","config_path":null,"root":"/saved"}}"#;
        let endpoint: RemoteEndpoint = serde_json::from_str(json).unwrap();
        let RemoteEndpoint::Sftp {
            alias,
            config_path,
            options,
            root,
        } = &endpoint
        else {
            panic!()
        };
        let mut form = SshForm::from_connection(alias, config_path, options);
        form.apply_hosts(Vec::new());
        assert_eq!(form.endpoint(root).unwrap(), endpoint);
        let Some(path) = ssh_config::default_config_path() else {
            return;
        };
        let host = host(path.clone());
        let mut existing =
            SshForm::from_connection(&host.alias, &Some(path.clone()), &Default::default());
        existing.apply_hosts(vec![host.clone()]);
        existing.select(&host, &mut String::new());
        let RemoteEndpoint::Sftp { config_path, .. } = existing.endpoint(".").unwrap() else {
            panic!()
        };
        assert_eq!(config_path, Some(path));
        let mut fresh = SshForm::default();
        fresh.select(&host, &mut String::new());
        let RemoteEndpoint::Sftp { config_path, .. } = fresh.endpoint(".").unwrap() else {
            panic!()
        };
        assert_eq!(config_path, None);
    }

    #[test]
    fn manual_ipv6_works_without_an_ssh_alias_and_invalid_edits_cannot_save() {
        let mut form = SshForm::default();
        form.fields.host = "::1".into();
        form.fields.user = "developer".into();
        let RemoteEndpoint::Sftp {
            alias,
            config_path,
            options,
            ..
        } = form.endpoint(".").unwrap()
        else {
            panic!()
        };
        assert_eq!(alias, "[::1]");
        assert_eq!(config_path, None);
        assert_eq!(options.user.as_deref(), Some("developer"));
        for port in ["0", "65536", "22x", "-1"] {
            form.fields.port = port.into();
            assert!(form.endpoint(".").is_err());
        }
        form.fields.port = "22".into();
        form.fields.user = "bad;touch".into();
        assert!(form.endpoint(".").is_err());
    }
}

impl Fields {
    fn from_host(host: &SshHost, options: &SshOptions) -> Self {
        Self {
            host: options
                .hostname
                .clone()
                .unwrap_or_else(|| host.hostname.clone()),
            user: options
                .user
                .clone()
                .or_else(|| host.user.clone())
                .unwrap_or_default(),
            port: options.port.unwrap_or(host.port).to_string(),
            identity: options
                .identity_file
                .as_ref()
                .or(host.identity_files.first())
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
        }
    }
}

/// Discovery is read-only and never runs ssh -G / Match exec. The selected
/// config destination stays intact; only edited fields become argv overrides.
pub(super) struct SshForm {
    fields: Fields,
    baseline: Fields,
    alias: Option<String>,
    config_path: Option<PathBuf>,
    source: Option<PathBuf>,
    selected_source: Option<PathBuf>,
    saved: SshOptions,
    hosts: Vec<SshHost>,
    scanned: bool,
    scan: Option<Task<Result<Vec<SshHost>, String>>>,
    error: Option<String>,
}

impl Default for SshForm {
    fn default() -> Self {
        let fields = Fields {
            port: "22".into(),
            ..Default::default()
        };
        Self {
            baseline: fields.clone(),
            fields,
            alias: None,
            config_path: None,
            source: ssh_config::default_config_path(),
            selected_source: None,
            saved: SshOptions::default(),
            hosts: Vec::new(),
            scanned: false,
            scan: None,
            error: None,
        }
    }
}

impl SshForm {
    pub(super) fn with_config(path: PathBuf) -> Self {
        Self {
            source: Some(std::path::absolute(&path).unwrap_or(path)),
            ..Default::default()
        }
    }

    pub(super) fn from_connection(
        alias: &str,
        config: &Option<PathBuf>,
        options: &SshOptions,
    ) -> Self {
        let fields = Fields {
            host: options.hostname.clone().unwrap_or_else(|| alias.into()),
            user: options.user.clone().unwrap_or_default(),
            port: options.port.unwrap_or(22).to_string(),
            identity: options
                .identity_file
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        let source = config
            .clone()
            .or_else(ssh_config::default_config_path)
            .map(|p| std::path::absolute(&p).unwrap_or(p));
        Self {
            alias: Some(alias.into()),
            config_path: config.clone(),
            source: source.clone(),
            selected_source: source,
            saved: options.clone(),
            baseline: fields.clone(),
            fields,
            ..Default::default()
        }
    }

    fn apply_hosts(&mut self, hosts: Vec<SshHost>) {
        if self.source == self.selected_source
            && let Some(host) = hosts.iter().find(|h| Some(&h.alias) == self.alias.as_ref())
        {
            let next = Fields::from_host(host, &self.saved);
            // A late scan must never erase input entered while it was loading.
            if self.fields.host == self.baseline.host {
                self.fields.host = next.host.clone();
            }
            if self.fields.user == self.baseline.user {
                self.fields.user = next.user.clone();
            }
            if self.fields.port == self.baseline.port {
                self.fields.port = next.port.clone();
            }
            if self.fields.identity == self.baseline.identity {
                self.fields.identity = next.identity.clone();
            }
            self.baseline = next;
        }
        self.hosts = hosts;
    }

    fn select(&mut self, host: &SshHost, name: &mut String) {
        if self.alias.as_ref() == Some(&host.alias)
            && self.selected_source.as_ref() == Some(&host.config_path)
        {
            return;
        }
        if name.is_empty() || self.alias.as_ref().is_some_and(|old| name == old) {
            *name = host.alias.clone();
        }
        self.alias = Some(host.alias.clone());
        self.selected_source = Some(host.config_path.clone());
        // -F replaces default/system config loading. Do not add it for the
        // default user file, but retain custom paths and legacy saved -F.
        self.config_path = (Some(&host.config_path) != ssh_config::default_config_path().as_ref())
            .then(|| host.config_path.clone());
        self.saved = SshOptions::default();
        self.fields = Fields::from_host(host, &self.saved);
        self.baseline = self.fields.clone();
    }

    pub(super) fn endpoint(&self, root: &str) -> Result<RemoteEndpoint, String> {
        let port = if self.fields.port.trim().is_empty() {
            None
        } else {
            Some(
                self.fields
                    .port
                    .trim()
                    .parse::<u16>()
                    .ok()
                    .filter(|p| *p != 0)
                    .ok_or_else(|| tr("유효한 포트를 입력하세요").to_owned())?,
            )
        };
        let mut options = self.saved.clone();
        let alias = if let Some(alias) = &self.alias {
            if self.fields.host != self.baseline.host {
                options.hostname =
                    (!self.fields.host.trim().is_empty()).then(|| self.fields.host.trim().into());
            }
            if self.fields.user != self.baseline.user {
                options.user =
                    (!self.fields.user.trim().is_empty()).then(|| self.fields.user.trim().into());
            }
            if self.fields.port != self.baseline.port {
                options.port = port;
            }
            if self.fields.identity != self.baseline.identity {
                options.identity_file = (!self.fields.identity.trim().is_empty())
                    .then(|| PathBuf::from(self.fields.identity.trim()));
            }
            alias.clone()
        } else {
            let host = self.fields.host.trim();
            let alias = ssh_config::manual_destination(host)
                .map_err(|_| tr("호스트 주소를 입력하세요").to_owned())?;
            options = SshOptions {
                hostname: None,
                user: (!self.fields.user.trim().is_empty()).then(|| self.fields.user.trim().into()),
                port,
                identity_file: (!self.fields.identity.trim().is_empty())
                    .then(|| PathBuf::from(self.fields.identity.trim())),
            };
            alias
        };
        options.validate().map_err(|e| {
            match e.to_string().as_str() {
                "Invalid SSH username" => tr("유효한 SSH 사용자 이름을 입력하세요"),
                "Invalid SSH hostname" => tr("유효한 호스트 주소를 입력하세요"),
                "Invalid SSH port" => tr("유효한 포트를 입력하세요"),
                _ => tr("유효한 개인 키 경로를 입력하세요"),
            }
            .to_owned()
        })?;
        Ok(RemoteEndpoint::Sftp {
            alias,
            config_path: self.config_path.clone(),
            root: root.into(),
            options,
        })
    }

    pub(super) fn ui(&mut self, ui: &mut egui::Ui, name: &mut String) {
        if !self.scanned {
            self.scanned = true;
            if !cfg!(test)
                && let Some(path) = self.source.clone()
            {
                self.scan = Some(Task::spawn(ui.ctx(), move || {
                    if !path.exists() {
                        return Ok(Vec::new());
                    }
                    ssh_config::discover_hosts(&path).map_err(|e| e.to_string())
                }));
            }
        }
        if let Some(result) = self.scan.as_mut().and_then(Task::take) {
            self.scan = None;
            match result {
                Ok(hosts) => self.apply_hosts(hosts),
                Err(error) => self.error = Some(error),
            }
        }
        if self.scan.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(40));
        }
        ui.label(
            egui::RichText::new(tr("SSH Config 호스트"))
                .small()
                .color(Theme::current().text_dim),
        );
        let selected = self.alias.clone().unwrap_or_else(|| tr("직접 입력").into());
        let mut picked = None;
        let mut manual = false;
        egui::ComboBox::from_id_salt("remote-ssh-host")
            .selected_text(selected)
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(self.alias.is_none(), tr("직접 입력"))
                    .clicked()
                {
                    manual = true;
                }
                for (i, host) in self.hosts.iter().enumerate() {
                    let label = format!("{} · {}", host.alias, host.hostname);
                    if ui
                        .selectable_label(self.alias.as_deref() == Some(&host.alias), label)
                        .clicked()
                    {
                        picked = Some(i);
                    }
                }
            });
        if manual {
            self.alias = None;
            self.config_path = None;
            self.selected_source = None;
            self.saved = SshOptions::default();
        }
        if let Some(i) = picked {
            let host = self.hosts[i].clone();
            self.select(&host, name);
        }
        if self.scan.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("SSH 연결을 읽는 중…"));
            });
        } else if self.hosts.is_empty() && self.error.is_none() {
            ui.label(
                egui::RichText::new(tr("등록된 SSH 호스트가 없습니다"))
                    .small()
                    .color(Theme::current().text_dim),
            );
        }
        if let Some(error) = &self.error {
            ui.collapsing(tr("SSH Config를 읽을 수 없습니다"), |ui| {
                ui.colored_label(Theme::current().red, error);
            });
        }
        ui.add_space(8.0);
        super::panel::field(ui, tr("호스트 주소"), &mut self.fields.host, false);
        super::panel::field(ui, tr("사용자 이름"), &mut self.fields.user, false);
        super::panel::field(ui, tr("포트"), &mut self.fields.port, false);
        ui.label(
            egui::RichText::new(tr("개인 키 (선택)"))
                .small()
                .color(Theme::current().text_dim),
        );
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.fields.identity)
                    .desired_width((ui.available_width() - 32.0).max(80.0)),
            );
            if super::icon(ui, Icon::Folder, tr("개인 키 선택"))
                && let Some(path) = rfd::FileDialog::new().pick_file()
            {
                self.fields.identity = path.to_string_lossy().into_owned();
            }
        });
        egui::CollapsingHeader::new(tr("고급 연결 설정"))
            .id_salt("remote-ssh-advanced")
            .show(ui, |ui| {
                if ui.button(tr("SSH Config 파일 선택")).clicked()
                    && let Some(path) = rfd::FileDialog::new().pick_file()
                {
                    // Changing the discovery source alone does not change an
                    // existing saved destination. Selection makes that deliberate.
                    self.source = Some(path);
                    self.scanned = false;
                    self.scan = None;
                    self.hosts.clear();
                    self.error = None;
                }
                if let Some(path) = &self.source {
                    ui.label(
                        egui::RichText::new(path.to_string_lossy())
                            .small()
                            .color(Theme::current().text_dim),
                    );
                }
            });
    }
}
