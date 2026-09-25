//! UI 에 쓰는 기호가 기본 폰트에 있는지 확인한다(없으면 빈 네모로 그려진다).

#[test]
fn ui_symbols_exist_in_default_fonts() {
    let ctx = egui::Context::default();
    kiln::app::fonts::install(&ctx);
    let mut out = ctx.run_ui(Default::default(), |_| {});
    out.textures_delta.clear();
    let used = ['✓', '✗', '✕', '↑', '↓', '×', '●', '…', '⌘', '⇧', '⌥'];
    let missing: Vec<char> = ctx.fonts_mut(|f| used.iter().copied().filter(|c| !f.has_glyph(&egui::FontId::proportional(13.0), *c)).collect());
    let mut out = ctx.run_ui(Default::default(), |_| {});
    out.textures_delta.clear();
    assert!(missing.is_empty(), "missing glyphs: {missing:?}");
}
