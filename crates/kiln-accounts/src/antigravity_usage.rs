//! Official read-only slash command, supported since agy 1.1.11. Never a model prompt.
use std::{path::Path, process::{Command, Stdio}, time::Duration};
use anyhow::{Result, ensure, Context};
use crate::{Usage, QuotaGroup, QuotaBucket};

pub fn probe(binary: &Path, home: &Path) -> Result<Usage> {
    let output = crate::store::bounded_output_with_input(Command::new(binary).arg("--version"), Duration::from_secs(5), None)?;
    ensure!(output.status.success(), "Antigravity version unavailable");
    let version = String::from_utf8_lossy(&output.stdout);
    let numbers: Vec<u32> = version.trim().split('.').map(str::parse).collect::<std::result::Result<_,_>>().unwrap_or_default();
    ensure!(numbers.len() == 3 && (numbers[0], numbers[1], numbers[2]) >= (1,1,11), "Update Antigravity to 1.1.11 or later to read usage safely");
    let cwd = tempfile::tempdir()?;
    let mut command = Command::new(binary);
    command.args(["-p", "/usage", "--output-format", "json"])
        .envs(crate::antigravity::environment(home)).current_dir(cwd.path()).stdin(Stdio::null());
    let output = crate::store::bounded_output_with_input(&mut command, Duration::from_secs(35), None)?;
    ensure!(output.status.success(), "Antigravity usage request failed");
    parse(&output.stdout)
}

fn parse(bytes: &[u8]) -> Result<Usage> {
    let value: serde_json::Value = serde_json::from_slice(bytes).context("invalid Antigravity usage response")?;
    ensure!(value["status"] == "SUCCESS" && value["command"]["name"] == "usage", "Antigravity usage unavailable");
    ensure!(value["num_turns"].as_u64() == Some(0), "unexpected Antigravity agent turn");
    let source = value.pointer("/command/data/groups").and_then(|x| x.as_array()).context("Antigravity quota groups unavailable")?;
    ensure!(source.len() <= 32, "too many Antigravity quota groups");
    let mut groups = Vec::new();
    for group in source {
        let name = group["name"].as_str().context("quota group name missing")?;
        ensure!(name.len() <= 256 && !name.chars().any(char::is_control), "invalid quota group name");
        let buckets = group["buckets"].as_array().context("quota buckets missing")?;
        ensure!(buckets.len() <= 32, "too many quota buckets");
        let mut parsed = Vec::new();
        for bucket in buckets {
            let window = bucket["window"].as_str().context("quota window missing")?;
            ensure!(window.len() <= 64 && !window.chars().any(char::is_control), "invalid quota window");
            let used = match bucket.get("remaining_fraction").filter(|x| !x.is_null()) {
                Some(x) => { let remaining=x.as_f64().context("invalid quota fraction")?; ensure!((0.0..=1.0).contains(&remaining), "invalid quota fraction"); Some((1.0-remaining) as f32) },
                None => None,
            };
            let reset = bucket["reset_time"].as_str().and_then(|x| chrono::DateTime::parse_from_rfc3339(x).ok()).map(|x| x.timestamp());
            parsed.push(QuotaBucket { window: window.into(), used, reset });
        }
        groups.push(QuotaGroup { name: name.into(), buckets: parsed });
    }
    ensure!(!groups.is_empty(), "Antigravity quota groups unavailable");
    Ok(Usage { groups, status: "allowed".into(), ..Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quota_groups_preserve_model_windows_and_do_not_invent_disabled_usage() {
        let value = serde_json::json!({"status":"SUCCESS","num_turns":0,"command":{"name":"usage","data":{"groups":[{"name":"Gemini Models","buckets":[{"window":"5h","remaining_fraction":0.75,"reset_time":"2026-10-29T09:20:20Z"},{"window":"weekly","remaining_fraction":null}]}]}}});
        let usage = parse(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(usage.groups[0].buckets[0].used, Some(0.25));
        assert!(usage.groups[0].buckets[0].reset.is_some());
        assert_eq!(usage.groups[0].buckets[1].used, None);
        let mut invalid = value.clone(); invalid["command"]["data"]["groups"][0]["buckets"][0]["remaining_fraction"]=2.into();
        assert!(parse(&serde_json::to_vec(&invalid).unwrap()).is_err());
        let mut turn = value; turn["num_turns"]=1.into();
        assert!(parse(&serde_json::to_vec(&turn).unwrap()).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn old_cli_is_rejected_before_a_slash_command_can_become_a_model_prompt() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join("agy");
        let marker = root.path().join("must-not-run");
        std::fs::write(&binary, format!("#!/bin/sh\nif [ \"$1\" = --version ]; then printf '1.1.10\\n'; exit 0; fi\ntouch '{}'\n", marker.display())).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(probe(&binary, root.path()).is_err());
        assert!(!marker.exists());
    }
}
