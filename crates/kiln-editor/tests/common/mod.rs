//! 통합 테스트 공용 도우미.
#![allow(dead_code)]

/// 약 `lines` 줄짜리 현실적인 Rust 소스를 만든다.
pub fn rust_source(lines: usize) -> String {
    const CHUNK: &str = r#"/// Represents a connection pool entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry<'a> {
    pub name: &'a str,
    pub id: u64,
    values: Vec<f64>, // cached values
}

impl<'a> Entry<'a> {
    /// Creates a new entry with the given `name`.
    pub fn new(name: &'a str, id: u64) -> Self {
        let values = (0..16).map(|i| i as f64 * 1.5).collect();
        Self { name, id, values }
    }

    pub fn score(&self) -> Option<f64> {
        if self.values.is_empty() {
            return None;
        }
        let sum: f64 = self.values.iter().sum();
        match self.id % 3 {
            0 => Some(sum / 2.0),
            1 => Some(sum * 0x1F as f64),
            _ => {
                println!("entry {} has id {:?}", self.name, self.id);
                Some(-sum)
            }
        }
    }
}

/* block comment
   spanning lines */
fn helper_{N}(input: &str) -> Result<String, std::io::Error> {
    let s = format!("{}-{}", input, "suffix\"quoted\"");
    Ok(s.trim().to_owned())
}

"#;
    let per = CHUNK.lines().count();
    let mut out = String::with_capacity(lines * 40);
    let mut i = 0;
    while out.lines().count() < lines && i * per < lines {
        out.push_str(&CHUNK.replace("{N}", &i.to_string()));
        i += 1;
    }
    out
}

/// 테스트용 앱 스타일(테마)을 적용한다.
pub fn apply_theme(ctx: &egui::Context) {
    kiln_common::Theme::current().apply(ctx);
    install_korean_font(ctx);
    ctx.global_style_mut(|s| s.visuals.text_cursor.blink = false);
}

/// 조건이 참이 될 때까지(최대 `secs` 초) 프레임을 돌린다.
pub fn wait_until<S>(h: &mut egui_kittest::Harness<'_, S>, secs: f32, mut cond: impl FnMut(&mut egui_kittest::Harness<'_, S>) -> bool) -> bool {
    let start = std::time::Instant::now();
    loop {
        h.step();
        if cond(h) {
            h.run_ok();
            return true;
        }
        if start.elapsed().as_secs_f32() > secs {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

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
