//! 시스템 폰트 폴백(한중일, Nerd Font 아이콘) 등록.

use egui::{FontData, FontDefinitions, FontFamily};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn cjk_candidates() -> Vec<(PathBuf, u32)> {
    let mut v: Vec<(PathBuf, u32)> = Vec::new();
    #[cfg(target_os = "macos")]
    {
        v.push(("/System/Library/Fonts/AppleSDGothicNeo.ttc".into(), 0));
        v.push(("/System/Library/Fonts/Supplemental/AppleGothic.ttf".into(), 0));
        v.push(("/System/Library/Fonts/Hiragino Sans GB.ttc".into(), 0));
    }
    #[cfg(target_os = "windows")]
    {
        v.push(("C:\\Windows\\Fonts\\malgun.ttf".into(), 0));
        v.push(("C:\\Windows\\Fonts\\msyh.ttc".into(), 0));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for (p, idx) in [
            ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 1),
            ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 1),
            ("/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc", 1),
            ("/usr/share/fonts/truetype/nanum/NanumGothic.ttf", 0),
            ("/usr/share/fonts/truetype/unfonts-core/UnDotum.ttf", 0),
        ] {
            v.push((p.into(), idx));
        }
    }
    v
}

fn symbol_candidates() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    #[cfg(target_os = "macos")]
    {
        v.push("/System/Library/Fonts/Apple Symbols.ttf".into());
        v.push("/System/Library/Fonts/Supplemental/Arial Unicode.ttf".into());
    }
    #[cfg(target_os = "windows")]
    {
        v.push("C:\\Windows\\Fonts\\seguisym.ttf".into());
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        v.push("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf".into());
        v.push("/usr/share/fonts/dejavu/DejaVuSans.ttf".into());
        v.push("/usr/share/fonts/TTF/DejaVuSans.ttf".into());
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
    for d in dirs {
        if let Some(p) = scan_for(&d, 2) {
            return Some(p);
        }
    }
    None
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
        for s in subdirs {
            if let Some(p) = scan_for(&s, depth - 1) {
                return Some(p);
            }
        }
    }
    None
}

pub fn install(ctx: &egui::Context) {
    let mut defs = FontDefinitions::default();
    let mut add = |name: &str, bytes: Vec<u8>, index: u32| {
        let mut fd = FontData::from_owned(bytes);
        fd.index = index;
        defs.font_data.insert(name.to_owned(), Arc::new(fd));
        for fam in [FontFamily::Monospace, FontFamily::Proportional] {
            defs.families.entry(fam).or_default().push(name.to_owned());
        }
    };
    if let Some(p) = find_nerd_font() {
        if let Ok(b) = std::fs::read(&p) {
            add("nerd", b, 0);
        }
    }
    for (p, idx) in cjk_candidates() {
        if let Ok(b) = std::fs::read(&p) {
            add("cjk", b, idx);
            break;
        }
    }
    let mut n = 0;
    for p in symbol_candidates() {
        if let Ok(b) = std::fs::read(&p) {
            add(&format!("symbols{n}"), b, 0);
            n += 1;
        }
    }
    ctx.set_fonts(defs);
}
