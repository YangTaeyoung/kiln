//! Read-only Antigravity MCP inventory for the session worker.
use super::*;

const AGY_MCP_LIMIT: usize = 256;

fn agy_mcp_status(output: &[u8]) -> Option<NativeStatus> {
    let text = std::str::from_utf8(output).ok()?;
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let header = lines.next()?.trim();
    let mut status = NativeStatus {
        subject: "mcp".into(),
        ..Default::default()
    };
    if header == "No MCP servers configured." {
        return Some(status);
    }
    if !header
        .split_whitespace()
        .eq(["NAME", "TYPE", "STATUS", "COMMAND/URL"])
    {
        return None;
    }
    let offsets = [
        header.find("TYPE")?,
        header.find("STATUS")?,
        header.find("COMMAND/URL")?,
    ];
    for (index, line) in lines.enumerate() {
        if index == AGY_MCP_LIMIT {
            status.unavailable = Some("The MCP server list is partial.".into());
            break;
        }
        let Some([name, transport, configured]) = agy_mcp_columns(line, offsets) else {
            status.unavailable = Some("The MCP server list is partial.".into());
            continue;
        };
        // COMMAND/URL can contain credentials. Retain only the first three columns.
        add_agy_mcp_row(&mut status, index, name, transport, configured, "global");
    }
    Some(status)
}

fn agy_mcp_columns(line: &str, offsets: [usize; 3]) -> Option<[&str; 3]> {
    // The CLI pads to the header's column positions, counting characters.
    // Names can contain spaces and non-ASCII characters.
    let byte = |position| line.char_indices().nth(position).map(|(index, _)| index);
    let [transport_start, status_start, command_start] = offsets;
    let name = line.get(..byte(transport_start)?)?.trim();
    let transport = line
        .get(byte(transport_start)?..byte(status_start)?)?
        .trim();
    let configured = line.get(byte(status_start)?..byte(command_start)?)?.trim();
    (agy_mcp_name_valid(name)
        && matches!(transport, "http" | "stdio" | "sse")
        && matches!(configured, "enabled" | "disabled"))
    .then_some([name, transport, configured])
}

fn agy_mcp_name_valid(name: &str) -> bool {
    !name.is_empty() && name.len() <= 256 && !name.chars().any(char::is_control)
}
fn add_agy_mcp_row(
    status: &mut NativeStatus,
    index: usize,
    name: &str,
    transport: &str,
    configured: &str,
    scope: &str,
) {
    for (key, value) in [
        ("name", name),
        ("transport", transport),
        ("status", configured),
        ("scope", scope),
    ] {
        status
            .fields
            .insert(format!("{index}.{key}"), ControlValue::Text(value.into()));
    }
}

// The management CLI lists global definitions. This is the documented workspace
// configuration location, and only display metadata leaves the worker.
fn add_agy_workspace_mcp(cwd: &Path, status: &mut NativeStatus) -> Result<()> {
    let path = cwd.join(".agents/mcp_config.json");
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    const MAX_CONFIG: u64 = 1024 * 1024;
    ensure!(
        metadata.is_file() && metadata.len() <= MAX_CONFIG,
        "Invalid MCP workspace configuration"
    );
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct WorkspaceMcp {
        #[serde(default)]
        mcp_servers: std::collections::BTreeMap<String, Server>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Server {
        command: Option<String>,
        server_url: Option<String>,
        url: Option<String>,
        #[serde(default)]
        disabled: bool,
    }
    let config: WorkspaceMcp = serde_json::from_reader(fs::File::open(path)?.take(MAX_CONFIG))?;
    let offset = status
        .fields
        .keys()
        .filter_map(|key| key.strip_suffix(".name")?.parse::<usize>().ok())
        .max()
        .map_or(0, |index| index + 1);
    for (i, (name, server)) in config.mcp_servers.iter().enumerate() {
        if i == AGY_MCP_LIMIT {
            status.unavailable = Some("The MCP server list is partial.".into());
            break;
        }
        let transport = if server
            .command
            .as_ref()
            .is_some_and(|value| !value.is_empty())
        {
            "stdio"
        } else if [&server.server_url, &server.url]
            .into_iter()
            .flatten()
            .any(|value| !value.is_empty())
        {
            "http"
        } else {
            continue;
        };
        if agy_mcp_name_valid(name) {
            let configured = if server.disabled {
                "disabled"
            } else {
                "enabled"
            };
            add_agy_mcp_row(status, offset + i, name, transport, configured, "workspace");
        }
    }
    Ok(())
}

pub(super) fn query(
    config: &Config,
    accounts: &AccountManager,
) -> Result<crossbeam_channel::Receiver<NativeStatus>> {
    let mut command = base_command(config, accounts)?;
    command.args(["mcp", "list"]);
    let cwd = config.descriptor.cwd.clone();
    let (send, receive) = crossbeam_channel::bounded(1);
    std::thread::Builder::new()
        .name("agy-mcp-status".into())
        .spawn(move || {
            let mut status = crate::app::agent_runtime::bounded(command, Duration::from_secs(8))
                .ok()
                .and_then(|output| agy_mcp_status(&output))
                .unwrap_or_else(unavailable);
            if add_agy_workspace_mcp(&cwd, &mut status).is_err() {
                status.unavailable =
                    Some("Antigravity's workspace MCP configuration could not be read.".into());
            }
            let _ = send.send(status);
        })?;
    Ok(receive)
}

pub(super) fn unavailable() -> NativeStatus {
    NativeStatus {
        subject: "mcp".into(),
        unavailable: Some("Antigravity's configured MCP server list is unavailable.".into()),
        ..Default::default()
    }
}
