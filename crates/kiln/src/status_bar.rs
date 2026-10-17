//! A single menu-bar companion survives GUI closure while the daemon is alive.
//! AppKit status items must live on the main run loop; the daemon stays headless.
//! https://developer.apple.com/documentation/appkit/nsstatusitem
use crate::companion::{self, Status};
use kiln_daemon::client::Client;
use kiln_common::i18n::{self, tr};
use kiln_proto::SessionInfo;
use objc2::{
    ClassType, msg_send_id,
    rc::Retained,
    runtime::{AnyObject, Bool, ClassBuilder, Sel},
    sel,
};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSApplicationActivationPolicy,
    NSCellImagePosition, NSImage, NSMenu, NSMenuItem, NSRunningApplication, NSStatusBar,
    NSStatusItem, NSAlert, NSAlertStyle, NSAlertSecondButtonReturn, NSStatusBarButton,
    NSColor, NSFont, NSFontAttributeName, NSForegroundColorAttributeName,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSSize, NSString, NSTimer, NSMutableAttributedString, NSRange};
use std::{
    cell::RefCell,
    fs::OpenOptions,
    os::fd::AsRawFd,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

fn menu_title(icon_loaded: bool, badge: &str) -> String {
    match (icon_loaded, badge.is_empty()) {
        (true, _) => badge.into(),
        (false, true) => "Kiln".into(),
        (false, false) => format!("Kiln {badge}"),
    }
}
/// Keep AppKit's template icon unchanged; style only the unread indicator.
/// https://developer.apple.com/documentation/appkit/nsbutton/attributedtitle
unsafe fn set_menu_title(button: &NSStatusBarButton, icon_loaded: bool, badge: &str) {
    unsafe {
        let title = menu_title(icon_loaded, badge);
        button.setTitle(&NSString::from_str(&title));
        button.setAlternateTitle(&NSString::new());
        if !badge.is_empty() {
            let mut attributed = NSMutableAttributedString::initWithString(
                NSMutableAttributedString::alloc(),
                &NSString::from_str(&title),
            );
            let range = NSRange::new(title.encode_utf16().count() - 1, 1);
            // A 9pt dot occupies roughly one third of the 18pt app symbol.
            // Attribute the dot alone so the fallback app name remains legible.
            attributed.addAttribute_value_range(
                NSFontAttributeName,
                &NSFont::systemFontOfSize(9.0),
                range,
            );
            attributed.addAttribute_value_range(
                NSForegroundColorAttributeName,
                &NSColor::systemRedColor(),
                range,
            );
            button.setAttributedTitle(&attributed);
            button.setAttributedAlternateTitle(&attributed);
        }
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

extern "C" fn stop_sessions(_: *mut AnyObject, _: Sel, _: *mut AnyObject) {
    if STOPPING.swap(true, Ordering::SeqCst) { return; }
    let (tx, rx) = std::sync::mpsc::channel();
    STOP_RESULT.with(|state| *state.borrow_mut() = Some(rx));
    std::thread::spawn(move || {
        let _ = tx.send(StopResult::Snapshot(companion::running_sessions()));
    });
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
                std::thread::spawn(move || { let _ = tx.send(StopResult::Finished(companion::stop_reviewed(&client, &sessions))); });
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
    let (title, message) = companion::stop_message(sessions);
    unsafe {
        let alert = NSAlert::new(mt);
        alert.setAlertStyle(NSAlertStyle::Warning);
        alert.setMessageText(&NSString::from_str(&title));
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
                        set_menu_title(&button, menu.icon_loaded, &status.title());
                        button.setToolTip(Some(&NSString::from_str(&kiln_common::trf!(
                            "{} · 읽지 않은 알림 {}개",
                            status.description(),
                            companion::persisted_unread()
                        ))));
                    }
                }
                menu.summary
                    .setTitle(&NSString::from_str(&status.description()));
                menu.open.setTitle(&NSString::from_str(tr("Kiln 열기")));
                menu.settings.setTitle(&NSString::from_str(tr("설정…")));
                menu.hint.setTitle(&NSString::from_str(tr("창을 닫아도 세션은 유지됩니다")));
                let busy = STOPPING.load(Ordering::SeqCst);
                menu.stop.setEnabled(!busy && status.running() > 0);
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
    std::thread::spawn(move || companion::watch(socket, background));
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
            set_menu_title(&button, icon_loaded, "");
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
    fn menu_title_falls_back_to_the_app_name_without_an_icon() {
        assert_eq!(menu_title(true, ""), "");
        assert_eq!(menu_title(true, "●"), "●");
        assert_eq!(menu_title(false, ""), "Kiln");
        assert_eq!(menu_title(false, "●"), "Kiln ●");
    }
}
