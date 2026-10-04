//! User key bindings. Invalid/colliding bindings never replace a working map.
use egui::{Key, Modifiers, KeyboardShortcut};
use std::collections::BTreeMap;
#[derive(Default)]
pub struct Keymap { bindings: BTreeMap<String,String>, draft: BTreeMap<String,String>, pub error: Option<String>, saved: bool, source: Option<Vec<u8>>, load_failed: bool, reload_confirm: bool, backup: Option<std::path::PathBuf> }
impl Keymap {
    pub fn load() -> Self {
        Self::load_path(&kiln_common::paths::config_file("keybindings.json"))
    }
    pub(super) fn load_path(path:&std::path::Path)->Self {
        match std::fs::read(path) {
            Ok(bytes)=>match serde_json::from_slice::<BTreeMap<String,String>>(&bytes).map_err(|e| e.to_string()).and_then(|bindings| {validate(&bindings)?; Ok(bindings)}) {
                Ok(bindings)=>Self{draft:bindings.clone(),bindings,source:Some(bytes),..Default::default()},
                Err(e)=>Self{error:Some(format!("단축키 파일을 적용하지 않았습니다. 다시 읽거나 원본 백업 후 기본값으로 복구하세요: {} · {e}",path.display())),source:Some(bytes),load_failed:true,..Default::default()}
            },
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>Self::default(),
            Err(e)=>Self{error:Some(format!("단축키 파일을 읽지 못했습니다. 기존 파일을 덮어쓰지 않습니다: {e}")),load_failed:true,..Default::default()}
        }
    }
    fn save_path(&mut self,path:&std::path::Path)->Result<(),String> {
        if self.load_failed { return Err("기존 단축키 파일을 읽거나 검증하지 못해 저장을 중단했습니다. 다시 읽거나 원본 백업 후 기본값으로 복구하세요.".into()); }
        validate(&self.draft)?;
        let current=match std::fs::read(path){Ok(bytes)=>Some(bytes),Err(e) if e.kind()==std::io::ErrorKind::NotFound=>None,Err(e)=>return Err(e.to_string())};
        if current!=self.source{return Err("단축키 파일이 외부에서 변경되었습니다. 덮어쓰지 않았습니다. 다시 읽기로 최신 설정을 불러오세요.".into());}
        kiln_common::store::save_json(path,&self.draft).map_err(|e|e.to_string())?;
        self.source=Some(serde_json::to_vec_pretty(&self.draft).map_err(|e|e.to_string())?);
        self.bindings=self.draft.clone();
        Ok(())
    }
    fn reload_path(&mut self, path: &std::path::Path) {
        let loaded=Self::load_path(path);
        if loaded.load_failed {
            self.error=loaded.error;
            self.source=loaded.source;
            self.load_failed=true;
            self.reload_confirm=false;
        } else { *self=loaded; }
    }
    fn recover_defaults(&mut self, path: &std::path::Path)->Result<(),String> {
        use std::io::Write;
        let bytes=std::fs::read(path).map_err(|e|format!("원본 백업을 위해 파일을 읽지 못했습니다: {e}"))?;
        if self.source.as_ref()!=Some(&bytes) {return Err("파일이 변경되었습니다. 다시 읽은 뒤 복구하세요.".into());}
        let mut backup=None;
        for n in 1..=1000 {
            let candidate=path.with_extension(format!("json.backup-{n}"));
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&candidate) {
                Ok(mut file)=>{
                    file.write_all(&bytes).and_then(|_|file.sync_all()).map_err(|e|format!("백업을 저장하지 못했습니다. 원본은 유지됩니다: {e}"))?;
                    backup=Some(candidate);break;
                }
                Err(e) if e.kind()==std::io::ErrorKind::AlreadyExists=>continue,
                Err(e)=>return Err(format!("백업을 만들지 못했습니다. 원본은 유지됩니다: {e}")),
            }
        }
        let backup=backup.ok_or("백업 이름을 만들지 못했습니다. 원본은 유지됩니다.")?;
        // Recheck after writing the backup, before replacing the source.
        if std::fs::read(path).map_err(|e|e.to_string())?!=bytes {return Err("백업 중 원본이 변경되었습니다. 다시 읽어 주세요.".into());}
        let defaults=BTreeMap::<String,String>::new();
        kiln_common::store::save_json(path,&defaults).map_err(|e|format!("기본값 저장 실패 (백업: {}): {e}",backup.display()))?;
        *self=Self::load_path(path);
        if self.load_failed{return Err("복구한 파일을 다시 읽지 못했습니다.".into());}
        self.saved=true;self.backup=Some(backup);Ok(())
    }
    pub fn has_unsaved_edits(&self)->bool {
        ENTRIES.iter().any(|(id,_,default)| self.draft.get(*id).map(String::as_str).unwrap_or(default) != self.bindings.get(*id).map(String::as_str).unwrap_or(default))
    }
    pub fn label(&self,id:&str,default:&str)->String{self.bindings.get(id).cloned().unwrap_or_else(||default.into())}
    pub fn resolve(&self, id:&str, default:KeyboardShortcut)->KeyboardShortcut { self.bindings.get(id).and_then(|s|parse(s).ok()).unwrap_or(default) }
    pub fn fields_ui(&mut self, ui:&mut egui::Ui) {
        ui.label(if cfg!(target_os="macos") { "예: Cmd+Shift+J" } else { "예: Ctrl+Shift+J" })
            .on_hover_text("앱 탐색용 단축키입니다. 복사·붙여넣기·저장과 터미널 제어 입력은 예약됩니다. F1~F12도 보조 키(Cmd·Ctrl·Alt·Shift)와 함께 사용할 수 있습니다.");
        ui.add_space(4.0);
        let compact=ui.available_width()<260.0;
        let label_width=(ui.available_width()*0.38).clamp(100.0,150.0);
        let edit_width=if compact {ui.available_width().min(240.0)}else{(ui.available_width()-label_width-20.0).clamp(110.0,240.0)};
        egui::Grid::new("keybinding-fields").num_columns(if compact {1}else{2}).spacing(egui::vec2(10.0,6.0)).show(ui,|ui| {
            for (id,label,default) in ENTRIES {
                let label_response=ui.add_sized([if compact {edit_width}else{label_width},24.0],egui::Label::new(*label).truncate()).on_hover_text(*label);
                if compact {ui.end_row();}
                let draft=self.draft.entry((*id).into()).or_insert_with(||(*default).into());
                if ui.add(egui::TextEdit::singleline(draft).id_salt(id).desired_width(edit_width)).labelled_by(label_response.id).changed(){self.saved=false;}
                ui.end_row();
            }
        });
    }
    pub fn footer_height(&self) -> f32 { (if self.load_failed {64.0}else{32.0}) + if self.reload_confirm { 52.0 } else { 0.0 } + if self.error.is_some() { 48.0 } else if self.saved { 22.0 } else { 0.0 } }
    pub fn footer_ui(&mut self, ui:&mut egui::Ui) {
        if self.saved {ui.label("단축키가 저장되었습니다.").on_hover_text(self.backup.as_ref().map(|p|format!("원본 백업: {}",p.display())).unwrap_or_default());}
        if let Some(e)=&self.error { egui::ScrollArea::vertical().id_salt("keybinding-error").max_height(40.0).show(ui,|ui| { ui.colored_label(kiln_common::Theme::current().red,e); }); }
        let mut reload=false;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x=8.0;
            if !self.load_failed && ui.button("단축키 저장").clicked() {
                let result=self.save_path(&kiln_common::paths::config_file("keybindings.json"));
                self.saved=result.is_ok(); self.error=result.err();
            }
            if !self.load_failed {
                ui.menu_button("관리",|ui| {
                    if ui.button("기본값 복원").clicked(){self.saved=false;self.draft=ENTRIES.iter().map(|(id,_,value)|((*id).into(),(*value).into())).collect();ui.close();}
                    if ui.button("다시 읽기").clicked(){reload=true;ui.close();}
                });
            }
            if self.error.is_some() && ui.button("다시 읽기").clicked(){reload=true;}
        });
        if self.load_failed && ui.button("원본 백업 후 기본값 복구").clicked() {
            self.error=self.recover_defaults(&kiln_common::paths::config_file("keybindings.json")).err();
        }
        if reload {
            if self.has_unsaved_edits(){self.reload_confirm=true;}
            else {self.reload_path(&kiln_common::paths::config_file("keybindings.json"));}
        }
        if self.reload_confirm {
            ui.label("편집 중인 단축키를 버리고 다시 읽을까요?");
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x=8.0;
                if ui.button("편집 유지").clicked(){self.reload_confirm=false;}
                if ui.button("버리고 다시 읽기").clicked(){self.reload_path(&kiln_common::paths::config_file("keybindings.json"));}
            });
        }
    }
}
pub const ENTRIES:&[(&str,&str,&str)]=&[
    ("palette","명령 검색",if cfg!(target_os="macos"){"Cmd+K"}else{"Ctrl+Shift+K"}),
    ("recent","최근 작업",if cfg!(target_os="macos"){"Cmd+J"}else{"Ctrl+Shift+J"}),
    ("launchers","저장 명령",if cfg!(target_os="macos"){"Cmd+Shift+J"}else{"Ctrl+Shift+Alt+J"}),
    ("projects","프로젝트 열기",if cfg!(target_os="macos"){"Cmd+N"}else{"Ctrl+Shift+N"}),
    ("files","파일 빠르게 열기",if cfg!(target_os="macos"){"Cmd+P"}else{"Ctrl+Shift+P"}),
    ("sidebar","사이드바",if cfg!(target_os="macos"){"Cmd+B"}else{"Ctrl+Shift+B"}),
    ("new_task","새 터미널 탭",if cfg!(target_os="macos"){"Cmd+T"}else{"Ctrl+Shift+T"}),
    ("split_right","오른쪽 분할",if cfg!(target_os="macos"){"Cmd+D"}else{"Ctrl+Shift+D"}),
    ("split_down","아래로 분할",if cfg!(target_os="macos"){"Cmd+Shift+D"}else{"Ctrl+Shift+Alt+D"}),
    ("close_panel","현재 패널 닫기",if cfg!(target_os="macos"){"Cmd+W"}else{"Ctrl+Shift+W"}),
    ("focus_panel","패널 확대 / 복원",if cfg!(target_os="macos"){"Cmd+Shift+Enter"}else{"Ctrl+Shift+Alt+Enter"}),
    ("equalize","분할 크기 균등하게",if cfg!(target_os="macos"){"Cmd+Alt+Equals"}else{"Ctrl+Alt+Equals"}),
    ("next_task","다음 탭",if cfg!(target_os="macos"){"Cmd+Shift+CloseBracket"}else{"Ctrl+Shift+Alt+CloseBracket"}),
    ("previous_task","이전 탭",if cfg!(target_os="macos"){"Cmd+Shift+OpenBracket"}else{"Ctrl+Shift+Alt+OpenBracket"}),
    ("unread","읽지 않은 알림으로 이동",if cfg!(target_os="macos"){"Cmd+Shift+U"}else{"Ctrl+Shift+Alt+U"}),
];
pub fn parse(s:&str)->Result<KeyboardShortcut,String>{
    let mut modifiers=Modifiers::NONE; let mut key=None;
    for part in s.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str(){
            "cmd"|"command"=> modifiers|=Modifiers::COMMAND,
            "ctrl"|"control"=>modifiers|=Modifiers::CTRL,
            "shift"=>modifiers|=Modifiers::SHIFT,
            "alt"|"option"=>modifiers|=Modifiers::ALT,
            _=>{if key.is_some(){return Err("키는 하나만 지정하세요.".into());} key=Key::from_name(part); if key.is_none(){return Err(format!("인식할 수 없는 키: {part}"));}}
        }
    }
    let key=key.ok_or("키를 입력하세요.")?;
    if !modifiers.command && !modifiers.ctrl && !modifiers.alt && !modifiers.mac_cmd{return Err("Cmd / Ctrl / Alt 등의 수식키를 포함하세요.".into());}
    Ok(KeyboardShortcut::new(modifiers,key))
}
fn canonical(mut key:KeyboardShortcut)->KeyboardShortcut {
    if cfg!(target_os="macos") { key.modifiers.mac_cmd=key.modifiers.command || key.modifiers.mac_cmd; key.modifiers.command=key.modifiers.mac_cmd; }
    else { key.modifiers.ctrl|=key.modifiers.command; key.modifiers.command=key.modifiers.ctrl; }
    key
}
fn reserved(key:KeyboardShortcut)->bool {
    let key=canonical(key); let m=key.modifiers; let k=key.logical_key;
    if cfg!(target_os="macos") && m.ctrl && m.mac_cmd && k==Key::F { return true; }
    // Preserve terminal control sequences and Option/Alt character entry.
    if m.ctrl && !m.mac_cmd && !m.shift && !m.alt { return true; }
    if cfg!(target_os="macos") && m.ctrl && !m.mac_cmd && k.name().len()==1 && k.name().as_bytes()[0].is_ascii_alphabetic() { return true; }
    if m.alt && !m.ctrl && !m.command && !m.mac_cmd { return true; }
    let command=if cfg!(target_os="macos"){m.mac_cmd}else{m.ctrl};
    if command && matches!(k,Key::C|Key::V|Key::X|Key::A|Key::Z|Key::S|Key::Q|Key::Backspace|Key::Delete|Key::Home|Key::End|Key::Enter) { return true; }
    let base=if cfg!(target_os="macos"){m.mac_cmd && !m.ctrl}else{m.ctrl && m.shift};
    if base && matches!(k,Key::T|Key::D|Key::W|Key::F|Key::Equals|Key::Plus|Key::Minus|Key::Num0|Key::Comma|Key::Enter|Key::Num1|Key::Num2|Key::Num3|Key::Num4|Key::Num5|Key::Num6|Key::Num7|Key::Num8|Key::Num9) { return true; }
    let extra=if cfg!(target_os="macos"){m.shift}else{m.alt};
    if base && extra && matches!(k,Key::P|Key::E|Key::L|Key::G|Key::R|Key::B|Key::M|Key::U|Key::OpenBracket|Key::CloseBracket){return true;}
    if command && matches!(k,Key::ArrowLeft|Key::ArrowRight|Key::ArrowUp|Key::ArrowDown){return true;}
    false
}
fn validate(bindings:&BTreeMap<String,String>)->Result<(),String> {
    if let Some(id)=bindings.keys().find(|id|!ENTRIES.iter().any(|(known,_,_)| known==id)) {return Err(format!("알 수 없는 동작: {id}"));}
    let mut seen=Vec::new();
    for (id,label,default) in ENTRIES {
        let parsed=canonical(parse(bindings.get(*id).map(String::as_str).unwrap_or(default))?);
        let own_default=canonical(parse(default)?);
        let other_default=ENTRIES.iter().any(|(other,_,value)| other!=id && parse(value).is_ok_and(|key|canonical(key)==parsed));
        if (reserved(parsed) && parsed!=own_default) || other_default {return Err(format!("{label}: 편집·터미널 입력 또는 기본 작업 단축키와 겹칩니다."));}
        if seen.contains(&parsed){return Err(format!("{label}: 다른 동작과 단축키가 겹칩니다."));}
        seen.push(parsed);
    }
    Ok(())
}
#[cfg(test)] mod tests {
    use super::*;
    #[test]
    fn narrow_320px_editor_uses_two_columns() {
        use egui_kittest::{Harness,kittest::Queryable};
        let mut installed=false;
        let mut h=Harness::builder().with_size([320.0,340.0]).build_ui_state(|ui,map:&mut Keymap|{
            if !installed {kiln_common::fonts::install(ui.ctx());kiln_common::Theme::current().apply(ui.ctx());installed=true;return;}
            egui::ScrollArea::vertical().show(ui,|ui|map.fields_ui(ui));
        },Keymap::default());
        h.run_steps(3);
        let first=h.get_by_label("명령 검색").rect();let second=h.get_by_label("최근 작업").rect();
        assert!(second.top()-first.top()<40.0,"fields should occupy one row each");
        assert!(h.ctx.content_rect().contains_rect(first));
        h.render().unwrap().save("/tmp/kiln-keybindings-320.png").unwrap();
    }
    #[test] fn expanded_actions_keep_defaults_and_reserve_each_others_original_chords(){
        assert_eq!(ENTRIES.len(),15);
        let defaults:BTreeMap<String,String>=ENTRIES.iter().map(|(id,_,value)|((*id).into(),(*value).into())).collect();
        validate(&defaults).unwrap();
        for (id,_,value) in &ENTRIES[6..] {
            let mut changed=defaults.clone();changed.insert((*id).into(),"Cmd+F6".into());
            validate(&changed).unwrap();
            changed.insert("recent".into(),(*value).into());
            assert!(validate(&changed).is_err(),"original binding of {id} must stay reserved");
        }
    }
    #[test] fn corrupt_recovery_backs_up_then_saves_and_can_edit_again(){
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("keys.json");
        std::fs::write(&path,b"broken original").unwrap();
        std::fs::write(path.with_extension("json.backup-1"),b"older backup").unwrap();
        let mut map=Keymap::load_path(&path);map.recover_defaults(&path).unwrap();
        assert_eq!(std::fs::read(map.backup.as_ref().unwrap()).unwrap(),b"broken original");
        assert_eq!(std::fs::read(path.with_extension("json.backup-1")).unwrap(),b"older backup");
        assert!(!map.load_failed);assert!(!map.has_unsaved_edits());assert!(map.saved);
        map.draft.insert("recent".into(),"Cmd+Shift+Y".into());map.save_path(&path).unwrap();
        assert_eq!(Keymap::load_path(&path).bindings["recent"],"Cmd+Shift+Y");
    }
    #[test] fn recovery_requires_current_original_and_reload_retries_in_app(){
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("keys.json");
        std::fs::write(&path,b"bad").unwrap();let mut map=Keymap::load_path(&path);
        std::fs::write(&path,b"new bad").unwrap();assert!(map.recover_defaults(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(),b"new bad");
        map.reload_path(&path);map.recover_defaults(&path).unwrap();
        assert_eq!(std::fs::read(map.backup.as_ref().unwrap()).unwrap(),b"new bad");
        std::fs::write(&path,br#"{"recent":"Cmd+Shift+Y"}"#).unwrap();
        map.reload_path(&path);assert_eq!(map.bindings["recent"],"Cmd+Shift+Y");
        std::fs::write(&path,b"bad again").unwrap();map.reload_path(&path);
        assert_eq!(map.bindings["recent"],"Cmd+Shift+Y");assert!(map.load_failed);
    }
    #[test] fn opening_default_fields_is_clean_but_edits_and_reset_are_dirty(){
        let mut map=Keymap::default();assert!(!map.has_unsaved_edits());
        for (id,_,value) in ENTRIES {map.draft.insert((*id).into(),(*value).into());}
        assert!(!map.has_unsaved_edits());
        map.draft.insert("recent".into(),"Cmd+Shift+Y".into());assert!(map.has_unsaved_edits());
        map.bindings=map.draft.clone();assert!(!map.has_unsaved_edits());
        map.draft.insert("recent".into(),ENTRIES[1].2.into());assert!(map.has_unsaved_edits());
    }
    #[test] fn valid_and_invalid(){assert!(parse("Cmd+Shift+J").is_ok());for value in ["J","Shift+J","Ctrl+Bogus","Ctrl+J+K"] {assert!(parse(value).is_err(),"{value}");}}
    #[test] fn defaults_validate_and_core_input_cannot_be_overridden(){
        assert!(validate(&BTreeMap::new()).is_ok());
        for key in ["Cmd+C","Cmd+V","Cmd+Shift+Z","Ctrl+C","Ctrl+D","Alt+X","Cmd+Shift+Enter","Cmd+Alt+Enter","Ctrl+Cmd+F"] {
            let bindings=BTreeMap::from([("recent".into(),key.into())]); assert!(validate(&bindings).is_err(),"{key}");
        }
        let bindings=BTreeMap::from([("recent".into(),ENTRIES[0].2.into())]); assert!(validate(&bindings).is_err());
    }
    #[test] fn invalid_load_keeps_defaults_and_original_bytes(){
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("keys.json");
        for bytes in [b"broken".as_slice(), br#"{"recent":"Shift+J"}"#, br#"{"recent":"Cmd+C"}"#] {
            std::fs::write(&path,bytes).unwrap();let mut map=Keymap::load_path(&path);
            assert!(map.error.is_some());assert!(map.bindings.is_empty());assert!(map.save_path(&path).is_err());
            assert_eq!(std::fs::read(&path).unwrap(),bytes);
        }
    }
    #[test] fn external_change_is_never_overwritten_and_valid_save_is_reloaded(){
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("keys.json");
        let mut map=Keymap::load_path(&path);map.draft.insert("recent".into(),"Cmd+Shift+Y".into());map.save_path(&path).unwrap();
        let mut loaded=Keymap::load_path(&path);assert_eq!(loaded.bindings["recent"],"Cmd+Shift+Y");
        std::fs::write(&path,b"external").unwrap();assert!(loaded.save_path(&path).is_err());assert_eq!(std::fs::read(&path).unwrap(),b"external");
    }
}
