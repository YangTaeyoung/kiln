//! Native GUI launch/restore against a real isolated daemon and structured CLI fixture.
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
        if let Ok(c) =
            kiln_daemon::client::Client::connect(&self.0.join("socket").to_string_lossy(), None)
        {
            c.send(kiln_proto::ClientMsg::Shutdown);
        }
    }
}
fn pump(h: &mut Harness<'_, KilnApp>, mut ready: impl FnMut(&Harness<'_, KilnApp>) -> bool) {
    let until = Instant::now() + Duration::from_secs(25);
    loop {
        h.step();
        if ready(h) {
            return;
        }
        assert!(
            Instant::now() < until,
            "native GUI fixture timed out: {:?}",
            h.state().debug_focused_text()
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}
#[test]
fn gui_default_menu_workspace_request_exact_once_and_draft_restore() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    let workspace = base.join("workspace");
    let bin = base.join("bin");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(base.join("home")).unwrap();
    let python = std::process::Command::new("/usr/bin/which")
        .arg("python3")
        .output()
        .unwrap();
    assert!(python.status.success());
    let fixture = format!(
        "#!{}\n{}",
        String::from_utf8(python.stdout).unwrap().trim(),
        r#"
import json,sys,pathlib,time
if '--version' in sys.argv:
 print('fixture 1.0');sys.exit(0)
def emit(v): print(json.dumps(v),flush=True)
for line in sys.stdin:
 v=json.loads(line)
 if v.get('type')=='control_request':
  subtype=v['request']['subtype']
  emit({'type':'control_response','response':{'subtype':'success','request_id':v['request_id'],'response':{'models':[{'value':'fixture','displayName':'Fixture model'}]}}})
  if subtype=='initialize':emit({'type':'system','subtype':'init','session_id':'fixture-session','model':'fixture'})
 elif v.get('type')=='user':
  text=v['message']['content'][0]['text']
  with pathlib.Path('received.jsonl').open('a') as f:f.write(json.dumps(text)+'\n')
  time.sleep(0.2)
  emit({'type':'assistant','message':{'id':'reply-'+str(pathlib.Path('received.jsonl').stat().st_size),'content':[{'type':'text','text':'Native streamed **reply**'}]}})
  emit({'type':'result','subtype':'success','result':'Native streamed **reply**','session_id':'fixture-session'})
"#
    );
    let executable = bin.join("claude");
    std::fs::write(&executable, fixture).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let _cleanup = Cleanup(base.to_owned());
    unsafe {
        for (key, path) in [
            ("HOME", base.join("home")),
            ("ZDOTDIR", base.join("home")),
            ("KILN_SOCKET", base.join("socket")),
            ("KILN_CONFIG_DIR", base.join("config")),
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
        for key in [
            "KILN_NO_AUTO_UPGRADE",
            "KILN_DB_NO_KEYCHAIN",
            "KILN_REMOTE_NO_KEYCHAIN",
        ] {
            std::env::set_var(key, "1");
        }
    }
    let mut h = Harness::builder()
        .with_size([1024., 760.])
        .build_eframe(|cc| KilnApp::new(&cc.egui_ctx, Some(workspace.clone())));
    let ctx = h.ctx.clone();
    h.run_steps(3);
    h.get_by_label("새 작업").click();
    h.run_steps(2);
    h.get_by_label("대화 GUI");
    h.get_by_label("Claude Code").click();
    pump(&mut h, |h| h.query_by_label("준비됨").is_some());
    let spec = h.state().debug_focused_launch_spec().unwrap();
    assert!(spec.env.iter().any(|(k, _)| k == "KILN_GUI_AGENT"));
    assert_eq!(spec.args[0], "agent-gui-host");
    let original_session = h.state().debug_focused_session();
    let original_pane = h.state().debug_focused_pane_id().unwrap();
    h.get_by_label("메시지").click();
    h.run_steps(2);
    h.event(egui::Event::Text("보존할 한글 초안  \n".into()));
    h.run_steps(2);
    h.state_mut().debug_checkpoint_restore(&ctx);
    h.run_steps(3);
    assert_eq!(h.state().debug_focused_session(), original_session);
    assert_eq!(
        h.state().debug_focused_agent_draft().unwrap().composer.text,
        "보존할 한글 초안  \n"
    );
    assert!(!workspace.join("received.jsonl").exists());
    pump(&mut h, |h| h.query_by_label("준비됨").is_some());
    h.state_mut()
        .debug_apply_action(&ctx, Action::ClosePane(original_pane, false));
    h.run_steps(2);
    h.get_by_label("대화 초안을 버릴까요?");
    h.get_by_label("취소").click();
    h.run_steps(2);
    h.get_by_label("보내기").click();
    h.run_steps(1);
    h.state_mut().debug_apply_action(&ctx, Action::NewPage);
    pump(&mut h, |_| workspace.join("received.jsonl").exists());
    pump(&mut h, |h| {
        h.state()
            .debug_agent_draft(original_pane)
            .is_some_and(|draft| draft.pending.is_none())
    });
    assert_eq!(
        std::fs::read_to_string(workspace.join("received.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert_ne!(h.state().debug_focused_pane_id(), Some(original_pane));
    let context = "repository context\nwith spaces  ";
    let request = "정확한 최초 요청  \n두 번째 줄\n";
    h.state_mut().debug_apply_action(
        &ctx,
        Action::LaunchAgent {
            cwd: workspace.clone(),
            program: "claude".into(),
            context: context.into(),
            request: request.into(),
        },
    );
    pump(&mut h, |_| {
        std::fs::read_to_string(workspace.join("received.jsonl"))
            .is_ok_and(|s| s.lines().count() == 2)
    });
    let received = std::fs::read_to_string(workspace.join("received.jsonl")).unwrap();
    let values = received
        .lines()
        .map(|line| serde_json::from_str::<String>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(values[1], format!("{context}\n## 사용자 요청\n{request}"));
    let request_spec = h.state().debug_focused_launch_spec().unwrap();
    h.state_mut().debug_checkpoint_restore(&ctx);
    h.run_steps(8);
    assert_eq!(h.state().debug_focused_launch_spec(), Some(request_spec));
    assert_eq!(
        std::fs::read_to_string(workspace.join("received.jsonl"))
            .unwrap()
            .lines()
            .count(),
        2
    );
}
