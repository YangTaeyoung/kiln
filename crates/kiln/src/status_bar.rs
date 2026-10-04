//! A single menu-bar companion survives GUI closure while the daemon is alive.
//! AppKit status items must live on the main run loop; the daemon stays headless.
//! https://developer.apple.com/documentation/appkit/nsstatusitem
use kiln_daemon::client::Client;
use kiln_common::i18n::{self, tr};
use kiln_proto::{ClientMsg, ServerMsg, SessionInfo, SessionId};
use objc2::{
    ClassType, msg_send_id,
    rc::Retained,
    runtime::{AnyObject, Bool, ClassBuilder, Sel},
    sel,
};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSApplicationActivationPolicy,
    NSCellImagePosition, NSImage, NSMenu, NSMenuItem, NSRunningApplication, NSStatusBar,
    NSStatusItem, NSAlert, NSAlertStyle, NSAlertSecondButtonReturn,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSSize, NSString, NSTimer};
use std::{
    cell::RefCell,
    fs::OpenOptions,
    os::fd::AsRawFd,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Status {
    Connecting,
    Connected { running: usize, attention: usize },
    Disconnected,
}
impl Status {
    fn title(&self) -> String {
        let attention = matches!(self, Self::Connected {attention,..} if *attention>0);
        badge_title(attention, persisted_unread())
    }
    fn description(&self) -> String {
        match self {
            Self::Connecting => tr("Kiln에 연결하는 중…").into(),
            Self::Connected {
                running: 0,
                attention: 0,
            } => tr("Kiln 실행 중 · 활성 세션 없음").into(),
            Self::Connected {
                running: 0,
                attention,
            } => kiln_common::trf!("활성 세션 없음 · 확인 필요 {attention}개"),
            Self::Connected {
                running,
                attention: 0,
            } => kiln_common::trf!("백그라운드 세션 {running}개 실행 중"),
            Self::Connected { running, attention } => {
                kiln_common::trf!("세션 {running}개 실행 중 · 확인 필요 {attention}개")
            }
            Self::Disconnected => tr("Kiln 연결이 끊겼습니다").into(),
        }
    }
}
fn badge_title(attention: bool, unread: usize) -> String {
    if attention || unread > 0 {
        "●".into()
    } else {
        String::new()
    }
}
fn menu_title(icon_loaded: bool, badge: &str) -> String {
    match (icon_loaded, badge.is_empty()) {
        (true, _) => badge.into(),
        (false, true) => "Kiln".into(),
        (false, false) => format!("Kiln {badge}"),
    }
}
struct MenuState {
    executable: std::path::PathBuf,
    executable_identity: Option<(u64,u64)>,
    icon_loaded: bool,
    item: Retained<NSStatusItem>,
    summary: Retained<NSMenuItem>,
    stop: Retained<NSMenuItem>,
    open: Retained<NSMenuItem>,
    settings: Retained<NSMenuItem>,
    hint: Retained<NSMenuItem>,
    status: Arc<Mutex<Status>>,
}
thread_local! { static MENU:RefCell<Option<MenuState>> = const { RefCell::new(None) }; }

/// A tiny shared badge record contains only the unread count, never notification text.
pub fn publish_unread(count: usize) {
    static LAST: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(usize::MAX);
    if LAST.load(Ordering::Relaxed) == count {
        return;
    }
    let path = kiln_common::paths::config_file("menu-bar-unread");
    let temp = path.with_extension("tmp");
    if std::fs::write(&temp, count.to_string())
        .and_then(|_| std::fs::rename(&temp, &path))
        .is_ok()
    {
        LAST.store(count, Ordering::Relaxed);
    }
}
fn persisted_unread() -> usize {
    std::fs::read_to_string(kiln_common::paths::config_file("menu-bar-unread"))
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Test fixtures and alternate sockets must not litter the user's system menu.
pub fn ensure_running() {
    if std::env::var_os("KILN_NO_AUTO_UPGRADE").is_some()
        || std::env::var_os("KILN_SOCKET").is_some()
    {
        return;
    }
    if let Ok(exe) = std::env::current_exe() {
        let helper = exe
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("Library/Kiln Status.app/Contents/MacOS/kiln-status"))
            .filter(|p| p.is_file())
            .unwrap_or(exe);
        if let Ok(mut child) = Command::new(helper)
            .arg("status-bar")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}
extern "C" fn open_window(_: *mut AnyObject, _: Sel, _: *mut AnyObject) {
    if let Err(error) = show_window() { show_error(tr("Kiln을 열지 못했습니다"), &error.to_string()); }
}
extern "C" fn reopen(_: *mut AnyObject, _: Sel, _: *mut NSApplication, _: Bool) -> Bool {
    if let Err(error) = show_window() { show_error(tr("Kiln을 열지 못했습니다"), &error.to_string()); }
    Bool::YES
}
static GUI_LAUNCHING: AtomicBool = AtomicBool::new(false);
fn gui_executable() -> std::io::Result<std::path::PathBuf> {
    let exe = std::env::current_exe()?;
    if exe.file_name().is_some_and(|n| n == "kiln-status") {
        return exe
            .ancestors()
            .nth(5)
            .map(|p| p.join("MacOS/kiln"))
            .ok_or_else(|| std::io::Error::other("invalid menu-bar bundle"));
    }
    Ok(exe)
}
fn show_window() -> std::io::Result<()> {
    // Activate an existing regular GUI, excluding this accessory and daemons.
    unsafe {
        let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(
            &NSString::from_str(if cfg!(feature = "updater-test") { "dev.kiln.updater-test" } else { "dev.kiln.app" }),
        );
        for app in apps.iter() {
            if app.activationPolicy() == NSApplicationActivationPolicy::Regular {
                if app.activateWithOptions(
                    NSApplicationActivationOptions::NSApplicationActivateIgnoringOtherApps,
                ) { return Ok(()); }
                return Err(std::io::Error::other(tr("실행 중인 Kiln 창을 활성화하지 못했습니다.")));
            }
        }
    }
    if GUI_LAUNCHING.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let result = gui_executable().and_then(|exe| Command::new(exe)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn());
    match result {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
                GUI_LAUNCHING.store(false, Ordering::SeqCst);
            });
            Ok(())
        }
        Err(error) => {
            GUI_LAUNCHING.store(false, Ordering::SeqCst);
            Err(error)
        }
    }
}

extern "C" fn open_settings(_: *mut AnyObject, _: Sel, _: *mut AnyObject) {
    if let Err(error) = crate::native_actions::request_settings_and_launch(show_window) {
        show_error(tr("설정을 열지 못했습니다"), &error.to_string());
    }
}

fn show_error(title: &str, message: &str) {
    let Some(mt) = MainThreadMarker::new() else { return; };
    unsafe {
        let alert = NSAlert::new(mt);
        alert.setAlertStyle(NSAlertStyle::Warning);
        alert.setMessageText(&NSString::from_str(title));
        alert.setInformativeText(&NSString::from_str(message));
        alert.addButtonWithTitle(&NSString::from_str(tr("확인")));
        NSApplication::sharedApplication(mt).activateIgnoringOtherApps(true);
        alert.runModal();
    }
}

enum StopResult {
    Snapshot(anyhow::Result<(Client, Vec<SessionInfo>)>),
    Finished(anyhow::Result<()>),
}
thread_local! {
    static STOP_RESULT: RefCell<Option<std::sync::mpsc::Receiver<StopResult>>> = const { RefCell::new(None) };
}
static STOPPING: AtomicBool = AtomicBool::new(false);

fn list_sessions(client: &Client) -> anyhow::Result<Vec<SessionInfo>> {
    match client.request(|req| ClientMsg::ListSessions { req }, Duration::from_secs(3))? {
        ServerMsg::Sessions { sessions, .. } => Ok(sessions),
        _ => anyhow::bail!("{}", tr("세션 목록을 확인하지 못했습니다. 다시 시도하세요.")),
    }
}

extern "C" fn stop_sessions(_: *mut AnyObject, _: Sel, _: *mut AnyObject) {
    if STOPPING.swap(true, Ordering::SeqCst) { return; }
    let (tx, rx) = std::sync::mpsc::channel();
    STOP_RESULT.with(|state| *state.borrow_mut() = Some(rx));
    std::thread::spawn(move || {
        let result = (|| {
            // Connecting never starts, upgrades, or shuts down a daemon.
            let client = Client::connect(&kiln_proto::socket_name(), None)?;
            let sessions = list_sessions(&client)?.into_iter().filter(|s| s.exited.is_none()).collect();
            Ok((client, sessions))
        })();
        let _ = tx.send(StopResult::Snapshot(result));
    });
}

fn reviewed_session_ids(reviewed: &[SessionInfo], current: &[SessionInfo]) -> Vec<SessionId> {
    reviewed.iter().filter(|old| current.iter().any(|now|
        now.exited.is_none() && now.id == old.id && now.pid == old.pid && now.created_unix == old.created_unix
    )).map(|s| s.id).collect()
}

fn finish_stop(client: Client, reviewed: Vec<SessionInfo>) -> anyhow::Result<()> {
    stop_reviewed(&client, &reviewed)
}

fn stop_reviewed(client: &Client, reviewed: &[SessionInfo]) -> anyhow::Result<()> {
    // Use the original connection; never reconnect to a restarted daemon whose
    // IDs could be reused. New sessions created during confirmation are excluded.
    let ids = reviewed_session_ids(reviewed, &list_sessions(client)?);
    for &session in &ids { client.send(ClientMsg::Kill { session }); }
    let remaining = list_sessions(client)?;
    anyhow::ensure!(!remaining.iter().any(|s| ids.contains(&s.id) && s.exited.is_none()),
        "{}", tr("일부 세션의 종료를 확인하지 못했습니다. 세션 목록을 확인하세요."));
    Ok(())
}

fn poll_stop_result() {
    // Release RefCell borrows before entering AppKit's nested alert run loop.
    let result = STOP_RESULT.with(|s| s.borrow().as_ref().and_then(|rx| rx.try_recv().ok()));
    let Some(result) = result else { return; };
    STOP_RESULT.with(|s| *s.borrow_mut() = None);
    match result {
        StopResult::Snapshot(Ok((client, sessions))) if !sessions.is_empty() => {
            if confirm_stop(&sessions) {
                let (tx, rx) = std::sync::mpsc::channel();
                STOP_RESULT.with(|s| *s.borrow_mut() = Some(rx));
                std::thread::spawn(move || { let _ = tx.send(StopResult::Finished(finish_stop(client, sessions))); });
                return;
            }
        }
        StopResult::Snapshot(Ok(_)) => show_error(tr("실행 중인 세션이 없습니다"), tr("종료할 백그라운드 세션이 없습니다.")),
        StopResult::Snapshot(Err(error)) | StopResult::Finished(Err(error)) =>
            show_error(tr("세션 종료를 완료하지 못했습니다"), &error.to_string()),
        StopResult::Finished(Ok(())) => {}
    }
    STOPPING.store(false, Ordering::SeqCst);
}

fn confirm_stop(sessions: &[SessionInfo]) -> bool {
    let Some(mt) = MainThreadMarker::new() else { return false; };
    let names = sessions.iter().take(8).map(|s| {
        let label = s.name.as_deref().filter(|s| !s.is_empty()).unwrap_or(&s.title);
        let label: String = label.chars().filter(|c| !c.is_control()).take(80).collect();
        format!("• #{} {}", s.id, label)
    }).collect::<Vec<_>>().join("\n");
    let extra = if sessions.len() > 8 { kiln_common::trf!("\n외 {}개", sessions.len() - 8) } else { String::new() };
    let message = kiln_common::trf!("모든 워크스페이스의 터미널 세션 {}개를 종료합니다. 현재 창에 표시된 세션도 포함됩니다. 실행 중인 셸, 에이전트, 빌드와 서버가 중단되며 저장하지 않은 터미널 작업은 사라질 수 있습니다.\n\n{}{}\n\nKiln 창과 세션 서비스는 계속 실행됩니다.", sessions.len(), names, extra);
    unsafe {
        let alert = NSAlert::new(mt);
        alert.setAlertStyle(NSAlertStyle::Warning);
        alert.setMessageText(&NSString::from_str(&kiln_common::trf!("백그라운드 세션 {}개를 종료할까요?", sessions.len())));
        alert.setInformativeText(&NSString::from_str(&message));
        // Return cancels; the destructive action has no key equivalent.
        let cancel = alert.addButtonWithTitle(&NSString::from_str(tr("취소")));
        cancel.setKeyEquivalent(&NSString::from_str("\r"));
        let stop = alert.addButtonWithTitle(&NSString::from_str(tr("세션 종료")));
        stop.setKeyEquivalent(&NSString::new());
        NSApplication::sharedApplication(mt).activateIgnoringOtherApps(true);
        alert.runModal() == NSAlertSecondButtonReturn
    }
}
extern "C" fn tick(_: *mut AnyObject, _: Sel, _: *mut AnyObject) {
    i18n::sync_language();
    MENU.with(|cell| {
        if let Some(menu) = cell.borrow().as_ref() {
            // An atomic app update replaces the helper's inode. Replace this
            // helper in place; its CLOEXEC lock closes and the new process
            // reacquires it. Neither the session daemon nor PTYs are touched.
            if let Some(identity)=executable_identity(&menu.executable) {
                if menu.executable_identity.is_some_and(|old|old!=identity) {
                    use std::os::unix::process::CommandExt;
                    let error=Command::new(&menu.executable).arg("status-bar").exec();
                    log::error!("menu-bar update failed: {error}");
                }
            }
            let status = menu
                .status
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            if status == Status::Disconnected {
                std::process::exit(0);
            }
            unsafe {
                if let Some(mt) = MainThreadMarker::new() {
                    if let Some(button) = menu.item.button(mt) {
                        button.setTitle(&NSString::from_str(&menu_title(menu.icon_loaded, &status.title())));
                        button.setToolTip(Some(&NSString::from_str(&kiln_common::trf!(
                            "{} · 읽지 않은 알림 {}개",
                            status.description(),
                            persisted_unread()
                        ))));
                    }
                }
                menu.summary
                    .setTitle(&NSString::from_str(&status.description()));
                menu.open.setTitle(&NSString::from_str(tr("Kiln 열기")));
                menu.settings.setTitle(&NSString::from_str(tr("설정…")));
                menu.hint.setTitle(&NSString::from_str(tr("창을 닫아도 세션은 유지됩니다")));
                let busy = STOPPING.load(Ordering::SeqCst);
                menu.stop.setEnabled(!busy && matches!(status, Status::Connected { running, .. } if running > 0));
                menu.stop.setTitle(&NSString::from_str(if busy { tr("세션 확인 중…") } else { tr("백그라운드 세션 종료…") }));
            }
        }
    });
    poll_stop_result();
}

fn executable_identity(path:&std::path::Path)->Option<(u64,u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m|(m.dev(),m.ino()))
}

pub fn run() -> anyhow::Result<()> {
    i18n::set_language(i18n::load_language());
    let socket = kiln_proto::socket_name();
    let lock_path = format!("{socket}.menubar.lock");
    if let Some(parent) = std::path::Path::new(&lock_path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    // Retain the file for this entire process. Never unlink an active lock inode.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Ok(());
    }
    let status = Arc::new(Mutex::new(Status::Connecting));
    let background = status.clone();
    std::thread::spawn(move || {
        let mut failures = 0;
        let mut client: Option<Client> = None;
        loop {
            if client.is_none() {
                client = Client::connect(&socket, None).ok();
            }
            let snapshot = client.as_ref().and_then(|c| {
                c.request(
                    |req| ClientMsg::ListSessions { req },
                    Duration::from_secs(2),
                )
                .ok()
            });
            if let Some(ServerMsg::Sessions { sessions, .. }) = snapshot {
                failures = 0;
                let running = sessions.iter().filter(|s| s.exited.is_none()).count();
                let attention = sessions.iter().filter(|s| s.attention).count();
                *background.lock().unwrap_or_else(|e| e.into_inner()) =
                    Status::Connected { running, attention };
            } else {
                client = None;
                failures += 1;
                *background.lock().unwrap_or_else(|e| e.into_inner()) = if failures >= 10 {
                    Status::Disconnected
                } else {
                    Status::Connecting
                };
                if failures >= 10 {
                    break;
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
    let mt = MainThreadMarker::new().expect("menu bar runs on the main thread");
    let app = NSApplication::sharedApplication(mt);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let mut builder = ClassBuilder::new("KilnStatusBarController", NSObject::class())
        .expect("status controller class");
    unsafe {
        builder.add_method(
            sel!(openKiln:),
            open_window as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
        );
        builder.add_method(
            sel!(openKilnSettings:),
            open_settings as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
        );
        builder.add_method(
            sel!(stopKilnSessions:),
            stop_sessions as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
        );
        builder.add_method(
            sel!(refreshKiln:),
            tick as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
        );
        builder.add_method(
            sel!(applicationShouldHandleReopen:hasVisibleWindows:),
            reopen as extern "C" fn(*mut AnyObject, Sel, *mut NSApplication, Bool) -> Bool,
        );
    }
    let class = builder.register();
    let controller: Retained<AnyObject> = unsafe { msg_send_id![class, new] };
    unsafe {
        let _: () = objc2::msg_send![&app,setDelegate:&*controller];
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(-1.0);
        let mut icon_loaded = false;
        if let Some(button) = item.button(mt) {
            if let Ok(exe) = gui_executable() {
                let path = exe
                    .parent()
                    .and_then(|p| p.parent())
                    .map(|p| p.join("Resources/KilnStatusTemplate.png"));
                if let Some(path) = path {
                    if let Some(image) = NSImage::initWithContentsOfFile(
                        NSImage::alloc(),
                        &NSString::from_str(&path.to_string_lossy()),
                    ) {
                        image.setSize(NSSize::new(18.0, 18.0));
                        // AppKit derives light/dark menu ink from this alpha mask.
                        image.setTemplate(true);
                        button.setImage(Some(&image));
                        icon_loaded = true;
                        button.setImagePosition(NSCellImagePosition::NSImageLeft);
                    }
                }
            }
            button.setTitle(&NSString::from_str(&menu_title(icon_loaded, "")));
        }
        let menu = NSMenu::new(mt);
        menu.setAutoenablesItems(false);
        let summary = NSMenuItem::initWithTitle_action_keyEquivalent(
            mt.alloc(),
            &NSString::from_str(tr("Kiln에 연결하는 중…")),
            None,
            &NSString::from_str(""),
        );
        summary.setEnabled(false);
        menu.addItem(&summary);
        menu.addItem(&NSMenuItem::separatorItem(mt));
        let open = NSMenuItem::initWithTitle_action_keyEquivalent(
            mt.alloc(),
            &NSString::from_str(tr("Kiln 열기")),
            Some(sel!(openKiln:)),
            &NSString::from_str(""),
        );
        open.setTarget(Some(&controller));
        menu.addItem(&open);
        let settings = NSMenuItem::initWithTitle_action_keyEquivalent(mt.alloc(),
            &NSString::from_str(tr("설정…")), Some(sel!(openKilnSettings:)), &NSString::new());
        settings.setTarget(Some(&controller));
        menu.addItem(&settings);
        menu.addItem(&NSMenuItem::separatorItem(mt));
        let stop = NSMenuItem::initWithTitle_action_keyEquivalent(mt.alloc(),
            &NSString::from_str(tr("백그라운드 세션 종료…")), Some(sel!(stopKilnSessions:)), &NSString::new());
        stop.setTarget(Some(&controller));
        stop.setEnabled(false);
        menu.addItem(&stop);
        let hint = NSMenuItem::initWithTitle_action_keyEquivalent(
            mt.alloc(),
            &NSString::from_str(tr("창을 닫아도 세션은 유지됩니다")),
            None,
            &NSString::from_str(""),
        );
        hint.setEnabled(false);
        menu.addItem(&hint);
        item.setMenu(Some(&menu));
        // NSStatusItem's menu handles primary clicks; the button's NSView
        // contextual menu also exposes the same actions on secondary clicks.
        if let Some(button) = item.button(mt) { button.setMenu(Some(&menu)); }
        MENU.with(|c| {
            *c.borrow_mut() = Some(MenuState {
                executable: std::env::current_exe().unwrap_or_default(),
                executable_identity: std::env::current_exe().ok().and_then(|p|executable_identity(&p)),
                icon_loaded,
                item,
                summary,
                stop,
                open,
                settings,
                hint,
                status,
            })
        });
        let _timer = NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
            1.0,
            &controller,
            sel!(refreshKiln:),
            None,
            true,
        );
        app.run();
    }
    drop(lock);
    Ok(())
}
#[cfg(test)]
mod tests {
    #[test]
    fn menu_template_has_transparent_corners_and_an_enlarged_symbol() {
        let image=image::load_from_memory(include_bytes!("../../../assets/KilnStatusTemplate.png")).unwrap().into_rgba8();
        assert_eq!(image.dimensions(),(54,54));
        for (x,y) in [(0,0),(53,0),(0,53),(53,53),(27,27)] {assert_eq!(image.get_pixel(x,y)[3],0,"the tile and inner opening must stay transparent");}
        let pixels:Vec<_>=image.enumerate_pixels().filter(|(_,_,p)|p[3]>128).collect();
        let extent=|horizontal:bool| {
            let coordinates:Vec<_>=pixels.iter().map(|(x,y,_)|if horizontal{*x}else{*y}).collect();
            coordinates.iter().max().unwrap()-coordinates.iter().min().unwrap()
        };
        assert!(extent(true)>=46 && extent(false)>=46,"symbol should fill the 18pt slot, not inherit the Dock tile's padding");
    }

    use super::*;
    fn session(id: u64) -> SessionInfo {
        SessionInfo { id, pid: id as u32 + 100, created_unix: id + 1000, ..Default::default() }
    }
    #[test]
    fn termination_excludes_new_exited_and_reused_sessions() {
        let reviewed = vec![session(1), session(2), session(3), session(4)];
        let mut exited = session(2); exited.exited = Some(0);
        let mut reused = session(3); reused.created_unix += 1;
        let mut changed_pid = session(4); changed_pid.pid += 1;
        assert_eq!(reviewed_session_ids(&reviewed, &[session(1), exited, reused, changed_pid, session(5)]), vec![1]);
    }
    #[test]
    fn confirmed_stop_sends_only_reviewed_kills_and_keeps_connection_alive() {
        use kiln_proto::{read_msg, write_msg, PROTO_VERSION};
        use std::os::unix::net::UnixListener;
        let dir = tempfile::Builder::new().prefix("kiln-menu-test-").tempdir_in("/tmp").unwrap();
        let socket = dir.path().join("d.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            assert!(matches!(read_msg::<_, ClientMsg>(&mut stream).unwrap(), Some(ClientMsg::Hello { .. })));
            write_msg(&mut stream, &ServerMsg::Hello { proto: PROTO_VERSION, build: "test".into(), pid: 1, can_upgrade: false }).unwrap();
            let mut sessions = vec![session(1), session(2)];
            let mut killed = vec![];
            loop {
                match read_msg::<_, ClientMsg>(&mut stream).unwrap().unwrap() {
                    ClientMsg::ListSessions { req } => write_msg(&mut stream, &ServerMsg::Sessions { req, sessions: sessions.clone() }).unwrap(),
                    ClientMsg::Kill { session } => {
                        killed.push(session);
                        sessions.retain(|s| s.id != session);
                    }
                    ClientMsg::Ping { req } => {
                        write_msg(&mut stream, &ServerMsg::Pong { req }).unwrap();
                        assert_eq!(killed, vec![1]);
                        assert_eq!(sessions, vec![session(2)]);
                        break;
                    }
                    unexpected => panic!("must not shut down or create sessions: {unexpected:?}"),
                }
            }
        });
        let client = Client::connect(socket.to_str().unwrap(), None).unwrap();
        // This fixture is a protocol peer only; no real daemon or PTY exists.
        stop_reviewed(&client, &[session(1)]).unwrap();
        assert!(matches!(client.request(|req| ClientMsg::Ping { req }, Duration::from_secs(3)).unwrap(), ServerMsg::Pong { .. }));
        server.join().unwrap();
    }
    #[test]
    fn helper_identity_changes_after_atomic_bundle_replacement() {
        let dir=tempfile::tempdir().unwrap();
        let path=dir.path().join("kiln-status");
        std::fs::write(&path,b"old").unwrap();
        let old=executable_identity(&path).unwrap();
        assert_eq!(executable_identity(&path),Some(old));
        let replacement=dir.path().join("replacement");
        std::fs::write(&replacement,b"new").unwrap();
        std::fs::rename(replacement,&path).unwrap();
        assert_ne!(executable_identity(&path),Some(old));
        std::fs::remove_file(&path).unwrap();
        assert_eq!(executable_identity(&path),None);
    }
    #[test]
    fn menu_distinguishes_connecting_idle_running_and_attention() {
        assert_eq!(Status::Connecting.description(), "Kiln에 연결하는 중…");
        assert_eq!(
            Status::Connected {
                running: 0,
                attention: 0
            }
            .description(),
            "Kiln 실행 중 · 활성 세션 없음"
        );
        assert_eq!(
            Status::Connected {
                running: 0,
                attention: 2
            }
            .description(),
            "활성 세션 없음 · 확인 필요 2개"
        );
        assert_eq!(menu_title(true, ""), "");
        assert_eq!(menu_title(true, "●"), "●");
        assert_eq!(menu_title(false, ""), "Kiln");
        assert_eq!(menu_title(false, "●"), "Kiln ●");
        assert_eq!(badge_title(false, 0), "");
        assert_eq!(badge_title(true, 0), "●");
        assert_eq!(badge_title(false, 1), "●");
        assert_eq!(
            Status::Connected {
                running: 2,
                attention: 1
            }
            .description(),
            "세션 2개 실행 중 · 확인 필요 1개"
        );
    }
}
