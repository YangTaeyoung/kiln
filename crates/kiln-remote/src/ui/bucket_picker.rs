//! Identity-scoped discovery; replacing or dropping a picker cancels its request.
use crate::{
    RemoteEndpoint, Secrets,
    bucket_discovery::{BucketChoice, BucketDiscoveryJob, BucketScope},
};
use kiln_common::{Task, Theme, i18n::tr, icons::Icon};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
pub type BucketLoader = Arc<
    dyn Fn(RemoteEndpoint, Secrets, BucketScope) -> Result<Vec<BucketChoice>, String> + Send + Sync,
>;
#[derive(Default)]
pub(super) struct BucketPicker {
    identity: Option<String>,
    changed: Option<Instant>,
    pub manual: bool,
    rows: Vec<BucketChoice>,
    job: Option<BucketDiscoveryJob>,
    injected: Option<Task<Result<Vec<BucketChoice>, String>>>,
    credentials: Option<Task<Result<Secrets, String>>>,
    error: Option<String>,
    attempted: bool,
}
impl BucketPicker {
    pub fn selected(&self, bucket: &str) -> Option<&BucketChoice> {
        self.rows.iter().find(|row| row.name == bucket)
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn prepare(&mut self, key: Option<String>, bucket: &mut String) {
        if self.identity != key {
            let previous = self.identity.is_some();
            self.job = None;
            self.injected = None;
            self.credentials = None;
            self.rows.clear();
            self.error = None;
            self.attempted = false;
            self.identity = key;
            self.changed = Some(Instant::now());
            if previous {
                bucket.clear();
            }
        }
    }
    fn accept(&mut self, result: Result<Vec<BucketChoice>, String>, bucket: &mut String) {
        match result {
            Ok(mut rows) => {
                rows.sort_by(|a, b| a.name.cmp(&b.name));
                rows.dedup_by(|a, b| a.name == b.name);
                if !rows.iter().any(|row| row.name == *bucket) {
                    *bucket = if rows.len() == 1 {
                        rows[0].name.clone()
                    } else {
                        String::new()
                    };
                }
                self.rows = rows;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        bucket: &mut String,
        endpoint: Option<RemoteEndpoint>,
        scope: BucketScope,
        secrets: impl FnOnce() -> Result<Secrets, String> + Send + 'static,
        loader: Option<BucketLoader>,
    ) -> Option<BucketChoice> {
        let loading = self.credentials.is_some() || self.job.is_some() || self.injected.is_some();
        let ready = endpoint.is_some() && self.identity.is_some();
        let mut refresh = false;
        let was_manual = self.manual;
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(tr("버킷"))
                    .small()
                    .color(Theme::current().text_dim),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                refresh = ui
                    .add_enabled_ui(ready && !loading, |ui| {
                        super::icon(ui, Icon::Refresh, tr("버킷 새로고침"))
                    })
                    .inner;
                ui.checkbox(&mut self.manual, tr("직접 입력"));
            });
        });
        if self.manual != was_manual {
            self.job = None;
            self.credentials = None;
            self.injected = None;
            self.attempted = false;
        }
        if self.manual {
            super::panel::field(ui, tr("버킷 이름"), bucket, false);
        } else {
            ui.add_enabled_ui(!loading, |ui| {
                egui::ComboBox::from_id_salt("remote-bucket-choice")
                    .selected_text(if bucket.is_empty() {
                        tr("버킷 선택")
                    } else {
                        bucket.as_str()
                    })
                    .width(ui.available_width())
                    .truncate()
                    .show_ui(ui, |ui| {
                        ui.set_max_width(360.0_f32.min(ui.ctx().content_rect().width() - 60.0));
                        for row in &self.rows {
                            ui.selectable_value(bucket, row.name.clone(), &row.name)
                                .on_hover_text(row.region.as_deref().unwrap_or(""));
                        }
                    });
            });
        }
        if loading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("버킷 불러오는 중…"));
            });
            ui.ctx().request_repaint_after(Duration::from_millis(40));
        }
        if let Some(error) = &self.error {
            ui.colored_label(
                Theme::current().yellow,
                tr("버킷 목록을 불러오지 못했습니다. 직접 입력하거나 다시 시도하세요."),
            )
            .on_hover_text(error);
        } else if self.attempted && !loading && self.rows.is_empty() {
            ui.label(
                egui::RichText::new(tr("조회 범위에 버킷이 없습니다"))
                    .small()
                    .color(Theme::current().text_dim),
            );
        }
        if refresh {
            self.manual = false;
            self.attempted = false;
            self.changed = Some(Instant::now() - Duration::from_secs(1));
        }
        if (!cfg!(test) || loader.is_some())
            && ready
            && !self.attempted
            && !self.manual
            && self
                .changed
                .is_some_and(|at| at.elapsed() >= Duration::from_millis(450))
        {
            self.attempted = true;
            if !cfg!(test) || loader.is_some() {
                self.credentials = Some(Task::spawn(ui.ctx(), secrets));
            }
        } else if (!cfg!(test) || loader.is_some()) && ready && !self.attempted && !self.manual {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        if let Some(result) = self.credentials.as_mut().and_then(Task::take) {
            self.credentials = None;
            match result {
                Ok(secrets) => {
                    if let Some(endpoint) = endpoint {
                        if let Some(loader) = loader {
                            self.injected = Some(Task::spawn(ui.ctx(), move || {
                                loader(endpoint, secrets, scope)
                            }));
                        } else {
                            self.job = Some(crate::bucket_discovery::spawn_bucket_discovery(
                                endpoint, secrets, scope,
                            ));
                        }
                    }
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(result) = self.injected.as_mut().and_then(Task::take) {
            self.injected = None;
            self.accept(result, bucket);
        }
        if let Some(result) = self.job.as_ref().and_then(|job| job.try_recv()) {
            self.job = None;
            self.accept(result.map_err(|e| e.to_string()), bucket);
        }
        self.rows.iter().find(|row| row.name == *bucket).cloned()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn row(name: &str) -> BucketChoice {
        BucketChoice {
            name: name.into(),
            region: None,
            namespace: None,
        }
    }
    #[test]
    fn only_single_bucket_is_automatic_and_identity_changes_cancel_and_clear() {
        let mut picker = BucketPicker::default();
        let mut bucket = String::new();
        picker.prepare(Some("a".into()), &mut bucket);
        picker.accept(Ok(vec![row("first"), row("second")]), &mut bucket);
        assert!(bucket.is_empty());
        bucket = "second".into();
        picker.accept(Ok(vec![row("first"), row("second")]), &mut bucket);
        assert_eq!(bucket, "second");
        picker.accept(Ok(vec![row("only")]), &mut bucket);
        assert_eq!(bucket, "only");
        picker.prepare(Some("b".into()), &mut bucket);
        assert!(bucket.is_empty());
        assert!(picker.rows.is_empty());
        assert!(!picker.attempted);
    }
}
