//! Real isolated daemon/PTY probes; the agent and SSH executables are fixtures.
#![cfg(unix)]
use egui_kittest::{Harness, kittest::Queryable};
use kiln::app::{Action, KilnApp};
use std::{
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    time::{Duration, Instant},
};

struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Ok(client) =
            kiln_daemon::client::Client::connect(&self.0.join("socket").to_string_lossy(), None)
        {
            client.send(kiln_proto::ClientMsg::Shutdown);
        }
    }
}
fn pump(h: &mut Harness<'_, KilnApp>, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        h.step();
        if h.state()
            .debug_focused_text()
            .is_some_and(|s| s.contains(expected))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "expected {expected}; got {:?}",
            h.state().debug_focused_text()
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn direct_agents_and_ssh_open_without_shell_commands_and_survive_gui_restore() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    let workspace = base.join("parent workspace");
    let bin = base.join("bin");
    for path in [&workspace, &bin] {
        std::fs::create_dir_all(path).unwrap();
    }
    let codex_cwd = base.join("codex-observed-cwd");
    let claude_cwd = base.join("claude-observed-cwd");
    let quote =
        |path: &std::path::Path| format!("'{}'", path.display().to_string().replace('\'', "'\\''"));
    for (program, script) in [
        (
            "codex",
            format!(r#"#!/bin/sh
/bin/pwd -P > {}
printf 'CODEX_READY\n'
read line
printf 'RECEIVED=%s\n' "$line"
read hold
"#, quote(&codex_cwd)),
        ),
        (
            "claude",
            format!(r#"#!/bin/sh
/bin/pwd -P > {}
printf 'CLAUDE_READY\n'
read line
"#, quote(&claude_cwd)),
        ),
        (
            "ssh",
            "#!/bin/sh\nprintf 'SSH_READY\\n'\nfor arg do printf 'ARG=[%s]\\n' \"$arg\"; done\nprintf 'SSH_ARGV_DONE\\n'\nread line\n".into(),
        ),
    ] {
        let path = bin.join(program);
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let config = base.join("ssh config's ; name");
    std::fs::write(&config, "Host fixture\n HostName invalid.example\n").unwrap();
    let _cleanup = Cleanup(base.to_owned());
    unsafe {
        for (key, path) in [
            ("KILN_SOCKET", base.join("socket")),
            ("KILN_CONFIG_DIR", base.join("cfg")),
            ("KILN_ACCOUNTS_SANDBOX", base.join("accounts")),
        ] {
            std::env::set_var(key, path);
        }
        std::env::set_var(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        );
        std::env::set_var("KILN_EXE", env!("CARGO_BIN_EXE_kiln"));
        std::env::set_var("KILN_NO_AUTO_UPGRADE", "1");
        std::env::set_var("KILN_DB_NO_KEYCHAIN", "1");
        std::env::set_var("KILN_REMOTE_NO_KEYCHAIN", "1");
    }
    let mut h = Harness::builder()
        .with_size([920., 600.])
        .build_eframe(|cc| KilnApp::new(&cc.egui_ctx, Some(workspace.clone())));
    let ctx = h.ctx.clone();
    h.run_steps(3);
    h.get_by_label("새 작업").click();
    h.run_steps(2);
    h.get_by_label("Codex").click();
    pump(&mut h, "CODEX_READY");
    // Probe the child's physical directory directly. The shell's inherited PWD
    // spelling and terminal scrollback rendering must not decide this assertion.
    assert_eq!(
        std::fs::read_to_string(&codex_cwd).unwrap().trim_end(),
        workspace.canonicalize().unwrap().to_str().unwrap(),
        "real Codex fixture must start in the workspace root"
    );
    let agent = h.state().debug_focused_launch_spec().unwrap();
    assert_eq!(agent.program.as_deref(), bin.join("codex").to_str());
    assert!(agent.args.is_empty());
    assert_eq!(
        agent.cwd.as_deref(),
        workspace.canonicalize().unwrap().to_str()
    );
    h.event(egui::Event::Ime(egui::ImeEvent::Preedit {
        text: "한".into(),
        active_range_chars: Some(1..1),
    }));
    h.run_steps(2);
    h.event(egui::Event::Ime(egui::ImeEvent::Commit("한".into())));
    h.event(egui::Event::Text("?".into()));
    h.key_press(egui::Key::Enter);
    pump(&mut h, "RECEIVED=한?");
    h.state_mut().debug_checkpoint_restore(&ctx);
    h.run_steps(3);
    assert_eq!(h.state().debug_focused_launch_spec(), Some(agent));
    pump(&mut h, "CODEX_READY");
    h.state_mut().debug_apply_action(
        &ctx,
        Action::DirectAgent {
            tool: kiln_accounts::Tool::Claude,
            cwd: None,
        },
    );
    pump(&mut h, "CLAUDE_READY");
    assert_eq!(
        std::fs::read_to_string(&claude_cwd).unwrap().trim_end(),
        workspace.canonicalize().unwrap().to_str().unwrap(),
        "real Claude fixture must start in the workspace root"
    );
    assert!(
        h.state()
            .debug_focused_launch_spec()
            .unwrap()
            .args
            .is_empty()
    );
    h.state_mut().debug_apply_action(
        &ctx,
        Action::OpenSsh {
            alias: "fixture".into(),
            config_path: Some(config.clone()),
        },
    );
    pump(&mut h, "SSH_ARGV_DONE");
    let ssh = h.state().debug_focused_launch_spec().unwrap();
    assert_eq!(
        ssh.args,
        [
            "-t",
            "-oStrictHostKeyChecking=ask",
            "-oPermitLocalCommand=no",
            "-oConnectTimeout=15",
            "-F",
            config.to_str().unwrap(),
            "--",
            "fixture"
        ]
    );
    let screen = h.state().debug_focused_text().unwrap();
    assert!(screen.contains("ARG=[--]"));
    assert!(screen.contains("ARG=[fixture]"));
    h.state_mut().debug_checkpoint_restore(&ctx);
    h.run_steps(3);
    assert_eq!(h.state().debug_focused_launch_spec(), Some(ssh));
    pump(&mut h, "SSH_ARGV_DONE");
}
