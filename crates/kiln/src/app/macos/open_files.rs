//! Main-thread AppKit document callbacks, installed before initial open events.
use objc2::{
    ClassType, Encode, msg_send_id,
    rc::Retained,
    runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Imp, Sel},
    sel,
};
use objc2_app_kit::{
    NSApplication, NSApplicationDelegateReply, NSApplicationWillFinishLaunchingNotification,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSNotification, NSNotificationCenter, NSObject, NSString,
};
use parking_lot::Mutex;
use std::{cell::RefCell, path::PathBuf, sync::OnceLock};

static REQUESTS: Mutex<Vec<OpenRequest>> = Mutex::new(Vec::new());
static CONTEXT: OnceLock<egui::Context> = OnceLock::new();
thread_local! {
    // NSNotificationCenter does not retain selector-based observers.
    static LAUNCH_OBSERVER: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
}

#[derive(Debug)]
pub(crate) struct OpenRequest {
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) needs_reply: bool,
}

/// Called from NativeOptions.event_loop_builder, BEFORE winit creates/assigns its
/// delegate. Observe will-finish so no private winit class name or swizzle is needed.
/// AppKit sends initial open-file events before did-finish/app construction:
/// https://developer.apple.com/documentation/appkit/nsapplicationdelegate/application(_:openfile:)
/// https://developer.apple.com/documentation/appkit/nsapplication/willfinishlaunchingnotification
pub(crate) fn prepare_open_file_events() {
    let Some(mt) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mt);
    LAUNCH_OBSERVER.with(|slot| {
        if slot.borrow().is_some() {
            return;
        }
        let mut builder = ClassBuilder::new("KilnDocumentOpenObserver", NSObject::class())
            .expect("document launch observer class");
        unsafe {
            builder.add_method(
                sel!(kilnWillFinishLaunching:),
                will_finish_launching as extern "C" fn(*mut AnyObject, Sel, *mut NSNotification),
            );
            let observer: Retained<AnyObject> = msg_send_id![builder.register(), new];
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                &observer,
                sel!(kilnWillFinishLaunching:),
                Some(NSApplicationWillFinishLaunchingNotification),
                Some(&app),
            );
            *slot.borrow_mut() = Some(observer);
        }
    });
    // Also support an already-created event loop, without replacing any delegate.
    install_on_delegate(&app);
}

extern "C" fn will_finish_launching(_: *mut AnyObject, _: Sel, _: *mut NSNotification) {
    if let Some(mt) = MainThreadMarker::new() {
        install_on_delegate(&NSApplication::sharedApplication(mt));
    }
}

fn install_on_delegate(app: &NSApplication) {
    if let Some(delegate) = unsafe { app.delegate() } {
        let class: &AnyClass = unsafe { objc2::msg_send![&*delegate, class] };
        if install_callbacks(class) {
            // Refresh AppKit's cached optional-method capabilities on the SAME object.
            app.setDelegate(Some(&delegate));
        }
    }
}

fn install_callbacks(class: &AnyClass) -> bool {
    let single_encoding = std::ffi::CString::new(format!("{}@:@@", Bool::ENCODING)).unwrap();
    // SAFETY: Each function has the exact public NSApplicationDelegate selector's
    // ABI. class_addMethod copies the encoding and never replaces an existing or
    // inherited implementation. AnyClass is repr(C).
    unsafe {
        let callbacks = [
            (
                sel!(application:openFile:),
                std::mem::transmute::<
                    extern "C" fn(&AnyObject, Sel, &NSApplication, &NSString) -> Bool,
                    Imp,
                >(open_file),
                single_encoding.as_c_str(),
            ),
            (
                sel!(application:openFiles:),
                std::mem::transmute::<
                    extern "C" fn(&AnyObject, Sel, &NSApplication, &NSArray<NSString>),
                    Imp,
                >(open_files),
                c"v@:@@",
            ),
        ];
        let mut added = false;
        for (selector, implementation, encoding) in callbacks {
            if class.instance_method(selector).is_none() {
                added |= objc2::ffi::class_addMethod(
                    (class as *const AnyClass).cast_mut().cast(),
                    selector.as_ptr(),
                    Some(implementation),
                    encoding.as_ptr(),
                ) != objc2::ffi::NO;
            }
        }
        added
    }
}

extern "C" fn open_file(_: &AnyObject, _: Sel, _: &NSApplication, filename: &NSString) -> Bool {
    let path = PathBuf::from(filename.to_string());
    if !path.is_file() && !path.is_dir() {
        return Bool::NO;
    }
    enqueue(OpenRequest {
        paths: vec![path],
        needs_reply: false,
    });
    Bool::YES
}

extern "C" fn open_files(_: &AnyObject, _: Sel, _: &NSApplication, filenames: &NSArray<NSString>) {
    // Reply only after routing has attempted every file, including during startup.
    // https://developer.apple.com/documentation/appkit/nsapplicationdelegate/application(_:openfiles:)
    enqueue(OpenRequest {
        paths: filenames
            .to_vec()
            .into_iter()
            .map(|s| PathBuf::from(s.to_string()))
            .collect(),
        needs_reply: true,
    });
}

fn enqueue(request: OpenRequest) {
    REQUESTS.lock().push(request);
    wake_window();
}

fn wake_window() {
    if let Some(ctx) = CONTEXT.get() {
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::Minimized(false),
        );
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
}

pub(crate) fn attach_open_file_context(ctx: &egui::Context) {
    let _ = CONTEXT.set(ctx.clone());
    if has_open_requests() {
        wake_window();
    }
}

pub(crate) fn has_open_requests() -> bool {
    !REQUESTS.lock().is_empty()
}

pub(crate) fn take_open_requests() -> Vec<OpenRequest> {
    std::mem::take(&mut *REQUESTS.lock())
}

pub(crate) fn finish_open_request(request: &OpenRequest, opened: bool) {
    if request.needs_reply
        && let Some(mt) = MainThreadMarker::new()
    {
        let reply = if opened {
            NSApplicationDelegateReply::Success
        } else {
            NSApplicationDelegateReply::Failure
        };
        unsafe {
            NSApplication::sharedApplication(mt).replyToOpenOrPrint(reply);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn callbacks_are_added_once_and_inherited_implementations_are_preserved() {
        use super::*;
        extern "C" fn existing_open(
            _: *mut AnyObject,
            _: Sel,
            _: *mut NSApplication,
            _: *mut NSString,
        ) -> Bool {
            Bool::NO
        }
        let class = ClassBuilder::new("KilnDocumentOpenTest", NSObject::class())
            .unwrap()
            .register();
        assert!(install_callbacks(class));
        assert!(!install_callbacks(class));
        let subclass = ClassBuilder::new("KilnDocumentOpenSubclassTest", class)
            .unwrap()
            .register();
        assert!(!install_callbacks(subclass));
        for selector in [sel!(application:openFile:), sel!(application:openFiles:)] {
            assert_eq!(
                class.instance_method(selector).unwrap().arguments_count(),
                4
            );
        }
        let mut builder =
            ClassBuilder::new("KilnExistingDocumentHandlerTest", NSObject::class()).unwrap();
        unsafe {
            builder.add_method(
                sel!(application:openFile:),
                existing_open
                    as extern "C" fn(
                        *mut AnyObject,
                        Sel,
                        *mut NSApplication,
                        *mut NSString,
                    ) -> Bool,
            );
        }
        let existing = builder.register();
        let original = existing
            .instance_method(sel!(application:openFile:))
            .unwrap()
            .implementation();
        assert!(
            install_callbacks(existing),
            "add only the missing multi-file method"
        );
        assert_eq!(
            existing
                .instance_method(sel!(application:openFile:))
                .unwrap()
                .implementation() as usize,
            original as usize
        );
        let child = ClassBuilder::new("KilnExistingDocumentHandlerChildTest", existing)
            .unwrap()
            .register();
        assert!(!install_callbacks(child));
        assert_eq!(
            child
                .instance_method(sel!(application:openFile:))
                .unwrap()
                .implementation() as usize,
            original as usize
        );
    }
}
