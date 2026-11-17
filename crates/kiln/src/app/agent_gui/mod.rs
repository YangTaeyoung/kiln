//! Native structured agent runtime, owned by the persistent terminal session.
mod launch;
pub use launch::*;
mod controls;
pub use controls::*;
mod tool_presentation;
mod model;
pub use model::*;
mod client;
mod host;
mod journal;
mod providers;
use anyhow::{Context, Result, ensure};
pub use client::{Client, HistorySummary};
pub use host::serve;
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
    let mime = image_mime(&bytes).context("unsupported image format")?;
    let dir = journal::directory(&descriptor.id)?.join("attachments");
    journal::private_dir(&dir)?;
    let target = dir.join(journal::unique());
    // Image payloads have their own higher bound; journal records remain bounded independently.
    let mut f = journal::open(&target, false)?;
    use std::io::Write;
    f.write_all(&bytes)?;
    f.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o400))?;
    }
    Ok(Attachment {
        path: target,
        name: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        mime: mime.into(),
        sha256: image_digest(&bytes),
    })
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
