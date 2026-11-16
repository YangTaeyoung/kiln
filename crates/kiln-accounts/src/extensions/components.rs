//! Typed author-component validation. Parsing never starts a hook/server or installs
//! anything. Identity-bound credentials/configuration stay in native setup.
use super::plugins::{boolean, obj, strings, text};
use super::*;

fn file<'a>(s: &'a Snapshot, path: &str) -> R<&'a FileData> {
    let relative = path.strip_prefix("./").ok_or("NONPORTABLE_PLUGIN_PATH")?;
    fs::relative(relative)?;
    s.files
        .iter()
        .find(|f| f.path == relative)
        .ok_or("PLUGIN_COMPONENT_MISSING")
}
fn json_file(s: &Snapshot, path: &str) -> R<Value> {
    let f = file(s, path)?;
    if !path.ends_with(".json") {
        return Err("PLUGIN_BINARY_COMPONENT_REQUIRES_NATIVE_SETUP");
    }
    serde_json::from_slice(&f.bytes).map_err(|_| "INVALID_PLUGIN_COMPONENT")
}
fn path(s: &Snapshot, value: &str, root_allowed: bool) -> R<()> {
    if root_allowed && [".", "./"].contains(&value) {
        return Ok(());
    }
    let p = value
        .strip_prefix("./")
        .ok_or("NONPORTABLE_PLUGIN_PATH")?
        .trim_end_matches('/');
    fs::relative(p)?;
    if s.files
        .iter()
        .any(|f| f.path == p || f.path.starts_with(&format!("{p}/")))
        || s.dirs.iter().any(|d| d == p)
    {
        Ok(())
    } else {
        Err("PLUGIN_COMPONENT_MISSING")
    }
}
pub(super) fn paths(s: &Snapshot, v: &Value, root: bool) -> R<()> {
    match v {
        Value::String(v) => path(s, v, root),
        Value::Array(a) => {
            for v in a {
                path(
                    s,
                    v.as_str().ok_or("UNKNOWN_PLUGIN_COMPONENT_SCHEMA")?,
                    root,
                )?;
            }
            Ok(())
        }
        _ => Err("UNKNOWN_PLUGIN_COMPONENT_SCHEMA"),
    }
}
fn str_list(v: &Value) -> R<()> {
    if !v.as_array().is_some_and(|a| a.iter().all(Value::is_string)) {
        return Err("UNKNOWN_PLUGIN_COMPONENT_SCHEMA");
    }
    Ok(())
}
fn numeric(v: &Value, keys: &[&str]) -> R<()> {
    for k in keys {
        if v.get(*k).is_some_and(|v| !v.is_number()) {
            return Err("UNKNOWN_PLUGIN_COMPONENT_SCHEMA");
        }
    }
    Ok(())
}
fn sensitive_key(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    [
        "authorization",
        "password",
        "secret",
        "token",
        "api_key",
        "apikey",
        "api-key",
        "cookie",
        "credential",
        "bearer",
        "oauth",
    ]
    .iter()
    .any(|s| k.contains(s))
}
fn portable_string(s: &Snapshot, v: &str) -> R<()> {
    let lowered = v.to_ascii_lowercase();
    if [
        "--token",
        "--api-key",
        "--password",
        "authorization:",
        "bearer ",
        "${user_config.",
        "${claude_plugin_data}",
    ]
    .iter()
    .any(|p| lowered.contains(p))
    {
        return Err("PLUGIN_REQUIRES_CONFIGURATION");
    }
    let mut tail = v;
    while let Some(at) = tail.find("${") {
        tail = &tail[at + 2..];
        let end = tail.find('}').ok_or("PLUGIN_REQUIRES_CONFIGURATION")?;
        let key = &tail[..end];
        if !["CLAUDE_PLUGIN_ROOT", "CODEX_PLUGIN_ROOT"].contains(&key) {
            return Err("PLUGIN_REQUIRES_CONFIGURATION");
        }
        tail = &tail[end + 1..];
        if let Some(relative) = tail.strip_prefix('/') {
            let relative = relative
                .split(|c: char| c.is_whitespace() || ['\'', '"', ';', ')'].contains(&c))
                .next()
                .unwrap_or("");
            path(s, &format!("./{relative}"), false)?;
        }
    }
    if v.contains("~/") || v.contains("$HOME") || v.contains("%USERPROFILE%") {
        return Err("PLUGIN_REQUIRES_CONFIGURATION");
    }
    Ok(())
}
fn configuration(s: &Snapshot, v: &Value) -> R<()> {
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                if sensitive_key(k) {
                    return Err("PLUGIN_REQUIRES_CONFIGURATION");
                }
                configuration(s, v)?;
            }
        }
        Value::Array(a) => {
            for v in a {
                configuration(s, v)?;
            }
        }
        Value::String(v) => portable_string(s, v)?,
        _ => {}
    }
    Ok(())
}
fn environment(s: &Snapshot, v: &Value) -> R<()> {
    let m = v.as_object().ok_or("UNKNOWN_PLUGIN_COMPONENT_SCHEMA")?;
    for (k, v) in m {
        let value = v.as_str().ok_or("UNKNOWN_PLUGIN_COMPONENT_SCHEMA")?;
        if !["NODE_ENV", "LOG_LEVEL", "RUST_LOG", "PYTHONUNBUFFERED"].contains(&k.as_str())
            && !value.starts_with("${CLAUDE_PLUGIN_ROOT}/")
            && !value.starts_with("${CODEX_PLUGIN_ROOT}/")
        {
            return Err("PLUGIN_REQUIRES_CONFIGURATION");
        }
        if sensitive_key(k) {
            return Err("PLUGIN_REQUIRES_CONFIGURATION");
        }
        portable_string(s, value)?;
    }
    Ok(())
}
fn headers(v: &Value) -> R<()> {
    let map = v.as_object().ok_or("UNKNOWN_PLUGIN_COMPONENT_SCHEMA")?;
    for (k, v) in map {
        if !["accept", "content-type", "user-agent"].contains(&k.to_ascii_lowercase().as_str())
            || !v.is_string()
        {
            return Err("PLUGIN_REQUIRES_CONFIGURATION");
        }
    }
    Ok(())
}
fn endpoint(v: &str) -> R<()> {
    let u = url::Url::parse(v).map_err(|_| "UNKNOWN_PLUGIN_COMPONENT_SCHEMA")?;
    if !["http", "https"].contains(&u.scheme())
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
    {
        return Err("PLUGIN_REQUIRES_CONFIGURATION");
    }
    Ok(())
}
fn server(s: &Snapshot, v: &Value, lsp: bool) -> R<()> {
    configuration(s, v)?;
    obj(
        v,
        if lsp {
            &[
                "command",
                "args",
                "extensionToLanguage",
                "transport",
                "env",
                "initializationOptions",
                "settings",
                "workspaceFolder",
                "startupTimeout",
                "shutdownTimeout",
                "restartOnCrash",
                "maxRestarts",
            ]
        } else {
            &[
                "type",
                "command",
                "args",
                "env",
                "cwd",
                "url",
                "headers",
                "timeout",
                "startup_timeout_sec",
                "tool_timeout_sec",
                "enabled",
                "disabled",
                "enabled_tools",
                "disabled_tools",
            ]
        },
    )?;
    strings(
        v,
        &[
            "type",
            "command",
            "cwd",
            "url",
            "transport",
            "workspaceFolder",
        ],
    )?;
    numeric(
        v,
        &[
            "timeout",
            "startup_timeout_sec",
            "tool_timeout_sec",
            "startupTimeout",
            "shutdownTimeout",
            "maxRestarts",
        ],
    )?;
    for k in ["enabled", "disabled", "restartOnCrash"] {
        boolean(v, k)?;
    }
    if let Some(command) = v.get("command").and_then(Value::as_str) {
        if Path::new(command).is_absolute() {
            return Err("NONPORTABLE_PLUGIN_PATH");
        }
        if command.starts_with("./") {
            path(s, command, false)?;
        }
    }
    if let Some(args) = v.get("args") {
        str_list(args)?;
    }
    if let Some(v) = v.get("env") {
        environment(s, v)?;
    }
    if let Some(v) = v.get("headers") {
        headers(v)?;
    }
    if let Some(v) = v.get("url").and_then(Value::as_str) {
        endpoint(v)?;
    }
    if let Some(v) = v.get("cwd").and_then(Value::as_str) {
        if v.starts_with("./") {
            path(s, v, true)?;
        } else if !v.starts_with("${CLAUDE_PLUGIN_ROOT}") && !v.starts_with("${CODEX_PLUGIN_ROOT}")
        {
            return Err("NONPORTABLE_PLUGIN_PATH");
        }
    }
    if let Some(v) = v.get("extensionToLanguage") {
        let m = v.as_object().ok_or("UNKNOWN_PLUGIN_COMPONENT_SCHEMA")?;
        if !m.values().all(Value::is_string) {
            return Err("UNKNOWN_PLUGIN_COMPONENT_SCHEMA");
        }
    }
    for k in ["enabled_tools", "disabled_tools"] {
        if let Some(v) = v.get(k) {
            str_list(v)?;
        }
    }
    Ok(())
}
fn servers(s: &Snapshot, v: &Value, lsp: bool, file_form: bool) -> R<()> {
    let v = if file_form && !lsp && v.get("mcpServers").is_some() {
        obj(v, &["mcpServers"])?;
        &v["mcpServers"]
    } else {
        v
    };
    let map = v.as_object().ok_or("UNKNOWN_PLUGIN_COMPONENT_SCHEMA")?;
    if map.len() > MAX_RECORDS {
        return Err("REGISTRY_RECORDS_1024");
    }
    for (name, v) in map {
        fs::component(name)?;
        server(s, v, lsp)?;
    }
    Ok(())
}
fn hooks(s: &Snapshot, v: &Value, file_form: bool) -> R<()> {
    let v = if file_form {
        obj(v, &["hooks", "description"])?;
        v.get("hooks").ok_or("UNKNOWN_PLUGIN_COMPONENT_SCHEMA")?
    } else {
        v
    };
    let events = v.as_object().ok_or("UNKNOWN_PLUGIN_COMPONENT_SCHEMA")?;
    for (event, groups) in events {
        fs::component(event)?;
        for group in groups.as_array().ok_or("UNKNOWN_PLUGIN_COMPONENT_SCHEMA")? {
            obj(group, &["matcher", "hooks"])?;
            strings(group, &["matcher"])?;
            for hook in group
                .get("hooks")
                .and_then(Value::as_array)
                .ok_or("UNKNOWN_PLUGIN_COMPONENT_SCHEMA")?
            {
                configuration(s, hook)?;
                obj(
                    hook,
                    &[
                        "type",
                        "command",
                        "prompt",
                        "timeout",
                        "statusMessage",
                        "once",
                        "async",
                        "asyncRewake",
                        "model",
                        "url",
                        "headers",
                    ],
                )?;
                strings(
                    hook,
                    &["type", "command", "prompt", "statusMessage", "model", "url"],
                )?;
                numeric(hook, &["timeout"])?;
                for k in ["once", "async", "asyncRewake"] {
                    boolean(hook, k)?;
                }
                match text(hook, "type")? {
                    "command" => {
                        text(hook, "command")?;
                    }
                    "prompt" | "agent" => {
                        text(hook, "prompt")?;
                    }
                    "http" => {
                        endpoint(text(hook, "url")?)?;
                        if let Some(v) = hook.get("headers") {
                            headers(v)?;
                        }
                    }
                    _ => return Err("UNKNOWN_PLUGIN_COMPONENT_SCHEMA"),
                }
            }
        }
    }
    Ok(())
}
fn components(s: &Snapshot, v: &Value, kind: &str, inline_wrapper: bool) -> R<()> {
    if let Value::Array(a) = v {
        for v in a {
            components(s, v, kind, inline_wrapper)?;
        }
        return Ok(());
    }
    let loaded;
    if let Some(p) = v.as_str() {
        loaded = json_file(s, p)?;
        return component_value(s, &loaded, kind, true);
    }
    component_value(s, v, kind, inline_wrapper)
}
fn component_value(s: &Snapshot, v: &Value, kind: &str, file_form: bool) -> R<()> {
    match kind {
        "hooks" => hooks(s, v, file_form),
        "mcp" => servers(s, v, false, file_form),
        "lsp" => servers(s, v, true, file_form),
        _ => Err("UNKNOWN_PLUGIN_COMPONENT_SCHEMA"),
    }
}
fn optional_file(s: &Snapshot, path: &str, kind: &str) -> R<()> {
    if s.files.iter().any(|f| f.path == path) {
        let v = json_file(s, &format!("./{path}"))?;
        component_value(s, &v, kind, true)?;
    }
    Ok(())
}
fn commands(s: &Snapshot, v: &Value) -> R<()> {
    if let Some(map) = v.as_object() {
        for (name, command) in map {
            fs::component(name)?;
            obj(
                command,
                &[
                    "source",
                    "content",
                    "description",
                    "argument-hint",
                    "argumentHint",
                    "model",
                    "allowed-tools",
                    "disable-model-invocation",
                    "user-invocable",
                ],
            )?;
            strings(
                command,
                &[
                    "source",
                    "content",
                    "description",
                    "argument-hint",
                    "argumentHint",
                    "model",
                ],
            )?;
            for k in ["disable-model-invocation", "user-invocable"] {
                boolean(command, k)?;
            }
            if command.get("source").is_some() == command.get("content").is_some() {
                return Err("UNKNOWN_PLUGIN_COMPONENT_SCHEMA");
            }
            if let Some(p) = command.get("source").and_then(Value::as_str) {
                path(s, p, false)?;
            }
        }
        Ok(())
    } else {
        paths(s, v, false)
    }
}
pub(super) fn claude(s: &Snapshot, v: &Value) -> R<()> {
    for k in ["skills", "agents", "outputStyles", "workflows"] {
        if let Some(v) = v.get(k) {
            paths(s, v, k == "skills")?;
        }
    }
    if let Some(v) = v.get("commands") {
        commands(s, v)?;
    }
    for (field, kind, default) in [
        ("mcpServers", "mcp", ".mcp.json"),
        ("lspServers", "lsp", ".lsp.json"),
        ("hooks", "hooks", "hooks/hooks.json"),
    ] {
        optional_file(s, default, kind)?;
        if let Some(v) = v.get(field) {
            components(s, v, kind, false)?;
        }
    }
    if v.get("userConfig")
        .is_some_and(|v| v.as_object().is_none_or(|m| !m.is_empty()))
        || s.files.iter().any(|f| f.path.starts_with("data/"))
    {
        return Err("PLUGIN_REQUIRES_CONFIGURATION");
    }
    Ok(())
}
fn interface(s: &Snapshot, v: &Value) -> R<()> {
    let scalar = [
        "displayName",
        "shortDescription",
        "longDescription",
        "developerName",
        "category",
        "websiteUrl",
        "websiteURL",
        "privacyPolicyUrl",
        "privacyPolicyURL",
        "termsOfServiceUrl",
        "termsOfServiceURL",
        "brandColor",
        "composerIcon",
        "logo",
        "logoDark",
    ];
    let mut fields = scalar.to_vec();
    fields.extend(["capabilities", "defaultPrompt", "screenshots"]);
    obj(v, &fields)?;
    strings(v, &scalar)?;
    for k in ["capabilities", "screenshots"] {
        if let Some(v) = v.get(k) {
            str_list(v)?;
        }
    }
    if let Some(v) = v.get("defaultPrompt") {
        if !v.is_string() {
            str_list(v)?;
        }
    }
    for k in ["composerIcon", "logo", "logoDark"] {
        if let Some(v) = v.get(k).and_then(Value::as_str) {
            path(s, v, false)?;
        }
    }
    if let Some(a) = v.get("screenshots").and_then(Value::as_array) {
        for v in a {
            path(s, v.as_str().unwrap(), false)?;
        }
    }
    Ok(())
}
fn codex_extension(s: &Snapshot, v: &Value) -> R<()> {
    obj(
        v,
        &[
            "name",
            "version",
            "description",
            "keywords",
            "skills",
            "commands",
            "mcpServers",
            "hooks",
            "interface",
            "onboardingSkill",
            "extensions",
            "apps",
        ],
    )?;
    strings(v, &["name", "version", "description", "apps"])?;
    for k in ["skills", "commands"] {
        if let Some(v) = v.get(k) {
            paths(s, v, false)?;
        }
    }
    if let Some(v) = v.get("interface") {
        interface(s, v)?;
    }
    if let Some(v) = v.get("onboardingSkill") {
        let value = v.as_str().ok_or("UNKNOWN_PLUGIN_COMPONENT_SCHEMA")?;
        path(
            s,
            &format!("./{}", value.strip_prefix("./").unwrap_or(value)),
            false,
        )?;
    }
    if let Some(v) = v.get("mcpServers") {
        components(s, v, "mcp", false)?;
    }
    if let Some(v) = v.get("hooks") {
        components(s, v, "hooks", true)?;
    }
    // App declarations select account-backed connectors; no configuration data is
    // transplanted until its exact schema has a separate portable contract.
    if v.get("apps").is_some() {
        return Err("PLUGIN_REQUIRES_CONFIGURATION");
    }
    if let Some(v) = v.get("extensions") {
        obj(v, &["com.openai"])?;
        if let Some(extension) = v.get("com.openai") {
            obj(extension, &["onboardingSkill"])?;
            if let Some(value) = extension.get("onboardingSkill") {
                let value = value.as_str().ok_or("UNKNOWN_PLUGIN_COMPONENT_SCHEMA")?;
                path(
                    s,
                    &format!("./{}", value.strip_prefix("./").unwrap_or(value)),
                    false,
                )?;
            }
        }
    }
    Ok(())
}
pub(super) fn codex(s: &Snapshot, v: &Value, v1: bool) -> R<()> {
    // Validate both known defaults in the full copied bundle, even when the
    // selected manifest overrides one. Never copy a dormant credential sidecar.
    optional_file(s, ".mcp.json", "mcp")?;
    optional_file(s, "mcp.json", "mcp")?;
    optional_file(s, "hooks/hooks.json", "hooks")?;
    if v1 {
        if let Some(extensions) = v.get("extensions") {
            obj(extensions, &["com.openai"])?;
            if let Some(extension) = extensions.get("com.openai") {
                codex_extension(s, extension)?;
            }
        }
        for p in [".codex-plugin/plugin.json", ".claude-plugin/plugin.json"] {
            if let Some(f) = s.files.iter().find(|f| f.path == p) {
                let overlay: Value =
                    serde_json::from_slice(&f.bytes).map_err(|_| "INVALID_PLUGIN_MANIFEST")?;
                codex_extension(s, &overlay)?;
            }
        }
    } else {
        codex_extension(s, v)?;
    }
    if s.files
        .iter()
        .any(|f| f.path == ".app.json" || f.path.starts_with("data/"))
    {
        return Err("PLUGIN_REQUIRES_CONFIGURATION");
    }
    Ok(())
}
