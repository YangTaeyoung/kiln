//! SQL 구문 강조 레이아웃. 같은 텍스트면 캐시한 LayoutJob 을 다시 쓴다.

use crate::Driver;
use crate::sql::{TokKind, is_keyword, tokenize};
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId};
use kiln_common::Theme;
use std::hash::{Hash, Hasher};

#[derive(Default)]
pub(crate) struct SqlHighlighter {
    key: u64,
    job: Option<LayoutJob>,
}

fn hash_of(text: &str, wrap: f32, driver: Driver, font: f32) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    wrap.to_bits().hash(&mut h);
    driver.hash(&mut h);
    font.to_bits().hash(&mut h);
    h.finish()
}

impl SqlHighlighter {
    /// 강조된 LayoutJob. 입력이 같으면 캐시를 복제해 돌려준다.
    pub fn job(
        &mut self,
        text: &str,
        wrap_width: f32,
        driver: Driver,
        font_size: f32,
    ) -> LayoutJob {
        let key = hash_of(text, wrap_width, driver, font_size);
        if self.key == key
            && let Some(j) = &self.job
        {
            return j.clone();
        }
        let mut job = highlight(text, driver, font_size);
        job.wrap.max_width = wrap_width;
        self.key = key;
        self.job = Some(job.clone());
        job
    }
}

/// SQL 텍스트를 토큰 색으로 칠한 LayoutJob 을 만든다.
pub(crate) fn highlight(text: &str, driver: Driver, font_size: f32) -> LayoutJob {
    let theme = Theme::current();
    let font = FontId::monospace(font_size);
    let mut job = LayoutJob::default();
    let fmt = |color: Color32, italics: bool| TextFormat {
        font_id: font.clone(),
        color,
        italics,
        ..Default::default()
    };
    for t in tokenize(text, driver) {
        let s = &text[t.start..t.end];
        let f = match t.kind {
            TokKind::Word if is_keyword(s) => fmt(theme.purple, false),
            TokKind::Word => fmt(theme.text, false),
            TokKind::QuotedIdent => fmt(theme.yellow, false),
            TokKind::Str => fmt(theme.green, false),
            TokKind::Number => fmt(theme.orange, false),
            TokKind::Comment => fmt(theme.text_faint, true),
            TokKind::Param => fmt(theme.accent, false),
            TokKind::Punct | TokKind::Semicolon => fmt(theme.text_dim, false),
            TokKind::Space => fmt(theme.text, false),
        };
        job.append(s, 0.0, f);
    }
    job
}
