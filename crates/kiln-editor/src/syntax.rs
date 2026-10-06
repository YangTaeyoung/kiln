//! 문법 정의(two-face/bat 세트), Kiln 팔레트 기반 syntect 테마, 언어 감지, 주석 토큰.

use std::path::Path;
use std::str::FromStr;
use std::sync::{Mutex, OnceLock};

use egui::Color32;
use kiln_common::Theme;
use syntect::highlighting::{
    Color, FontStyle, Highlighter, ScopeSelectors, StyleModifier, Theme as SynTheme, ThemeItem,
    ThemeSettings,
};
use syntect::parsing::{SyntaxReference, SyntaxSet};

/// 하이라이트에 필요한 전역 자산(문법 세트).
pub struct SyntaxAssets {
    pub set: SyntaxSet,
}

static ASSETS: OnceLock<SyntaxAssets> = OnceLock::new();
/// 테마 이름별 syntect 하이라이터. 테마마다 한 번 만들어 프로세스가 끝날 때까지 둔다.
static HIGHLIGHTERS: Mutex<Vec<(&'static str, &'static Highlighter<'static>)>> = Mutex::new(Vec::new());

/// 문법 세트를 (필요하면 로드해서) 돌려준다.
pub fn assets() -> &'static SyntaxAssets {
    ASSETS.get_or_init(|| SyntaxAssets { set: two_face::syntax::extra_newlines() })
}

/// 현재 앱 테마(`Theme::current()`)에 맞춘 syntect 하이라이터.
pub fn highlighter() -> &'static Highlighter<'static> {
    highlighter_for(&Theme::current())
}

/// 주어진 앱 테마에 맞춘 syntect 하이라이터. 밝은 테마면 밝은 배경용 색 규칙을 쓴다.
pub fn highlighter_for(t: &Theme) -> &'static Highlighter<'static> {
    let mut list = HIGHLIGHTERS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, h)) = list.iter().find(|(n, _)| *n == t.name) {
        return h;
    }
    let theme: &'static SynTheme = Box::leak(Box::new(build_theme(t)));
    let h: &'static Highlighter<'static> = Box::leak(Box::new(Highlighter::new(theme)));
    list.push((t.name, h));
    h
}

/// 백그라운드 스레드에서 문법 세트를 미리 로드한다.
pub fn prewarm() {
    if ASSETS.get().is_none() {
        std::thread::spawn(|| {
            let _ = assets();
            let _ = highlighter();
        });
    }
}

/// 구문 강조용 색 묶음. 밝은 테마는 GitHub 라이트 팔레트로 바꾼다.
fn syntax_palette(t: &Theme) -> Theme {
    if t.dark {
        return *t;
    }
    let hex = |v: u32| Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
    let mut p = *t;
    p.text = hex(0x1f2328);
    p.text_dim = hex(0x4b535d);
    p.text_faint = hex(0x6e7781);
    p.green = hex(0x0a3069);
    p.orange = hex(0x0550ae);
    p.purple = hex(0xcf222e);
    p.blue = hex(0x8250df);
    p.yellow = hex(0x953800);
    p.red = hex(0x116329);
    p.ansi[6] = hex(0x0550ae);
    p
}

fn c(c: Color32) -> Color {
    Color { r: c.r(), g: c.g(), b: c.b(), a: 255 }
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Kiln 팔레트로 syntect 테마를 구성한다. 어두운 테마는 밝은 글자색, 밝은 테마(`!t.dark`)는
/// 흰 바탕에서 대비가 충분한 GitHub 라이트 계열 색을 쓴다.
pub fn build_theme(t: &Theme) -> SynTheme {
    let t = &syntax_palette(t);
    let cyan = t.ansi[6];
    let comment = if t.dark { mix(t.text_faint, t.text_dim, 0.25) } else { mix(t.text_faint, t.text_dim, 0.45) };
    let operator = mix(t.text_dim, cyan, 0.35);
    let rules: &[(&str, Color32, FontStyle)] = &[
        ("comment, punctuation.definition.comment", comment, FontStyle::ITALIC),
        ("string, string.quoted, punctuation.definition.string", t.green, FontStyle::empty()),
        ("string.regexp, constant.character.escape, constant.other.placeholder", cyan, FontStyle::empty()),
        ("constant.numeric, constant.language, constant.character, constant.other, support.constant", t.orange, FontStyle::empty()),
        ("keyword, storage, keyword.control, storage.type, storage.modifier", t.purple, FontStyle::empty()),
        ("keyword.operator, punctuation.separator.key-value, keyword.operator.assignment", operator, FontStyle::empty()),
        ("keyword.operator.word, keyword.operator.logical.python, keyword.operator.new", t.purple, FontStyle::empty()),
        ("entity.name.function, support.function, meta.function-call entity.name, variable.function, entity.name.method", t.blue, FontStyle::empty()),
        ("entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, entity.name.trait, entity.name.interface, support.type, support.class, entity.other.inherited-class, storage.type.primitive, storage.type.numeric, storage.type.builtin", t.yellow, FontStyle::empty()),
        ("storage.type.function, storage.type.struct, storage.type.enum, storage.type.impl, storage.type.trait, storage.type.class, storage.type.module, storage.type.type", t.purple, FontStyle::empty()),
        ("entity.name.macro, support.macro, meta.macro entity.name, entity.name.function.macro", cyan, FontStyle::empty()),
        ("variable.parameter", mix(t.text, t.orange, 0.55), FontStyle::empty()),
        ("variable.language, variable.other.readwrite.instance", t.red, FontStyle::ITALIC),
        ("entity.name.tag, meta.tag.sgml, punctuation.definition.tag", t.red, FontStyle::empty()),
        ("entity.other.attribute-name", t.orange, FontStyle::empty()),
        ("entity.name.namespace, entity.name.module, meta.path entity.name", mix(t.text, t.yellow, 0.4), FontStyle::empty()),
        ("meta.annotation, meta.attribute, punctuation.definition.annotation, entity.name.attribute, storage.modifier.attribute", mix(t.text_dim, t.yellow, 0.4), FontStyle::empty()),
        ("support.type.property-name, meta.mapping.key string, entity.name.tag.yaml, meta.object-literal.key", t.blue, FontStyle::empty()),
        ("entity.name.section, markup.heading, markup.heading punctuation.definition.heading", t.blue, FontStyle::BOLD),
        ("entity.name.label, entity.name.lifetime, storage.modifier.lifetime, punctuation.definition.lifetime", t.orange, FontStyle::ITALIC),
        ("markup.bold", t.orange, FontStyle::BOLD),
        ("markup.italic", t.purple, FontStyle::ITALIC),
        ("markup.raw, markup.inline.raw", t.green, FontStyle::empty()),
        ("markup.underline.link, string.other.link", t.blue, FontStyle::UNDERLINE),
        ("markup.inserted", t.green, FontStyle::empty()),
        ("markup.deleted", t.red, FontStyle::empty()),
        ("markup.changed", t.yellow, FontStyle::empty()),
        ("meta.diff.range, meta.diff.header", t.blue, FontStyle::empty()),
        ("punctuation, meta.brace, punctuation.section, punctuation.separator, punctuation.terminator, punctuation.accessor", t.text_dim, FontStyle::empty()),
        ("invalid, invalid.illegal", t.red, FontStyle::empty()),
    ];
    let scopes = rules
        .iter()
        .filter_map(|(sel, color, fs)| {
            Some(ThemeItem {
                scope: ScopeSelectors::from_str(sel).ok()?,
                style: StyleModifier {
                    foreground: Some(c(*color)),
                    background: None,
                    font_style: Some(*fs),
                },
            })
        })
        .collect();
    SynTheme {
        name: Some("Kiln".into()),
        author: None,
        settings: ThemeSettings {
            foreground: Some(c(t.text)),
            background: Some(c(t.bg)),
            ..Default::default()
        },
        scopes,
    }
}

/// 경로(파일 이름·확장자)와 첫 줄로 문법을 찾는다. 찾지 못하면 `None`(일반 텍스트).
pub fn detect(path: &Path, first_line: &str) -> Option<&'static SyntaxReference> {
    let set = &assets().set;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if let Some(s) = set.find_syntax_by_extension(name) {
        return non_plain(s);
    }
    let lower = name.to_ascii_lowercase();
    let special = match lower.as_str() {
        "dockerfile" | "containerfile" => Some("Dockerfile"),
        "makefile" | "gnumakefile" | "justfile" => Some("Makefile"),
        "cmakelists.txt" => Some("CMake"),
        ".zshrc" | ".bashrc" | ".profile" | ".zprofile" | ".bash_profile" | ".envrc" => {
            Some("Bourne Again Shell (bash)")
        }
        _ if lower.starts_with("dockerfile.") || lower.ends_with(".dockerfile") => Some("Dockerfile"),
        _ if lower.starts_with(".env") => Some("DotENV"),
        _ => None,
    };
    if let Some(n) = special.and_then(|n| set.find_syntax_by_name(n)) {
        return Some(n);
    }
    let ext_override = match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("jsonc" | "json5") => Some("JSON"),
        Some("mdx") => Some("Markdown"),
        Some("h") => Some("C++"),
        Some("kts") => Some("Kotlin"),
        _ => None,
    };
    if let Some(s) = ext_override.and_then(|n| set.find_syntax_by_name(n)) {
        return Some(s);
    }
    // 여러 겹의 확장자(`foo.html.erb`)는 긴 것부터 시도한다.
    let mut rest = name;
    while let Some(i) = rest.find('.') {
        rest = &rest[i + 1..];
        if rest.is_empty() {
            break;
        }
        let ext_syntax = set
            .find_syntax_by_extension(rest)
            .or_else(|| set.find_syntax_by_extension(&rest.to_ascii_lowercase()));
        if let Some(s) = ext_syntax {
            return non_plain(s);
        }
    }
    set.find_syntax_by_first_line(first_line).and_then(non_plain)
}

fn non_plain(s: &'static SyntaxReference) -> Option<&'static SyntaxReference> {
    (s.name != "Plain Text").then_some(s)
}

/// 언어별 주석 토큰.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommentStyle {
    Line(&'static str),
    Block(&'static str, &'static str),
}

/// 문법 이름으로 주석 토큰을 고른다.
pub fn comment_style(syntax_name: &str) -> Option<CommentStyle> {
    use CommentStyle::*;
    let n = syntax_name.to_ascii_lowercase();
    let is = |keys: &[&str]| keys.iter().any(|k| n == *k || n.starts_with(&format!("{k} ")));
    if is(&["html", "xml", "markdown", "multimarkdown", "vue component", "svelte", "asp", "java server page (jsp)"]) {
        return Some(Block("<!--", "-->"));
    }
    if is(&["css"]) {
        return Some(Block("/*", "*/"));
    }
    if is(&["ocaml", "ocamllex", "ocamlyacc", "f#", "sml", "pascal"]) {
        return Some(if n == "f#" { Line("//") } else { Block("(*", "*)") });
    }
    if is(&[
        "rust", "c", "c++", "c#", "objective-c", "objective-c++", "java", "javascript", "typescript",
        "typescriptreact", "go", "swift", "kotlin", "scala", "dart", "zig", "php", "groovy",
        "protocol buffer", "d", "glsl", "wgsl", "solidity", "less", "scss", "sass", "stylus", "json",
        "jsonnet", "actionscript", "qml", "verilog", "systemverilog", "cfml script", "rego", "varlink",
        "graphviz (dot)",
    ]) {
        return Some(Line("//"));
    }
    if is(&[
        "python", "ruby", "bourne again shell (bash)", "shell-unix-generic", "fish", "perl", "r",
        "yaml", "toml", "dockerfile", "makefile", "cmake", "nix", "elixir", "powershell", "terraform",
        "git ignore", "git config", "git attributes", "julia", "nim", "crystal", "coffeescript", "tcl",
        "awk", "dotenv", "puppet", "requirements.txt", "ninja", "nginx", "apache conf", "crontab",
        "graphql", "jq", "gnuplot", "salt state (sls)", "ssh config", "sshd config", "robot framework",
        "vyper", "cabal", "hosts", "java properties", "livescript", "rd (r documentation)",
    ]) {
        return Some(Line("#"));
    }
    if is(&["sql", "lua", "haskell", "elm", "ada", "purescript", "literate haskell", "applescript"]) {
        return Some(Line("--"));
    }
    if is(&["lisp", "clojure", "racket", "ini", "assembly (x86_64)", "llvm", "nsis"]) {
        return Some(Line(";"));
    }
    if is(&["erlang", "latex", "tex", "matlab", "bibtex"]) {
        return Some(Line("%"));
    }
    if is(&["arm assembly"]) {
        return Some(Line("@"));
    }
    if is(&["viml"]) {
        return Some(Line("\""));
    }
    if is(&["batch file"]) {
        return Some(Line("REM"));
    }
    if is(&["fortran (modern)"]) {
        return Some(Line("!"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(p: &str, first: &str) -> String {
        detect(Path::new(p), first).map(|s| s.name.clone()).unwrap_or_else(|| "Plain Text".into())
    }

    #[test]
    fn detects_common_languages_by_name_extension_and_shebang() {
        assert_eq!(name("a/main.rs", ""), "Rust");
        assert_eq!(name("x.ts", ""), "TypeScript");
        assert_eq!(name("x.tsx", ""), "TypeScriptReact");
        assert_eq!(name("Cargo.toml", ""), "TOML");
        assert_eq!(name("Dockerfile", ""), "Dockerfile");
        assert_eq!(name("Dockerfile.dev", ""), "Dockerfile");
        assert_eq!(name("Makefile", ""), "Makefile");
        assert_eq!(name("x.py", ""), "Python");
        assert_eq!(name("script", "#!/usr/bin/env bash"), "Bourne Again Shell (bash)");
        assert_eq!(name("x.go", ""), "Go");
        assert_eq!(name("notes.unknownext", "hello"), "Plain Text");
    }

    #[test]
    fn comment_tokens_follow_language() {
        assert_eq!(comment_style("Rust"), Some(CommentStyle::Line("//")));
        assert_eq!(comment_style("Python"), Some(CommentStyle::Line("#")));
        assert_eq!(comment_style("SQL"), Some(CommentStyle::Line("--")));
        assert_eq!(comment_style("HTML"), Some(CommentStyle::Block("<!--", "-->")));
        assert_eq!(comment_style("CSS"), Some(CommentStyle::Block("/*", "*/")));
        assert_eq!(comment_style("Bourne Again Shell (bash)"), Some(CommentStyle::Line("#")));
        assert_eq!(comment_style("Plain Text"), None);
    }
}

/// Bounded, conservative content detection. Explicit filenames/extensions take precedence.
pub fn detect_document(path: &Path, text: &str) -> Option<&'static SyntaxReference> {
    let sample: String = text.chars().take(8192).collect();
    let first = sample.lines().next().unwrap_or("").trim_start_matches('\u{feff}');
    if let Some(s) = detect(path, "") { return Some(s); }
    let trimmed = sample.trim_start_matches('\u{feff}').trim_start();
    let name = if trimmed.starts_with("#!/") {
        let executable = first.split_whitespace().collect::<Vec<_>>();
        if executable.iter().any(|s| s.contains("python")) { "Python" }
        else if executable.iter().any(|s| s.ends_with("node") || s.ends_with("nodejs")) { "JavaScript" }
        else if executable.iter().any(|s| s.ends_with("ruby")) { "Ruby" }
        else { return detect(path, first); }
    } else if trimmed.starts_with("<?xml") { "XML" }
    else if trimmed.to_ascii_lowercase().starts_with("<!doctype html") { "HTML" }
    else if trimmed.starts_with("<?php") { "PHP" }
    else if (trimmed.starts_with('{') || trimmed.starts_with('[')) && serde_json::from_str::<serde_json::Value>(trimmed).is_ok() { "JSON" }
    else { return detect(path,first); };
    assets().set.find_syntax_by_name(name)
}

/// Map the selected highlighting grammar to the LSP language identity.
pub fn language_id(name: &str) -> Option<&'static str> {
    Some(match name {
        "Rust"=>"rust", "Go"=>"go", "Python"=>"python", "C"=>"c", "C++"=>"cpp",
        "JavaScript"=>"javascript", "TypeScript"=>"typescript", "TypeScriptReact"|"TSX"=>"typescriptreact",
        "JavaScript (Babel)"|"JSX"=>"javascriptreact", "Lua"=>"lua", "Ruby"=>"ruby",
        "JSON"=>"json", "YAML"=>"yaml", "TOML"=>"toml", "HTML"=>"html", "CSS"=>"css",
        "Bourne Again Shell (bash)"=>"shellscript", "Dockerfile"=>"dockerfile", "Markdown"=>"markdown",
        "Java"=>"java", "Kotlin"=>"kotlin", "Swift"=>"swift", "SQL"=>"sql", "PHP"=>"php",
        _=>return None,
    })
}

#[cfg(test)]
mod document_detection_tests {
    use super::*;
    #[test]
    fn extension_shebang_content_and_override_precedence() {
        for (p,text,expected) in [("script","#!/usr/bin/env python3\nprint(1)","Python"),("data","{\"a\":1}","JSON"),("page","<!DOCTYPE html>\n<html>","HTML"),("x.py","{\"a\":1}","Python")]{
            assert_eq!(detect_document(Path::new(p),text).unwrap().name,expected);
        }
        assert!(detect_document(Path::new("unknown"),"Some ordinary prose about fn and class").is_none());
    }
    #[test]
    fn bundled_shell_shebang_detection_survives_document_inference() {
        for first in ["#!/usr/bin/env bash", "#!/bin/bash"] {
            let text=format!("{first}\necho hello");
            assert_eq!(detect_document(Path::new("script"),&text).unwrap().name,"Bourne Again Shell (bash)");
            assert_eq!(crate::Editor::from_text("script",&text).status().language,"Bourne Again Shell (bash)");
        }
    }
}
