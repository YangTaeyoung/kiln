//! Remote folders and transfers live in the workspace inspector.
//! Opening a file creates an independent editor in the main workspace.
mod browser;
mod cloud_form;
mod bucket_picker;
pub use bucket_picker::BucketLoader;
mod panel;
mod ssh_form;
pub use browser::{RemoteBrowser, RemoteDraft};
pub use panel::{RemoteEvent, RemoteManager, RemotePanel, RemoteNavigation};

use crate::{ConnectionProfile, RemoteEndpoint};
pub fn protocol(profile: &ConnectionProfile) -> &'static str {
    match profile.endpoint {
        RemoteEndpoint::S3 { .. } => "S3",
        RemoteEndpoint::ObjectStorage { provider, .. } => provider.label(),
        RemoteEndpoint::Ftp { tls: true, .. } => "FTPS",
        RemoteEndpoint::Ftp { .. } => "FTP",
        RemoteEndpoint::Sftp { .. } => "SFTP",
    }
}
/// Integration identity shared by the connection picker, browser and central tab.
pub fn provider_icon(profile: &ConnectionProfile) -> kiln_common::icons::Icon {
    use kiln_common::icons::Icon;
    match profile.endpoint {
        RemoteEndpoint::S3 { .. } => Icon::S3,
        RemoteEndpoint::ObjectStorage { provider, .. } => match provider {
            crate::ObjectProvider::Oracle => Icon::OracleCloud,
            crate::ObjectProvider::Google => Icon::GoogleCloud,
            crate::ObjectProvider::Cloudflare => Icon::Cloudflare,
        },
        RemoteEndpoint::Ftp { .. } => Icon::Server,
        RemoteEndpoint::Sftp { .. } => Icon::Terminal,
    }
}
pub fn root_path(profile: &ConnectionProfile) -> String {
    match &profile.endpoint {
        RemoteEndpoint::S3 { prefix, .. } | RemoteEndpoint::ObjectStorage { prefix, .. } => prefix.clone(),
        RemoteEndpoint::Ftp { root, .. } | RemoteEndpoint::Sftp { root, .. } => {
            if root.is_empty() {
                ".".into()
            } else {
                root.clone()
            }
        }
    }
}
pub fn location(profile: &ConnectionProfile, path: &str) -> String {
    match &profile.endpoint {
        RemoteEndpoint::S3 { bucket, .. } => format!("s3://{bucket}/{}", path.trim_start_matches('/')),
        RemoteEndpoint::ObjectStorage { provider, bucket, .. } => format!("{} · {bucket}/{}", provider.label(), path.trim_start_matches('/')),
        RemoteEndpoint::Ftp { host, port, tls, .. } => format!("{}://{host}:{port}/{path}", if *tls { "ftps" } else { "ftp" }),
        RemoteEndpoint::Sftp { alias, .. } => format!("{alias}:{path}"),
    }
}
pub fn icon(ui: &mut egui::Ui, kind: kiln_common::icons::Icon, label: &str) -> bool {
    kiln_common::widgets::icon_button(ui, kind, 28.0, false, label).clicked()
}

#[cfg(test)]
fn test_fonts(ctx: &egui::Context) -> bool {
    let id = egui::Id::new("remote-test-fonts");
    if ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false) {
        return true;
    }
    ctx.data_mut(|d| d.insert_temp(id, true));
    ctx.set_fonts(kiln_common::fonts::definitions(false));
    ctx.set_zoom_factor(1.3);
    false
}

fn file_row(ui: &mut egui::Ui, entry: &crate::RemoteEntry, selected: bool) -> egui::Response {
    row_with_icon(
        ui,
        entry,
        selected,
        if entry.is_dir {
            kiln_common::icons::Icon::Folder
        } else {
            kiln_common::icons::Icon::File
        },
    )
}

fn row_with_icon(
    ui: &mut egui::Ui,
    entry: &crate::RemoteEntry,
    selected: bool,
    icon: kiln_common::icons::Icon,
) -> egui::Response {
    let theme = kiln_common::Theme::current();
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            &entry.name,
        )
    });
    if selected || response.hovered() {
        ui.painter().rect_filled(
            rect,
            6,
            if selected {
                theme.bg_selected
            } else {
                theme.bg_hover
            },
        );
    }
    kiln_common::widgets::focus_ring(ui, &response, 6);
    kiln_common::icons::paint(
        ui.painter(),
        egui::Rect::from_center_size(
            egui::pos2(rect.left() + 14.0, rect.center().y),
            egui::vec2(18.0, 18.0),
        ),
        icon,
        theme.text_dim,
    );
    let metadata = if entry.is_dir { 0.0 } else { 76.0 };
    let text_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 32.0, rect.top()),
        egui::pos2(rect.right() - metadata - 8.0, rect.bottom()),
    );
    let mut text = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(text_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    text.set_clip_rect(text_rect.intersect(ui.clip_rect()));
    text.add(egui::Label::new(&entry.name).selectable(false).truncate());
    if !entry.is_dir {
        ui.painter().text(
            egui::pos2(rect.right() - 8.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            bytes(entry.size),
            kiln_common::fonts::regular(11.0),
            theme.text_dim,
        );
    }
    response
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
