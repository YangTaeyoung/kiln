//! Bundled UI, monospace and regional CJK fonts, with optional system symbol fallbacks.

use crate::i18n::{self, Language};
use egui::{FontData, FontDefinitions, FontFamily, FontId};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const MEDIUM: &str = "medium";
pub const SEMIBOLD: &str = "semibold";
pub const MONO_BOLD: &str = "mono-bold";

static PRETENDARD: &[u8] = include_bytes!("../fonts/Pretendard-Regular.otf");
static PRETENDARD_MEDIUM: &[u8] = include_bytes!("../fonts/Pretendard-Medium.otf");
static PRETENDARD_SEMIBOLD: &[u8] = include_bytes!("../fonts/Pretendard-SemiBold.otf");
static JBM: &[u8] = include_bytes!("../fonts/JetBrainsMono-Regular.ttf");
static JBM_BOLD: &[u8] = include_bytes!("../fonts/JetBrainsMono-Bold.ttf");
// Upstream collection shares outlines between regional faces. See fonts/README.md.
static CJK: &[u8] = include_bytes!("../fonts/NotoSansCJK-Regular.ttc");

pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(MEDIUM.into()))
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

pub fn regular(size: f32) -> FontId {
    FontId::proportional(size)
}

pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

fn symbol_candidates() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    #[cfg(target_os = "macos")]
    {
        v.push("/System/Library/Fonts/Apple Symbols.ttf".into());
    }
    #[cfg(target_os = "windows")]
    {
        v.push("C:\\Windows\\Fonts\\seguisym.ttf".into());
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        v.push("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf".into());
        v.push("/usr/share/fonts/dejavu/DejaVuSans.ttf".into());
    }
    v
}

fn find_nerd_font() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home = PathBuf::from(home);
        dirs.push(home.join("Library/Fonts"));
        dirs.push(home.join(".local/share/fonts"));
        dirs.push(home.join(".fonts"));
        dirs.push(home.join("AppData/Local/Microsoft/Windows/Fonts"));
    }
    dirs.push("/Library/Fonts".into());
    dirs.push("/usr/share/fonts".into());
    dirs.into_iter().find_map(|d| scan_for(&d, 2))
}

fn scan_for(dir: &Path, depth: u32) -> Option<PathBuf> {
    let rd = std::fs::read_dir(dir).ok()?;
    let mut subdirs = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            subdirs.push(p);
            continue;
        }
        let name = p.file_name()?.to_string_lossy().to_lowercase();
        if name.contains("nerdfont") && name.contains("mono") && name.contains("regular") && (name.ends_with(".ttf") || name.ends_with(".otf")) {
            return Some(p);
        }
    }
    if depth > 0 {
        subdirs.into_iter().find_map(|s| scan_for(&s, depth - 1))
    } else {
        None
    }
}

/// 글꼴 정의를 만든다. 가족: Proportional(Pretendard), `medium`, `semibold`, Monospace(JetBrains Mono), `mono-bold`.
/// Each family includes bundled CJK coverage, even without system fonts.
pub fn definitions(system_fallbacks: bool) -> FontDefinitions {
    definitions_for_language(system_fallbacks, i18n::language())
}

/// Build fonts for an explicit language without changing the application language.
pub fn definitions_for_language(system_fallbacks: bool, language: Language) -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    let egui_fallbacks: Vec<String> = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut put = |name: &str, bytes: &'static [u8]| {
        defs.font_data.insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    };
    put("pretendard", PRETENDARD);
    put("pretendard-medium", PRETENDARD_MEDIUM);
    put("pretendard-semibold", PRETENDARD_SEMIBOLD);
    put("jbm", JBM);
    put("jbm-bold", JBM_BOLD);
    let cjk_index = match language {
        Language::Japanese => 0,
        Language::ChineseSimplified => 2,
        Language::Korean | Language::English => 1,
    };
    let mut cjk = FontData::from_static(CJK);
    cjk.index = cjk_index;
    defs.font_data.insert("cjk".into(), Arc::new(cjk));

    let mut extra: Vec<String> = Vec::new();
    if system_fallbacks {
        if let Some(p) = find_nerd_font() {
            if let Ok(b) = std::fs::read(&p) {
                defs.font_data.insert("nerd".into(), Arc::new(FontData::from_owned(b)));
                extra.push("nerd".into());
            }
        }
        for (i, p) in symbol_candidates().into_iter().enumerate() {
            if let Ok(b) = std::fs::read(&p) {
                let n = format!("symbols{i}");
                defs.font_data.insert(n.clone(), Arc::new(FontData::from_owned(b)));
                extra.push(n);
            }
        }
    }
    let chain = |first: &[&str]| -> Vec<String> {
        let mut v: Vec<String> = first.iter().map(|s| s.to_string()).collect();
        // Bundled Pretendard has kana and Hangul but no Han. Regional Han
        // therefore comes from CJK while Latin/monospace typography stays intact.
        v.push("cjk".into());
        v.extend(extra.iter().cloned());
        v.extend(egui_fallbacks.iter().filter(|f| !v.contains(f)).cloned().collect::<Vec<_>>());
        v
    };
    defs.families.insert(FontFamily::Proportional, chain(&["pretendard"]));
    defs.families
        .insert(FontFamily::Name(MEDIUM.into()), chain(&["pretendard-medium", "pretendard"]));
    defs.families
        .insert(FontFamily::Name(SEMIBOLD.into()), chain(&["pretendard-semibold", "pretendard"]));
    defs.families.insert(FontFamily::Monospace, chain(&["jbm", "pretendard"]));
    defs.families
        .insert(FontFamily::Name(MONO_BOLD.into()), chain(&["jbm-bold", "jbm", "pretendard"]));
    defs
}

/// 컨텍스트에 글꼴을 설치한다.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(definitions(true));
}

#[cfg(test)]
mod tests {
    use super::*;

    const LANGUAGES: [Language; 4] = [Language::Korean, Language::English, Language::Japanese, Language::ChineseSimplified];

    #[test]
    fn bundled_fonts_render_cjk_in_every_family_without_system_fonts() {
        // Inspect the rendered glyph, not just the retained source character:
        // missing characters retain their char in a galley but use a replacement UV.
        let samples = "日本語設定接続画面検索保存取消こんにちはカタカナ简体中文语言设置终端文件夹编辑器한국어설정연결터미널";
        for language in LANGUAGES {
            let ctx = egui::Context::default();
            ctx.set_fonts(definitions_for_language(false, language));
            let mut output = ctx.run_ui(Default::default(), |ui| {
                ui.fonts_mut(|fonts| {
                    for font in [
                        regular(16.0),
                        medium(16.0),
                        semibold(16.0),
                        mono(16.0),
                        FontId::new(16.0, FontFamily::Name(MONO_BOLD.into())),
                    ] {
                        let missing = fonts.layout_no_wrap("\u{10ffff}".into(), font.clone(), egui::Color32::WHITE);
                        let replacement = missing.rows[0].glyphs[0].uv_rect;
                        for ch in samples.chars() {
                            let galley = fonts.layout_no_wrap(ch.to_string(), font.clone(), egui::Color32::WHITE);
                            assert!(galley.num_vertices > 0, "invisible glyph {ch} in {language:?} / {:?}", font.family);
                            assert_ne!(
                                galley.rows[0].glyphs[0].uv_rect, replacement,
                                "replacement glyph for {ch} in {language:?} / {:?}",
                                font.family
                            );
                        }
                    }
                });
            });
            output.textures_delta.clear();
        }
    }

    #[test]
    fn regional_faces_are_selected_before_system_fallbacks() {
        for (language, index) in [
            (Language::Japanese, 0),
            (Language::ChineseSimplified, 2),
            (Language::Korean, 1),
            (Language::English, 1),
        ] {
            let defs = definitions_for_language(false, language);
            assert_eq!(defs.font_data["cjk"].index, index);
            for family in defs.families.values() {
                let cjk_position = family.iter().position(|name| name == "cjk").unwrap();
                assert!(family[..cjk_position].iter().any(|name| name.starts_with("pretendard")));
                assert!(family[cjk_position + 1..].iter().all(|name| !name.starts_with("pretendard")));
            }
        }
    }

    #[test]
    fn changing_language_preserves_monospace_ascii_widths() {
        let mut expected_widths = None;
        for language in LANGUAGES {
            let ctx = egui::Context::default();
            ctx.set_fonts(definitions_for_language(false, language));
            let mut output = ctx.run_ui(Default::default(), |ui| {
                ui.fonts_mut(|fonts| {
                    let widths = ['W', 'i', '0'].map(|ch| fonts.glyph_width(&mono(16.0), ch));
                    assert_eq!(widths[0], widths[1]);
                    assert_eq!(widths[1], widths[2]);
                    if let Some(expected) = expected_widths {
                        assert_eq!(widths, expected);
                    }
                    expected_widths = Some(widths);
                });
            });
            output.textures_delta.clear();
        }
    }
}
