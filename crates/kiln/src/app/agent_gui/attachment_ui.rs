//! Pane-scoped image gestures and previews. Clipboard access only follows explicit paste.
use super::*;
use std::path::PathBuf;

#[derive(Default)]
pub(super) struct ImageUi {
    pub(super) picker: Option<Task<Option<Vec<PathBuf>>>>,
    notice: Option<String>,
    previews: HashMap<PathBuf, Preview>,
}
enum Preview {
    Loading(Task<Option<egui::ColorImage>>),
    Ready(Option<egui::TextureHandle>),
}

/// An empty Paste is emitted by egui-winit when an explicit paste has no text.
/// Nonempty text is left to TextEdit, including clipboard formats containing both.
fn take_image_paste(events: &mut Vec<egui::Event>, focused: bool) -> bool {
    if !focused {
        return false;
    }
    let gesture = events
        .iter()
        .any(|event| matches!(event, egui::Event::Paste(text) if text.is_empty()));
    if gesture {
        events.retain(|event| !matches!(event, egui::Event::Paste(text) if text.is_empty()));
    }
    gesture
}
fn drop_targets_pane(rect: egui::Rect, position: Option<egui::Pos2>) -> bool {
    position.is_some_and(|pos| rect.contains(pos))
}
fn clipboard_png(image: arboard::ImageData<'_>) -> anyhow::Result<Vec<u8>> {
    let width = u32::try_from(image.width)?;
    let height = u32::try_from(image.height)?;
    anyhow::ensure!(
        width > 0
            && height > 0
            && width <= MAX_IMAGE_EDGE
            && height <= MAX_IMAGE_EDGE
            && u64::from(width) * u64::from(height) <= MAX_IMAGE_PIXELS,
        "image dimensions exceed the attachment limit"
    );
    let expected = image
        .width
        .checked_mul(image.height)
        .and_then(|n| n.checked_mul(4));
    anyhow::ensure!(
        expected == Some(image.bytes.len()),
        "invalid clipboard image pixels"
    );
    let mut png = Vec::new();
    use image::ImageEncoder;
    image::codecs::png::PngEncoder::new(&mut png).write_image(
        &image.bytes,
        width,
        height,
        image::ExtendedColorType::Rgba8,
    )?;
    anyhow::ensure!(png.len() as u64 <= MAX_IMAGE, "image exceeds 20 MiB");
    Ok(png)
}
fn clipboard_attachment(descriptor: &Descriptor) -> Result<Option<Attachment>, String> {
    // Windows owns a single system clipboard; serialize image reads from rapid paste gestures.
    static IMAGE_READ: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let guard = IMAGE_READ.lock().map_err(|e| e.to_string())?;
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    let image = match clipboard.get_image() {
        Ok(image) => image,
        Err(arboard::Error::ContentNotAvailable) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    drop(clipboard);
    drop(guard);
    let png = clipboard_png(image).map_err(|e| e.to_string())?;
    import_image_bytes(descriptor, "Clipboard.png", &png)
        .map(Some)
        .map_err(|e| e.to_string())
}

impl AgentPane {
    pub(super) fn image_import_pending(&self) -> bool {
        !self.imports.is_empty() || self.image_ui.picker.is_some()
    }
    pub(super) fn poll_image_picker(&mut self, ctx: &egui::Context) {
        if let Some(task) = &mut self.image_ui.picker
            && !task.is_pending()
        {
            let paths = task.take().flatten();
            self.image_ui.picker = None;
            if let Some(paths) = paths {
                for path in paths {
                    self.import(ctx, path);
                }
            }
        }
    }
    pub(super) fn image_drop_ui(&mut self, ui: &mut Ui) {
        let rect = ui.max_rect().intersect(ui.clip_rect());
        let paths = ui.input_mut(|input| {
            if !drop_targets_pane(
                rect,
                input.pointer.hover_pos().or(input.pointer.latest_pos()),
            ) {
                return Vec::new();
            }
            // Consume at the pane boundary, before nested toolbar clipping and other splits.
            std::mem::take(&mut input.raw.dropped_files)
                .into_iter()
                .map(|file| file.path().to_path_buf())
                .collect::<Vec<_>>()
        });
        for path in paths {
            self.import(ui.ctx(), path);
        }
    }
    pub(super) fn image_paste_ui(&mut self, ui: &mut Ui, focused: bool) {
        self.image_paste_with(ui, focused, clipboard_attachment);
    }
    fn image_paste_with(
        &mut self,
        ui: &mut Ui,
        focused: bool,
        read: impl FnOnce(&Descriptor) -> Result<Option<Attachment>, String> + Send + 'static,
    ) {
        let paste = ui.input_mut(|input| take_image_paste(&mut input.events, focused));
        if !paste {
            return;
        }
        if self.composer.attachments.len() + self.imports.len() >= MAX_ATTACHMENTS {
            self.image_limit_notice();
            return;
        }
        let descriptor = self.descriptor.clone();
        self.imports
            .push(Task::spawn(ui.ctx(), move || read(&descriptor)));
    }
    pub(super) fn image_limit_notice(&mut self) {
        self.image_ui.notice =
            Some(kiln_common::i18n::tr("이미지는 최대 8개까지 첨부할 수 있습니다.").into());
    }
    pub(super) fn image_import_notice(&mut self, error: String) {
        self.image_ui.notice = Some(error);
    }
    pub(super) fn image_picker_ui(&mut self, ui: &mut Ui) {
        if ui
            .add_enabled_ui(
                self.image_ui.picker.is_none()
                    && self.composer.attachments.len() + self.imports.len() < MAX_ATTACHMENTS,
                |ui| {
                    widgets::icon_button(
                        ui,
                        kiln_common::icons::Icon::Plus,
                        28.,
                        false,
                        kiln_common::i18n::tr("이미지 첨부"),
                    )
                },
            )
            .inner
            .clicked()
        {
            self.image_ui.picker = Some(Task::spawn(ui.ctx(), || {
                rfd::FileDialog::new()
                    .add_filter("Image", &["png", "jpg", "jpeg", "gif", "webp"])
                    .pick_files()
            }));
        }
    }
    pub(super) fn attachment_preview_ui(&mut self, ui: &mut Ui, attachment: &Attachment) {
        let preview = self
            .image_ui
            .previews
            .entry(attachment.path.clone())
            .or_insert_with(|| {
                let attachment = attachment.clone();
                let id = self.descriptor.id.clone();
                Preview::Loading(Task::spawn(ui.ctx(), move || {
                    let bytes = attachment_bytes(&id, &attachment).ok()?;
                    let image = decode_attachment_image(&bytes)
                        .ok()?
                        .thumbnail(96, 96)
                        .into_rgba8();
                    Some(egui::ColorImage::from_rgba_unmultiplied(
                        [image.width() as usize, image.height() as usize],
                        image.as_raw(),
                    ))
                }))
            });
        if let Preview::Loading(task) = preview
            && !task.is_pending()
        {
            *preview = Preview::Ready(task.take().flatten().map(|image| {
                ui.ctx().load_texture(
                    format!("attachment-{}", attachment.sha256),
                    image,
                    egui::TextureOptions::LINEAR,
                )
            }));
        }
        if let Preview::Ready(Some(texture)) = preview {
            let response = ui.add(egui::Image::new(&*texture).max_size(egui::vec2(40., 40.)));
            response.on_hover_ui(|ui| {
                ui.add(egui::Image::new(&*texture).max_size(egui::vec2(240., 240.)));
                ui.label(&attachment.name);
            });
        }
    }
    pub(super) fn image_notice_ui(&mut self, ui: &mut Ui) {
        if let Some(notice) = self.image_ui.notice.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::Label::new(RichText::new(notice).color(Theme::current().red)).wrap());
                if ui.small_button("×").clicked() {
                    self.image_ui.notice = None;
                }
            });
        }
    }
    pub(super) fn prune_image_previews(&mut self) {
        self.image_ui
            .previews
            .retain(|path, _| self.composer.attachments.iter().any(|a| &a.path == path));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_paste_is_an_explicit_focused_gesture_and_preserves_text() {
        let mut events = vec![
            egui::Event::Paste("normal text".into()),
            egui::Event::Paste(String::new()),
        ];
        assert!(!take_image_paste(&mut events, false));
        assert_eq!(events.len(), 2);
        assert!(take_image_paste(&mut events, true));
        assert!(matches!(&events[..], [egui::Event::Paste(text)] if text == "normal text"));
        assert!(!take_image_paste(&mut events, true));
    }
    #[test]
    fn drops_choose_only_the_containing_split_including_its_composer() {
        let left = egui::Rect::from_min_max(egui::pos2(0., 0.), egui::pos2(300., 600.));
        let right = egui::Rect::from_min_max(egui::pos2(301., 0.), egui::pos2(600., 600.));
        for pos in [egui::pos2(450., 120.), egui::pos2(450., 550.)] {
            assert!(!drop_targets_pane(left, Some(pos)));
            assert!(drop_targets_pane(right, Some(pos)));
        }
        assert!(!drop_targets_pane(right, None));
    }
    #[test]
    fn injected_clipboard_pixels_round_trip_without_touching_os_clipboard() {
        let pixels = vec![255, 0, 30, 255, 10, 20, 255, 128];
        let png = clipboard_png(arboard::ImageData {
            width: 2,
            height: 1,
            bytes: pixels.clone().into(),
        })
        .unwrap();
        assert_eq!(
            decode_attachment_image(&png)
                .unwrap()
                .into_rgba8()
                .into_raw(),
            pixels
        );
        assert!(
            clipboard_png(arboard::ImageData {
                width: 2,
                height: 1,
                bytes: vec![0; 4].into()
            })
            .is_err()
        );
        assert!(
            clipboard_png(arboard::ImageData {
                width: usize::MAX,
                height: 2,
                bytes: Vec::new().into()
            })
            .is_err()
        );
    }
    fn pane() -> AgentPane {
        AgentPane::fixture(Descriptor {
            id: "image-fixture".into(),
            tool: kiln_accounts::Tool::Codex,
            cwd: "/fixture".into(),
            account_label: "Fixture".into(),
        })
    }
    fn fixture_attachment(index: usize) -> Attachment {
        Attachment {
            path: format!("/fixture/{index}").into(),
            name: format!("{index}.png"),
            mime: "image/png".into(),
            sha256: "fixture".into(),
        }
    }
    #[test]
    fn paste_dispatch_reads_only_for_the_focused_pane_and_never_sends() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let reads = Arc::new(AtomicUsize::new(0));
        let mut p = pane();
        p.composer.text = "existing draft".into();
        let ctx = egui::Context::default();
        for (focused, text, expected) in [
            (false, "", 0),
            (true, "text from clipboard", 0),
            (true, "", 1),
        ] {
            let counter = reads.clone();
            ctx.run_ui(
                egui::RawInput {
                    events: vec![egui::Event::Paste(text.into())],
                    ..Default::default()
                },
                |ui| {
                    let counter = counter.clone();
                    p.image_paste_with(ui, focused, move |_| {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Ok(None)
                    });
                },
            )
            .drop_without_applying_deltas();
            for task in p.imports.drain(..) {
                assert!(task.wait().unwrap().unwrap().is_none());
            }
            assert_eq!(reads.load(Ordering::SeqCst), expected);
        }
        assert_eq!(p.composer.text, "existing draft");
        assert!(p.sent.is_empty());
    }
    #[test]
    fn picker_cancel_and_capacity_keep_drafts_and_nonfatal_notices() {
        let mut p = pane();
        let ctx = egui::Context::default();
        p.composer.text = "keep this".into();
        p.composer.attachments = (0..MAX_ATTACHMENTS).map(fixture_attachment).collect();
        let before = p.composer.attachments.clone();
        p.image_ui.picker = Some(Task::ready(None));
        p.poll_image_picker(&ctx);
        assert!(p.image_ui.picker.is_none());
        assert!(p.image_ui.notice.is_none());
        p.import(&ctx, "/not-imported.png".into());
        assert!(p.imports.is_empty());
        assert!(p.image_ui.notice.is_some());
        assert!(p.error.is_none());
        assert_eq!(p.composer.attachments, before);
        assert_eq!(p.composer.text, "keep this");
        assert!(p.sent.is_empty());
    }
    #[test]
    fn removing_last_image_retires_validation_and_keeps_text_sendable() {
        let mut p = pane();
        let ctx = egui::Context::default();
        p.composer.text = "keep the draft".into();
        p.validation = Some(Task::ready(Err("removed image is missing".into())));
        p.attachment_error = Some("old image error".into());
        p.validate_attachments(&ctx);
        p.tick(&ctx);
        assert!(p.validation.is_none());
        assert!(p.attachment_error.is_none());
        assert_eq!(p.composer.text,"keep the draft");
        assert!(p.sent.is_empty());
    }
    #[test]
    fn dropped_files_are_consumed_by_the_right_split_not_its_toolbar_clip() {
        #[derive(Debug)]
        struct File(PathBuf);
        impl egui::DroppedFile for File {
            fn path(&self) -> &std::path::Path {
                &self.0
            }
            fn bytes(&self) -> Result<Vec<u8>, String> {
                panic!("Native import uses the bounded file reader")
            }
        }
        let root = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut left = pane();
        let mut right = pane();
        let rects = [
            egui::Rect::from_min_max(egui::pos2(0., 0.), egui::pos2(300., 600.)),
            egui::Rect::from_min_max(egui::pos2(301., 0.), egui::pos2(600., 600.)),
        ];
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600., 600.),
                )),
                events: vec![egui::Event::PointerMoved(egui::pos2(450., 550.))],
                dropped_files: vec![std::sync::Arc::new(File(root.path().join("missing.png")))],
                ..Default::default()
            },
            |ui| {
                for (pane, rect) in [(&mut left, rects[0]), (&mut right, rects[1])] {
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                    child.set_clip_rect(rect);
                    pane.image_drop_ui(&mut child);
                }
                assert!(ui.input(|input| input.raw.dropped_files.is_empty()));
            },
        )
        .drop_without_applying_deltas();
        assert!(left.imports.is_empty());
        assert_eq!(right.imports.len(), 1);
        assert!(right.imports.pop().unwrap().wait().unwrap().is_err());
        assert!(left.sent.is_empty() && right.sent.is_empty());
    }
}
