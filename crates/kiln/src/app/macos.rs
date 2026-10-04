//! Defer native termination to the root GUI, including when an auxiliary quick
//! terminal is key. The root close guard saves state or asks about unsaved work;
//! on_exit replies yes for a clean close. Dirty work ends native deferral with
//! a no reply before showing egui's confirmation, keeping winit's default-mode
//! input alive. A system logout with dirty work is therefore cancelled; after
//! resolving edits the user can retry logout.
//! https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/AppArchitecture/Tasks/GracefulAppTermination.html

use objc2::{sel, Encode};
use std::sync::atomic::{AtomicBool, Ordering};
static TERMINATION_PENDING: AtomicBool = AtomicBool::new(false);
static TERMINATION_REQUESTED: AtomicBool = AtomicBool::new(false);
static APP_CONTEXT: std::sync::OnceLock<egui::Context> = std::sync::OnceLock::new();
pub(super) fn take_termination_request() -> bool { TERMINATION_REQUESTED.swap(false, Ordering::SeqCst) }

pub(super) fn reply_to_termination(allow: bool) {
    #[cfg(feature = "updater-test")]
    crate::updater_fixture_event(if allow { "native-reply-allow" } else { "native-reply-cancel" });
    if TERMINATION_PENDING.swap(false, Ordering::SeqCst) {
        if let Some(mt) = MainThreadMarker::new() { unsafe { NSApplication::sharedApplication(mt).replyToApplicationShouldTerminate(allow); } }
    }
}
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2_app_kit::{NSApplication, NSApplicationTerminateReply};
use objc2_foundation::MainThreadMarker;

pub(super) fn install_quit_guard(ctx: &egui::Context) {
    let _ = APP_CONTEXT.set(ctx.clone());
    let Some(main_thread) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(main_thread);
    // Preserve winit's delegate instance and every existing method. The public
    // optional delegate callback is added only when absent, including ancestors.
    if let Some(delegate) = unsafe { app.delegate() } {
        // NSObjectProtocol guarantees the standard `class` query.
        let class: &AnyClass = unsafe { objc2::msg_send![&*delegate, class] };
        if install_termination_guard(class) {
            // Reassign the same retained delegate so AppKit can refresh cached
            // optional-method capabilities. The delegate identity is unchanged.
            app.setDelegate(Some(&delegate));
        }
    }
}

/// AppKit sends this callback for Cmd-Q, menu/Dock Quit and normal OS termination requests.
/// It does not send windowShouldClose: automatically. Force Quit is not cancellable.
/// https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/AppArchitecture/Tasks/GracefulAppTermination.html
extern "C" fn should_terminate(_delegate: &AnyObject, _selector: Sel, app: &NSApplication) -> NSApplicationTerminateReply {
    #[cfg(feature = "updater-test")]
    crate::updater_fixture_event("native-termination-request");
    // Route every native Quit to the root app, never whichever auxiliary
    // terminal happens to be key. The root logic handles dirty work and replies.
    if let Some(ctx) = APP_CONTEXT.get() {
        TERMINATION_PENDING.store(true, Ordering::SeqCst);
        TERMINATION_REQUESTED.store(true, Ordering::SeqCst);
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
        ctx.request_repaint_of(egui::ViewportId::ROOT);
        NSApplicationTerminateReply::NSTerminateLater
    } else {
        let _ = app;
        NSApplicationTerminateReply::NSTerminateCancel
    }
}

fn install_termination_guard(class: &AnyClass) -> bool {
    let selector = sel!(applicationShouldTerminate:);
    if class.instance_method(selector).is_some() {
        return false;
    }
    // Objective-C method encoding: NSUInteger return, self, selector, NSApplication*.
    // Derive the integer encoding from the SDK binding rather than assuming its width.
    let encoding = std::ffi::CString::new(format!("{}@:@", NSApplicationTerminateReply::ENCODING)).expect("Objective-C type encoding has no NUL");
    // SAFETY: The callback has exactly the public NSApplicationDelegate method's
    // ABI. AnyClass is repr(C), so casting its address to objc_class is valid.
    // class_addMethod copies the encoding and NEVER replaces an existing method.
    // No delegate/class name assumptions, class changes, or implementation swizzling.
    unsafe {
        let implementation: Imp = std::mem::transmute(should_terminate as extern "C" fn(&AnyObject, Sel, &NSApplication) -> NSApplicationTerminateReply);
        let added = objc2::ffi::class_addMethod(
            (class as *const AnyClass).cast_mut().cast(), selector.as_ptr(), Some(implementation), encoding.as_ptr(),
        );
        added != objc2::ffi::NO
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::{ClassType, runtime::ClassBuilder};
    use objc2_foundation::NSObject;

    #[test]
    fn missing_termination_callback_is_added_once_and_inherited_handler_is_preserved() {
        let builder = ClassBuilder::new("KilnTerminationGuardTest", NSObject::class()).unwrap();
        let class = builder.register();
        assert!(install_termination_guard(class));
        assert!(!install_termination_guard(class));
        let subclass = ClassBuilder::new("KilnTerminationGuardSubclassTest", class).unwrap().register();
        assert!(!install_termination_guard(subclass), "inherited termination policy must not be overwritten");
        let method = class.instance_method(sel!(applicationShouldTerminate:)).unwrap();
        assert_eq!(method.arguments_count(), 3);
    }
}
