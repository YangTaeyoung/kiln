//! A single menu-bar companion survives GUI closure while the daemon is alive.
//! AppKit status items must live on the main run loop; the daemon stays headless.
//! https://developer.apple.com/documentation/appkit/nsstatusitem
use kiln_daemon::client::Client;
use kiln_proto::{ClientMsg, ServerMsg};
use objc2::{
    ClassType, msg_send_id,
    rc::Retained,
    runtime::{AnyObject, Bool, ClassBuilder, Sel},
    sel,
};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSApplicationActivationPolicy,
    NSCellImagePosition, NSImage, NSMenu, NSMenuItem, NSRunningApplication, NSStatusBar,
    NSStatusItem,
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
            Self::Connecting => "Kiln에 연결하는 중…".into(),
            Self::Connected {
                running: 0,
                attention: 0,
            } => "Kiln 실행 중 · 활성 세션 없음".into(),
            Self::Connected {
                running: 0,
                attention,
            } => format!("활성 세션 없음 · 확인 필요 {attention}개"),
            Self::Connected {
                running,
                attention: 0,
            } => format!("백그라운드 세션 {running}개 실행 중"),
            Self::Connected { running, attention } => {
                format!("세션 {running}개 실행 중 · 확인 필요 {attention}개")
            }
            Self::Disconnected => "Kiln 연결이 끊겼습니다".into(),
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
    show_window();
}
extern "C" fn reopen(_: *mut AnyObject, _: Sel, _: *mut NSApplication, _: Bool) -> Bool {
    show_window();
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
fn show_window() {
    // Activate an existing regular GUI, excluding this accessory and daemons.
    unsafe {
        let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(
            &NSString::from_str(if cfg!(feature = "updater-test") { "dev.kiln.updater-test" } else { "dev.kiln.app" }),
        );
        for app in apps.iter() {
            if app.activationPolicy() == NSApplicationActivationPolicy::Regular {
                let _ = app.activateWithOptions(
                    NSApplicationActivationOptions::NSApplicationActivateIgnoringOtherApps,
                );
                return;
            }
        }
    }
    if GUI_LAUNCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Ok(exe) = gui_executable() {
        if let Ok(mut child) = Command::new(exe)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            std::thread::spawn(move || {
                let _ = child.wait();
                GUI_LAUNCHING.store(false, Ordering::SeqCst);
            });
            return;
        }
    }
    GUI_LAUNCHING.store(false, Ordering::SeqCst);
}
extern "C" fn tick(_: *mut AnyObject, _: Sel, _: *mut AnyObject) {
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
                        button.setToolTip(Some(&NSString::from_str(&format!(
                            "{} · 읽지 않은 알림 {}개",
                            status.description(),
                            persisted_unread()
                        ))));
                    }
                }
                menu.summary
                    .setTitle(&NSString::from_str(&status.description()));
            }
        }
    });
}

fn executable_identity(path:&std::path::Path)->Option<(u64,u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m|(m.dev(),m.ino()))
}

pub fn run() -> anyhow::Result<()> {
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
                    .map(|p| p.join("Resources/Kiln.icns"));
                if let Some(path) = path {
                    if let Some(image) = NSImage::initWithContentsOfFile(
                        NSImage::alloc(),
                        &NSString::from_str(&path.to_string_lossy()),
                    ) {
                        image.setSize(NSSize::new(18.0, 18.0));
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
            &NSString::from_str("Kiln에 연결하는 중…"),
            None,
            &NSString::from_str(""),
        );
        summary.setEnabled(false);
        menu.addItem(&summary);
        menu.addItem(&NSMenuItem::separatorItem(mt));
        let open = NSMenuItem::initWithTitle_action_keyEquivalent(
            mt.alloc(),
            &NSString::from_str("Kiln 열기"),
            Some(sel!(openKiln:)),
            &NSString::from_str(""),
        );
        open.setTarget(Some(&controller));
        menu.addItem(&open);
        let hint = NSMenuItem::initWithTitle_action_keyEquivalent(
            mt.alloc(),
            &NSString::from_str("창을 닫아도 세션은 유지됩니다"),
            None,
            &NSString::from_str(""),
        );
        hint.setEnabled(false);
        menu.addItem(&hint);
        item.setMenu(Some(&menu));
        MENU.with(|c| {
            *c.borrow_mut() = Some(MenuState {
                executable: std::env::current_exe().unwrap_or_default(),
                executable_identity: std::env::current_exe().ok().and_then(|p|executable_identity(&p)),
                icon_loaded,
                item,
                summary,
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
    use super::*;
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
