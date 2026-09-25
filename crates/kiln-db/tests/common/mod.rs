//! UI 테스트 공용 도우미.
#![allow(dead_code)]

/// 한글 글리프가 있는 시스템 폰트를 Proportional·Monospace 대체 폰트로 컨텍스트마다 한 번 설치한다.
pub fn install_korean_font(ctx: &egui::Context) {
    use std::sync::{Arc, OnceLock};
    const PATH: &str = "/System/Library/Fonts/AppleSDGothicNeo.ttc";
    static BYTES: OnceLock<Option<&'static [u8]>> = OnceLock::new();
    let flag = egui::Id::new("kiln-test-korean-font");
    if ctx.data(|d| d.get_temp::<bool>(flag)).unwrap_or(false) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(flag, true));
    let bytes = *BYTES.get_or_init(|| std::fs::read(PATH).ok().map(|b| &*Box::leak(b.into_boxed_slice())));
    let Some(bytes) = bytes else { return };
    let mut defs = egui::FontDefinitions::default();
    let mut fd = egui::FontData::from_static(bytes);
    fd.index = 0;
    defs.font_data.insert("korean".to_owned(), Arc::new(fd));
    for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        defs.families.entry(fam).or_default().push("korean".to_owned());
    }
    ctx.set_fonts(defs);
}
