//! Exact argv launches: never interpolate a destination or executable into a shell.
use kiln_proto::SpawnSpec;
use std::path::{Path, PathBuf};

#[cfg(test)]
pub fn ssh(alias: &str, config_path: Option<&Path>) -> Result<SpawnSpec, String> {
    ssh_with_options(alias, config_path, &kiln_remote::ssh_config::SshOptions::default())
}

pub fn ssh_with_options(alias: &str, config_path: Option<&Path>, options: &kiln_remote::ssh_config::SshOptions) -> Result<SpawnSpec, String> {
    kiln_remote::ssh_config::validate_alias(alias).map_err(|error| error.to_string())?;
    let binary = super::agent_launch::executable("ssh")
        .ok_or_else(|| kiln_common::i18n::tr("SSH 클라이언트를 찾을 수 없습니다.").to_owned())?;
    let binary = std::path::absolute(binary).map_err(|error| error.to_string())?;
    let mut args = vec!["-t".to_owned(), "-oStrictHostKeyChecking=ask".to_owned(), "-oPermitLocalCommand=no".to_owned(), "-oConnectTimeout=15".to_owned()];
    if let Some(path) = config_path {
        if !path.is_file() {
            return Err(kiln_common::i18n::tr("SSH 설정 파일을 찾을 수 없습니다.").to_owned());
        }
        let path = std::path::absolute(path).map_err(|error| error.to_string())?;
        args.extend(["-F".to_owned(), path.to_string_lossy().into_owned()]);
    }
    args.extend(options.arguments().map_err(|error| error.to_string())?);
    // Alias validation plus the option terminator prevent option injection.
    args.extend(["--".to_owned(), alias.to_owned()]);
    Ok(SpawnSpec {
        program: Some(binary.to_string_lossy().into_owned()), args,
        cols: 100, rows: 30, name: Some(format!("SSH · {alias}")), ..Default::default()
    })
}

pub fn agent(tool: kiln_accounts::Tool, cwd: PathBuf) -> Result<SpawnSpec, String> {
    if !cwd.is_dir() {
        return Err(kiln_common::i18n::tr("워크스페이스 폴더를 찾을 수 없습니다.").to_owned());
    }
    let program = match tool { kiln_accounts::Tool::Codex => "codex", kiln_accounts::Tool::Claude => "claude" };
    let binary = super::agent_launch::executable(program)
        .ok_or_else(|| kiln_common::trf!("{program}을 찾을 수 없습니다. 설치 후 다시 시작해 주세요."))?;
    let binary = std::path::absolute(binary).map_err(|error| error.to_string())?;
    Ok(SpawnSpec {
        cwd: Some(cwd.to_string_lossy().into_owned()),
        program: Some(binary.to_string_lossy().into_owned()),
        cols: 100, rows: 30,
        name: Some(match tool { kiln_accounts::Tool::Codex => "Codex", kiln_accounts::Tool::Claude => "Claude Code" }.into()),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ssh_keeps_config_and_alias_in_separate_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config with ' and ; spaces");
        std::fs::write(&config, "Host review\n HostName localhost\n").unwrap();
        let spec = ssh("review", Some(&config)).unwrap();
        assert_eq!(spec.args, ["-t", "-oStrictHostKeyChecking=ask", "-oPermitLocalCommand=no", "-oConnectTimeout=15", "-F", config.to_str().unwrap(), "--", "review"]);
        assert_eq!(spec.name.as_deref(), Some("SSH · review"));
        for alias in ["-oProxyCommand=touch", "host;touch", "host\ncommand", "", "*", "$(cmd)"] {
            assert!(ssh(alias, Some(&config)).is_err(), "{alias:?}");
        }
    }
    #[test]
    fn missing_config_is_an_error_before_a_pane_can_be_created() {
        let dir = tempfile::tempdir().unwrap();
        assert!(ssh("review", Some(&dir.path().join("missing"))).is_err());
    }
}
