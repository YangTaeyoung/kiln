//! Read-only task context, kept separate from the draft for the next task.
use super::{KilnApp,ToastKind};
use std::{io::Read,path::Path};
pub(super) struct RequestView { title:String, body:Result<(String,String),String> }
fn read_request(path:&Path,offset:usize)->Result<(String,String),String>{
    let file=std::fs::File::open(path).map_err(|e|kiln_common::trf!("요청 파일을 열 수 없습니다: {e}"))?;
    let mut text=String::new();
    file.take(120_001).read_to_string(&mut text).map_err(|e|kiln_common::trf!("요청을 읽을 수 없습니다: {e}"))?;
    if text.len()>120_000 || offset>text.len() || !text.is_char_boundary(offset){return Err(kiln_common::i18n::tr("저장된 요청 형식을 확인할 수 없습니다.").into());}
    Ok((text[offset..].to_owned(),text[..offset].strip_suffix("\n## 사용자 요청\n").unwrap_or(&text[..offset]).to_owned()))
}
impl KilnApp {
    fn request_view(&self,index:usize)->Option<RequestView>{
        let page=self.workspaces.get(self.active)?.pages.get(index)?;
        Some(RequestView {title:page.title.clone().unwrap_or_else(||kiln_common::i18n::tr("에이전트 요청").into()),body:read_request(page.agent_request.as_deref()?,page.agent_request_offset)})
    }
    pub(super) fn show_agent_request(&mut self,index:usize){self.agent_request_view=self.request_view(index);}
    pub(super) fn copy_agent_request(&mut self,index:usize,ctx:&egui::Context){
        if let Some(view)=self.request_view(index){match view.body {
            Ok((body,_))=>ctx.copy_text(body),
            Err(error)=>self.toast(kiln_common::i18n::tr("요청을 복사할 수 없습니다"),error,ToastKind::Error,None),
        }}
    }
    pub(super) fn ui_agent_request(&mut self,ctx:&egui::Context){
        let Some(view)=&self.agent_request_view else{return;};
        let mut close=false;
        let response=egui::Modal::new(egui::Id::new("agent-original-request")).show(ctx,|ui|{
            ui.set_width((ctx.content_rect().width()-64.0).clamp(240.0,640.0));
            ui.heading(kiln_common::i18n::tr("원래 요청"));
            ui.label(&view.title);
            ui.separator();
            match &view.body {
                Ok((body,context))=>{
                    egui::ScrollArea::vertical().max_height((ctx.content_rect().height()-210.0).max(70.0)).show(ui,|ui|{
                        ui.add(egui::TextEdit::multiline(&mut body.as_str()).desired_width(f32::INFINITY));
                        egui::CollapsingHeader::new(kiln_common::i18n::tr("함께 전달한 작업 공간 정보")).show(ui,|ui|{
                            ui.add(egui::TextEdit::multiline(&mut context.as_str()).desired_width(f32::INFINITY));
                        });
                    });
                }
                Err(error)=>{ui.label(error);}
            }
            ui.horizontal(|ui| {
                if let Ok((body,_))=&view.body {
                    if ui.button(kiln_common::i18n::tr("요청 복사")).clicked(){ctx.copy_text(body.clone());}
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui| {
                    if ui.button(kiln_common::i18n::tr("닫기")).clicked(){close=true;}
                });
            });
        });
        if close || response.should_close(){self.agent_request_view=None;self.focus_terminal=true;}
    }
}
#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn request_reader_separates_exact_unicode_body_and_rejects_bad_or_missing_files(){
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("request.md");
        let prefix="공통 지침\n## 사용자 요청\n";let body="프론트 / 백 변경\n## 사용자 요청\n끝\n\n";
        std::fs::write(&path,format!("{prefix}{body}")).unwrap();
        assert_eq!(read_request(&path,prefix.len()).unwrap(),(body.into(),"공통 지침".into()));
        assert!(read_request(&path,1).is_err());assert!(read_request(&path,usize::MAX).is_err());
        assert!(read_request(&dir.path().join("missing"),0).is_err());
    }
}
