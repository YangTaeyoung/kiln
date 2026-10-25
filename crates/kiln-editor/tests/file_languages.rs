use std::path::Path;

use kiln_editor::{
    Editor,
    buffer::Buffer,
    highlight::{Highlighter, Span},
    syntax,
};

fn assert_languages(cases: &[(&str, &str)]) {
    let failures: Vec<_> = cases
        .iter()
        .filter_map(|(path, expected)| {
            let actual = Editor::from_text(*path, "").status().language;
            (actual != *expected).then(|| format!("{path}: {actual:?}, expected {expected:?}"))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn hidden_and_extensionless_configuration_files_use_their_own_grammar() {
    let cases = [
        (".env", "DotENV"),
        (".env.local", "DotENV"),
        (".env.production.local", "DotENV"),
        ("service.env", "DotENV"),
        (".envrc", "Bourne Again Shell (bash)"),
        ("home/.ssh/config", "SSH Config"),
        ("home/.ssh/config.d/work.conf", "SSH Config"),
        ("home/.ssh/config.d/work", "SSH Config"),
        ("etc/ssh/ssh_config", "SSH Config"),
        ("etc/ssh/ssh_config.d/20-office.conf", "SSH Config"),
        ("etc/ssh/sshd_config", "SSHD Config"),
        ("etc/ssh/sshd_config.d/ports.conf", "SSHD Config"),
        (".gitignore", "Git Ignore"),
        (".git/config", "Git Config"),
        (".git/config.worktree", "Git Config"),
        (".gitconfig", "Git Config"),
        (".gitmodules", "Git Config"),
        (".gitattributes", "Git Attributes"),
        (".editorconfig", "INI"),
        (".npmrc", "INI"),
        (".yarnrc.yml", "YAML"),
        (".eslintrc", "JSON"),
        (".prettierrc", "JSON"),
        (".babelrc", "JSON"),
        (".swcrc", "JSON"),
        (".stylelintrc", "JSON"),
        ("tsconfig.json", "JSON"),
        (".eslintrc.yaml", "YAML"),
        (".babelrc.js", "JavaScript (Babel)"),
        ("Dockerfile.dev", "Dockerfile"),
        ("Containerfile.dev", "Dockerfile"),
        ("docker-compose.yaml", "YAML"),
    ];
    assert_languages(&cases);
}

#[test]
fn common_source_data_and_build_formats_match_the_locked_runtime_grammars() {
    // Literal expected names were checked against the locked two-face runtime,
    // including Babel for .js/.jsx and TypeScriptReact for .tsx.
    assert_languages(&[
        ("main.rs", "Rust"),
        ("server.go", "Go"),
        ("main.py", "Python"),
        ("types.pyi", "Python"),
        ("index.js", "JavaScript (Babel)"),
        ("index.mjs", "JavaScript (Babel)"),
        ("index.cjs", "JavaScript (Babel)"),
        ("View.jsx", "JavaScript (Babel)"),
        ("index.ts", "TypeScript"),
        ("index.mts", "TypeScript"),
        ("index.cts", "TypeScript"),
        ("View.tsx", "TypeScriptReact"),
        ("main.c", "C"),
        ("header.h", "C++"),
        ("main.cpp", "C++"),
        ("header.hpp", "C++"),
        ("main.cs", "C#"),
        ("Main.java", "Java"),
        ("Main.kt", "Kotlin"),
        ("build.gradle.kts", "Kotlin"),
        ("View.swift", "Swift"),
        ("main.rb", "Ruby"),
        ("init.lua", "Lua"),
        ("main.php", "PHP"),
        ("main.pl", "Perl"),
        ("server.ex", "Elixir"),
        ("script.exs", "Elixir"),
        ("Main.scala", "Scala"),
        ("main.dart", "Dart"),
        ("Main.hs", "Haskell"),
        ("main.zig", "Zig"),
        ("script.sh", "Bourne Again Shell (bash)"),
        ("script.zsh", "Bourne Again Shell (bash)"),
        ("script.fish", "Fish"),
        ("script.ps1", "PowerShell"),
        ("setup.bat", "Batch File"),
        ("analysis.R", "R"),
        ("index.html", "HTML"),
        ("style.css", "CSS"),
        ("style.scss", "SCSS"),
        ("style.sass", "Sass"),
        ("View.vue", "Vue Component"),
        ("View.svelte", "Svelte"),
        ("view.html.erb", "HTML (Rails)"),
        ("view.html.j2", "HTML (Jinja2)"),
        ("view.js.erb", "JavaScript (Rails)"),
        ("README.md", "Markdown"),
        ("guide.mdx", "Markdown"),
        ("query.sql", "SQL"),
        ("config.json", "JSON"),
        ("config.jsonc", "JSON"),
        ("config.json5", "JSON"),
        ("data.yaml", "YAML"),
        ("data.yml", "YAML"),
        ("Cargo.toml", "TOML"),
        ("Cargo.lock", "TOML"),
        ("Pipfile", "TOML"),
        ("uv.lock", "TOML"),
        ("config.ini", "INI"),
        ("setup.cfg", "INI"),
        ("document.xml", "XML"),
        ("icon.svg", "XML"),
        ("messages.properties", "Java Properties"),
        ("data.csv", "Comma Separated Values"),
        ("data.tsv", "Comma Separated Values"),
        ("main.tf", "Terraform"),
        ("values.tfvars", "Terraform"),
        ("main.hcl", "Terraform"),
        ("default.nix", "Nix"),
        ("schema.proto", "Protocol Buffer"),
        ("schema.graphql", "GraphQL"),
        ("change.diff", "Diff"),
        ("change.patch", "Diff"),
        ("paper.tex", "LaTeX"),
        ("Dockerfile", "Dockerfile"),
        ("Containerfile", "Dockerfile"),
        ("image.dockerfile", "Dockerfile"),
        ("Makefile", "Makefile"),
        ("GNUmakefile", "Makefile"),
        ("CMakeLists.txt", "CMake"),
        ("CMakeLists.TXT", "CMake"),
        ("build.cmake", "CMake"),
        ("Gemfile", "Ruby"),
        ("Rakefile", "Ruby"),
        ("Brewfile", "Ruby"),
        ("requirements.txt", "Requirements.txt"),
        ("requirements.in", "Requirements.txt"),
        (".bashrc", "Bourne Again Shell (bash)"),
        (".zshrc", "Bourne Again Shell (bash)"),
        (".profile", "Bourne Again Shell (bash)"),
        ("MAIN.RS", "Rust"),
    ]);
}

#[test]
fn similar_names_and_unrelated_config_files_are_not_misclassified() {
    for path in [
        ".environment",
        ".envoy",
        ".envoy.yaml",
        "project/config",
        "notes.txt",
        "ssh/config",
        "home/.ssh/notes",
        "home/.ssh/project/config",
        "sshd_config.backup",
        "home/.ssh/config.d/README.md",
        "etc/ssh/sshd_config.d/script.py",
    ] {
        let expected = if path.ends_with(".yaml") {
            "YAML"
        } else if path.ends_with(".md") {
            "Markdown"
        } else if path.ends_with(".py") {
            "Python"
        } else {
            "Plain Text"
        };
        assert_eq!(
            Editor::from_text(path, "").status().language,
            expected,
            "{path}"
        );
    }
}

#[test]
fn common_shebangs_use_the_executable_and_respect_explicit_file_types() {
    for (first, expected) in [
        ("#!/bin/bash", "Bourne Again Shell (bash)"),
        ("#!/bin/sh -e", "Bourne Again Shell (bash)"),
        ("#!/usr/bin/env zsh", "Bourne Again Shell (bash)"),
        ("#!/usr/bin/env python3", "Python"),
        ("#!/usr/bin/python3.13 -I", "Python"),
        ("#!/usr/bin/env -S python3 -I", "Python"),
        ("#!/usr/bin/env -i MODE=dev python3", "Python"),
        ("#!/usr/bin/env -u PYTHONPATH python3", "Python"),
        ("#!/usr/bin/env pypy3", "Python"),
        ("#!/usr/bin/env ruby", "Ruby"),
        ("#!/usr/bin/ruby3.3", "Ruby"),
        ("#!/usr/bin/env node", "JavaScript"),
        ("#!/usr/bin/env nodejs", "JavaScript"),
        (
            "#!/usr/bin/env -S node --experimental-strip-types",
            "JavaScript",
        ),
        ("#!/usr/bin/env bun", "JavaScript"),
        ("#!/usr/bin/env deno", "JavaScript"),
        ("#!/usr/bin/env perl", "Perl"),
        ("#!/usr/bin/env lua", "Lua"),
        ("#!/usr/bin/env fish", "Fish"),
        ("#!/usr/bin/env pwsh", "PowerShell"),
        ("#!/usr/bin/env Rscript", "R"),
        ("#!/usr/bin/env swift", "Swift"),
        ("#!/usr/bin/env php", "PHP"),
        ("#!/usr/bin/php8.3", "PHP"),
        ("#!/usr/bin/lua5.4", "Lua"),
        ("#!/usr/bin/env julia", "Julia"),
        ("#!/usr/bin/env awk", "AWK"),
        ("#!/usr/bin/osascript", "AppleScript"),
        ("#!/usr/bin/env dmd -run", "D"),
        ("#!/usr/bin/env escript", "Erlang"),
        ("#!/usr/bin/env make -f", "Makefile"),
        ("#!/usr/bin/env coffee", "CoffeeScript"),
        ("#!/usr/bin/env crystal", "Crystal"),
        ("#!/usr/bin/env elixir", "Elixir"),
        ("#!/usr/bin/env jq -f", "JQ"),
    ] {
        let text = format!("{first}\nexample");
        assert_eq!(
            Editor::from_text("script", &text).status().language,
            expected,
            "{first}"
        );
        assert_eq!(
            Editor::from_text("script.rs", &text).status().language,
            "Rust",
            "{first}"
        );
        assert_eq!(
            Editor::from_text("script.txt", &text).status().language,
            "Plain Text",
            "{first}"
        );
    }
    for first in [
        "#!/usr/bin/env python-helper",
        "#!/usr/bin/notpython",
        "#!/usr/bin/env echo python3",
        "#!/usr/bin/env",
        "#!/usr/bin/env -u",
    ] {
        assert_eq!(
            Editor::from_text("script", first).status().language,
            "Plain Text",
            "{first}"
        );
    }
}

#[test]
fn content_detection_stays_bounded_and_does_not_guess_from_prose() {
    for (text, expected) in [
        ("\u{feff}#!/usr/bin/env python3\nprint(1)", "Python"),
        (" {\"enabled\":true}", "JSON"),
        ("[1, 2, 3]", "JSON"),
        ("<?xml version=\"1.0\"?><root/>", "XML"),
        ("<!DOCTYPE html>\n<html></html>", "HTML"),
        ("<?php echo 1;", "PHP"),
        ("Some prose about fn, class and python.", "Plain Text"),
        ("A tutorial mentions <html> in a sentence.", "Plain Text"),
        ("[a section heading]", "Plain Text"),
        ("{ unfinished", "Plain Text"),
        ("PORT=3000", "Plain Text"),
        ("Host development\n Port 2222", "Plain Text"),
        ("\n#!/usr/bin/env python3", "Plain Text"),
    ] {
        assert_eq!(
            Editor::from_text("unknown", text).status().language,
            expected,
            "{text}"
        );
        assert_eq!(
            Editor::from_text("notes.txt", text).status().language,
            "Plain Text",
            "{text}"
        );
    }
    let large = format!("{}{{\"enabled\":true}}", " ".repeat(8192));
    assert!(syntax::detect_document(Path::new("unknown"), &large).is_none());
}

fn highlighted(path: &str, text: &str) -> Vec<Vec<Span>> {
    let buffer = Buffer::from_text(text);
    let mut hl = Highlighter::new(
        syntax::detect_document(Path::new(path), text),
        buffer.line_count(),
    );
    assert!(!hl.is_plain(), "{path}");
    assert!(hl.ensure(buffer.lines(), buffer.line_count(), None));
    (0..buffer.line_count())
        .map(|i| hl.line_spans(i).expect(path).to_vec())
        .collect()
}

fn token<'a>(spans: &'a [Vec<Span>], text: &str, line: &str, literal: &str) -> &'a Span {
    let row = text.lines().position(|s| s == line).expect(line);
    let col = line.find(literal).expect(literal) as u32;
    spans[row]
        .iter()
        .find(|s| s.start <= col && col < s.end)
        .expect(literal)
}

#[test]
fn dotenv_and_ssh_real_spans_remain_distinct_in_dark_and_light_themes() {
    let original = kiln_common::Theme::current();
    for theme in ["kiln-dark", "kiln-light"] {
        kiln_common::Theme::set_current(theme);
        let env = concat!(
            "# local settings\n",
            "export PORT=\"3000\"\n",
            "API_URL=https://example.test\n",
            "  SPACED_KEY   =  \"ready\" # spaced\n",
            "ESCAPED=\"hello \\\"quoted\\\" ${HOME}\\nnext\"\n",
            "MULTILINE=\"first\nsecond # literal\"\n",
            "SINGLE='literal # value'\n",
            "AFTER=done # trailing\n",
        );
        let spans = highlighted(".env.local", env);
        let color = |line, value| token(&spans, env, line, value).color;
        let comment = color("# local settings", "local");
        let key = color("export PORT=\"3000\"", "PORT");
        let value = color("export PORT=\"3000\"", "3000");
        let assignment = color("export PORT=\"3000\"", "=");
        for (a, b) in [
            (key, value),
            (key, comment),
            (value, comment),
            (assignment, key),
            (assignment, value),
        ] {
            assert_ne!(a, b, "{theme}: DotENV key/value/comment/assignment");
        }
        assert_ne!(
            color("API_URL=https://example.test", "API_URL"),
            color("API_URL=https://example.test", "https")
        );
        assert_ne!(
            color("  SPACED_KEY   =  \"ready\" # spaced", "SPACED_KEY"),
            color("  SPACED_KEY   =  \"ready\" # spaced", "ready")
        );
        assert_ne!(
            color("  SPACED_KEY   =  \"ready\" # spaced", "="),
            color("  SPACED_KEY   =  \"ready\" # spaced", "ready")
        );
        assert_eq!(
            color("  SPACED_KEY   =  \"ready\" # spaced", "spaced"),
            comment
        );
        assert_ne!(
            color("ESCAPED=\"hello \\\"quoted\\\" ${HOME}\\nnext\"", "\\n"),
            value
        );
        assert_ne!(
            color("ESCAPED=\"hello \\\"quoted\\\" ${HOME}\\nnext\"", "HOME"),
            value
        );
        assert_eq!(
            color("second # literal\"", "#"),
            value,
            "multiline # stays a string"
        );
        assert_eq!(
            color("SINGLE='literal # value'", "#"),
            value,
            "single-quoted # stays a string"
        );
        assert_eq!(color("AFTER=done # trailing", "trailing"), comment);
        assert_ne!(
            color("AFTER=done # trailing", "AFTER"),
            value,
            "multiline state must close"
        );
        assert!(token(&spans, env, "# local settings", "local").italic);

        for (path, text, directive, key, value_line, value_literal) in [
            (
                "home/.ssh/config",
                concat!(
                    "# connection\nHost development !production\n  hOsTnAmE = example.test\n",
                    "  Port 2222\n  IdentityFile \"${HOME}/.ssh/key %h\" # identity\n",
                    "Match host *.test user developer\n  ForwardAgent yes\n",
                    "Include ~/.ssh/config.d/*.conf\n  ProxyCommand ssh -W %h:%p jump\n",
                ),
                "Host development !production",
                "Host",
                "  hOsTnAmE = example.test",
                "example.test",
            ),
            (
                "etc/ssh/sshd_config",
                concat!(
                    "# connection\nPort 2222\nPermitRootLogin no\n",
                    "Match User deploy Address 192.0.2.0/24\n",
                    "  AuthorizedKeysFile \"${HOME}/.ssh/keys-%u\" # identity\n",
                    "  ForceCommand \"echo # literal\"\nInclude /etc/ssh/sshd_config.d/*.conf\n",
                ),
                "Port 2222",
                "Port",
                "  ForceCommand \"echo # literal\"",
                "echo",
            ),
        ] {
            let spans = highlighted(path, text);
            let color = |line, value| token(&spans, text, line, value).color;
            let directive_color = color(directive, key);
            let value_color = color(value_line, value_literal);
            let comment = color("# connection", "connection");
            assert_ne!(
                directive_color, value_color,
                "{theme} {path}: directive/value"
            );
            assert_ne!(
                directive_color, comment,
                "{theme} {path}: directive/comment"
            );
            assert_ne!(comment, value_color, "{theme} {path}: comment/value");
            let port_line = if path.ends_with("sshd_config") {
                "Port 2222"
            } else {
                "  Port 2222"
            };
            assert_ne!(
                color(port_line, "Port"),
                color(port_line, "2222"),
                "{theme} {path}: port"
            );
            let identity = text.lines().find(|l| l.contains("# identity")).unwrap();
            assert_eq!(
                color(identity, "identity"),
                comment,
                "{theme} {path}: inline comment"
            );
            assert_ne!(
                color(identity, "${HOME}"),
                value_color,
                "{theme} {path}: environment variable"
            );
            let percent = if path.ends_with("sshd_config") {
                "%u"
            } else {
                "%h"
            };
            assert_ne!(
                color(identity, percent),
                value_color,
                "{theme} {path}: percent expansion"
            );
            let include = text.lines().find(|l| l.starts_with("Include")).unwrap();
            assert_ne!(
                color(include, "Include"),
                color(include, ".conf"),
                "{theme} {path}: include path"
            );
        }
        let broken = "Host dev\n  IdentityFile \"unfinished\n  Port 2222\n";
        let spans = highlighted("home/.ssh/config", broken);
        assert_ne!(
            token(&spans, broken, "  Port 2222", "Port").color,
            token(&spans, broken, "  Port 2222", "2222").color,
            "{theme}: incomplete SSH quotes must not leak into the next directive"
        );

        // Exercise actual spans for representative common source/config formats,
        // independently of the grammar-name audit above.
        for (path, text, key_line, key, value_line, value) in [
            (
                "main.rs",
                "fn main() { let text = \"ready\"; }",
                "fn main() { let text = \"ready\"; }",
                "fn",
                "fn main() { let text = \"ready\"; }",
                "ready",
            ),
            (
                "main.py",
                "return \"ready\"",
                "return \"ready\"",
                "return",
                "return \"ready\"",
                "ready",
            ),
            (
                "main.js",
                "const text = \"ready\";",
                "const text = \"ready\";",
                "const",
                "const text = \"ready\";",
                "ready",
            ),
            (
                "main.ts",
                "const text: string = \"ready\";",
                "const text: string = \"ready\";",
                "const",
                "const text: string = \"ready\";",
                "ready",
            ),
            (
                "main.go",
                "package main\nvar text = \"ready\"",
                "package main",
                "package",
                "var text = \"ready\"",
                "ready",
            ),
            (
                "main.cpp",
                "const char* text = \"ready\";",
                "const char* text = \"ready\";",
                "const",
                "const char* text = \"ready\";",
                "ready",
            ),
            (
                "main.java",
                "public class Demo { String text = \"ready\"; }",
                "public class Demo { String text = \"ready\"; }",
                "public",
                "public class Demo { String text = \"ready\"; }",
                "ready",
            ),
            (
                "main.kt",
                "val text = \"ready\"",
                "val text = \"ready\"",
                "val",
                "val text = \"ready\"",
                "ready",
            ),
            (
                "main.swift",
                "let text = \"ready\"",
                "let text = \"ready\"",
                "let",
                "let text = \"ready\"",
                "ready",
            ),
            (
                "main.rb",
                "return \"ready\"",
                "return \"ready\"",
                "return",
                "return \"ready\"",
                "ready",
            ),
            (
                "main.lua",
                "return \"ready\"",
                "return \"ready\"",
                "return",
                "return \"ready\"",
                "ready",
            ),
            (
                "script.sh",
                "if true; then echo \"ready\"; fi",
                "if true; then echo \"ready\"; fi",
                "if",
                "if true; then echo \"ready\"; fi",
                "ready",
            ),
            (
                "data.json",
                "{\"name\": \"ready\"}",
                "{\"name\": \"ready\"}",
                "name",
                "{\"name\": \"ready\"}",
                "ready",
            ),
            (
                "data.yaml",
                "name: \"ready\"",
                "name: \"ready\"",
                "name",
                "name: \"ready\"",
                "ready",
            ),
            (
                "data.toml",
                "name = \"ready\"",
                "name = \"ready\"",
                "name",
                "name = \"ready\"",
                "ready",
            ),
            (
                ".editorconfig",
                "[*.rs]\nindent_size = 4",
                "[*.rs]",
                "rs",
                "indent_size = 4",
                "4",
            ),
            (
                ".npmrc",
                "# local\nregistry=https://example.test",
                "# local",
                "local",
                "registry=https://example.test",
                "https",
            ),
            (
                ".git/config",
                "[core]\n  editor = \"kiln\"",
                "[core]",
                "core",
                "  editor = \"kiln\"",
                "kiln",
            ),
            (
                "doc.xml",
                "<item name=\"ready\"/>",
                "<item name=\"ready\"/>",
                "item",
                "<item name=\"ready\"/>",
                "ready",
            ),
            (
                "index.html",
                "<div title=\"ready\">text</div>",
                "<div title=\"ready\">text</div>",
                "div",
                "<div title=\"ready\">text</div>",
                "ready",
            ),
            (
                "style.css",
                ".item { color: red; }",
                ".item { color: red; }",
                "color",
                ".item { color: red; }",
                "red",
            ),
            (
                "query.sql",
                "SELECT 'ready';",
                "SELECT 'ready';",
                "SELECT",
                "SELECT 'ready';",
                "ready",
            ),
            (
                "Dockerfile",
                "FROM alpine:3.20",
                "FROM alpine:3.20",
                "FROM",
                "FROM alpine:3.20",
                "alpine",
            ),
            (
                "CMakeLists.txt",
                "set(NAME \"ready\")",
                "set(NAME \"ready\")",
                "set",
                "set(NAME \"ready\")",
                "ready",
            ),
        ] {
            let spans = highlighted(path, text);
            assert_ne!(
                token(&spans, text, key_line, key).color,
                token(&spans, text, value_line, value).color,
                "{theme}: representative tokens in {path}"
            );
        }
    }
    kiln_common::Theme::set_current(original.name);
}

#[test]
fn new_language_grammars_link_without_unresolved_contexts_and_support_comments() {
    assert!(
        syntax::assets().set.find_unlinked_contexts().is_empty(),
        "unresolved grammar includes"
    );
    for (path, grammar) in [
        (".env", "DotENV"),
        ("home/.ssh/config", "SSH Config"),
        ("sshd_config", "SSHD Config"),
    ] {
        assert_eq!(Editor::from_text(path, "").status().language, grammar);
        assert_eq!(
            syntax::comment_style(grammar),
            Some(syntax::CommentStyle::Line("#"))
        );
        assert_eq!(
            syntax::assets()
                .set
                .syntaxes()
                .iter()
                .filter(|s| s.name == grammar && !s.hidden)
                .count(),
            1,
            "one visible language-selector entry for {grammar}"
        );
        let mut e = Editor::from_text(path, "Port 2222");
        e.toggle_comment();
        assert!(e.text().starts_with("# "));
    }
}

#[test]
fn overrides_dirty_edits_recovery_and_reload_keep_language_identity_current() {
    // Automatic detection runs at the start of a UI frame, keyed by buffer
    // version. Exercise that real refresh path after unsaved API edits too.
    let frame = |editor: &mut Editor| {
        let context = egui::Context::default();
        context.set_fonts(kiln_common::fonts::definitions(false));
        kiln_common::Theme::current().apply(&context);
        context
            .run_ui(egui::RawInput::default(), |ui| editor.ui(ui))
            .drop_without_applying_deltas();
    };
    let mut editor = Editor::from_text("script", "#!/usr/bin/env python3\nprint(1)");
    assert_eq!(editor.status().language, "Python");
    editor.select_all();
    editor.insert_text("#!/usr/bin/env ruby\nputs 1");
    frame(&mut editor);
    assert_eq!(editor.status().language, "Ruby");
    assert!(editor.is_dirty());
    editor.set_language_override(Some("Plain Text".into()));
    editor.select_all();
    editor.insert_text("#!/usr/bin/env bash\necho 1");
    frame(&mut editor);
    assert_eq!(editor.status().language, "Plain Text");
    editor.set_language_override(None);
    assert_eq!(editor.status().language, "Bourne Again Shell (bash)");
    let draft = editor.recovery_draft().unwrap();
    let mut recovered = Editor::from_text("script", "");
    recovered.restore_draft(&draft);
    frame(&mut recovered);
    assert_eq!(recovered.status().language, "Bourne Again Shell (bash)");
    assert!(recovered.is_dirty());

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("script");
    std::fs::write(&path, "#!/usr/bin/env python3\nprint(1)").unwrap();
    let mut editor = Editor::open(&path).unwrap();
    std::fs::write(&path, "#!/usr/bin/env ruby\nputs 1").unwrap();
    editor.reload_from_disk().unwrap();
    assert_eq!(editor.status().language, "Ruby");
    editor.set_language_override(Some("Plain Text".into()));
    std::fs::write(&path, "#!/usr/bin/env node\nconsole.log(1)").unwrap();
    editor.reload_from_disk().unwrap();
    assert_eq!(editor.status().language, "Plain Text");
    editor.set_language_override(None);
    assert_eq!(editor.status().language, "JavaScript");
}
