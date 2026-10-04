//! Sparkle is loaded only from the signed app's embedded framework, on the main
//! thread. CLI, daemon, tests and source builds do not initialize an updater.
//! https://sparkle-project.org/documentation/programmatic-setup/
use objc2::{msg_send, msg_send_id, rc::{Allocated, Retained}, runtime::{AnyClass, AnyObject, Bool}, sel};
use objc2_app_kit::{NSApplication, NSMenuItem};
use objc2_foundation::{MainThreadMarker, NSString};
use std::{cell::RefCell, ffi::CString};

thread_local! {
    static CONTROLLER: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
    static ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub(super) fn install() {
    let Some(mt) = MainThreadMarker::new() else { return };
    let isolated = std::env::var_os("KILN_CONFIG_DIR").is_some() || std::env::var_os("KILN_SOCKET").is_some();
    if isolated && !native_fixture_enabled() { return; }
    let Some(contents) = std::env::current_exe().ok().and_then(|p|p.parent()?.parent().map(|p|p.to_path_buf())) else {return};
    let framework = contents.join("Frameworks/Sparkle.framework/Sparkle");
    if !framework.is_file() { return; }
    let result = (|| -> Result<(), String> {
        let path = CString::new(framework.as_os_str().as_encoded_bytes()).map_err(|e|e.to_string())?;
        // Keep the library loaded for the process lifetime. AppKit may retain its
        // classes even after the updater controller has finished a check.
        if unsafe {libc::dlopen(path.as_ptr(),libc::RTLD_NOW|libc::RTLD_LOCAL)}.is_null() {
            return Err("자동 업데이트 구성 요소를 불러오지 못했습니다.".into());
        }
        let class = AnyClass::get("SPUStandardUpdaterController").ok_or("업데이트 컨트롤러를 찾지 못했습니다.")?;
        let nil: *mut AnyObject = std::ptr::null_mut();
        let controller: Allocated<AnyObject> = unsafe {msg_send_id![class, alloc]};
        let controller: Retained<AnyObject> = unsafe {msg_send_id![controller, initWithStartingUpdater: Bool::NO updaterDelegate: nil userDriverDelegate: nil]};
        let updater: *mut AnyObject = unsafe {msg_send![&*controller, updater]};
        let mut error: *mut AnyObject = std::ptr::null_mut();
        let started: Bool = unsafe {msg_send![updater, startUpdater: &mut error]};
        if !started.as_bool() { return Err("자동 업데이트를 시작하지 못했습니다. 공식 릴리스를 다시 설치하세요.".into()); }
        let app=NSApplication::sharedApplication(mt);
        if let Some(menu)=unsafe {app.mainMenu()}.and_then(|m|unsafe {m.itemAtIndex(0)}).and_then(|m|unsafe {m.submenu()}) {
            let item=unsafe {NSMenuItem::initWithTitle_action_keyEquivalent(mt.alloc(),&NSString::from_str("업데이트 확인…"),Some(sel!(checkForUpdates:)),&NSString::new())};
            unsafe {item.setTarget(Some(&controller));menu.insertItem_atIndex(&item,1);}
        }
        CONTROLLER.with(|state|*state.borrow_mut()=Some(controller));
        Ok(())
    })();
    if let Err(error)=result {log::error!("{error}"); ERROR.with(|s|*s.borrow_mut()=Some(error));}
}

fn native_fixture_enabled() -> bool {
    #[cfg(feature = "updater-test")]
    {
        let Some(root) = option_env!("KILN_UPDATER_TEST_ROOT") else { return false; };
        let Ok(root) = std::path::Path::new(root).canonicalize() else { return false; };
        return std::env::var_os("KILN_CONFIG_DIR").as_deref() == Some(root.join("config").as_os_str())
            && std::env::var_os("KILN_SOCKET").as_deref() == Some(root.join("daemon.sock").as_os_str());
    }
    #[cfg(not(feature = "updater-test"))]
    false
}

pub(super) fn settings(ui:&mut egui::Ui) {
    use kiln_common::widgets::{self,ButtonKind};
    CONTROLLER.with(|state| {
        let state=state.borrow();
        let Some(controller)=state.as_ref() else {
            let message=ERROR.with(|e|e.borrow().clone()).unwrap_or_else(||"자동 업데이트는 공식 macOS 배포판에서 제공됩니다.".into());
            ui.label(message);
            return;
        };
        let updater:*mut AnyObject=unsafe {msg_send![&**controller,updater]};
        let automatic:Bool=unsafe {msg_send![updater,automaticallyChecksForUpdates]};
        let mut automatic=automatic.as_bool();
        if ui.checkbox(&mut automatic,"자동으로 업데이트 확인").changed() {
            let _:()=unsafe {msg_send![updater,setAutomaticallyChecksForUpdates:Bool::new(automatic)]};
        }
        let can_check:Bool=unsafe {msg_send![updater,canCheckForUpdates]};
        ui.add_enabled_ui(can_check.as_bool(),|ui| {
            if widgets::button(ui,"업데이트 확인…",ButtonKind::Secondary).clicked() {
                let nil:*mut AnyObject=std::ptr::null_mut();
                let _:()=unsafe {msg_send![&**controller,checkForUpdates:nil]};
            }
        });
    });
}
