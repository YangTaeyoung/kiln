//! Real AppKit launch/open callbacks on the process main thread. No Kiln GUI,
//! session daemon, user configuration, installed bundle, or default association.
#[cfg(target_os = "macos")]
#[path = "../src/app/macos/open_files.rs"]
mod open_files;

#[cfg(target_os = "macos")]
mod native {
    use super::open_files;
    use objc2::{
        msg_send,
        runtime::{AnyObject, Bool},
    };
    use objc2_app_kit::NSApplication;
    use objc2_foundation::{MainThreadMarker, NSArray, NSString};
    use std::path::PathBuf;
    use winit::{
        application::ApplicationHandler,
        event::WindowEvent,
        event_loop::{ActiveEventLoop, EventLoop},
        platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS},
        window::WindowId,
    };

    struct Probe {
        root: PathBuf,
        passed: bool,
    }

    impl ApplicationHandler for Probe {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            // Assertions must unwind inside Rust, never across AppKit's launch ABI.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let cold = self.root.join("README");
                assert!(
                    open_files::has_open_requests(),
                    "cold requests must survive without an egui context"
                );
                let requests = open_files::take_open_requests();
                assert_eq!(
                    requests
                        .iter()
                        .flat_map(|r| r.paths.clone())
                        .collect::<Vec<_>>(),
                    [cold.clone()],
                    "AppKit must deliver NSOpen before winit's resumed/app creation callback"
                );
                for request in &requests {
                    open_files::finish_open_request(request, true);
                }
                let mt = MainThreadMarker::new().unwrap();
                let app = NSApplication::sharedApplication(mt);
                let delegate = unsafe { app.delegate() }.unwrap();
                let same_delegate = std::ptr::from_ref(&*delegate).cast::<AnyObject>();
                let paths = [
                    self.root.join(".env"),
                    self.root.join("한글 source.rs"),
                    self.root.join("folder"),
                ];
                let names = paths
                    .iter()
                    .map(|p| NSString::from_str(p.to_str().unwrap()))
                    .collect::<Vec<_>>();
                let array = NSArray::from_vec(names);
                // A native multi-path request arriving before GUI creation also
                // survives without a context; no startup-only single-path shortcut.
                unsafe {
                    let _: () = msg_send![&*delegate, application:&*app, openFiles:&*array];
                }
                let early = open_files::take_open_requests();
                assert_eq!(early.len(), 1);
                assert_eq!(early[0].paths, paths);
                assert!(early[0].needs_reply);
                open_files::finish_open_request(&early[0], true);
                let ctx = egui::Context::default();
                open_files::attach_open_file_context(&ctx);
                unsafe {
                    let _: () = msg_send![&*delegate, application:&*app, openFiles:&*array];
                    let accepted: Bool = msg_send![&*delegate, application:&*app, openFile:&*NSString::from_str(cold.to_str().unwrap())];
                    assert!(accepted.as_bool());
                }
                assert!(open_files::has_open_requests());
                let mut output = ctx.run_ui(Default::default(), |_| {});
                output.textures_delta.clear();
                let commands = &output.viewport_output[&egui::ViewportId::ROOT].commands;
                assert!(
                    commands
                        .iter()
                        .any(|cmd| matches!(cmd, egui::ViewportCommand::Focus))
                );
                assert!(
                    commands
                        .iter()
                        .any(|cmd| matches!(cmd, egui::ViewportCommand::Visible(true)))
                );
                assert!(
                    commands
                        .iter()
                        .any(|cmd| matches!(cmd, egui::ViewportCommand::Minimized(false)))
                );
                let requests = open_files::take_open_requests();
                assert_eq!(requests.len(), 2);
                assert_eq!(requests[0].paths, paths);
                assert!(requests[0].needs_reply);
                assert_eq!(requests[1].paths, [cold]);
                assert!(!requests[1].needs_reply);
                for request in &requests {
                    open_files::finish_open_request(request, true);
                }
                assert!(
                    open_files::take_open_requests().is_empty(),
                    "requests drain exactly once"
                );
                let after = unsafe { app.delegate() }.unwrap();
                assert_eq!(
                    same_delegate,
                    std::ptr::from_ref(&*after).cast::<AnyObject>(),
                    "preserve the winit delegate"
                );
            }));
            self.passed = result.is_ok();
            event_loop.exit();
        }
        fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
    }

    pub fn run() {
        let Some(root) = std::env::var_os("KILN_NATIVE_OPEN_FIXTURE") else {
            let dir = tempfile::tempdir().unwrap();
            for file in ["README", ".env", "한글 source.rs"] {
                std::fs::write(dir.path().join(file), "fixture text\n").unwrap();
            }
            std::fs::create_dir(dir.path().join("folder")).unwrap();
            // NSApplication.finishLaunching opens NSOpen before didFinishLaunching.
            // The NSOpen argument belongs to this short-lived subprocess only.
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("-NSOpen")
                .arg(dir.path().join("README"))
                .env("KILN_NATIVE_OPEN_FIXTURE", dir.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            print!("{}", String::from_utf8_lossy(&output.stdout));
            return;
        };
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(20));
            eprintln!("FAIL: AppKit open-file callback probe timed out");
            std::process::exit(1);
        });
        open_files::prepare_open_file_events();
        let mut builder = EventLoop::builder();
        builder
            .with_activation_policy(ActivationPolicy::Prohibited)
            .with_default_menu(false)
            .with_activate_ignoring_other_apps(false);
        let event_loop = builder.build().unwrap();
        let mut probe = Probe {
            root: root.into(),
            passed: false,
        };
        event_loop.run_app(&mut probe).unwrap();
        assert!(probe.passed);
        println!(
            "PASS: AppKit cold NSOpen, early/running multi-file/folder, repeated single and root-focus callbacks"
        );
    }
}

fn main() {
    #[cfg(target_os = "macos")]
    native::run();
}
