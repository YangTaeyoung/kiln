//! 번들 글꼴(Pretendard, JetBrains Mono)과 시스템 폴백(기호, Nerd Font) 등록.

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
/// 각 가족 뒤에 한글(Pretendard)·기호·이모지·Nerd Font 폴백을 붙인다.
pub fn definitions(system_fallbacks: bool) -> FontDefinitions {
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
        v.extend(extra.iter().cloned());
        v.extend(egui_fallbacks.iter().filter(|f| !v.contains(f)).cloned().collect::<Vec<_>>());
        v
    };
    defs.families.insert(FontFamily::Proportional, chain(&["pretendard"]));
    defs.families.insert(FontFamily::Name(MEDIUM.into()), chain(&["pretendard-medium", "pretendard"]));
    defs.families.insert(FontFamily::Name(SEMIBOLD.into()), chain(&["pretendard-semibold", "pretendard"]));
    defs.families.insert(FontFamily::Monospace, chain(&["jbm", "pretendard"]));
    defs.families.insert(FontFamily::Name(MONO_BOLD.into()), chain(&["jbm-bold", "jbm", "pretendard"]));
    defs
}

/// 컨텍스트에 글꼴을 설치한다.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(definitions(true));
}
