//! User palettes inherit a built-in theme; loading never rewrites user files.

use super::Theme;
use anyhow::{Context, Result, bail};
use egui::Color32;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::RwLock,
};

const MAX_FILE_BYTES: u64 = 64 * 1024;
static CUSTOM: RwLock<Vec<ThemeChoice>> = RwLock::new(Vec::new());
static ERRORS: RwLock<Vec<String>> = RwLock::new(Vec::new());

#[derive(Clone, Debug)]
pub struct ThemeChoice {
    /// Persisted ID, separate from the inherited palette's static name/label.
    pub name: String,
    pub label: String,
    pub theme: Theme,
}

impl ThemeChoice {
    pub fn display_label(&self) -> &str {
        if self.name.starts_with("custom:") {
            &self.label
        } else {
            crate::i18n::tr(&self.label)
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    name: String,
    label: String,
    base: String,
    #[serde(default)]
    colors: BTreeMap<String, String>,
    ansi: Option<[String; 16]>,
}

fn color(value: &str) -> Result<Color32> {
    let Some(hex) = value.strip_prefix('#') else {
        bail!("expected #RRGGBB, got {value:?}")
    };
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("expected #RRGGBB, got {value:?}");
    }
    let rgb = u32::from_str_radix(hex, 16)?;
    Ok(Color32::from_rgb(
        (rgb >> 16) as u8,
        (rgb >> 8) as u8,
        rgb as u8,
    ))
}

fn parse(bytes: &[u8]) -> Result<ThemeChoice> {
    let file: ThemeFile = serde_json::from_slice(bytes)?;
    if file.name.is_empty()
        || file.name.len() > 64
        || !file
            .name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        bail!("name must contain 1–64 lowercase ASCII letters, digits or hyphens");
    }
    if file.label.trim().is_empty()
        || file.label.chars().count() > 64
        || file.label.chars().any(char::is_control)
    {
        bail!("label must contain 1–64 characters without control characters");
    }
    let mut theme = Theme::ALL
        .into_iter()
        .find(|t| t.name == file.base)
        .with_context(|| format!("unknown built-in base {:?}", file.base))?;
    for (key, value) in file.colors {
        let value = color(&value).with_context(|| format!("invalid color {key}"))?;
        macro_rules! assign {
            ($($field:ident),+ $(,)?) => {
                match key.as_str() {
                    $(stringify!($field) => theme.$field = value,)+
                    _ => bail!("unknown color {key:?}"),
                }
            };
        }
        assign!(
            bg,
            bg_panel,
            bg_elevated,
            bg_hover,
            bg_selected,
            bg_input,
            border,
            border_strong,
            text,
            text_dim,
            text_faint,
            accent,
            accent_fg,
            green,
            red,
            yellow,
            blue,
            purple,
            orange
        );
    }
    if let Some(ansi) = file.ansi {
        for (index, value) in ansi.iter().enumerate() {
            theme.ansi[index] =
                color(value).with_context(|| format!("invalid ANSI color {index}"))?;
        }
    }
    Ok(ThemeChoice {
        name: format!("custom:{}", file.name),
        label: file.label,
        theme,
    })
}

fn read(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        bail!("theme exceeds 64 KiB");
    }
    Ok(bytes)
}

pub fn themes_dir() -> PathBuf {
    crate::paths::config_dir().join("themes")
}

/// Reads one folder deterministically, retaining valid files alongside errors.
fn load(dir: &Path) -> Result<(Vec<ThemeChoice>, Vec<String>)> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), Vec::new())),
        Err(e) => return Err(e.into()),
    };
    let mut files = entries
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    files.retain(|path| {
        path.extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
    });
    files.sort();
    let mut loaded = BTreeMap::new();
    let mut errors = Vec::new();
    for path in files {
        match read(&path).and_then(|bytes| parse(&bytes)) {
            Ok(choice) if loaded.contains_key(&choice.name) => errors.push(format!(
                "{}: duplicate theme {}",
                path.display(),
                choice.name
            )),
            Ok(choice) => {
                loaded.insert(choice.name.clone(), choice);
            }
            Err(e) => errors.push(format!("{}: {e:#}", path.display())),
        }
    }
    Ok((loaded.into_values().collect(), errors))
}

/// An unreadable folder preserves the last successfully loaded catalog.
pub fn load_custom_themes(dir: &Path) -> Vec<String> {
    let errors = match load(dir) {
        Ok((choices, errors)) => {
            *CUSTOM.write().unwrap_or_else(|e| e.into_inner()) = choices;
            errors
        }
        Err(e) => vec![format!("{}: {e:#}", dir.display())],
    };
    *ERRORS.write().unwrap_or_else(|e| e.into_inner()) = errors.clone();
    errors
}

pub fn load_errors() -> Vec<String> {
    ERRORS.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Validates before writing and never overwrites an existing theme.
pub fn import_theme(source: &Path, dir: &Path) -> Result<String> {
    let bytes = read(source)?;
    let choice = parse(&bytes)?;
    let (existing, _) = load(dir)?;
    if existing.iter().any(|t| t.name == choice.name) {
        bail!("theme {} already exists", choice.name);
    }
    fs::create_dir_all(dir)?;
    let path = dir.join(format!(
        "{}.json",
        choice.name.strip_prefix("custom:").unwrap()
    ));
    // Publish only the complete validated file, including on interrupted writes.
    let temp = tempfile::NamedTempFile::new_in(dir)?;
    temp.as_file().write_all(&bytes)?;
    temp.as_file().sync_all()?;
    temp.persist_noclobber(&path).map_err(|e| e.error)?;
    Ok(choice.name)
}

pub(super) fn find(name: &str) -> Option<Theme> {
    CUSTOM
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|t| t.name == name)
        .map(|t| t.theme)
}

pub(super) fn choices() -> Vec<ThemeChoice> {
    let mut all: Vec<_> = Theme::ALL
        .into_iter()
        .map(|theme| ThemeChoice {
            name: theme.name.into(),
            label: theme.label.into(),
            theme,
        })
        .collect();
    all.extend(
        CUSTOM
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned(),
    );
    all
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> serde_json::Value {
        json!({"name":"ocean", "label":"Ocean", "base":"midnight", "colors":{"bg":"#123456", "accent":"#AaBBcc"}})
    }

    #[test]
    fn overrides_preserve_unspecified_colors_and_base_appearance() {
        let choice = parse(&serde_json::to_vec(&sample()).unwrap()).unwrap();
        assert_eq!(choice.name, "custom:ocean");
        assert_eq!(choice.label, "Ocean");
        assert_eq!(
            choice.theme,
            Theme {
                bg: Color32::from_rgb(0x12, 0x34, 0x56),
                accent: Color32::from_rgb(0xaa, 0xbb, 0xcc),
                ..Theme::MIDNIGHT
            }
        );
        let mut light = sample();
        light["base"] = json!("kiln-light");
        light["ansi"] = json!(vec!["#112233"; 16]);
        let choice = parse(&serde_json::to_vec(&light).unwrap()).unwrap();
        assert!(!choice.theme.dark);
        assert_eq!(choice.theme.ansi, [Color32::from_rgb(0x11, 0x22, 0x33); 16]);
        assert_eq!(choice.theme.text, Theme::KILN_LIGHT.text);
    }

    #[test]
    fn rejects_unsafe_names_unknown_fields_invalid_colors_and_incomplete_ansi() {
        for name in ["", "../ocean", "..", "a/b", "a\\b", "Ocean", "바다"] {
            let mut value = sample();
            value["name"] = json!(name);
            assert!(
                parse(&serde_json::to_vec(&value).unwrap()).is_err(),
                "{name}"
            );
        }
        for (key, value) in [
            ("base", json!("missing")),
            ("label", json!(" ")),
            ("label", json!("a\nb")),
            ("unexpected", json!(true)),
            ("colors", json!({"background":"#ffffff"})),
            ("colors", json!({"text":"#ff00ff00"})),
            ("colors", json!({"text":"#１２３４５６"})),
            ("colors", json!({"text":"ff0000"})),
            ("ansi", json!(["#ffffff"])),
        ] {
            let mut input = sample();
            input[key] = value;
            assert!(
                parse(&serde_json::to_vec(&input).unwrap()).is_err(),
                "{key}"
            );
        }
    }

    #[test]
    fn imports_survive_reload_and_never_overwrite_existing_files() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.json");
        let dir = root.path().join("themes");
        let bytes = serde_json::to_vec(&sample()).unwrap();
        fs::write(&source, &bytes).unwrap();
        assert_eq!(import_theme(&source, &dir).unwrap(), "custom:ocean");
        let (loaded, errors) = load(&dir).unwrap();
        assert!(errors.is_empty());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "custom:ocean");
        assert_eq!(loaded[0].theme.bg, Color32::from_rgb(0x12, 0x34, 0x56));
        // Detect the ID even if the existing file has a different filename.
        fs::rename(dir.join("ocean.json"), dir.join("renamed.JSON")).unwrap();
        assert!(import_theme(&source, &dir).is_err());
        assert_eq!(fs::read(dir.join("renamed.JSON")).unwrap(), bytes);
        assert!(!dir.join("ocean.json").exists());
        // A malformed source cannot leave a partially installed theme.
        fs::write(&source, b"{}").unwrap();
        assert!(import_theme(&source, &dir).is_err());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    }

    #[test]
    fn loading_reports_invalid_and_duplicate_files_without_hiding_valid_themes() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = serde_json::to_vec(&sample()).unwrap();
        fs::write(dir.path().join("a.json"), &bytes).unwrap();
        fs::write(dir.path().join("b.json"), &bytes).unwrap();
        fs::write(dir.path().join("broken.json"), b"{").unwrap();
        fs::write(dir.path().join("ignored.txt"), b"{").unwrap();
        let (loaded, errors) = load(dir.path()).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "custom:ocean");
        assert_eq!(errors.len(), 2);
        assert!(errors[0].contains("b.json: duplicate theme custom:ocean"));
        assert!(errors[1].contains("broken.json"));
        assert_eq!(load(&dir.path().join("missing")).unwrap().0.len(), 0);
        let oversized = dir.path().join("oversized.json");
        fs::write(&oversized, vec![b' '; MAX_FILE_BYTES as usize + 1]).unwrap();
        assert!(read(&oversized).unwrap_err().to_string().contains("64 KiB"));
    }
}
