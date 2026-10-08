//! Provider-specific authentication choices; discovery never implies a login succeeded.
use super::panel::field;
use crate::aws_profiles::{self, Authentication, AwsProfile};
use crate::object_profiles::{self, ObjectProfile, ProfileFiles};
use crate::{ObjectAuthentication, ObjectProvider, RemoteEndpoint, S3Authentication};
use kiln_common::{Task, Theme, i18n::tr, icons::Icon};

fn unavailable_reason(provider: ObjectProvider, profile: &ObjectProfile) -> String {
    if matches!(profile.authentication, ObjectAuthentication::Cli { .. }) {
        kiln_common::trf!("{} 설치 후 이 프로필을 사용할 수 있습니다.", provider.cli())
    } else {
        tr("이 프로필의 인증을 사용할 수 없습니다").into()
    }
}

type Scan = (
    Result<Vec<ObjectProfile>, String>,
    Result<Vec<AwsProfile>, String>,
    bool,
);
#[derive(Default)]
pub(super) struct CloudForm {
    pub mode: usize, // CLI, manual S3 keys, named AWS profile
    pub selected: String,
    pub profiles: Vec<ObjectProfile>,
    pub authentication: Option<ObjectAuthentication>,
    pub region: String,
    pub region_touched: bool,
    account_touched: bool,
    pub namespace: String,
    pub account: String,
    pub endpoint: String,
    pub aws_profile: String,
    pub aws_profiles: Vec<AwsProfile>,
    pub cli_available: bool,
    pub scanned: bool,
    pub files: Option<ProfileFiles>,
    scan: Option<Task<Scan>>,
    error: Option<String>,
    aws_error: Option<String>,
}
impl CloudForm {
    pub fn from_endpoint(endpoint: &RemoteEndpoint) -> Self {
        let RemoteEndpoint::ObjectStorage {
            authentication,
            region,
            namespace,
            account_id,
            ..
        } = endpoint
        else {
            return Self::default();
        };
        let mut form = Self {
            authentication: Some(authentication.clone()),
            region: region.clone().unwrap_or_default(),
            namespace: namespace.clone().unwrap_or_default(),
            account: account_id.clone().unwrap_or_default(),
            region_touched: region.is_some(),
            account_touched: account_id.is_some(),
            ..Self::default()
        };
        match authentication {
            ObjectAuthentication::Cli { profile, .. } => form.selected = profile.clone(),
            ObjectAuthentication::GoogleAdc { .. } => {
                form.selected = "Application Default Credentials".into()
            }
            ObjectAuthentication::S3 {
                region,
                endpoint,
                aws_profile,
                ..
            } => {
                form.mode = if aws_profile.is_some() { 2 } else { 1 };
                form.region = region.clone();
                form.endpoint = endpoint.clone();
                form.aws_profile = aws_profile.clone().unwrap_or_default();
            }
        }
        form
    }
    pub fn loading(&self) -> bool {
        self.scan.is_some()
    }
    #[cfg(test)]
    fn apply(&mut self, profiles: Vec<ObjectProfile>, aws: Vec<AwsProfile>, cli: bool) {
        self.profiles = profiles;
        self.aws_profiles = aws;
        self.cli_available = cli;
        // An existing identity stays selected; new connections require an
        // explicit choice instead of guessing the CLI's active account.
    }
    fn consume_scan(&mut self, (profiles, aws, cli): Scan) {
        self.cli_available = cli;
        match profiles {
            Ok(profiles) => {
                self.profiles = profiles;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        match aws {
            Ok(profiles) => {
                self.aws_profiles = profiles;
                self.aws_error = None;
            }
            Err(error) => self.aws_error = Some(error),
        }
    }
    fn select(&mut self, profile: &ObjectProfile) {
        self.selected = profile.name.clone();
        self.authentication = Some(profile.authentication.clone());
        if !self.region_touched {
            self.region = profile.region.clone().unwrap_or_default();
        }
        if !self.account_touched {
            self.account = profile.account_id.clone().unwrap_or_default();
        }
        self.error = None;
    }
    fn discovery_files(&self, provider: ObjectProvider) -> ProfileFiles {
        let mut files = self
            .files
            .clone()
            .unwrap_or_else(|| ProfileFiles::current(provider));
        match &self.authentication {
            Some(ObjectAuthentication::Cli {
                config_path: Some(path),
                ..
            }) if provider != ObjectProvider::Cloudflare => files.config = path.clone(),
            Some(ObjectAuthentication::GoogleAdc {
                credentials_path: Some(path),
            }) => files.credentials = Some(path.clone()),
            _ => {}
        }
        files
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, provider: ObjectProvider) {
        if !self.scanned {
            self.scanned = true;
            if !cfg!(test) {
                let files = self.discovery_files(provider);
                self.scan = Some(Task::spawn(ui.ctx(), move || {
                    let profiles =
                        object_profiles::discover(provider, &files).map_err(|e| e.to_string());
                    let aws = aws_profiles::discover(&aws_profiles::AwsFiles::current())
                        .map_err(|e| e.to_string());
                    (profiles, aws, aws_profiles::cli_path().is_some())
                }));
            }
        }
        if let Some(result) = self.scan.as_mut().and_then(Task::take) {
            self.scan = None;
            self.consume_scan(result);
        }
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(tr("인증 방식"))
                    .small()
                    .color(Theme::current().text_dim),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if super::icon(ui, Icon::Refresh, tr("프로필 새로고침")) && self.scan.is_none()
                {
                    self.scanned = false;
                    self.error = None;
                }
            });
        });
        egui::ComboBox::from_id_salt("object-auth-source")
            .selected_text(tr(
                ["CLI 프로필", "직접 키 입력", "저장된 AWS 프로필"][self.mode]
            ))
            .width(ui.available_width())
            .truncate()
            .show_ui(ui, |ui| {
                for (mode, label) in [
                    (0, "CLI 프로필"),
                    (1, "직접 키 입력"),
                    (2, "저장된 AWS 프로필"),
                ] {
                    ui.selectable_value(&mut self.mode, mode, tr(label));
                }
            });
        if self.mode == 0 {
            ui.horizontal(|ui| {
                let width = ui.available_width();
                let selected = if self.selected.is_empty() {
                    tr("프로필 선택")
                } else {
                    &self.selected
                };
                let mut choice = None;
                egui::ComboBox::from_id_salt("object-cli-profile")
                    .selected_text(selected)
                    .width(width)
                    .truncate()
                    .show_ui(ui, |ui| {
                        ui.set_max_width(360.0_f32.min(ui.ctx().content_rect().width() - 60.0));
                        for profile in &self.profiles {
                            let response = ui.add_enabled(
                                profile.available,
                                egui::Button::selectable(
                                    self.selected == profile.name,
                                    &profile.name,
                                )
                                .truncate(),
                            );
                            if response.clicked() {
                                choice = Some(profile.clone());
                            }
                            if !profile.available {
                                response
                                    .on_disabled_hover_text(unavailable_reason(provider, profile));
                            }
                        }
                    });
                if let Some(profile) = choice {
                    self.select(&profile);
                }
            });
            if self.loading() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr("프로필 읽는 중…"));
                });
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(40));
            } else if self.profiles.is_empty() {
                ui.label(
                    egui::RichText::new(tr("저장된 프로필이 없습니다"))
                        .small()
                        .color(Theme::current().text_dim),
                );
            }
            if let Some(profile) = self
                .profiles
                .iter()
                .find(|profile| profile.name == self.selected)
            {
                if !profile.available {
                    ui.colored_label(
                        Theme::current().yellow,
                        unavailable_reason(provider, profile),
                    );
                }
                if let Some(project) = &profile.project {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(project)
                                .small()
                                .color(Theme::current().text_dim),
                        )
                        .truncate(),
                    )
                    .on_hover_text(project);
                }
            }
        } else if self.mode == 2 {
            let selected = if self.aws_profile.is_empty() {
                tr("AWS 프로필을 선택하세요")
            } else {
                &self.aws_profile
            };
            egui::ComboBox::from_id_salt("object-aws-profile")
                .selected_text(selected)
                .width(ui.available_width())
                .truncate()
                .show_ui(ui, |ui| {
                    for profile in &self.aws_profiles {
                        let available = profile.authentication == Authentication::Static
                            || profile.authentication == Authentication::External
                                && self.cli_available;
                        if ui
                            .add_enabled(
                                available,
                                egui::Button::selectable(
                                    self.aws_profile == profile.name,
                                    &profile.name,
                                )
                                .truncate(),
                            )
                            .on_disabled_hover_text(
                                if profile.authentication == Authentication::Missing {
                                    tr("이 AWS 프로필에 인증 정보가 없습니다")
                                } else {
                                    tr("이 AWS 프로필은 AWS CLI v2가 필요합니다")
                                },
                            )
                            .clicked()
                        {
                            self.aws_profile = profile.name.clone();
                        }
                    }
                });
            if self.aws_profiles.is_empty() && !self.loading() && self.aws_error.is_none() {
                ui.label(
                    egui::RichText::new(tr("저장된 AWS 프로필이 없습니다"))
                        .small()
                        .color(Theme::current().text_dim),
                );
            }
        }
        if let Some(error) = if self.mode == 2 {
            &self.aws_error
        } else if self.mode == 0 {
            &self.error
        } else {
            &None
        } {
            ui.colored_label(Theme::current().red, error);
        }
        ui.add_space(6.0);
        match provider {
            ObjectProvider::Oracle if self.mode != 0 => {
                self.region_touched |= field(ui, tr("리전"), &mut self.region, false).changed();
                field(ui, tr("네임스페이스"), &mut self.namespace, false);
            }
            ObjectProvider::Cloudflare => {
                self.account_touched |= field(ui, "Account ID", &mut self.account, false).changed();
            }
            ObjectProvider::Google | ObjectProvider::Oracle => {}
        }
    }
    pub fn endpoint(
        &self,
        provider: ObjectProvider,
        bucket: &str,
        prefix: &str,
    ) -> Result<RemoteEndpoint, String> {
        if bucket.trim().is_empty() {
            return Err(tr("버킷 이름을 입력하세요").into());
        }
        let authentication = if self.mode == 0 {
            let selected = self
                .profiles
                .iter()
                .find(|profile| {
                    profile.name == self.selected
                        && Some(&profile.authentication) == self.authentication.as_ref()
                })
                .ok_or_else(|| tr("프로필을 선택하세요").to_owned())?;
            if !selected.available {
                return Err(unavailable_reason(provider, selected));
            }
            selected.authentication.clone()
        } else {
            let endpoint = if !self.endpoint.trim().is_empty() {
                self.endpoint.trim().into()
            } else {
                match provider {
                    ObjectProvider::Google => "https://storage.googleapis.com".into(),
                    ObjectProvider::Cloudflare => {
                        if self.account.trim().is_empty() {
                            return Err(tr("Account ID를 입력하세요").into());
                        }
                        format!("https://{}.r2.cloudflarestorage.com", self.account.trim())
                    }
                    ObjectProvider::Oracle => {
                        if self.namespace.trim().is_empty() || self.region.trim().is_empty() {
                            return Err(tr("네임스페이스와 리전을 입력하세요").into());
                        }
                        format!(
                            "https://{}.compat.objectstorage.{}.oraclecloud.com",
                            self.namespace.trim(),
                            self.region.trim()
                        )
                    }
                }
            };
            if self.mode == 2
                && !self.aws_profiles.iter().any(|profile| {
                    profile.name == self.aws_profile
                        && (profile.authentication == Authentication::Static
                            || profile.authentication == Authentication::External
                                && self.cli_available)
                })
            {
                return Err(tr("AWS 프로필을 선택하세요").into());
            }
            ObjectAuthentication::S3 {
                region: if self.region.trim().is_empty() {
                    "auto".into()
                } else {
                    self.region.trim().into()
                },
                endpoint,
                path_style: true,
                aws_profile: (self.mode == 2).then(|| self.aws_profile.clone()),
                aws_auth: if self.mode == 2 {
                    None
                } else {
                    Some(S3Authentication::Manual)
                },
            }
        };
        Ok(RemoteEndpoint::ObjectStorage {
            provider,
            bucket: bucket.trim().into(),
            prefix: prefix.trim_start_matches('/').into(),
            authentication,
            region: (self.mode == 0 && self.region_touched && !self.region.trim().is_empty())
                .then(|| self.region.trim().into()),
            namespace: (!self.namespace.trim().is_empty()).then(|| self.namespace.trim().into()),
            account_id: (!self.account.trim().is_empty()).then(|| self.account.trim().into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn profile(name: &str, authentication: ObjectAuthentication, available: bool) -> ObjectProfile {
        ObjectProfile {
            name: name.into(),
            region: None,
            project: None,
            account_id: None,
            config_path: None,
            available,
            reason: (!available).then(|| "CLI unavailable".into()),
            authentication,
        }
    }
    #[test]
    fn discovery_keeps_authentication_identity_and_refresh_never_replaces_it() {
        let adc = profile(
            "Application Default Credentials",
            ObjectAuthentication::GoogleAdc {
                credentials_path: Some("/fixture/adc.json".into()),
            },
            true,
        );
        let cli = profile(
            "studio",
            ObjectAuthentication::Cli {
                profile: "studio".into(),
                config_path: Some("/fixture/gcloud".into()),
            },
            true,
        );
        let mut form = CloudForm::default();
        form.apply(vec![adc.clone(), cli.clone()], vec![], true);
        assert!(
            form.authentication.is_none(),
            "discovery must not guess the active account"
        );
        form.select(&adc);
        assert_eq!(form.authentication, Some(adc.authentication.clone()));
        form.select(&cli);
        form.region = "edited-region".into();
        form.region_touched = true;
        form.apply(vec![adc.clone(), cli.clone()], vec![], true);
        assert_eq!(form.authentication, Some(cli.authentication.clone()));
        assert_eq!(form.region, "edited-region");
        let endpoint = form
            .endpoint(ObjectProvider::Google, "fixture", "/assets/")
            .unwrap();
        assert!(
            matches!(endpoint,RemoteEndpoint::ObjectStorage{authentication,prefix,..} if authentication==cli.authentication && prefix=="assets/")
        );
        form.apply(vec![adc], vec![], true);
        assert!(
            form.endpoint(ObjectProvider::Google, "fixture", "")
                .is_err(),
            "a removed CLI profile must not fall back to another account"
        );
    }
    #[test]
    fn independent_discovery_errors_and_profile_defaults_preserve_explicit_edits() {
        let mut seoul = profile(
            "seoul",
            ObjectAuthentication::Cli {
                profile: "seoul".into(),
                config_path: None,
            },
            true,
        );
        seoul.region = Some("ap-seoul-1".into());
        seoul.account_id = Some("first-account".into());
        let mut ashburn = profile(
            "ashburn",
            ObjectAuthentication::Cli {
                profile: "ashburn".into(),
                config_path: None,
            },
            true,
        );
        ashburn.region = Some("us-ashburn-1".into());
        ashburn.account_id = Some("second-account".into());
        let mut form = CloudForm::default();
        form.consume_scan((
            Ok(vec![seoul.clone(), ashburn.clone()]),
            Err("Malformed AWS configuration".into()),
            false,
        ));
        assert_eq!(form.profiles.len(), 2);
        assert!(form.error.is_none());
        assert!(form.aws_error.is_some());
        form.select(&seoul);
        form.select(&ashburn);
        assert_eq!(form.region, "us-ashburn-1");
        assert_eq!(form.account, "second-account");
        form.region = "custom-region".into();
        form.region_touched = true;
        form.account = "custom-account".into();
        form.account_touched = true;
        form.select(&seoul);
        assert_eq!(form.region, "custom-region");
        assert_eq!(form.account, "custom-account");
    }
    #[test]
    fn wrangler_source_change_can_be_reselected_without_changing_other_provider_sources() {
        let old = ObjectAuthentication::Cli {
            profile: "studio".into(),
            config_path: Some("/fixture/old".into()),
        };
        let mut form = CloudForm::default();
        form.authentication = Some(old.clone());
        form.selected = "studio".into();
        form.files = Some(ProfileFiles {
            config: "/fixture/current".into(),
            credentials: None,
            executable: None,
        });
        assert_eq!(
            form.discovery_files(ObjectProvider::Cloudflare).config,
            std::path::PathBuf::from("/fixture/current")
        );
        assert_eq!(
            form.discovery_files(ObjectProvider::Google).config,
            std::path::PathBuf::from("/fixture/old")
        );
        assert_eq!(
            form.discovery_files(ObjectProvider::Oracle).config,
            std::path::PathBuf::from("/fixture/old")
        );
        let current = profile(
            "studio",
            ObjectAuthentication::Cli {
                profile: "studio".into(),
                config_path: Some("/fixture/current".into()),
            },
            true,
        );
        form.consume_scan((Ok(vec![current.clone()]), Ok(vec![]), true));
        assert_eq!(form.authentication, Some(old));
        form.select(&current);
        assert_eq!(form.authentication, Some(current.authentication));
    }
    #[test]
    fn unavailable_cli_is_distinct_from_explicit_s3_fallback_and_reedit_preserves_auth() {
        let p = profile(
            "logged-out",
            ObjectAuthentication::Cli {
                profile: "logged-out".into(),
                config_path: None,
            },
            false,
        );
        for language in kiln_common::i18n::Language::ALL {
            kiln_common::i18n::with_language(language, || {
                let warning = unavailable_reason(ObjectProvider::Cloudflare, &p);
                assert!(warning.contains("wrangler"));
                assert_ne!(warning, p.reason.clone().unwrap());
                if language != kiln_common::i18n::Language::Korean {
                    assert!(!warning.contains("설치"));
                }
            });
        }
        let mut form = CloudForm::default();
        form.apply(vec![p.clone()], vec![], false);
        form.select(&p);
        assert!(
            form.endpoint(ObjectProvider::Cloudflare, "bucket", "")
                .is_err()
        );
        form.mode = 1;
        form.account = "0123456789abcdef0123456789abcdef".into();
        let endpoint = form
            .endpoint(ObjectProvider::Cloudflare, "bucket", "objects/")
            .unwrap();
        assert!(
            matches!(&endpoint,RemoteEndpoint::ObjectStorage{authentication:ObjectAuthentication::S3{endpoint,aws_auth:Some(S3Authentication::Manual),..},..} if endpoint=="https://0123456789abcdef0123456789abcdef.r2.cloudflarestorage.com")
        );
        let restored = CloudForm::from_endpoint(&endpoint);
        assert_eq!(restored.mode, 1);
        assert_eq!(
            restored
                .endpoint(ObjectProvider::Cloudflare, "bucket", "objects/")
                .unwrap(),
            endpoint
        );
        form.mode = 2;
        form.aws_profile = "missing".into();
        assert!(
            form.endpoint(ObjectProvider::Cloudflare, "bucket", "")
                .is_err()
        );
    }
}
