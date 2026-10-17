//! A notification-area companion on Windows that survives GUI closure while the daemon is
//! alive. It runs from the daemon's per-build copy of the CLI, so the installed executable
//! stays replaceable, and the newest build replaces an older companion.
//! https://learn.microsoft.com/windows/win32/shell/notification-area
use crate::companion::{self, Status};
use kiln_common::i18n::{self, tr};
use kiln_daemon::client::Client;
use kiln_proto::SessionInfo;
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::Receiver,
    },
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, WAIT_ABANDONED, WAIT_OBJECT_0, WPARAM},
    Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC,
    },
    System::LibraryLoader::GetModuleHandleW,
    System::Threading::{CreateMutexW, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW, WaitForSingleObject},
    UI::Shell::{NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIN_SELECT, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW},
    UI::WindowsAndMessaging::{
        AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow, DispatchMessageW, EnumWindows,
        GetCursorPos, GetIconInfo, GetMessageW, GetSystemMetrics, GetWindowTextLengthW, GetWindowThreadProcessId, HICON, ICONINFO,
        IDI_APPLICATION, IDOK, IMAGE_ICON, IsIconic, LoadIconW, IsWindowVisible, LR_DEFAULTCOLOR, LoadImageW, MB_DEFBUTTON2, MB_ICONWARNING, MB_OK, MB_OKCANCEL, MB_SETFOREGROUND,
        MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG, MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SM_CXSMICON,
        SM_CYSMICON, SW_RESTORE, SetForegroundWindow, SetTimer, ShowWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage,
        WM_APP, WM_CONTEXTMENU, WM_DESTROY, WM_NULL, WM_TIMER, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
    },
};

const CALLBACK: u32 = WM_APP + 1;
/// `NIN_SELECT | NINF_KEY`: the icon chosen with the keyboard (shellapi.h).
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
const OPEN: usize = 1;
const SETTINGS: usize = 2;
const STOP: usize = 3;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// `text` in a fixed UTF-16 field, cut to fit with its terminating NUL.
fn fill<const N: usize>(field: &mut [u16; N], text: &str) {
    let units: Vec<u16> = text.encode_utf16().take(N - 1).collect();
    field[..units.len()].copy_from_slice(&units);
    field[units.len()] = 0;
}

/// The build ID in a per-build copy's name (`kiln-daemon-<build>.exe`), else this executable's.
fn own_build() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.file_stem().and_then(|s| s.to_str()).and_then(|s| s.strip_prefix("kiln-daemon-")).map(str::to_owned))
        .unwrap_or_else(|| kiln_daemon::build_id().to_owned())
}

fn wanted_build_path() -> PathBuf {
    kiln_common::paths::config_file("tray-build")
}

/// Starts the companion for the default daemon from the per-build copy of the CLI, hidden and
/// outside the caller's job. Test fixtures and alternate sockets get none.
pub fn ensure_running() {
    if std::env::var_os("KILN_NO_AUTO_UPGRADE").is_some() || std::env::var_os("KILN_SOCKET").is_some() {
        return;
    }
    let Ok(current) = std::env::current_exe() else { return };
    let cli = kiln_daemon::client::cli_executable(&current);
    let Some(copy) = kiln_daemon::client::daemon_copy(&cli) else { return };
    let gui = gui_beside(&cli).or_else(|| current.file_name().is_some_and(|n| n.eq_ignore_ascii_case("Kiln.exe")).then(|| current.clone()));
    let _ = std::fs::write(wanted_build_path(), kiln_daemon::exe_build_id(&cli));
    let mut cmd = std::process::Command::new(copy);
    cmd.arg("status-bar");
    if let Some(gui) = gui {
        cmd.arg("--gui").arg(gui);
    }
    let _ = kiln_daemon::client::spawn_detached(&mut cmd);
}

/// The GUI of an installed layout: `Kiln.exe` beside the `bin` folder of the CLI.
fn gui_beside(cli: &Path) -> Option<PathBuf> {
    let dir = cli.parent()?;
    let gui = dir.parent()?.join("Kiln.exe");
    (dir.file_name()? == "bin" && gui.is_file()).then_some(gui)
}

enum StopResult {
    Snapshot(anyhow::Result<(Client, Vec<SessionInfo>)>),
    Finished(anyhow::Result<()>),
}

struct Tray {
    window: HWND,
    normal: HICON,
    attention: HICON,
    shown: Option<(bool, String)>,
    status: Arc<Mutex<Status>>,
    gui: Option<PathBuf>,
    build: String,
    stopping: Option<Receiver<StopResult>>,
    taskbar_created: u32,
}

thread_local! { static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) }; }
static STOPPING: AtomicBool = AtomicBool::new(false);

impl Tray {
    fn icon_data(&self) -> NOTIFYICONDATAW {
        // SAFETY: an all-zero NOTIFYICONDATAW is valid; the size and identity are set below.
        let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = self.window;
        data.uID = 1;
        data
    }

    fn add_icon(&mut self) {
        let mut data = self.icon_data();
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        data.uCallbackMessage = CALLBACK;
        data.hIcon = self.normal;
        fill(&mut data.szTip, tr("Kiln에 연결하는 중…"));
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        // SAFETY: `data` describes this companion's icon and window.
        unsafe {
            Shell_NotifyIconW(NIM_ADD, &data);
            Shell_NotifyIconW(NIM_SETVERSION, &data);
        }
        self.shown = None;
    }

    fn refresh(&mut self, status: &Status) {
        let attention = !status.title().is_empty();
        let tip = kiln_common::trf!("{} · 읽지 않은 알림 {}개", status.description(), companion::persisted_unread());
        if self.shown.as_ref() == Some(&(attention, tip.clone())) {
            return;
        }
        let mut data = self.icon_data();
        data.uFlags = NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        data.hIcon = if attention && !self.attention.is_null() { self.attention } else { self.normal };
        fill(&mut data.szTip, &tip);
        // SAFETY: `data` describes this companion's existing icon.
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
        self.shown = Some((attention, tip));
    }
}

/// The app icon from the executable's resources at the small-icon size, or the system's
/// application icon when the executable has none.
fn app_icon() -> HICON {
    // SAFETY: resource 1 is the icon the build script embeds; a missing icon yields null.
    unsafe {
        let (cx, cy) = (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
        let icon = LoadImageW(GetModuleHandleW(std::ptr::null()), 1usize as *const u16, IMAGE_ICON, cx, cy, LR_DEFAULTCOLOR) as HICON;
        if icon.is_null() { LoadIconW(std::ptr::null_mut(), IDI_APPLICATION) } else { icon }
    }
}

/// `icon` with a red dot in its upper-right corner.
fn with_dot(icon: HICON) -> HICON {
    if icon.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: every GDI object is checked, used within this function and deleted once; the
    // pixel buffer matches the 32-bit top-down DIB requested from GetDIBits.
    unsafe {
        let mut info: ICONINFO = std::mem::zeroed();
        if GetIconInfo(icon, &mut info) == 0 || info.hbmColor.is_null() {
            return std::ptr::null_mut();
        }
        let mut bitmap: BITMAP = std::mem::zeroed();
        GetObjectW(info.hbmColor, std::mem::size_of::<BITMAP>() as i32, (&mut bitmap as *mut BITMAP).cast());
        let (width, height) = (bitmap.bmWidth, bitmap.bmHeight);
        let mut header: BITMAPINFO = std::mem::zeroed();
        header.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let screen = GetDC(std::ptr::null_mut());
        let mut pixels = vec![0u32; (width * height) as usize];
        let read = GetDIBits(screen, info.hbmColor, 0, height as u32, pixels.as_mut_ptr().cast(), &mut header, DIB_RGB_COLORS);
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let color = CreateDIBSection(screen, &header, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        ReleaseDC(std::ptr::null_mut(), screen);
        let mut result = std::ptr::null_mut();
        if width > 0 && height > 0 && read != 0 && !color.is_null() && !bits.is_null() {
            let radius = width as f32 * 0.22;
            let (cx, cy) = (width as f32 - radius - 0.5, radius + 0.5);
            for y in 0..height {
                for x in 0..width {
                    let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                    if dx * dx + dy * dy <= radius * radius {
                        // Opaque #FF3B30 in BGRA.
                        pixels[(y * width + x) as usize] = 0xFF_FF3B30;
                    }
                }
            }
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u32>(), pixels.len());
            let dotted = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: info.hbmMask, hbmColor: color };
            result = CreateIconIndirect(&dotted);
        }
        if !color.is_null() {
            DeleteObject(color);
        }
        DeleteObject(info.hbmColor);
        DeleteObject(info.hbmMask);
        result
    }
}

fn message_box(text: &str, caption: &str, flags: u32) -> i32 {
    let window = TRAY.with(|t| t.borrow().as_ref().map(|t| t.window)).unwrap_or(std::ptr::null_mut());
    // SAFETY: both strings are NUL-terminated for the call.
    unsafe { MessageBoxW(window, wide(text).as_ptr(), wide(caption).as_ptr(), flags | MB_SETFOREGROUND) }
}

fn show_error(title: &str, message: &str) {
    message_box(message, title, MB_OK | MB_ICONWARNING);
}

/// The executable path of process `pid`.
fn image_of(pid: u32) -> Option<PathBuf> {
    // SAFETY: the handle is checked and closed; the buffer length is passed in and out.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(process, 0, buf.as_mut_ptr(), &mut len) != 0;
        CloseHandle(process);
        use std::os::windows::ffi::OsStringExt;
        ok.then(|| std::ffi::OsString::from_wide(&buf[..len as usize]).into())
    }
}

/// A visible, titled top-level window of the Kiln GUI, if one is open.
fn gui_window(gui: Option<&Path>) -> Option<HWND> {
    struct Search<'a> {
        gui: Option<&'a Path>,
        found: HWND,
    }
    unsafe extern "system" fn visit(window: HWND, param: LPARAM) -> i32 {
        // SAFETY: `param` is the Search passed to EnumWindows below, alive for the call.
        let search = unsafe { &mut *(param as *mut Search) };
        let mut pid = 0u32;
        // SAFETY: `window` comes from EnumWindows.
        let shown = unsafe { (IsWindowVisible(window) != 0 || IsIconic(window) != 0) && GetWindowTextLengthW(window) > 0 };
        if !shown {
            return 1;
        }
        // SAFETY: as above.
        unsafe { GetWindowThreadProcessId(window, &mut pid) };
        let matches = image_of(pid).is_some_and(|image| match search.gui {
            Some(gui) => image.as_os_str().eq_ignore_ascii_case(gui.as_os_str()),
            None => image.file_name().is_some_and(|n| n.eq_ignore_ascii_case("Kiln.exe")),
        });
        if matches {
            search.found = window;
            return 0;
        }
        1
    }
    let mut search = Search { gui, found: std::ptr::null_mut() };
    // SAFETY: the callback only reads windows and writes `search` for this call.
    unsafe { EnumWindows(Some(visit), &mut search as *mut Search as LPARAM) };
    (!search.found.is_null()).then_some(search.found)
}

static GUI_LAUNCHING: AtomicBool = AtomicBool::new(false);

fn show_window() -> std::io::Result<()> {
    let gui = TRAY.with(|t| t.borrow().as_ref().and_then(|t| t.gui.clone()));
    if let Some(window) = gui_window(gui.as_deref()) {
        // SAFETY: `window` is a live top-level window; the companion is in the foreground
        // after its own menu or dialog, so it may hand the foreground on.
        unsafe {
            if IsIconic(window) != 0 {
                ShowWindow(window, SW_RESTORE);
            }
            SetForegroundWindow(window);
        }
        return Ok(());
    }
    let gui = gui.ok_or_else(|| std::io::Error::other(tr("Kiln 실행 파일을 찾지 못했습니다.")))?;
    if GUI_LAUNCHING.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let result = std::process::Command::new(gui).spawn();
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

fn start_stop() {
    if STOPPING.swap(true, Ordering::SeqCst) {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    TRAY.with(|t| {
        if let Some(t) = t.borrow_mut().as_mut() {
            t.stopping = Some(rx);
        }
    });
    std::thread::spawn(move || {
        let _ = tx.send(StopResult::Snapshot(companion::running_sessions()));
    });
}

fn poll_stop() {
    let result = TRAY.with(|t| t.borrow().as_ref().and_then(|t| t.stopping.as_ref()).and_then(|rx| rx.try_recv().ok()));
    let Some(result) = result else { return };
    TRAY.with(|t| {
        if let Some(t) = t.borrow_mut().as_mut() {
            t.stopping = None;
        }
    });
    match result {
        StopResult::Snapshot(Ok((client, sessions))) if !sessions.is_empty() => {
            let (title, message) = companion::stop_message(&sessions);
            // Cancel is the default button; the destructive choice needs a deliberate click.
            if message_box(&message, &title, MB_OKCANCEL | MB_ICONWARNING | MB_DEFBUTTON2) == IDOK {
                let (tx, rx) = std::sync::mpsc::channel();
                TRAY.with(|t| {
                    if let Some(t) = t.borrow_mut().as_mut() {
                        t.stopping = Some(rx);
                    }
                });
                std::thread::spawn(move || {
                    let _ = tx.send(StopResult::Finished(companion::stop_reviewed(&client, &sessions)));
                });
                return;
            }
        }
        StopResult::Snapshot(Ok(_)) => show_error(tr("실행 중인 세션이 없습니다"), tr("종료할 백그라운드 세션이 없습니다.")),
        StopResult::Snapshot(Err(error)) | StopResult::Finished(Err(error)) => show_error(tr("세션 종료를 완료하지 못했습니다"), &error.to_string()),
        StopResult::Finished(Ok(())) => {}
    }
    STOPPING.store(false, Ordering::SeqCst);
}

fn show_menu(window: HWND) {
    let status = TRAY.with(|t| t.borrow().as_ref().map(|t| t.status.lock().unwrap_or_else(|e| e.into_inner()).clone())).unwrap_or(Status::Connecting);
    let busy = STOPPING.load(Ordering::SeqCst);
    // SAFETY: the menu is created, shown and destroyed here; strings live across each call.
    let command = unsafe {
        let menu = CreatePopupMenu();
        let item = |flags: u32, id: usize, text: &str| {
            AppendMenuW(menu, flags, id, wide(text).as_ptr());
        };
        item(MF_STRING | MF_GRAYED, 0, &status.description());
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        item(MF_STRING, OPEN, tr("Kiln 열기"));
        item(MF_STRING, SETTINGS, tr("설정…"));
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        let stop_flags = if !busy && status.running() > 0 { MF_STRING } else { MF_STRING | MF_GRAYED };
        item(stop_flags, STOP, if busy { tr("세션 확인 중…") } else { tr("백그라운드 세션 종료…") });
        item(MF_STRING | MF_GRAYED, 0, tr("창을 닫아도 세션은 유지됩니다"));
        let mut point = POINT { x: 0, y: 0 };
        GetCursorPos(&mut point);
        // The owner must be in the foreground for the menu to close when it loses focus.
        SetForegroundWindow(window);
        let command = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, point.x, point.y, 0, window, std::ptr::null());
        PostMessageW(window, WM_NULL, 0, 0);
        DestroyMenu(menu);
        command as usize
    };
    match command {
        OPEN => {
            if let Err(error) = show_window() {
                show_error(tr("Kiln을 열지 못했습니다"), &error.to_string());
            }
        }
        SETTINGS => {
            if let Err(error) = crate::native_actions::request_settings_and_launch(show_window) {
                show_error(tr("설정을 열지 못했습니다"), &error.to_string());
            }
        }
        STOP => start_stop(),
        _ => {}
    }
}

fn tick() {
    i18n::sync_language();
    let state = TRAY.with(|t| t.borrow().as_ref().map(|t| (t.status.lock().unwrap_or_else(|e| e.into_inner()).clone(), t.build.clone(), t.window)));
    let Some((status, build, window)) = state else { return };
    // A newer build's companion takes over; this one leaves the notification area to it.
    let superseded = std::fs::read_to_string(wanted_build_path()).is_ok_and(|wanted| wanted.trim() != build);
    if status == Status::Disconnected || superseded {
        // SAFETY: `window` is this companion's window.
        unsafe { DestroyWindow(window) };
        return;
    }
    TRAY.with(|t| {
        if let Some(t) = t.borrow_mut().as_mut() {
            t.refresh(&status);
        }
    });
    poll_stop();
}

unsafe extern "system" fn window_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let taskbar_created = TRAY.with(|t| t.borrow().as_ref().map(|t| t.taskbar_created)).unwrap_or(0);
    match message {
        WM_TIMER => tick(),
        CALLBACK => {
            // NOTIFYICON_VERSION_4 puts the event in the low word of lParam.
            let event = (lparam & 0xFFFF) as u32;
            if matches!(event, WM_CONTEXTMENU | NIN_SELECT | NIN_KEYSELECT) {
                show_menu(window);
            }
        }
        WM_DESTROY => {
            TRAY.with(|t| {
                if let Some(t) = t.borrow().as_ref() {
                    let data = t.icon_data();
                    // SAFETY: removes this companion's icon.
                    unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
                }
            });
            // SAFETY: ends this thread's message loop.
            unsafe { PostQuitMessage(0) };
        }
        _ if message != 0 && message == taskbar_created => {
            // Explorer restarted; the notification area forgot every icon.
            TRAY.with(|t| {
                if let Some(t) = t.borrow_mut().as_mut() {
                    t.add_icon();
                }
            });
        }
        _ => {}
    }
    // SAFETY: default handling for every message.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

/// The single-instance lock for `socket`: owned by this process, or by an older build that
/// leaves within a few seconds after a newer build asks for the notification area.
fn claim(socket: &str) -> bool {
    let hash = socket.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    let name = wide(&format!("Local\\kiln-tray-{hash:016x}"));
    // SAFETY: the mutex handle is kept for the life of the process.
    unsafe {
        let mutex = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
        if mutex.is_null() {
            return false;
        }
        if GetLastError() != ERROR_ALREADY_EXISTS {
            return true;
        }
        matches!(WaitForSingleObject(mutex, 5000), WAIT_OBJECT_0 | WAIT_ABANDONED)
    }
}

/// The companion's window, which owns the notification-area icon and runs on this thread.
fn create_window() -> HWND {
    let class = wide("KilnTrayWindow");
    // SAFETY: the class is registered once per process (a second registration fails
    // harmlessly) and the window is created on the calling thread.
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSW { lpfnWndProc: Some(window_proc), hInstance: instance, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        RegisterClassW(&wc);
        // A hidden top-level tool window: it hears the TaskbarCreated broadcast that a
        // message-only window would miss, and never appears on the taskbar.
        CreateWindowExW(WS_EX_TOOLWINDOW, class.as_ptr(), wide("Kiln").as_ptr(), WS_POPUP, 0, 0, 0, 0, std::ptr::null_mut(), std::ptr::null_mut(), instance, std::ptr::null())
    }
}

pub fn run(gui: Option<PathBuf>) -> anyhow::Result<()> {
    i18n::set_language(i18n::load_language());
    let socket = kiln_proto::socket_name();
    if !claim(&socket) {
        return Ok(());
    }
    let status = Arc::new(Mutex::new(Status::Connecting));
    let background = status.clone();
    std::thread::spawn(move || companion::watch(socket, background));
    let window = create_window();
    anyhow::ensure!(!window.is_null(), "notification-area window: {}", std::io::Error::last_os_error());
    let normal = app_icon();
    let mut tray = Tray {
        window,
        normal,
        attention: with_dot(normal),
        shown: None,
        status,
        gui,
        build: own_build(),
        stopping: None,
        // SAFETY: the string is NUL-terminated.
        taskbar_created: unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) },
    };
    tray.add_icon();
    TRAY.with(|t| *t.borrow_mut() = Some(tray));
    // SAFETY: a one-second timer on this companion's window; the loop ends at WM_QUIT.
    unsafe {
        SetTimer(window, 1, 1000, None);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_attention_icon_is_a_new_icon_from_the_app_icon() {
        let normal = app_icon();
        assert!(!normal.is_null());
        let dotted = with_dot(normal);
        assert!(!dotted.is_null() && dotted != normal);
    }

    #[test]
    fn the_companion_window_is_a_top_level_window_that_hears_broadcasts() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GA_PARENT, GetAncestor, GetDesktopWindow, IsWindowVisible};
        let window = create_window();
        assert!(!window.is_null());
        // SAFETY: `window` was just created on this thread.
        unsafe {
            assert_eq!(GetAncestor(window, GA_PARENT), GetDesktopWindow(), "message-only windows miss TaskbarCreated");
            assert_eq!(IsWindowVisible(window), 0);
            DestroyWindow(window);
        }
    }

    #[test]
    fn tooltips_are_cut_to_the_field_with_a_terminator() {
        let mut field = [1u16; 8];
        fill(&mut field, "Kiln 연결이 끊겼습니다");
        assert_eq!(field[7], 0);
        assert_eq!(String::from_utf16_lossy(&field[..7]), "Kiln 연결");
    }

    #[test]
    fn the_gui_is_found_beside_the_bin_folder_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("bin")).unwrap();
        std::fs::write(dir.path().join("Kiln.exe"), b"").unwrap();
        assert_eq!(gui_beside(&dir.path().join("bin").join("kiln.exe")), Some(dir.path().join("Kiln.exe")));
        assert_eq!(gui_beside(&dir.path().join("kiln.exe")), None);
    }
}
