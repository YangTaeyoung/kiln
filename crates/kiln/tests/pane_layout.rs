//! Pointer gestures on the actual renderer and an isolated four-PTY workspace.
#![cfg(unix)]
use egui::{Modifiers, PointerButton, Pos2, Rect, pos2};
use egui_kittest::Harness;
use kiln::app::{Action, KilnApp};
use std::{
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
fn pointer(h: &mut Harness<'_, KilnApp>, point: Pos2, pressed: Option<bool>, modifiers: Modifiers) {
    h.event(egui::Event::ModifiersChanged(modifiers));
    h.event(egui::Event::PointerMoved(point));
    if let Some(pressed) = pressed {
        h.event(egui::Event::PointerButton {
            pos: point,
            button: PointerButton::Primary,
            pressed,
            modifiers,
        });
    }
    h.step();
}
fn wait(h: &mut Harness<'_, KilnApp>, panes: usize) {
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        h.step();
        if h.state().debug_pane_sessions().len() == panes
            && h.state().debug_pane_rects().len() == panes
        {
            return;
        }
        assert!(Instant::now() < end, "PTYs not ready");
        std::thread::sleep(Duration::from_millis(25));
    }
}
fn sorted_sessions(h: &Harness<'_, KilnApp>) -> Vec<(u64, u64)> {
    let mut ids = h.state().debug_pane_sessions();
    ids.sort();
    ids
}
fn rect(h: &Harness<'_, KilnApp>, id: u64) -> Rect {
    h.state()
        .debug_pane_rects()
        .into_iter()
        .find(|p| p.0 == id)
        .unwrap()
        .1
}
fn screenshot(h: &mut Harness<'_, KilnApp>, label: &str) {
    h.render()
        .unwrap()
        .save(format!("/tmp/kiln-pane-layout-native-{label}.png"))
        .unwrap();
}

#[test]
fn header_drop_resize_snap_and_escape_preserve_all_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    let project = base.join("workspace");
    std::fs::create_dir(&project).unwrap();
    let _cleanup = Cleanup(base.to_owned());
    unsafe {
        for (key, path) in [
            ("KILN_SOCKET", base.join("socket")),
            ("KILN_CONFIG_DIR", base.join("cfg")),
            ("KILN_ACCOUNTS_SANDBOX", base.join("accounts")),
        ] {
            std::env::set_var(key, path);
        }
        std::env::set_var("KILN_EXE", env!("CARGO_BIN_EXE_kiln"));
        std::env::set_var("KILN_NO_AUTO_UPGRADE", "1");
        std::env::set_var("KILN_DB_NO_KEYCHAIN", "1");
        std::env::set_var("KILN_REMOTE_NO_KEYCHAIN", "1");
        std::env::set_var("SHELL", "/bin/sh");
    }
    let shell = base.join("review-shell");
    std::fs::write(&shell,"#!/bin/sh\nPS1='workspace> ';export PS1\nENV=/dev/null;BASH_ENV=/dev/null;export ENV BASH_ENV\nexec /bin/sh -i\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut h = Harness::builder()
        .with_step_dt(1.0 / 60.0)
        .with_size([1280., 900.])
        .build_eframe(|cc| {
            let mut app = KilnApp::new(&cc.egui_ctx, Some(project));
            app.debug_set_shell(shell.to_string_lossy().into_owned());
            app
        });
    let ctx = h.ctx.clone();
    wait(&mut h, 1);
    let first = h.state().debug_focused_pane_id().unwrap();
    h.state_mut().debug_split(&ctx, false);
    wait(&mut h, 2);
    h.state_mut().debug_split(&ctx, true);
    wait(&mut h, 3);
    // Three panes have a T junction. Its arm toward the unsplit pane has
    // no physical bar, yet must select both axes from the shared center.
    let three = h.state().debug_pane_rects();
    let full = rect(&h, first);
    let (top_id, top) = three
        .iter()
        .filter(|(_, r)| r.left() > full.right())
        .min_by(|a, b| a.1.top().total_cmp(&b.1.top()))
        .copied()
        .unwrap();
    let half_gap = (top.left() - full.right()) * 0.5;
    let t_center = pos2(full.right() + half_gap, top.bottom() + half_gap);
    let phantom = t_center - egui::vec2(16., 0.);
    pointer(&mut h, phantom, None, Modifiers::NONE);
    screenshot(&mut h, "hover-t-junction-dark");
    pointer(&mut h, phantom, Some(true), Modifiers::NONE);
    pointer(&mut h, phantom + egui::vec2(40., 0.), None, Modifiers::NONE);
    h.run_steps(2);
    assert!(
        (rect(&h, first).right() - full.right() - 40.).abs() < 1.,
        "T arm chooses vertical boundary from first x movement"
    );
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    pointer(&mut h, phantom, Some(false), Modifiers::NONE);
    h.run_steps(2);
    assert_eq!(h.state().debug_pane_rects(), three);
    pointer(&mut h, phantom, Some(true), Modifiers::NONE);
    pointer(&mut h, phantom + egui::vec2(0., 40.), None, Modifiers::NONE);
    h.run_steps(2);
    assert!(
        (rect(&h, top_id).bottom() - top.bottom() - 40.).abs() < 1.,
        "T arm chooses horizontal boundary from first y movement"
    );
    assert_eq!(
        rect(&h, first),
        full,
        "T resize leaves unsplit neighbor unchanged"
    );
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    pointer(&mut h, phantom, Some(false), Modifiers::NONE);
    h.run_steps(2);
    assert_eq!(h.state().debug_pane_rects(), three);
    h.state_mut()
        .debug_apply_action(&ctx, Action::FocusPane(first));
    h.state_mut().debug_split(&ctx, true);
    wait(&mut h, 4);
    h.run_steps(3);
    let identities = sorted_sessions(&h);
    let client =
        kiln_daemon::client::Client::connect(&base.join("socket").to_string_lossy(), None).unwrap();
    let before = client
        .request(
            |req| kiln_proto::ClientMsg::ListSessions { req },
            Duration::from_secs(3),
        )
        .unwrap();
    let initial = h.state().debug_pane_rects();
    let board = initial.iter().fold(Rect::NOTHING, |r, (_, p)| r.union(*p));
    pointer(
        &mut h,
        board.left_top() - egui::vec2(20., 20.),
        None,
        Modifiers::NONE,
    );
    screenshot(&mut h, "rest-dark");
    let left = rect(&h, first);
    let half_gap = initial
        .iter()
        .filter(|(_, r)| r.left() == left.left() && r.top() > left.bottom())
        .map(|(_, r)| (r.top() - left.bottom()) * 0.5)
        .next()
        .unwrap();
    let divider = pos2(left.center().x, left.bottom() + half_gap);
    pointer(&mut h, divider, None, Modifiers::NONE);
    screenshot(&mut h, "hover-local-dark");
    let peer = initial
        .iter()
        .find(|(_, r)| r.left() > left.right() && r.top() == left.top())
        .unwrap()
        .0;
    let peer_before = rect(&h, peer);
    pointer(&mut h, divider, Some(true), Modifiers::NONE);
    pointer(&mut h, divider + egui::vec2(0., 40.), None, Modifiers::NONE);
    h.run_steps(2);
    assert!(
        (rect(&h, first).bottom() - left.bottom() - 40.).abs() < 1.0,
        "far resize changes only this segment"
    );
    assert_eq!(
        rect(&h, peer),
        peer_before,
        "far resize leaves aligned neighbor untouched"
    );
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    pointer(&mut h, divider, Some(false), Modifiers::NONE);
    h.run_steps(2);
    let near = pos2(left.right() - 40.0, divider.y);
    pointer(&mut h, near, None, Modifiers::NONE);
    screenshot(&mut h, "hover-linked-dark");
    pointer(&mut h, near, Some(true), Modifiers::NONE);
    let quarter = board.top() + board.height() * 0.25;
    pointer(&mut h, pos2(near.x, quarter + 8.0), None, Modifiers::NONE);
    h.run_steps(2);
    screenshot(&mut h, "snapped-dark");
    assert!((rect(&h, first).bottom() + half_gap - quarter).abs() < 1.0);
    assert!(
        (rect(&h, peer).bottom() + half_gap - quarter).abs() < 1.0,
        "near junction links the full aligned boundary"
    );
    pointer(&mut h, pos2(near.x, quarter + 24.0), None, Modifiers::NONE);
    h.run_steps(2);
    assert!(
        (rect(&h, first).bottom() + half_gap - quarter).abs() < 1.0,
        "snap remains locked until 30 points"
    );
    pointer(&mut h, pos2(near.x, quarter + 31.0), None, Modifiers::NONE);
    h.run_steps(2);
    assert!(
        (rect(&h, first).bottom() + half_gap - quarter).abs() > 20.0,
        "snap releases beyond 30 points"
    );
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    pointer(&mut h, pos2(near.x, quarter), Some(false), Modifiers::NONE);
    h.run_steps(3);
    assert_eq!(
        h.state().debug_pane_rects(),
        initial,
        "Escape restores exact original geometry"
    );
    pointer(&mut h, near, Some(true), Modifiers::ALT);
    pointer(&mut h, pos2(near.x, quarter + 8.), None, Modifiers::ALT);
    h.run_steps(2);
    assert_eq!(
        rect(&h, peer),
        peer_before,
        "Alt isolates the hovered segment"
    );
    assert!(
        (rect(&h, first).bottom() + half_gap - quarter - 8.0).abs() < 1.0,
        "Alt bypasses fraction snap"
    );
    h.state_mut().debug_apply_action(&ctx, Action::OpenSettings);
    h.run_steps(2);
    assert_eq!(
        h.state().debug_pane_rects(),
        initial,
        "opening settings cancels and restores active resize"
    );
    pointer(&mut h, near, Some(false), Modifiers::NONE);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    // Junction keyboard movement uses the entire aligned axis, including when
    // no hover movement follows the key. Ordinary separators remain local.
    let junction = pos2(left.right() + half_gap, divider.y);
    pointer(&mut h, junction, Some(true), Modifiers::NONE);
    pointer(&mut h, junction, Some(false), Modifiers::NONE);
    h.run_steps(2);
    h.event(egui::Event::PointerGone);
    h.step();
    h.key_press(egui::Key::ArrowDown);
    h.run_steps(3);
    assert!((rect(&h, first).bottom() - left.bottom() - 10.).abs() < 1.);
    assert!((rect(&h, peer).bottom() - peer_before.bottom() - 10.).abs() < 1.);
    h.key_press(egui::Key::ArrowUp);
    h.run_steps(3);
    assert_eq!(h.state().debug_pane_rects(), initial);
    h.state_mut()
        .debug_apply_action(&ctx, Action::FocusPane(first));
    h.state_mut()
        .debug_apply_action(&ctx, Action::SetTheme("kiln-light".into()));
    h.run_steps(3);
    pointer(
        &mut h,
        board.left_top() - egui::vec2(20., 20.),
        None,
        Modifiers::NONE,
    );
    screenshot(&mut h, "rest-light");
    pointer(&mut h, divider, None, Modifiers::NONE);
    screenshot(&mut h, "hover-local-light");
    pointer(&mut h, near, None, Modifiers::NONE);
    screenshot(&mut h, "hover-linked-light");
    pointer(&mut h, near, Some(true), Modifiers::NONE);
    pointer(&mut h, pos2(near.x, quarter + 8.), None, Modifiers::NONE);
    h.run_steps(2);
    screenshot(&mut h, "snapped-light");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    pointer(&mut h, near, Some(false), Modifiers::NONE);
    h.run_steps(2);
    assert_eq!(h.state().debug_pane_rects(), initial);
    h.state_mut()
        .debug_apply_action(&ctx, Action::SetTheme("kiln-dark".into()));
    h.run_steps(3);
    // Preserve the established double-click behavior after a genuinely unequal
    // local resize; it resets only that segment's own parent split.
    pointer(&mut h, divider, Some(true), Modifiers::NONE);
    pointer(&mut h, divider + egui::vec2(0., 40.), None, Modifiers::NONE);
    pointer(
        &mut h,
        divider + egui::vec2(0., 40.),
        Some(false),
        Modifiers::NONE,
    );
    h.run_steps(2);
    let unequal = pos2(divider.x, rect(&h, first).bottom() + half_gap);
    for _ in 0..2 {
        pointer(&mut h, unequal, Some(true), Modifiers::NONE);
        pointer(&mut h, unequal, Some(false), Modifiers::NONE);
    }
    h.run_steps(3);
    assert!(
        (rect(&h, first).bottom() - left.bottom()).abs() < 1.,
        "double click equalizes the local parent: original={left:?}, actual={:?}, click={unequal:?}",
        rect(&h, first)
    );
    assert_eq!(
        rect(&h, peer),
        peer_before,
        "double click cannot change its neighbor"
    );
    let source = identities[1].0;
    let target = identities[3].0;
    let start = rect(&h, source).left_top() + egui::vec2(80., 15.);
    let destination = rect(&h, target).left_center() + egui::vec2(30., 0.);
    pointer(&mut h, start, Some(true), Modifiers::NONE);
    pointer(&mut h, start + egui::vec2(12., 0.), None, Modifiers::NONE);
    pointer(&mut h, destination, None, Modifiers::NONE);
    h.run_steps(2);
    screenshot(&mut h, "drop-preview-dark");
    pointer(&mut h, destination, Some(false), Modifiers::NONE);
    h.run_steps(3);
    assert_ne!(
        h.state()
            .debug_pane_rects()
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>(),
        initial
            .iter()
            .copied()
            .collect::<std::collections::BTreeMap<_, _>>(),
        "dropping changes placement"
    );
    assert_eq!(
        sorted_sessions(&h),
        identities,
        "moving and resizing never creates/kills/replaces a PTY"
    );
    assert_eq!(h.state().debug_focused_pane_id(), Some(source));
    h.state_mut().debug_checkpoint_restore(&ctx);
    h.run_steps(3);
    assert_eq!(
        sorted_sessions(&h),
        identities,
        "layout and identity survive persistence"
    );
    let after = client
        .request(
            |req| kiln_proto::ClientMsg::ListSessions { req },
            Duration::from_secs(3),
        )
        .unwrap();
    let pids = |message: kiln_proto::ServerMsg| match message {
        kiln_proto::ServerMsg::Sessions { sessions, .. } => {
            let mut ids = sessions
                .into_iter()
                .map(|s| (s.id, s.pid))
                .collect::<Vec<_>>();
            ids.sort();
            ids
        }
        _ => panic!("session response"),
    };
    assert_eq!(
        pids(before),
        pids(after),
        "same daemon session IDs and operating-system PIDs"
    );
    h.state_mut()
        .debug_apply_action(&ctx, Action::SetTheme("kiln-light".into()));
    h.run_steps(3);
    screenshot(&mut h, "rest-light");
    let light = h.state().debug_pane_rects();
    let (lid, lr) = light[0];
    let other = light.iter().find(|(id, _)| *id != lid).unwrap();
    let ls = lr.left_top() + egui::vec2(80., 15.);
    let ld = other.1.right_center() - egui::vec2(30., 0.);
    pointer(&mut h, ls, Some(true), Modifiers::NONE);
    pointer(&mut h, ls + egui::vec2(12., 0.), None, Modifiers::NONE);
    pointer(&mut h, ld, None, Modifiers::NONE);
    h.run_steps(2);
    screenshot(&mut h, "drop-preview-light");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    pointer(&mut h, ld, Some(false), Modifiers::NONE);
    h.run_steps(2);
    assert_eq!(
        h.state().debug_pane_rects(),
        light,
        "Escape cancels a pane move without changing the tree"
    );
    assert_eq!(sorted_sessions(&h), identities);
    // Window size changes cancel the gesture and retain all existing PTYs;
    // narrow-window and enlarged-font render checks exercise real layout.
    h.set_size(egui::vec2(720., 640.));
    h.run_steps(4);
    screenshot(&mut h, "rest-light-720");
    assert_eq!(sorted_sessions(&h), identities);
    let narrow = h.state().debug_pane_rects();
    let (small_id, small_rect) = narrow[0];
    let small_target = narrow.iter().find(|(id, _)| *id != small_id).unwrap();
    let ns = small_rect.left_top() + egui::vec2(80., 15.);
    let nd = small_target.1.left_center() + egui::vec2(3., 0.);
    pointer(&mut h, ns, Some(true), Modifiers::NONE);
    pointer(&mut h, ns + egui::vec2(12., 0.), None, Modifiers::NONE);
    pointer(&mut h, nd, None, Modifiers::NONE);
    h.run_steps(2);
    screenshot(&mut h, "drop-light-720");
    h.set_size(egui::vec2(1280., 900.));
    h.run_steps(4);
    pointer(&mut h, nd, Some(false), Modifiers::NONE);
    h.run_steps(2);
    assert_eq!(
        sorted_sessions(&h),
        identities,
        "viewport cancellation cannot replace terminals"
    );
    h.ctx.set_zoom_factor(1.3);
    h.run_steps(4);
    screenshot(&mut h, "rest-light-130");
    assert_eq!(sorted_sessions(&h), identities);
}
