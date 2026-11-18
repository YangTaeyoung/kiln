//! Native structured agent runtime, owned by the persistent terminal session.
mod launch;
pub use launch::*;
mod controls;
pub use controls::*;
mod tool_presentation;
mod activity;
mod model;
pub use model::*;
mod client;
mod host;
mod journal;
mod shell_launch;
mod legacy_return;
pub use legacy_return::NativeImport;
pub use shell_launch::{serve_shell, shell_binding, shell_launch_eligible, saved_shell_spec};
mod providers;
use anyhow::{Context, Result, ensure};
pub use client::{Client, HistorySummary};
pub use host::{serve, native_parent_identity};
use kiln_accounts::{AccountManager, Tool};
use kiln_proto::SpawnSpec;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Descriptor {
    pub id: String,
    pub tool: Tool,
    pub cwd: PathBuf,
    pub account_label: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct Config {
    descriptor: Descriptor,
    program: PathBuf,
    environment: Vec<(String, String)>,
    initial_prompt: Option<String>,
    #[serde(default)]
    launch_options: LaunchOptions,
    #[serde(default)]
    shell: Option<shell_launch::ShellConfig>,
}
/// Call on the existing background launch-preparation worker. Captures the profile once.
pub fn prepare(
    tool: Tool,
    cwd: PathBuf,
    accounts: &AccountManager,
    initial_prompt: Option<String>,
) -> Result<SpawnSpec> {
    prepare_with_options(
        tool,
        cwd,
        accounts,
        initial_prompt,
        LaunchOptions::Inherited,
    )
}
/// Explicit creation-only options; the ordinary prepare API retains inherited defaults.
pub fn prepare_with_options(
    tool: Tool,
    cwd: PathBuf,
    accounts: &AccountManager,
    initial_prompt: Option<String>,
    launch_options: LaunchOptions,
) -> Result<SpawnSpec> {
    launch_options.validate(tool)?;
    ensure!(
        initial_prompt
            .as_ref()
            .is_none_or(|p| p.len() <= 120_000 && !p.contains('\0')),
        "invalid initial prompt"
    );
    let agent = super::remote_terminal::agent(tool, cwd.clone()).map_err(anyhow::Error::msg)?;
    let (mut environment, extension_report) = accounts.prepare_launch_environment(tool)?;
    super::agent_launch::pin_runtime_path(&mut environment)?;
    if !environment
        .iter()
        .any(|(key, _)| key == "KILN_AGENT_PROFILE")
    {
        let env = accounts.env();
        let (key, value) = match tool {
            Tool::Claude => (
                "CLAUDE_CONFIG_DIR",
                env.claude_config_dir
                    .clone()
                    .unwrap_or_else(|| env.home.join(".claude")),
            ),
            Tool::Codex => ("CODEX_HOME", env.codex_home.clone()),
            Tool::Antigravity => ("HOME", env.home.clone()),
        };
        environment.push((key.into(), value.to_string_lossy().into_owned()));
        if tool == Tool::Antigravity {
            for key in ["ANTIGRAVITY_APP_DATA_DIR", "ANTIGRAVITY_GEMINI_DIR"] {
                if !environment.iter().any(|(name, _)|name == key)
                    && let Some(value) = std::env::var_os(key) {
                    environment.push((key.into(), value.to_string_lossy().into_owned()));
                }
            }
        }
    }
    let profile = environment
        .iter()
        .find(|(k, _)| k == "KILN_AGENT_PROFILE")
        .map(|(_, id)| id);
    let account_label = profile
        .and_then(|id| accounts.profiles(tool).into_iter().find(|p| &p.id == id))
        .map(|p| p.label)
        .unwrap_or_else(|| "Default".into());
    let id = journal::unique();
    let dir = journal::directory(&id)?;
    journal::private_dir(&dir)?;
    let descriptor = Descriptor {
        id: id.clone(),
        tool,
        cwd: cwd.clone(),
        account_label: account_label.clone(),
    };
    extension_report.write_prepared(&dir.join("extension-report.json"))?;
    let config = Config {
        descriptor,
        program: agent.program.context("missing CLI")?.into(),
        environment,
        initial_prompt,
        launch_options,
        shell: None,
    };
    journal::atomic(&dir.join("config.json"), &serde_json::to_vec(&config)?)?;
    host_spec(&config.descriptor)
}
fn host_spec(descriptor: &Descriptor) -> Result<SpawnSpec> {
    let exe = if std::env::var_os("KILN_ACCOUNTS_SANDBOX").is_some() {
        super::conn::daemon_exe()
    } else {
        kiln_daemon::client::cli_executable(&std::env::current_exe()?)
    };
    Ok(SpawnSpec {
        program: Some(exe.to_string_lossy().into_owned()),
        args: vec!["agent-gui-host".into(), "--id".into(), descriptor.id.clone()],
        cwd: Some(descriptor.cwd.to_string_lossy().into_owned()),
        name: Some(descriptor.tool.display_name().into()),
        env: vec![
            ("KILN_GUI_AGENT".into(), descriptor.id.clone()),
            ("KILN_GUI_TOOL".into(), descriptor.tool.key().into()),
            ("KILN_GUI_ACCOUNT".into(), descriptor.account_label.clone()),
            (
                "KILN_CONFIG_DIR".into(),
                std::path::absolute(kiln_common::paths::config_dir())?
                    .to_string_lossy()
                    .into_owned(),
            ),
        ],
        ..Default::default()
    })
}

/// Adopt only an explicitly tracked native conversation after its old PTY has
/// reported exit. Call on a background worker; never resolve the active account.
pub fn prepare_existing_native(record_path: &Path) -> Result<SpawnSpec> {
    let record = super::agent_runtime::read(record_path)?;
    ensure!(record.confirmed, "The terminal has no confirmed native conversation identity");
    let native_id = record.conversation.as_deref().context("Missing native conversation identity")?;
    let cwd: PathBuf = record.spec.cwd.as_ref().context("The original working directory is missing")?.into();
    let history = super::conversation_sync::native_display_history(record.tool, &record.spec.env, &cwd, native_id)?;
    let descriptor = Descriptor {
        id: journal::unique(), tool: record.tool, cwd,
        account_label: "Original terminal account".into(),
    };
    let dir = journal::directory(&descriptor.id)?;
    journal::private_dir(&dir)?;
    let config = Config {
        descriptor,
        program: record.spec.program.as_ref().context("The original CLI executable is missing")?.into(),
        environment: record.spec.env,
        initial_prompt: None,
        launch_options: LaunchOptions::Inherited,
        shell: None,
    };
    journal::atomic(&dir.join("config.json"), &serde_json::to_vec(&config)?)?;
    let mut journal = journal::Journal::open(&dir, journal::unique())?;
    journal.push(Event::Generation)?;
    journal.push(Event::Session(native_id.into()))?;
    for (index, (role, text)) in history.into_iter().enumerate() {
        journal.push(Event::Message { message: Message {
            id: format!("native-import-{index}"),
            role: if role == "user" { Role::User } else { Role::Assistant },
            text, attachments: vec![],
        }, append: false })?;
    }
    journal.flush()?;
    host_spec(&config.descriptor)
}
pub fn descriptor(spec: &SpawnSpec) -> Option<Descriptor> {
    let get = |key: &str| {
        spec.env
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    };
    let id = get("KILN_GUI_AGENT")?;
    if !journal::valid_id(&id) {
        return None;
    }
    let tool_key = get("KILN_GUI_TOOL")?;
    let tool = Tool::ALL.into_iter().find(|t| t.key() == tool_key)?;
    Some(Descriptor {
        id,
        tool,
        cwd: spec.cwd.as_ref()?.into(),
        account_label: get("KILN_GUI_ACCOUNT").unwrap_or_else(|| "Default".into()),
    })
}
const MAX_IMAGE: u64 = 20 * 1024 * 1024;
fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}
/// Bounded image read/copy. Call off the UI thread. Returned paths are runtime-private.
pub fn import_attachment(descriptor: &Descriptor, path: &Path) -> Result<Attachment> {
    use std::io::Read;
    let meta = std::fs::symlink_metadata(path)?;
    ensure!(
        meta.is_file() && meta.len() <= MAX_IMAGE,
        "invalid image attachment"
    );
    let mut bytes = Vec::new();
    let mut source = std::fs::OpenOptions::new();
    source.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        source.custom_flags(libc::O_NOFOLLOW);
    }
    let source = source.open(path)?;
    ensure!(source.metadata()?.is_file(), "invalid image attachment");
    source.take(MAX_IMAGE + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= MAX_IMAGE, "image exceeds 20 MiB");
    import_image_bytes(descriptor, &path.file_name().unwrap_or_default().to_string_lossy(), &bytes)
}
/// The same private, validated storage path is used for file and clipboard images.
fn import_image_bytes(descriptor: &Descriptor, name: &str, bytes: &[u8]) -> Result<Attachment> {
    let dir = journal::directory(&descriptor.id)?.join("attachments");
    store_image_bytes(&dir, name, bytes)
}
fn store_image_bytes(dir: &Path, name: &str, bytes: &[u8]) -> Result<Attachment> {
    ensure!(bytes.len() as u64 <= MAX_IMAGE, "image exceeds 20 MiB");
    let mime = image_mime(bytes).context("unsupported image format")?;
    decode_attachment_image(bytes).context("invalid image contents or dimensions")?;
    journal::private_dir(dir)?;
    let target = dir.join(journal::unique());
    let mut f = journal::open(&target, false)?;
    use std::io::Write;
    f.write_all(bytes)?;
    f.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o400))?;
    }
    Ok(Attachment {
        path: target,
        name: name.into(),
        mime: mime.into(),
        sha256: image_digest(bytes),
    })
}
const MAX_IMAGE_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_IMAGE_EDGE: u32 = 16384;
/// Inspect dimensions before allocating decoded pixels; preserve the original encoded bytes.
fn decode_attachment_image(bytes: &[u8]) -> Result<image::DynamicImage> {
    use std::io::Cursor;
    let limits = || {
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(MAX_IMAGE_EDGE);
        limits.max_image_height = Some(MAX_IMAGE_EDGE);
        limits.max_alloc = Some(96 * 1024 * 1024);
        limits
    };
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    reader.limits(limits());
    let (width, height) = reader.into_dimensions()?;
    ensure!(width > 0 && height > 0 && width <= MAX_IMAGE_EDGE && height <= MAX_IMAGE_EDGE
        && u64::from(width) * u64::from(height) <= MAX_IMAGE_PIXELS,
        "image dimensions exceed the attachment limit");
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    reader.limits(limits());
    Ok(reader.decode()?)
}

fn attachment_bytes(id: &str, attachment: &Attachment) -> Result<Vec<u8>> {
    use std::io::Read;
    let dir = journal::directory(id)?.join("attachments");
    ensure!(
        attachment.path.parent() == Some(dir.as_path()),
        "attachment is outside this conversation"
    );
    ensure!(
        attachment
            .path
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(journal::valid_id),
        "invalid image attachment ID"
    );
    ensure!(
        std::fs::symlink_metadata(&dir)?.file_type().is_dir(),
        "invalid attachment directory"
    );
    let meta = std::fs::symlink_metadata(&attachment.path)?;
    ensure!(
        meta.is_file() && meta.len() <= MAX_IMAGE,
        "invalid attachment"
    );
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut bytes = Vec::new();
    options
        .open(&attachment.path)?
        .take(MAX_IMAGE + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_IMAGE
            && verify_image(&bytes, &attachment.mime, &attachment.sha256),
        "invalid attachment contents"
    );
    decode_attachment_image(&bytes).context("invalid attachment image")?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_validation_rejects_forged_paths() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = Descriptor {
            id: journal::unique(),
            tool: Tool::Claude,
            cwd: root.path().into(),
            account_label: "Fixture".into(),
        };
        let outside = root.path().join("outside.png");
        std::fs::write(&outside, b"\x89PNG\r\n\x1a\nimage").unwrap();
        // No global config changes: a forged outside path is always rejected before any read.
        let forged = Attachment {
            path: outside,
            name: "outside".into(),
            mime: "image/png".into(),
            sha256: String::new(),
        };
        assert!(attachment_bytes(&descriptor.id, &forged).is_err());
        assert_eq!(image_mime(b"not a png"), None);
    }
}

#[cfg(test)]
mod digest_tests {
    use super::*;
    #[test]
    fn changed_private_image_is_rejected_even_when_signature_is_valid() {
        let original = b"\x89PNG\r\n\x1a\noriginal";
        let changed = b"\x89PNG\r\n\x1a\nchanged";
        assert!(verify_image(original, "image/png", &image_digest(original)));
        assert!(!verify_image(changed, "image/png", &image_digest(original)));
    }
}
fn image_digest(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn verify_image(bytes: &[u8], mime: &str, digest: &str) -> bool {
    image_mime(bytes) == Some(mime) && !digest.is_empty() && image_digest(bytes) == digest
}

mod approval;
mod view;
pub use view::{AgentPane, Presentation, SavedDraft};

#[cfg(test)]
mod image_import_tests {
    use super::*;
    #[test]
    fn imports_decode_real_supported_formats_and_preserve_original_bytes() {
        let root = tempfile::tempdir().unwrap();
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 1, image::Rgb([30, 60, 90])));
        for format in [image::ImageFormat::Png, image::ImageFormat::Jpeg, image::ImageFormat::Gif, image::ImageFormat::WebP] {
            let mut bytes = std::io::Cursor::new(Vec::new());
            image.write_to(&mut bytes, format).unwrap();
            let attachment = store_image_bytes(root.path(), "fixture", bytes.get_ref()).unwrap();
            assert_eq!(std::fs::read(&attachment.path).unwrap(), *bytes.get_ref());
            assert!(verify_image(bytes.get_ref(), &attachment.mime, &attachment.sha256));
            assert_eq!(decode_attachment_image(bytes.get_ref()).unwrap().width(), 2);
        }
    }
    #[test]
    fn malformed_oversize_and_excessive_dimensions_never_create_private_files() {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("attachments");
        assert!(store_image_bytes(&output, "fake.png", b"\x89PNG\r\n\x1a\nfake").is_err());
        assert!(store_image_bytes(&output, "huge.png", &vec![0; MAX_IMAGE as usize + 1]).is_err());
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::new(MAX_IMAGE_EDGE + 1, 1));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        assert!(store_image_bytes(&output, "too-wide.png", bytes.get_ref()).is_err());
        assert!(!output.exists());
    }
}

fn handoff_error(error: &str) -> Option<&'static str> {
    Some(match error {
        "KILN_HANDOFF_BUSY" => "현재 응답과 대기 중인 승인을 완료한 뒤 전환하세요.",
        "KILN_HANDOFF_HELPERS" => "백그라운드 작업이 실행 중이거나 프로세스 소유권을 확인하지 못했습니다. 작업을 종료한 뒤 전환하세요.",
        "KILN_HANDOFF_HISTORY" => "저장된 원본 대화를 확인하지 못했습니다. GUI 기록은 보존되었습니다.",
        "KILN_HANDOFF_STOP" => "이전 에이전트가 아직 종료되지 않았습니다. 터미널을 시작하지 않았습니다.",
        _ if error.starts_with("Finish the current response, approvals, queued work and background processes before switching") => "백그라운드 작업이 실행 중이거나 프로세스 소유권을 확인하지 못했습니다. 작업을 종료한 뒤 전환하세요.",
        _ => return None,
    })
}

pub(super) fn shell_fallback_unavailable(error: &anyhow::Error) -> bool { host::shell_unavailable(error) }

/// SDK message identity used to correlate a follow-up with its native receipt.
fn message_uuid() -> Result<String> {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 16];
    ring::rand::SystemRandom::new().fill(&mut bytes).map_err(|_| anyhow::anyhow!("Could not allocate message identity"))?;
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..]))
}
