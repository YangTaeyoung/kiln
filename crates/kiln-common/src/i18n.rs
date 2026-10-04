//! Application-owned text only. Never translate terminal output or user data.
use serde::{Deserialize, Serialize};
use std::{
    cell::Cell,
    collections::BTreeMap,
    sync::{
        OnceLock,
        atomic::{AtomicU8, Ordering},
    },
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    #[default]
    #[serde(rename = "ko")]
    Korean,
    #[serde(rename = "en")]
    English,
    #[serde(rename = "ja")]
    Japanese,
    #[serde(rename = "zh-CN")]
    ChineseSimplified,
}
impl Language {
    pub const ALL: [Self; 4] = [
        Self::Korean,
        Self::English,
        Self::Japanese,
        Self::ChineseSimplified,
    ];
    pub const fn code(self) -> &'static str {
        match self {
            Self::Korean => "ko",
            Self::English => "en",
            Self::Japanese => "ja",
            Self::ChineseSimplified => "zh-CN",
        }
    }
    pub const fn native_name(self) -> &'static str {
        match self {
            Self::Korean => "한국어",
            Self::English => "English",
            Self::Japanese => "日本語",
            Self::ChineseSimplified => "简体中文",
        }
    }
}
static LANGUAGE: AtomicU8 = AtomicU8::new(0);
thread_local! {static OVERRIDE:Cell<Option<Language>>=const{Cell::new(None)};}
pub fn language() -> Language {
    OVERRIDE
        .with(|v| v.get())
        .unwrap_or_else(|| match LANGUAGE.load(Ordering::Relaxed) {
            1 => Language::English,
            2 => Language::Japanese,
            3 => Language::ChineseSimplified,
            _ => Language::Korean,
        })
}
pub fn set_language(language: Language) {
    LANGUAGE.store(language as u8, Ordering::Relaxed);
}
/// Scoped overrides make translated layout tests independent of parallel tests.
pub fn with_language<T>(language: Language, f: impl FnOnce() -> T) -> T {
    struct Reset(Option<Language>);
    impl Drop for Reset {
        fn drop(&mut self) {
            OVERRIDE.with(|v| v.set(self.0));
        }
    }
    let _reset = Reset(OVERRIDE.with(|v| v.replace(Some(language))));
    f()
}
pub fn load_language() -> Language {
    std::fs::read(crate::paths::config_file("language.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}
pub fn save_language(language: Language) -> anyhow::Result<()> {
    crate::store::save_json(&crate::paths::config_file("language.json"), &language)?;
    set_language(language);
    Ok(())
}
pub fn sync_language() -> bool {
    let next = load_language();
    let changed = next != language();
    if changed {
        set_language(next);
    }
    changed
}
type Catalog = BTreeMap<String, BTreeMap<String, String>>;
pub fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("../locales/messages.json"))
            .expect("validated localization catalog")
    })
}
/// Returns a catalog entry only for an exact application message key.
pub fn tr(source: &str) -> &str {
    if language() == Language::Korean {
        return source;
    }
    catalog()
        .get(source)
        .and_then(|entry| entry.get(language().code()))
        .map(String::as_str)
        .unwrap_or(source)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_message_has_all_three_translations() {
        for (source, entry) in catalog() {
            for locale in ["en", "ja", "zh-CN"] {
                let translated = entry
                    .get(locale)
                    .unwrap_or_else(|| panic!("missing {locale}: {source}"));
                assert!(!translated.trim().is_empty(), "empty {locale}: {source}");
                assert!(
                    !translated.chars().any(|c| ('가'..='힣').contains(&c)),
                    "Korean text in {locale}: {source}"
                );
            }
        }
    }
    #[test]
    fn formatting_preserves_user_data_and_evaluates_arguments_once() {
        let user = "저장소 {name}: 日本語 / 中文";
        for locale in Language::ALL {
            with_language(locale, || {
                let mut calls = 0;
                let output = crate::trf!("저장 실패: {}", {
                    calls += 1;
                    user
                });
                assert_eq!(calls, 1);
                assert!(output.contains(user));
                let name = user;
                assert!(crate::trf!("{name} · 터미널").contains(user));
            });
        }
    }
    #[test]
    fn unknown_content_is_never_modified() {
        for l in Language::ALL {
            with_language(l, || {
                assert_eq!(
                    tr("user/project: not an application message"),
                    "user/project: not an application message"
                )
            });
        }
    }
    #[test]
    fn old_and_invalid_preferences_have_stable_fallback() {
        assert_eq!(Language::default(), Language::Korean);
        for language in Language::ALL {
            assert_eq!(
                serde_json::from_str::<Language>(&serde_json::to_string(&language).unwrap())
                    .unwrap(),
                language
            );
        }
    }
    #[test]
    fn scoped_language_restores_even_when_nested() {
        with_language(Language::Japanese, || {
            assert_eq!(language(), Language::Japanese);
            with_language(Language::English, || {
                assert_eq!(language(), Language::English)
            });
            assert_eq!(language(), Language::Japanese);
        });
    }
}
