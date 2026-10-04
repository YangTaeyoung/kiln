//! Main-thread AppKit callback regression. Uses a deterministic IME driver on a
//! hidden WinitView; no system input-source change or user keyboard automation.
#[cfg(target_os = "macos")]
mod macos {
    use objc2::{
        msg_send,
        runtime::{AnyClass, AnyObject, Sel},
        sel,
    };
    use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType};
    use objc2_foundation::{NSArray, NSPoint, NSRange, NSString};
    use std::{
        ffi::c_void,
        sync::atomic::{AtomicUsize, Ordering},
    };
    use winit::{
        application::ApplicationHandler,
        event::{ElementState, Ime, WindowEvent},
        event_loop::{ActiveEventLoop, EventLoop},
        raw_window_handle::{HasWindowHandle, RawWindowHandle},
        window::{Window, WindowId},
    };
    static CASE: AtomicUsize = AtomicUsize::new(0);
    // (composed text, physical trigger, driver mode, expected key text, key count, modifier).
    // Expected results are explicit, independent of the implementation's suffix guard.
    const CASES: [(&str, &str, u8, &str, usize, u8); 15] = [
        ("한", "?", 1, "?", 1, 0),
        ("한", "?", 0, "?", 1, 0),
        ("", "?", 0, "?", 1, 0),
        ("한", "5", 1, "5", 1, 0),
        ("한", ";", 0, ";", 1, 0),
        ("한 ", " ", 0, "", 0, 0),
        (" ", " ", 0, "", 0, 0),
        ("한", "\r", 1, "\r", 1, 0),
        ("한", "\u{7f}", 1, "\u{8}", 1, 0),
        ("日本語", "\r", 2, "", 0, 0),
        ("中文", " ", 2, "", 0, 0),
        ("", "a", 0, "a", 1, 0),
        ("abc", "c", 1, "c", 1, 1),
        ("abc", "c", 1, "c", 1, 2),
        ("abc", "c", 1, "c", 1, 3),
    ];
    #[link(name = "objc")]
    unsafe extern "C" {
        fn class_addMethod(
            cls: *const AnyClass,
            selector: Sel,
            implementation: *const c_void,
            types: *const std::ffi::c_char,
        ) -> bool;
    }
    extern "C" fn interpret(view: *mut AnyObject, _: Sel, _: *mut NSArray) {
        let (commit, trigger, command, _, _, _) = CASES[CASE.load(Ordering::SeqCst)];
        unsafe {
            let _: () = msg_send![view,insertText:&*NSString::from_str(commit),replacementRange:NSRange::new(usize::MAX,0)];
            if command == 1 {
                let _: () = msg_send![view,doCommandBySelector:if trigger=="\u{7f}"{sel!(deleteBackward:)}else{sel!(insertNewline:)}];
            } else if command == 0 {
                let _: () = msg_send![view,insertText:&*NSString::from_str(trigger),replacementRange:NSRange::new(usize::MAX,0)];
            }
        }
    }
    #[derive(Default)]
    struct Probe {
        window: Option<Window>,
        step: usize,
        commits: String,
        typed: String,
        keys: usize,
        started: bool,
    }
    impl Probe {
        fn send(&mut self) {
            let window = self.window.as_ref().unwrap();
            let RawWindowHandle::AppKit(raw) = window.window_handle().unwrap().as_raw() else {
                panic!()
            };
            let view = raw.ns_view.as_ptr().cast::<AnyObject>();
            let (commit, trigger, _, _, _, modifier) = CASES[self.step];
            CASE.store(self.step, Ordering::SeqCst);
            self.commits.clear();
            self.typed.clear();
            self.keys = 0;
            unsafe {
                let _: () = msg_send![view,setMarkedText:&*NSString::from_str(commit),selectedRange:NSRange::new(commit.encode_utf16().count(),0),replacementRange:NSRange::new(usize::MAX,0)];
                let code = match trigger {
                    "?" => 44,
                    "5" => 23,
                    ";" => 41,
                    " " => 49,
                    "\u{7f}" => 51,
                    "a" => 0,
                    "c" => 8,
                    _ => 36,
                };
                let flags = match modifier {
                    1 => NSEventModifierFlags::NSEventModifierFlagControl,
                    2 => NSEventModifierFlags::NSEventModifierFlagCommand,
                    3 => NSEventModifierFlags::NSEventModifierFlagOption,
                    _ if trigger == "?" => NSEventModifierFlags::NSEventModifierFlagShift,
                    _ => NSEventModifierFlags::empty(),
                };
                let event=NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                    NSEventType::KeyDown,NSPoint::new(0.0,0.0),flags,0.0,0,None,&NSString::from_str(trigger),&NSString::from_str(trigger),false,code).unwrap();
                let _: () = msg_send![view,keyDown:&*event];
            }
            self.started = true;
        }
    }
    impl ApplicationHandler for Probe {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }
            let window = event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("Kiln IME callback probe")
                        .with_visible(false),
                )
                .unwrap();
            window.set_ime_allowed(true);
            unsafe {
                let cls = AnyClass::get("WinitView").unwrap();
                assert!(class_addMethod(
                    cls,
                    sel!(interpretKeyEvents:),
                    interpret as *const c_void,
                    b"v@:@\0".as_ptr().cast()
                ));
            }
            self.window = Some(window);
            self.send();
        }
        fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
            match event {
                WindowEvent::Ime(Ime::Commit(s)) => self.commits.push_str(&s),
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == ElementState::Pressed =>
                {
                    self.keys += 1;
                    if let Some(text) = event.text {
                        self.typed.push_str(&text);
                    }
                }
                _ => {}
            }
        }
        fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
            if !self.started || self.step >= CASES.len() {
                return;
            }
            let (commit, _, _, expected_text, expected_keys, _) = CASES[self.step];
            assert_eq!(
                self.commits, commit,
                "case {}: only the composed string is an IME commit",
                self.step
            );
            assert_eq!(
                self.typed, expected_text,
                "case {}: trigger must arrive exactly once",
                self.step
            );
            assert_eq!(
                self.keys, expected_keys,
                "case {}: shortcut/control key events must survive",
                self.step
            );
            self.step += 1;
            if self.step == CASES.len() {
                self.started = false;
                println!("PASS: 15 AppKit commit/trigger callback cases");
                event_loop.exit();
            } else {
                self.send();
            }
        }
    }
    pub fn run() {
        EventLoop::new()
            .unwrap()
            .run_app(&mut Probe::default())
            .unwrap();
    }
}
fn main() {
    #[cfg(target_os = "macos")]
    macos::run();
}
