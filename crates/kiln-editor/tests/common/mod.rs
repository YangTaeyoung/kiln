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
