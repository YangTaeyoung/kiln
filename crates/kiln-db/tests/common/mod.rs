//! UI 테스트 공용 도우미.
#![allow(dead_code)]

/// 한글 글리프가 있는 시스템 폰트를 Proportional·Monospace 대체 폰트로 컨텍스트마다 한 번 설치한다.
pub fn install_korean_font(ctx: &egui::Context) {
    use std::sync::{Arc, OnceLock};
    // (경로, ttc 안의 글꼴 번호). OS 별로 먼저 찾은 것을 쓴다.
    const CANDIDATES: &[(&str, u32)] = &[
        ("/System/Library/Fonts/AppleSDGothicNeo.ttc", 0),
        ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 1),
        ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 1),
        ("C:\\Windows\\Fonts\\malgun.ttf", 0),
    ];
    static BYTES: OnceLock<Option<(&'static [u8], u32)>> = OnceLock::new();
    let flag = egui::Id::new("kiln-test-korean-font");
    if ctx.data(|d| d.get_temp::<bool>(flag)).unwrap_or(false) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(flag, true));
    let found = *BYTES.get_or_init(|| CANDIDATES.iter().find_map(|(p, i)| std::fs::read(p).ok().map(|b| (&*Box::leak(b.into_boxed_slice()), *i))));
    let Some((bytes, index)) = found else { return };
    let mut defs = egui::FontDefinitions::default();
    let mut fd = egui::FontData::from_static(bytes);
    fd.index = index;
    defs.font_data.insert("korean".to_owned(), Arc::new(fd));
    for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        defs.families.entry(fam).or_default().push("korean".to_owned());
    }
    ctx.set_fonts(defs);
}
