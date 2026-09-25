//! 언어 서버 설정: 내장 기본값, `lsp.json` 덮어쓰기, 실행 파일 찾기, 언어 판별.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// 언어 서버 하나의 실행 방법.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ServerSpec {
    pub command: String,
    pub args: Vec<String>,
    /// 이 언어로 볼 파일 확장자(점 없이).
    pub extensions: Vec<String>,
    pub enabled: bool,
}

impl Default for ServerSpec {
    fn default() -> Self {
        Self { command: String::new(), args: Vec::new(), extensions: Vec::new(), enabled: true }
    }
}

impl ServerSpec {
    pub fn new(command: impl Into<String>, args: &[&str]) -> Self {
        Self { command: command.into(), args: args.iter().map(|s| s.to_string()).collect(), ..Default::default() }
    }
}

/// 언어 이름(언어 ID 또는 서버 계열) → 서버 설정.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LspConfig {
    pub languages: HashMap<String, ServerSpec>,
}

#[derive(Deserialize, Default)]
struct FileConfig {
    #[serde(default)]
    languages: HashMap<String, ServerSpec>,
}

impl LspConfig {
    /// 내장 기본값.
    pub fn defaults() -> Self {
        let mut languages = HashMap::new();
        languages.insert("rust".into(), ServerSpec::new("rust-analyzer", &[]));
        languages.insert("go".into(), ServerSpec::new("gopls", &[]));
        languages.insert("typescript".into(), ServerSpec::new("typescript-language-server", &["--stdio"]));
        languages.insert("python".into(), ServerSpec::new("pyright-langserver", &["--stdio"]));
        languages.insert("c".into(), ServerSpec::new("clangd", &[]));
        languages.insert("lua".into(), ServerSpec::new("lua-language-server", &[]));
        Self { languages }
    }

    /// 기본값 위에 `config_file("lsp.json")` 을 덮어쓴다. 파일이 없거나 잘못되면 기본값만 쓴다.
    pub fn load() -> Self {
        let path = kiln_common::paths::config_file("lsp.json");
        let mut cfg = Self::defaults();
        if let Ok(text) = std::fs::read_to_string(&path) {
            match serde_json::from_str::<FileConfig>(&text) {
                Ok(f) => cfg.merge(f.languages),
                Err(e) => log::warn!("{} 해석 실패: {e}", path.display()),
            }
        }
        cfg
    }

    /// 사용자 설정을 합친다. 명령이 빈 항목은 기존 명령·인수를 이어받는다.
    pub fn merge(&mut self, user: HashMap<String, ServerSpec>) {
        for (k, mut spec) in user {
            if spec.command.is_empty()
                && let Some(base) = self.languages.get(&k).or_else(|| self.languages.get(family(&k)))
            {
                spec.command = base.command.clone();
                if spec.args.is_empty() {
                    spec.args = base.args.clone();
                }
            }
            self.languages.insert(k, spec);
        }
    }

    /// 확장자로 언어 ID 를 찾는다. 설정의 `extensions` 가 내장 표보다 우선한다.
    pub fn language_for(&self, path: &Path) -> Option<String> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        let mut keys: Vec<&String> = self.languages.keys().collect();
        keys.sort();
        for k in keys {
            if self.languages[k].extensions.iter().any(|e| e.trim_start_matches('.').eq_ignore_ascii_case(&ext)) {
                return Some(k.clone());
            }
        }
        builtin_language(&ext).map(str::to_owned)
    }

    /// 언어 ID 의 서버 설정과 계열 이름. 언어 ID 항목이 계열 항목보다 우선한다.
    pub fn spec_for(&self, language: &str) -> Option<(String, ServerSpec)> {
        if let Some(s) = self.languages.get(language) {
            return Some((family_key(self, language), s.clone()));
        }
        let fam = family(language);
        self.languages.get(fam).map(|s| (fam.to_owned(), s.clone()))
    }
}

/// 서버를 공유하는 언어 계열 키. 언어 ID 에 전용 항목이 있으면 그 이름을 쓴다.
fn family_key(cfg: &LspConfig, language: &str) -> String {
    let fam = family(language);
    if fam != language && cfg.languages.get(fam) == cfg.languages.get(language) {
        fam.to_owned()
    } else {
        language.to_owned()
    }
}

/// 내장 확장자 표.
pub fn builtin_language(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "rs" => "rust",
        "go" => "go",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "py" | "pyi" => "python",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "lua" => "lua",
        _ => return None,
    })
}

/// 언어 ID 가 속한 서버 계열.
pub fn family(language: &str) -> &str {
    match language {
        "typescript" | "typescriptreact" | "javascript" | "javascriptreact" => "typescript",
        "c" | "cpp" | "objective-c" | "objective-cpp" => "c",
        other => other,
    }
}

/// 명령이 없을 때 시도할 내장 대체 명령.
pub fn fallbacks(family: &str) -> Vec<ServerSpec> {
    match family {
        "python" => vec![ServerSpec::new("pylsp", &[])],
        _ => Vec::new(),
    }
}

fn extra_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home = PathBuf::from(home);
        v.push(home.join(".cargo/bin"));
        v.push(home.join("go/bin"));
        v.push(home.join(".local/bin"));
    }
    v.push(PathBuf::from("/opt/homebrew/bin"));
    v.push(PathBuf::from("/usr/local/bin"));
    v
}

fn is_executable(p: &Path) -> bool {
    let Ok(m) = std::fs::metadata(p) else { return false };
    if !m.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        m.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// 명령을 실행 파일 경로로 찾는다. 경로 구분자가 있으면 그대로 확인하고, 아니면 PATH 와 흔한 설치 위치를 본다.
pub fn find_executable(cmd: &str) -> Option<PathBuf> {
    if cmd.is_empty() {
        return None;
    }
    let p = Path::new(cmd);
    if p.components().count() > 1 {
        return is_executable(p).then(|| p.to_path_buf());
    }
    let exts: Vec<String> = if cfg!(windows) {
        vec![String::new(), ".exe".into(), ".cmd".into(), ".bat".into()]
    } else {
        vec![String::new()]
    };
    let path_dirs = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default();
    for dir in path_dirs.into_iter().chain(extra_dirs()) {
        for e in &exts {
            let cand = dir.join(format!("{cmd}{e}"));
            if is_executable(&cand) {
                return Some(cand);
            }
        }
    }
    None
}

/// 파일이 속한 작업 공간 루트. `root` 아래면 `root`, 아니면 표지 파일이 있는 가장 가까운 조상.
pub fn workspace_root(root: &Path, file: &Path) -> PathBuf {
    if file.starts_with(root) {
        return root.to_path_buf();
    }
    const MARKERS: &[&str] =
        &["Cargo.toml", "go.mod", "package.json", "pyproject.toml", "setup.py", "compile_commands.json", ".git"];
    let parent = file.parent().unwrap_or(file);
    let mut dir = Some(parent);
    while let Some(d) = dir {
        if MARKERS.iter().any(|m| d.join(m).exists()) {
            return d.to_path_buf();
        }
        dir = d.parent();
    }
    parent.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_ids_and_families() {
        let c = LspConfig::defaults();
        assert_eq!(c.language_for(Path::new("a/b.rs")).as_deref(), Some("rust"));
        assert_eq!(c.language_for(Path::new("x.TSX")).as_deref(), Some("typescriptreact"));
        assert_eq!(c.language_for(Path::new("x.hpp")).as_deref(), Some("cpp"));
        assert_eq!(c.language_for(Path::new("x.txt")), None);
        assert_eq!(c.spec_for("javascriptreact").unwrap().0, "typescript");
        assert_eq!(c.spec_for("cpp").unwrap().1.command, "clangd");
        assert!(c.spec_for("cobol").is_none());
    }

    #[test]
    fn user_config_overrides_and_adds_languages() {
        let mut c = LspConfig::defaults();
        let user: FileConfig = serde_json::from_str(
            r#"{"languages": {
                "rust": {"command": "/opt/ra", "args": ["--x"]},
                "go": {"enabled": false},
                "zig": {"command": "zls", "extensions": ["zig", ".zon"]},
                "cpp": {"command": "ccls"}
            }}"#,
        )
        .unwrap();
        c.merge(user.languages);
        assert_eq!(c.spec_for("rust").unwrap().1, ServerSpec { command: "/opt/ra".into(), args: vec!["--x".into()], extensions: vec![], enabled: true });
        let go = c.spec_for("go").unwrap().1;
        assert!(!go.enabled);
        assert_eq!(go.command, "gopls");
        assert_eq!(c.language_for(Path::new("a.zon")).as_deref(), Some("zig"));
        assert_eq!(c.spec_for("cpp").unwrap(), ("cpp".into(), ServerSpec::new("ccls", &[])));
        assert_eq!(c.spec_for("c").unwrap().1.command, "clangd");
    }

    #[test]
    fn workspace_root_prefers_manager_root_then_markers() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        let other = dir.path().join("other/crate");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(other.join("src")).unwrap();
        std::fs::write(other.join("Cargo.toml"), "").unwrap();
        assert_eq!(workspace_root(&root, &root.join("src/a.rs")), root);
        assert_eq!(workspace_root(&root, &other.join("src/a.rs")), other);
    }

    #[test]
    fn finds_executables_on_path() {
        assert!(find_executable("sh").is_some());
        assert!(find_executable("definitely-not-a-real-binary-kiln").is_none());
        assert!(find_executable("/bin/sh").is_some());
    }
}
