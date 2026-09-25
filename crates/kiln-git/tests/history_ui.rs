//! 이력(Log) 화면 kittest: 다중 선택·컨텍스트 메뉴 스쿼시, 끌어다 놓기 표시와 순서 이동,
//! 대화형 리베이스 대화상자, 충돌 배너, 다크 스냅샷.

mod common;
mod history_common;

use egui::{Modifiers, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use history_common::{State, harness, row, settle, subjects, ui_repo};
use kiln_git::history::{DropPlace, HistoryEvent};

#[test]
fn history_view_renders_table_and_detail() {
    let r = ui_repo();
    let mut h = harness(r.path.clone(), vec2(1180.0, 520.0));
    settle(&mut h);
    let n = h.state().view.commits().len();
    assert_eq!(n, 9);
    // 첫 커밋이 자동 선택되고 상세가 로드된다.
    let head = r.git(&["rev-parse", "HEAD"]).trim().to_string();
    assert_eq!(h.state().view.selection(), std::slice::from_ref(&head));
    h.snapshot("history_view");

    // 파일 행 클릭 → 부모와 비교
    h.get_by_label("src/detail.rs").click();
    h.run_steps(2);
    let parent = r.git(&["rev-parse", "HEAD~1"]).trim().to_string();
    assert!(h.state().events.contains(&HistoryEvent::OpenDiff { from: parent, to: Some(head.clone()) }));

    // Enter → OpenCommit
    h.state_mut().view.select(&head);
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(1);
    assert!(h.state().events.contains(&HistoryEvent::OpenCommit(head.clone())));

    // 키보드로 이동
    h.state_mut().view.select(&head);
    h.run_steps(1);
    h.key_press(egui::Key::ArrowDown);
    h.run_steps(1);
    let second = r.git(&["rev-parse", "HEAD~1"]).trim().to_string();
    assert_eq!(h.state().view.selection(), [second]);
}

#[test]
fn filters_narrow_the_log() {
    let r = ui_repo();
    let mut h = harness(r.path.clone(), vec2(1180.0, 480.0));
    settle(&mut h);
    let mut q = h.state().view.query().clone();
    q.author = "mina".into();
    h.state_mut().view.set_query(q);
    settle(&mut h);
    let subs: Vec<String> = h.state().view.commits().iter().map(|c| c.subject.clone()).collect();
    assert_eq!(subs, ["Support hunk staging", "Add diff parser"]);

    let mut q = h.state().view.query().clone();
    q.author.clear();
    q.text = "graph".into();
    h.state_mut().view.set_query(q);
    settle(&mut h);
    assert_eq!(h.state().view.commits().len(), 1);
    h.snapshot("history_filtered");

    let mut q = h.state().view.query().clone();
    q.text.clear();
    q.first_parent = true;
    q.branch = kiln_git::history::BranchFilter::Current;
    h.state_mut().view.set_query(q);
    settle(&mut h);
    assert_eq!(h.state().view.commits().len(), 7);
}

#[test]
fn multi_select_context_menu_squash_then_undo() {
    let r = ui_repo();
    let old_head = r.git(&["rev-parse", "HEAD"]).trim().to_string();
    let mut h = harness(r.path.clone(), vec2(1180.0, 560.0));
    settle(&mut h);

    row(&h, "Tweak graph colors").click();
    h.run_steps(2);
    row(&h, "Polish detail pane").click_modifiers(Modifiers::SHIFT);
    h.run_steps(2);
    assert_eq!(h.state().view.selection().len(), 3);
    // Cmd/Ctrl 클릭은 토글한다.
    row(&h, "Add log view").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    assert_eq!(h.state().view.selection().len(), 4);
    row(&h, "Add log view").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    assert_eq!(h.state().view.selection().len(), 3);

    row(&h, "Fix date column").click_secondary();
    h.run_steps(3);
    h.snapshot("history_context_menu");
    h.get_by_label("하나로 스쿼시…").click();
    settle(&mut h);
    let msg = h.state_mut().view.dialog_message_mut().expect("message dialog").clone();
    assert!(msg.starts_with("Tweak graph colors\n\nFix date column\n\nPolish detail pane"), "{msg}");
    h.run_steps(2);
    h.snapshot("history_squash_dialog");
    *h.state_mut().view.dialog_message_mut().unwrap() = "Graph, date and detail polish".into();
    h.run_steps(1);
    h.get_by_label("스쿼시").click();
    settle(&mut h);
    assert_eq!(&subjects(&r)[..2], ["Graph, date and detail polish", "Add log view"]);
    assert_eq!(h.state().view.notice(), Some("스쿼시 완료"));
    assert!(h.state().events.contains(&HistoryEvent::Toast("스쿼시 완료".into())));

    h.get_by_label("되돌리기").click();
    settle(&mut h);
    assert_eq!(r.git(&["rev-parse", "HEAD"]).trim(), old_head);
}

#[test]
fn drag_shows_target_and_moves_commit() {
    let r = ui_repo();
    let mut h = harness(r.path.clone(), vec2(1180.0, 520.0));
    settle(&mut h);

    let from = row(&h, "Polish detail pane").rect();
    let to = row(&h, "Tweak graph colors").rect();
    h.hover_at(from.center());
    h.run_steps(1);
    h.drag_at(from.center());
    h.run_steps(1);
    // 대상 행 아래쪽 절반으로.
    let mid = egui::pos2(from.center().x, (from.center().y + to.center().y) / 2.0);
    h.hover_at(mid);
    h.run_steps(1);
    let target = egui::pos2(to.center().x, to.bottom() - 4.0);
    h.hover_at(target);
    h.run_steps(2);
    let tweak = r.git(&["rev-parse", "HEAD~2"]).trim().to_string();
    assert_eq!(h.state().view.drag_hint(), Some((tweak.clone(), DropPlace::Below)));
    h.snapshot("history_drag");

    // 위쪽 절반이면 위에 끼운다.
    h.hover_at(egui::pos2(to.center().x, to.top() + 4.0));
    h.run_steps(2);
    assert_eq!(h.state().view.drag_hint(), Some((tweak.clone(), DropPlace::Above)));
    h.hover_at(target);
    h.run_steps(2);
    h.drop_at(target);
    h.run_steps(3);
    h.snapshot("history_drop_menu");
    h.get_by_label("여기로 순서 이동").click();
    settle(&mut h);
    assert_eq!(&subjects(&r)[..4], ["Fix date column", "Tweak graph colors", "Polish detail pane", "Add log view"]);
}

#[test]
fn drag_onto_commit_offers_fixup() {
    let r = ui_repo();
    let mut h = harness(r.path.clone(), vec2(1180.0, 520.0));
    settle(&mut h);
    let from = row(&h, "Fix date column").rect();
    let to = row(&h, "Add log view").rect();
    h.hover_at(from.center());
    h.run_steps(1);
    h.drag_at(from.center());
    h.run_steps(1);
    for k in 1..=4 {
        let y = from.center().y + (to.center().y - from.center().y) * k as f32 / 4.0;
        h.hover_at(egui::pos2(from.center().x, y));
        h.run_steps(1);
    }
    h.drop_at(to.center());
    h.run_steps(3);
    h.get_by_label_contains("이 커밋에 스쿼시(fixup)").click();
    settle(&mut h);
    assert_eq!(&subjects(&r)[..3], ["Polish detail pane", "Tweak graph colors", "Add log view"]);
    let files = r.git(&["show", "--name-only", "--format=", "HEAD~2"]);
    assert!(files.contains("src/date.rs") && files.contains("src/log.rs"), "{files}");
}

#[test]
fn interactive_rebase_dialog_reorder_and_start() {
    let r = ui_repo();
    let mut h = harness(r.path.clone(), vec2(1180.0, 720.0));
    settle(&mut h);

    row(&h, "Add log view").click_secondary();
    h.run_steps(3);
    h.get_by_label("이 커밋부터 대화형 리베이스…").click();
    settle(&mut h);
    h.run_steps(2);
    assert!(h.query_by_label("리베이스: Add log view").is_some());

    // "Add log view"(맨 위)를 "Fix date column" 아래로 끌어 옮긴다.
    let handle = h.get_by_label("순서 변경: Add log view").rect();
    let dest = h.get_by_label("리베이스: Polish detail pane").rect();
    h.hover_at(handle.center());
    h.run_steps(1);
    h.drag_at(handle.center());
    h.run_steps(1);
    for k in 1..=5 {
        let y = handle.center().y + (dest.top() + 6.0 - handle.center().y) * k as f32 / 5.0;
        h.hover_at(egui::pos2(handle.center().x, y));
        h.run_steps(1);
    }
    h.snapshot("history_rebase_drag");
    h.drop_at(egui::pos2(handle.center().x, dest.top() + 6.0));
    h.run_steps(2);

    // "Tweak graph colors" 를 삭제로(단축키 D).
    h.get_by_label("리베이스: Tweak graph colors").click();
    h.run_steps(1);
    h.key_press(egui::Key::D);
    h.run_steps(2);
    // "Polish detail pane" 는 드롭다운으로 픽스업.
    h.get_by_label("동작: Polish detail pane").click();
    h.run_steps(2);
    h.get_by_label("픽스업").click();
    h.run_steps(2);
    h.snapshot("history_rebase_dialog");

    h.get_by_label("리베이스 시작").click();
    settle(&mut h);
    assert_eq!(&subjects(&r)[..3], ["Add log view", "Fix date column", "Merge branch 'feature/diff-view'"]);
    let files = r.git(&["show", "--name-only", "--format=", "HEAD"]);
    assert!(files.contains("src/log_view.rs") && files.contains("src/detail.rs"), "{files}");
    assert!(!r.path.join("src/graph.rs").exists());
    // 푸시되지 않은 커밋만 다시 썼으므로 강제 푸시 제안이 없다.
    assert!(h.query_by_label("강제 푸시(--force-with-lease)").is_none());
}

#[test]
fn rewriting_pushed_commit_warns_and_offers_force_push() {
    let r = ui_repo();
    r.git(&["push", "-q"]);
    let mut h = harness(r.path.clone(), vec2(1180.0, 560.0));
    settle(&mut h);
    row(&h, "Fix date column").click_secondary();
    h.run_steps(3);
    h.get_by_label("커밋 삭제").click();
    h.run_steps(3);
    assert!(h.query_all_by_label_contains("이미 푸시된 커밋").next().is_some());
    settle(&mut h);
    h.snapshot("history_drop_pushed_confirm");
    h.get_by_label("삭제").click();
    settle(&mut h);
    assert!(!subjects(&r).contains(&"Fix date column".to_string()));
    h.get_by_label("강제 푸시(--force-with-lease)").click();
    settle(&mut h);
    assert_eq!(r.git(&["rev-parse", "origin/main"]).trim(), r.git(&["rev-parse", "HEAD"]).trim());
}

#[test]
fn reset_dialog_and_conflict_banner() {
    let r = ui_repo();
    let mut h = harness(r.path.clone(), vec2(1180.0, 620.0));
    settle(&mut h);
    row(&h, "Add log view").click_secondary();
    h.run_steps(3);
    h.get_by_label("현재 브랜치를 여기로 리셋…").click();
    h.run_steps(2);
    h.get_by_label("Hard").click();
    h.run_steps(2);
    assert!(h.query_all_by_label_contains("데이터가 사라질 수 있습니다").next().is_some());
    settle(&mut h);
    h.snapshot("history_reset_hard");
    h.get_by_label("Mixed").click();
    h.run_steps(1);
    h.get_by_label("Mixed 리셋").click();
    settle(&mut h);
    assert_eq!(subjects(&r)[0], "Add log view");
    r.git(&["reset", "-q", "--hard", "ORIG_HEAD"]);
    r.git(&["clean", "-qfd"]);

    // 같은 줄을 고치는 커밋을 옆 브랜치에서 체리픽해 충돌을 만든다.
    r.git(&["switch", "-q", "-c", "side", "HEAD~2"]);
    r.write("src/main.rs", &common::numbered(30).replace("line 1\n", "side one\n"));
    r.commit_all("Side edit of main");
    r.git(&["switch", "-q", "main"]);
    r.write("src/main.rs", &common::numbered(32).replace("line 1\n", "main one\n"));
    r.commit_all("Main edit of main");
    h.state_mut().view.refresh();
    settle(&mut h);
    row(&h, "Side edit of main").click_secondary();
    h.run_steps(3);
    h.get_by_label("체리픽").click();
    settle(&mut h);
    assert!(h.query_all_by_label_contains("충돌 1개").next().is_some());
    settle(&mut h);
    h.snapshot("history_conflict");
    h.get_all_by_label("src/main.rs").next().unwrap().click();
    h.run_steps(2);
    assert!(h.state().events.contains(&HistoryEvent::OpenFile(r.path.join("src/main.rs"))));
    h.get_by_label("중단").click();
    settle(&mut h);
    assert!(h.state().view.repo_state().is_some_and(|s| s.op.is_none()));
    assert_eq!(subjects(&r)[0], "Main edit of main");
}

#[test]
fn branch_and_tag_dialogs() {
    let r = ui_repo();
    let mut h = harness(r.path.clone(), vec2(1180.0, 520.0));
    settle(&mut h);
    row(&h, "Add log view").click_secondary();
    h.run_steps(3);
    h.get_by_label("여기서 브랜치 만들기…").click();
    h.run_steps(3);
    focused_input(&h).type_text("topic/log");
    h.run_steps(1);
    h.get_by_label("만든 뒤 체크아웃").click();
    h.run_steps(1);
    settle(&mut h);
    h.snapshot("history_new_branch");
    h.get_by_label("브랜치 만들기").click();
    settle(&mut h);
    assert_eq!(r.git(&["rev-parse", "topic/log"]).trim(), r.git(&["rev-parse", "HEAD~3"]).trim());
    assert_eq!(r.git(&["branch", "--show-current"]).trim(), "main");

    row(&h, "Fix date column").click_secondary();
    h.run_steps(3);
    h.get_by_label("태그 만들기…").click();
    h.run_steps(3);
    focused_input(&h).type_text("v0.3.0");
    h.run_steps(1);
    h.get_by_label("태그 만들기").click();
    settle(&mut h);
    assert_eq!(r.git(&["rev-parse", "v0.3.0"]).trim(), r.git(&["rev-parse", "HEAD~1"]).trim());
}

fn focused_input<'a>(h: &'a Harness<'_, State>) -> egui_kittest::Node<'a> {
    h.get_all_by_role(egui::accesskit::Role::TextInput).find(|n| n.is_focused()).expect("focused text input")
}
