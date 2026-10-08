//! Read-only SSH host discovery. No command, including `Match exec`, is run.
//! Metadata is a static hint; OpenSSH evaluates the original config on connect.
use anyhow::{Context, Result, bail};
use std::{collections::BTreeSet, path::{Path, PathBuf}};

/// Explicit user edits only. Discovered config values are display hints and
/// must not override OpenSSH's conditional configuration merely by being shown.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SshOptions {
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<PathBuf>,
}

impl SshOptions {
    pub fn validate(&self) -> Result<()> {
        if let Some(host) = &self.hostname { validate_hostname(host)?; }
        if let Some(user) = &self.user {
            if user.is_empty() || user.len() > 255 || user.starts_with('-')
                || !user.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
                bail!("Invalid SSH username");
            }
        }
        if self.port == Some(0) { bail!("Invalid SSH port"); }
        if let Some(path) = &self.identity_file {
            let text = path.to_str().context("Invalid SSH identity path")?;
            crate::safe_text(text)?;
            if text.is_empty() { bail!("Invalid SSH identity path"); }
        }
        Ok(())
    }

    /// Each option/value is its own argv element, never shell text.
    pub fn arguments(&self) -> Result<Vec<String>> {
        self.validate()?;
        let mut args = Vec::new();
        if let Some(host) = &self.hostname { args.extend(["-o".into(), format!("HostName={host}")]); }
        if let Some(user) = &self.user { args.extend(["-o".into(), format!("User={user}")]); }
        if let Some(port) = self.port { args.extend(["-o".into(), format!("Port={port}")]); }
        if let Some(path) = &self.identity_file {
            args.extend(["-i".into(), expand_home(path.to_str().unwrap()).to_string_lossy().into_owned()]);
        }
        Ok(args)
    }
}

pub fn validate_hostname(host: &str) -> Result<()> {
    if host.is_empty() || host.len() > 255 || host.starts_with('-')
        || !host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '[' | ']')) {
        bail!("Invalid SSH hostname");
    }
    Ok(())
}

pub fn manual_destination(host: &str) -> Result<String> {
    validate_hostname(host)?;
    if host.parse::<std::net::Ipv6Addr>().is_ok() { Ok(format!("[{host}]")) }
    else { Ok(host.to_owned()) }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshHost {
    pub alias: String,
    pub hostname: String,
    pub user: Option<String>,
    pub port: u16,
    pub identity_files: Vec<PathBuf>,
    pub config_path: PathBuf,
}

pub fn validate_alias(alias: &str) -> Result<()> {
    if alias.is_empty() || alias.len() > 255 || alias.starts_with('-')
        || !alias.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@' | ':' | '[' | ']')) {
        bail!("Invalid SSH host alias");
    }
    Ok(())
}

pub fn default_config_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".ssh/config"))
}

/// Import literal aliases, including aliases in nested Include files. Wildcard
/// patterns are applied as defaults but never become connection entries.
pub fn discover_hosts(config_path: &Path) -> Result<Vec<SshHost>> {
    let path = std::path::absolute(config_path)?;
    let base = default_config_path().and_then(|p| p.parent().map(Path::to_owned))
        .unwrap_or_else(|| path.parent().unwrap_or(Path::new(".")).to_owned());
    discover_with_base(&path, &base)
}

fn discover_with_base(path: &Path, base: &Path) -> Result<Vec<SshHost>> {
    let mut entries = Vec::new();
    read_entries(path, base, &mut Vec::new(), &mut entries, &mut 0)?;
    let mut aliases = BTreeSet::new();
    for (key, values) in &entries {
        if key == "host" {
            for alias in values {
                if validate_alias(alias).is_ok() { aliases.insert(alias.clone()); }
            }
        }
    }
    let mut hosts = Vec::new();
    for alias in aliases {
        let mut host = SshHost { hostname: alias.clone(), alias, user: None, port: 22, identity_files: Vec::new(), config_path: path.to_owned() };
        let (mut active, mut hostname_set, mut port_set) = (true, false, false);
        for (key, values) in &entries {
            if key == "host" { active = matches_host(values, &host.alias); continue; }
            // Match may contain local commands; discovery intentionally does
            // not evaluate those or inherit conditional settings.
            if key == "match" { active = false; continue; }
            if !active { continue; }
            let Some(value) = values.first() else { continue; };
            match key.as_str() {
                "hostname" if !hostname_set => { host.hostname = value.replace("%h", &host.alias); hostname_set = true; }
                "user" if host.user.is_none() => host.user = Some(value.clone()),
                "port" if !port_set => { if let Ok(port) = value.parse::<u16>() { if port != 0 { host.port = port; port_set = true; } } }
                "identityfile" => host.identity_files.push(expand_home(value)),
                _ => {}
            }
        }
        hosts.push(host);
    }
    Ok(hosts)
}

type Entry = (String, Vec<String>);
fn read_entries(path: &Path, base: &Path, stack: &mut Vec<PathBuf>, out: &mut Vec<Entry>, count: &mut usize) -> Result<()> {
    if stack.len() >= 16 || *count >= 512 { bail!("SSH config Include limit exceeded"); }
    let canonical = path.canonicalize().with_context(|| format!("Cannot read SSH config {}", path.display()))?;
    if stack.contains(&canonical) { bail!("SSH config Include cycle"); }
    let metadata = std::fs::metadata(&canonical)?;
    if metadata.len() > 1024 * 1024 { bail!("SSH config is too large"); }
    let text = std::fs::read_to_string(&canonical)?;
    stack.push(canonical);
    *count += 1;
    for line in text.lines() {
        let Some((key, values)) = parse_line(line) else { continue; };
        if key == "include" {
            for value in values {
                // Expanding arbitrary environment or OpenSSH runtime tokens
                // is unnecessary for discovery. The original file is retained.
                if value.contains('%') || value.contains('$') { continue; }
                let expanded = expand_home(&value);
                let pattern = if expanded.is_absolute() { expanded } else { base.join(expanded) };
                let mut includes = glob::glob(&pattern.to_string_lossy())?.collect::<std::result::Result<Vec<_>, _>>()?;
                includes.sort();
                for include in includes { read_entries(&include, base, stack, out, count)?; }
            }
        } else { out.push((key, values)); }
        if out.len() > 32_000 { bail!("SSH config contains too many directives"); }
    }
    stack.pop();
    Ok(())
}

fn expand_home(value: &str) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") { return PathBuf::from(home).join(rest); }
    }
    PathBuf::from(value)
}

fn parse_line(line: &str) -> Option<Entry> {
    let line = line.trim_start();
    let key_end = line.find(|c: char| c.is_whitespace() || c == '=').unwrap_or(line.len());
    let key = line[..key_end].to_ascii_lowercase();
    if key.is_empty() || key.starts_with('#') { return None; }
    let rest = line[key_end..].trim_start().strip_prefix('=').unwrap_or(line[key_end..].trim_start()).trim_start();
    let mut values = Vec::new();
    let (mut value, mut quoted, mut escaped, mut started) = (String::new(), false, false, false);
    for c in rest.chars() {
        if escaped { value.push(c); escaped = false; started = true; continue; }
        if c == '\\' { escaped = true; started = true; continue; }
        if c == '"' { quoted = !quoted; started = true; continue; }
        if c == '#' && !quoted { break; }
        if c.is_whitespace() && !quoted {
            if started { values.push(std::mem::take(&mut value)); started = false; }
        } else { value.push(c); started = true; }
    }
    if escaped { value.push('\\'); }
    if started { values.push(value); }
    Some((key, values))
}

fn matches_host(patterns: &[String], alias: &str) -> bool {
    let mut positive = false;
    for pattern in patterns {
        let (negated, pattern) = pattern.strip_prefix('!').map_or((false, pattern.as_str()), |p| (true, p));
        if wildcard(pattern.as_bytes(), alias.as_bytes()) {
            if negated { return false; }
            positive = true;
        }
    }
    positive
}

fn wildcard(pattern: &[u8], text: &[u8]) -> bool {
    // Bounded linear wildcard matcher; no recursive pattern backtracking.
    let (mut p, mut t, mut star, mut retry) = (0, 0, None, 0);
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p].eq_ignore_ascii_case(&text[t])) { p += 1; t += 1; }
        else if p < pattern.len() && pattern[p] == b'*' { star = Some(p); p += 1; retry = t; }
        else if let Some(s) = star { retry += 1; t = retry; p = s + 1; }
        else { return false; }
    }
    while p < pattern.len() && pattern[p] == b'*' { p += 1; }
    p == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_options_are_safe_separate_arguments_and_ipv6_is_normalized() {
        let options = SshOptions { hostname: Some("192.0.2.10".into()), user: Some("deploy-user".into()), port: Some(2202),
            identity_file: Some(PathBuf::from("/fixture/키 with ' quotes")) };
        assert_eq!(options.arguments().unwrap(), ["-o", "HostName=192.0.2.10", "-o", "User=deploy-user", "-o", "Port=2202", "-i", "/fixture/키 with ' quotes"]);
        for user in ["-root", "x;touch", "$(touch)", "`touch`", "a b", "a\nx", "foo|bar", "foo&bar", "\"root\""] {
            assert!(SshOptions { user: Some(user.into()), ..Default::default() }.validate().is_err());
        }
        assert!(SshOptions { port: Some(0), ..Default::default() }.validate().is_err());
        assert_eq!(manual_destination("::1").unwrap(), "[::1]");
        assert!(manual_destination("root@server").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn original_alias_keeps_match_and_jump_host_while_only_explicit_edits_override() {
        let dir = tempfile::tempdir().unwrap(); let config = dir.path().join("config");
        // This owned config deliberately contains no Match exec, network lookups
        // or credential providers; -G here is test-only, never passive UI discovery.
        std::fs::write(&config, "Host fixture\n HostName 192.0.2.10\n User from-config\n Port 2202\n ProxyJump gateway\nMatch originalhost fixture\n ServerAliveInterval 19\n").unwrap();
        let options = SshOptions { user: Some("edited".into()), ..Default::default() };
        let mut cmd = std::process::Command::new("/usr/bin/ssh");
        let output = cmd.args(["-G", "-F"]).arg(&config).args(options.arguments().unwrap()).args(["--", "fixture"]).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let text = String::from_utf8(output.stdout).unwrap();
        for line in ["hostname 192.0.2.10", "user edited", "port 2202", "proxyjump gateway", "serveraliveinterval 19"] { assert!(text.lines().any(|l| l == line), "missing {line}"); }
    }
    #[test]
    fn includes_quotes_first_value_defaults_and_negation() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("hosts")).unwrap();
        let path = dir.path().join("config");
        std::fs::write(&path, "Include hosts/*.conf\nHost prod-* !prod-private\n User deploy\n Port 2222\nHost *\n User fallback\n Port 22\n").unwrap();
        std::fs::write(dir.path().join("hosts/a.conf"), "Host prod-api prod-private\n HostName = \"example.test\" # comment\n IdentityFile \"/keys/private key\"\nHost staging\n User stage\n").unwrap();
        let hosts = discover_with_base(&path, dir.path()).unwrap();
        assert_eq!(hosts.iter().map(|h|h.alias.as_str()).collect::<Vec<_>>(), ["prod-api", "prod-private", "staging"]);
        assert_eq!((hosts[0].hostname.as_str(), hosts[0].user.as_deref(), hosts[0].port), ("example.test", Some("deploy"), 2222));
        assert_eq!(hosts[0].identity_files, [PathBuf::from("/keys/private key")]);
        assert_eq!((hosts[1].user.as_deref(), hosts[1].port), (Some("fallback"), 22));
        assert_eq!(hosts[2].user.as_deref(), Some("stage"));
    }
    #[test]
    fn match_exec_is_never_run_or_used_as_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        let marker = dir.path().join("MUST_NOT_EXIST");
        std::fs::write(&path, format!("Host work\n User expected\nMatch exec \"touch {}\"\n Port 3333\nHost *\n Port 22\n", marker.display())).unwrap();
        let hosts = discover_with_base(&path, dir.path()).unwrap();
        assert_eq!(hosts[0].port, 22);
        assert!(!marker.exists());
    }
    #[test]
    fn include_cycle_is_bounded_and_alias_options_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        std::fs::write(&path, "Include config\n").unwrap();
        assert!(discover_with_base(&path, dir.path()).is_err());
        for value in ["-ProxyCommand=x", "host;cmd", "h\ncmd", "h*", "!host", ""] { assert!(validate_alias(value).is_err()); }
        for value in ["prod-api", "deploy@example.test", "[::1]", "server_2"] { assert!(validate_alias(value).is_ok()); }
    }
}
