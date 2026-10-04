//! End-to-end navigation and visual proof for the compact development workspace.
use egui_kittest::{Harness,kittest::Queryable};
use kiln::app::{Action,KilnApp};
use std::{path::{Path,PathBuf},time::{Duration,Instant},process::Command};
fn git(path:&Path,args:&[&str]) {
 let out=Command::new("git").args(["-c","commit.gpgsign=false","-c","core.hooksPath=/dev/null","-c","user.name=Kiln Test","-c","user.email=test@example.invalid","-C"]).arg(path).args(args).output().unwrap(); assert!(out.status.success(),"{}",String::from_utf8_lossy(&out.stderr));
}
struct Cleanup(PathBuf);
impl Drop for Cleanup {fn drop(&mut self){if let Ok(c)=kiln_daemon::client::Client::connect(&self.0.join("d.sock").to_string_lossy(),None){c.send(kiln_proto::ClientMsg::Shutdown);}}}
fn pump(h:&mut Harness<'_,KilnApp>,mut f:impl FnMut(&Harness<'_,KilnApp>)->bool){let until=Instant::now()+Duration::from_secs(12);loop{h.step();if f(h){return;}assert!(Instant::now()<until,"workflow timed out");std::thread::sleep(Duration::from_millis(25));}}
#[test]
fn parent_main_worktree_navigation_and_quiet_chrome(){
 let base=PathBuf::from(format!("/tmp/kiln-distill-{}",std::process::id()));
 std::fs::create_dir_all(base.join("cfg")).unwrap();let _cleanup=Cleanup(base.clone());
 let parent=base.join("personal");let repo=parent.join("product");let wt=base.join("login-ui");std::fs::create_dir_all(&repo).unwrap();
 git(&repo,&["init","-q","-b","main"]);git(&repo,&["commit","--allow-empty","-qm","init"]);git(&repo,&["worktree","add","-qb","feature/login",wt.to_str().unwrap()]);
 // A second, independently versioned frontend makes the parent-folder review realistic.
 let frontend=parent.join("web");std::fs::create_dir_all(frontend.join("src")).unwrap();
 git(&frontend,&["init","-q","-b","main"]);
 std::fs::write(frontend.join("src/app.ts"),"export const app = 'ready';\n").unwrap();
 git(&frontend,&["add","."]);git(&frontend,&["commit","-qm","initial web"]);
 std::fs::write(frontend.join("src/app.ts"),"export const app = 'updated';\n").unwrap();
 let shell=base.join("shell");std::fs::create_dir_all(&shell).unwrap();std::fs::write(shell.join(".zshrc"),"PROMPT='%1~ > '\n").unwrap();
 unsafe {for (name,value) in [("KILN_SOCKET",base.join("d.sock")),("KILN_CONFIG_DIR",base.join("cfg")),("KILN_ACCOUNTS_SANDBOX",base.join("accounts")),("ZDOTDIR",shell)]{std::env::set_var(name,value);}std::env::set_var("KILN_EXE",env!("CARGO_BIN_EXE_kiln"));std::env::set_var("KILN_NO_AUTO_UPGRADE","1");std::env::set_var("KILN_DB_NO_KEYCHAIN","1");}
 let mut h=Harness::builder().with_size([1440.,900.]).wgpu().build_eframe(|cc|KilnApp::new(&cc.egui_ctx,Some(parent.clone())));
 pump(&mut h,|h|h.state().debug_focused_text().is_some_and(|s|!s.is_empty()));
 for p in [&repo,&wt]{h.state_mut().debug_queue_action(Action::NewWorkspace(Some(p.clone())));h.run_steps(3);}
 pump(&mut h,|h|h.query_by_label("워크트리 · feature/login").is_some() && h.query_by_label("메인 · main").is_some());
 pump(&mut h,|h|h.state().debug_focused_text().is_some_and(|s|s.contains('>')));h.run_steps(4);
 let output=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/reviews/2026-09-29-distill");std::fs::create_dir_all(&output).unwrap();h.render().unwrap().save(output.join("workspace-1440.png")).unwrap();
 let main=h.query_all_by_label("product").next().unwrap().rect();let linked=h.query_all_by_label("login-ui").next().unwrap().rect();assert!(linked.left()>main.left());assert!(linked.top()>main.top(),"main {main:?} linked {linked:?}");
 assert!(h.query_by_label("기록").is_none());assert!(h.query_by_label("연동").is_none());assert!(h.query_by_label("상태 미확인").is_none());
 let output=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/reviews/2026-09-29-distill");std::fs::create_dir_all(&output).unwrap();
 h.render().unwrap().save(output.join("workspace-1440.png")).unwrap();
 h.query_all_by_label("personal").next().unwrap().click();h.run_steps(3);assert_eq!(h.state().debug_active_workspace_root(),parent.canonicalize().unwrap());
 // The workspace sidebar only switches work. Inspection is a separate surface.
 assert!(h.query_by_label("작업 공간 메뉴").is_none());
 assert!(h.query_by_label("데이터베이스").is_none());
 h.get_by_label("작업 공간 살펴보기").click();h.run_steps(4);
 for label in ["파일","변경","GitHub"] {h.get_by_label(label);}
 h.render().unwrap().save(output.join("inspector-files-1440.png")).unwrap();
 h.get_by_label("변경").click();h.run_steps(5);
 if let Some(expand)=h.query_by_label("web 변경 펼치기") {expand.click();h.run_steps(3);}
 pump(&mut h,|h|h.query_by_label_contains("app.ts").is_some());
 assert_eq!(h.state().debug_active_workspace_root(),parent.canonicalize().unwrap());
 h.render().unwrap().save(output.join("inspector-changes-1440.png")).unwrap();
 // An oversized dock saved by an older build must not squeeze out terminals.
 let dock=egui::Id::new("tools-dock");
 h.ctx.data_mut(|d|d.insert_persisted(dock,egui::containers::panel::PanelState{outer_rect:egui::Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(1400.,800.))}));
 h.run_steps(4);
 let restored=egui::containers::panel::PanelState::load(&h.ctx,dock).unwrap();
 assert!(restored.size().x<=560.1,"oversized persisted inspector: {:?}",restored.size());
 h.render().unwrap().save(output.join("inspector-restored-width-1440.png")).unwrap();
 // Returning data and repeated paints must never override a user's chosen width.
 // In particular, shrinking from the limit must not creep back on every frame.
 for label in ["GitHub", "파일", "변경"] {
  h.get_by_label(label).click();h.run_steps(2);
  for chosen in [320.0_f32,390.0] {
   h.ctx.data_mut(|d|d.insert_persisted(dock,egui::containers::panel::PanelState{outer_rect:egui::Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(chosen,800.))}));
   for frame in 0..24 {
    h.step();
    let width=egui::containers::panel::PanelState::load(&h.ctx,dock).unwrap().size().x;
    assert!((width-chosen).abs()<0.5,"{label} width drift at frame {frame}: user chose {chosen}, rendered {width}");
   }
  }
 }


 // Exercise the real drag handle, not only restored size state.
 let before_drag=egui::containers::panel::PanelState::load(&h.ctx,dock).unwrap().outer_rect;
 let grab=egui::pos2(before_drag.left(),before_drag.center().y);
 h.event(egui::Event::PointerMoved(grab));h.step();
 h.event(egui::Event::PointerButton{pos:grab,button:egui::PointerButton::Primary,pressed:true,modifiers:egui::Modifiers::NONE});h.step();
 let end=grab+egui::vec2(60.0,0.0);
 h.event(egui::Event::PointerMoved(end));h.step();
 h.event(egui::Event::PointerButton{pos:end,button:egui::PointerButton::Primary,pressed:false,modifiers:egui::Modifiers::NONE});h.run_steps(2);
 let dragged=egui::containers::panel::PanelState::load(&h.ctx,dock).unwrap().size().x;
 assert!(dragged<before_drag.width()-40.0,"drag must actually narrow the inspector: {} -> {dragged}",before_drag.width());
 for _ in 0..30 {h.step();let width=egui::containers::panel::PanelState::load(&h.ctx,dock).unwrap().size().x;assert!((width-dragged).abs()<0.5,"dragged width grew: {dragged} -> {width}");}
 h.get_by_label("작업 공간 살펴보기").click();h.run_steps(3);assert!(!h.state().debug_sheet_open());
 h.get_by_label("작업 공간 살펴보기").click();h.run_steps(4);
 assert!(h.query_by_label_contains("app.ts").is_some(),"reopening inspection must return to changes");
 h.query_all_by_label("product").next().unwrap().click();h.run_steps(3);
 h.query_all_by_label("personal").next().unwrap().click_secondary();h.run_steps(3);
 h.get_by_label("소스 제어").click();h.run_steps(4);
 assert_eq!(h.state().debug_active_workspace_root(),parent.canonicalize().unwrap());
 assert!(h.state().debug_sheet_open(),"explicit workspace menu must open, never toggle closed");
 assert!(h.query_by_label_contains("app.ts").is_some());
 h.get_by_label("작업 공간 살펴보기").click();h.run_steps(3);
 // Identical shells stay distinguishable without numbering unrelated tabs.
 for _ in 0..2 {h.state_mut().debug_queue_action(Action::NewPage);h.run_steps(3);}
 pump(&mut h,|h|h.query_by_label("personal · 터미널 · 3").is_some());
 for n in 1..=3 {h.get_by_label(&format!("personal · 터미널 · {n}"));}
 pump(&mut h,|h|h.state().debug_focused_text().is_some_and(|s|s.contains('>')));
 h.render().unwrap().save(output.join("workspace-duplicate-tabs.png")).unwrap();
 for n in [2,1] {h.state_mut().debug_queue_action(Action::ClosePage(n,true));h.run_steps(3);}
 h.get_by_label("새 작업").click();h.run_steps(2);h.get_by_label("에이전트 요청").click();h.run_steps(4);assert!(h.state().debug_sheet_open());assert_eq!(h.state().debug_active_workspace_root(),parent.canonicalize().unwrap());
 h.render().unwrap().save(output.join("agent-task-1440.png")).unwrap();
 h.state_mut().debug_queue_action(Action::CloseSheet);h.run_steps(3);
 for (w,height,scale,name) in [(720.,440.,1.3,"workspace-minimum-130.png"),(2560.,1440.,1.,"workspace-2560.png")]{
  h.input_mut().viewports.get_mut(&egui::ViewportId::ROOT).unwrap().native_pixels_per_point=Some(1.0);
  h.ctx.set_zoom_factor(scale);h.run_steps(3);h.set_size(egui::vec2(w/scale,height/scale));h.run_steps(4);
  let size=h.ctx.content_rect().size();assert!((size.x-w/scale).abs()<1.0 && (size.y-height/scale).abs()<1.0,"expected {}x{} got {size:?}",w/scale,height/scale);
  for label in ["작업 공간 살펴보기","새 작업","설정 (⌘,)","알림 센터"] {assert!(h.ctx.content_rect().contains_rect(h.get_by_label(label).rect()),"{label} outside viewport");}
  assert!(h.query_by_label("작업 공간 메뉴").is_none());
  let img=h.render().unwrap();assert_eq!(img.dimensions(),(w as u32,height as u32));img.save(output.join(name)).unwrap();
  if scale>1.0 {
    h.get_by_label("작업 공간 살펴보기").click();h.run_steps(4);
    h.get_by_label("파일").click();h.run_steps(3);
    for label in ["파일","변경","GitHub","작업으로 돌아가기 (Esc)"] {assert!(h.ctx.content_rect().contains_rect(h.get_by_label(label).rect()));}
    h.render().unwrap().save(output.join("inspector-minimum-130.png")).unwrap();
    h.get_by_label("작업으로 돌아가기 (Esc)").click();h.run_steps(3);assert!(!h.state().debug_sheet_open());
    h.state_mut().debug_queue_action(Action::NewAgentTask);h.run_steps(4);h.get_by_label("작업 시작");let img=h.render().unwrap();assert_eq!(img.dimensions(),(w as u32,height as u32));img.save(output.join("agent-task-minimum-130.png")).unwrap();
    h.state_mut().debug_queue_action(Action::CloseSheet);h.run_steps(3);
    h.state_mut().debug_queue_action(Action::ToggleSidebar);h.run_steps(4);for label in ["작업 공간 살펴보기","새 작업","알림 센터"]{assert!(h.ctx.content_rect().contains_rect(h.get_by_label(label).rect()));}h.render().unwrap().save(output.join("workspace-collapsed-minimum-130.png")).unwrap();h.state_mut().debug_queue_action(Action::ToggleSidebar);h.run_steps(3);
  }
 }
 let ctx=h.ctx.clone();h.state_mut().debug_set_theme(&ctx,"kiln-light");h.run_steps(4);
 h.render().unwrap().save(output.join("workspace-light-2560.png")).unwrap();
 h.input_mut().viewports.get_mut(&egui::ViewportId::ROOT).unwrap().fullscreen=Some(true);h.run_steps(4);
 assert!(h.get_by_label("작업 공간 사이드바 (⌘B)").rect().left()<20.0);
 h.render().unwrap().save(output.join("workspace-fullscreen-light.png")).unwrap();
}
