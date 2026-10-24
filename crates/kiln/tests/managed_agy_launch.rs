#![cfg(target_os = "macos")]
use std::{os::unix::fs::PermissionsExt, process::Command, sync::Arc};
use kiln_accounts::{AccountManager, CredentialStore, Env, Tool, antigravity};

#[test]
fn managed_agy_keeps_kiln_config_when_its_home_changes() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("kiln-config");
    let bin = root.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let fake = bin.join("agy");
    std::fs::write(&fake, r#"#!/usr/bin/python3
import os, sys, json
from pathlib import Path
if '--version' in sys.argv:
    print('1.4.7')
else:
    Path(os.environ['AGY_FIXTURE_CAPTURE']).write_text(json.dumps(dict({k:os.environ.get(k) for k in ['HOME','KILN_CONFIG_DIR','KILN_AGENT_PROFILE','ANTIGRAVITY_APP_DATA_DIR']},cwd=os.getcwd())))
"#).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
    unsafe {
        std::env::set_var("KILN_CONFIG_DIR", &config);
        std::env::set_var("KILN_EXE", env!("CARGO_BIN_EXE_kiln"));
        std::env::set_var("KILN_ACCOUNTS_SANDBOX", root.path().join("sandbox"));
        std::env::set_var("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default()));
    }
    let (mut env, store) = Env::sandbox(root.path(), false);
    env.antigravity_identity = Arc::new(|_| Ok("selected@example.test".into()));
    let manager = AccountManager::with_env(env);
    store.set(antigravity::SERVICE, antigravity::ACCOUNT, r#"{"token":{"access_token":"fixture","refresh_token":"fixture"}}"#).unwrap();
    let profile = manager.save_current(Tool::Antigravity, "Selected").unwrap();
    let spec = kiln::app::agent_runtime::managed(Tool::Antigravity, root.path().to_path_buf(), &manager).unwrap();
    let capture = root.path().join("capture.json");
    let result = Command::new(spec.program.unwrap()).args(spec.args)
        .env_remove("KILN_CONFIG_DIR").envs(spec.env)
        .env("AGY_FIXTURE_CAPTURE", &capture).output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let actual: serde_json::Value = serde_json::from_slice(&std::fs::read(capture).unwrap()).unwrap();
    assert_eq!(std::path::Path::new(actual["cwd"].as_str().unwrap()).canonicalize().unwrap(), root.path().canonicalize().unwrap());
    assert_eq!(actual["KILN_CONFIG_DIR"], config.to_string_lossy().as_ref());
    assert_eq!(actual["KILN_AGENT_PROFILE"], profile.id);
    assert!(actual["HOME"].as_str().unwrap().contains("agent-accounts/antigravity"));
    assert!(actual["ANTIGRAVITY_APP_DATA_DIR"].as_str().unwrap().starts_with(actual["HOME"].as_str().unwrap()));
}
