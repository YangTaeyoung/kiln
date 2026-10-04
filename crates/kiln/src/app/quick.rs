//! A separate, persistent terminal surface summoned without replacing the main layout.
use global_hotkey::{GlobalHotKeyManager, GlobalHotKeyEvent, HotKeyState, hotkey::{HotKey, Modifiers, Code}};
use std::sync::{Arc,atomic::{AtomicBool,Ordering}};
#[derive(Default)]
pub struct QuickTerminal {
    pub open: bool,
    pub focus_pending: bool,
    pub pane: Option<u64>,
    pub view: Option<super::terminal::TermView>,
    manager: Option<GlobalHotKeyManager>,
    pressed: Arc<AtomicBool>,
    pub error: Option<String>,
}
impl QuickTerminal {
    pub fn register(&mut self, ctx: &egui::Context) {
        let result=(||->Result<GlobalHotKeyManager,String>{
            let manager=GlobalHotKeyManager::new().map_err(|e|e.to_string())?;
            let key=HotKey::new(Some(Modifiers::CONTROL),Code::Backquote);
            manager.register(key).map_err(|e|e.to_string())?;
            let pressed=self.pressed.clone(); let ctx=ctx.clone(); let id=key.id();
            GlobalHotKeyEvent::set_event_handler(Some(move |event:GlobalHotKeyEvent|{
                if event.id==id && event.state==HotKeyState::Pressed { pressed.store(true,Ordering::SeqCst); ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true)); ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false)); ctx.request_repaint(); }
            }));
            Ok(manager)
        })();
        match result { Ok(manager)=>self.manager=Some(manager), Err(e)=>self.error=Some(kiln_common::trf!("빠른 터미널 전역 단축키 Ctrl+` 등록 실패: {e}")) }
    }
    pub fn poll(&mut self) { if self.pressed.swap(false,Ordering::SeqCst){self.open=!self.open;self.focus_pending=self.open;} }
}
