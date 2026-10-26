//! Local command structure and bounded project argument providers.
use super::{Choice, ChoiceKind, quoted, words};
use serde::Deserialize;
use std::{
    collections::{BTreeSet, HashSet},
    io::Read,
    path::{Path, PathBuf},
    sync::OnceLock,
};

#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub(super) struct Spec {
    pub name: String,
    pub description: String,
    pub hint: String,
    pub provider: String,
    pub subcommands: Vec<Spec>,
    pub options: Vec<OptionDef>,
}
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub(super) struct OptionDef {
    pub name: Vec<String>,
    pub description: String,
    pub hint: String,
    pub provider: String,
    pub values: Vec<String>,
    pub persistent: bool,
}
fn catalog() -> &'static [Spec] {
    static SPECS: OnceLock<Vec<Spec>> = OnceLock::new();
    SPECS.get_or_init(|| {
        let mut specs: Vec<Spec> =
            serde_json::from_str(include_str!("specs.json")).expect("bundled completion catalog");
        let docker: Spec =
            serde_json::from_str(include_str!("docker.json")).expect("bundled Docker reference");
        if let Some(known) = specs.iter_mut().find(|s| s.name == "docker") {
            enrich(known, docker);
        }
        specs
    })
}

pub(super) fn candidates(
    buffer: &str,
    cursor: usize,
    cwd: &Path,
    native_commands: &[String],
) -> Vec<Choice> {
    if buffer.len() > 4096
        || buffer.chars().any(char::is_control)
        || cursor != buffer.chars().count()
    {
        return vec![];
    }
    let Some(tokens) = words(buffer) else {
        return vec![];
    };
    let Some((start, prefix)) = tokens.last() else {
        return vec![];
    };
    let mut rows = Vec::<Item>::new();
    if tokens.len() == 1 {
        rows.extend(catalog().iter().map(|s| Item::spec(s, ChoiceKind::Command)));
        rows.extend(
            native_commands
                .iter()
                .filter(|n| !n.contains('/') && valid(n))
                .map(|n| Item::new(n, "설치된 명령", "", ChoiceKind::Command)),
        );
        return finish(rows, buffer, *start, prefix);
    }
    let name = &tokens[0].1;
    let executable = native_commands
        .iter()
        .map(Path::new)
        .find(|p| p.is_absolute() && p.file_name().is_some_and(|n| n == name.as_str()));
    let fallback = catalog().iter().find(|s| s.name == *name);
    let mut root = fallback.cloned().unwrap_or_else(|| Spec {
        name: name.clone(),
        ..Default::default()
    });
    if let Some(executable) = executable {
        expand_help(&mut root, executable, name, &tokens);
    }
    if root.subcommands.is_empty() && root.options.is_empty() && root.provider.is_empty() {
        if prefix.contains('/') {
            rows = paths(cwd, prefix, false);
        }
        return finish(rows, buffer, *start, prefix);
    }

    from_spec(&root, buffer, &tokens, cwd)
}

// Installed help is authoritative for options. Bundled metadata fills path kinds
// and translated explanations, and is also available when a tool has no help.
fn enrich(fallback: &mut Spec, mut fresh: Spec) {
    if fresh.name.is_empty() {
        fresh.name = fallback.name.clone();
    }
    if !fallback.description.is_empty() {
        fresh.description = fallback.description.clone();
    }
    if !fallback.hint.is_empty() {
        fresh.hint = fallback.hint.clone();
    }
    if fresh.provider.is_empty() {
        fresh.provider = fallback.provider.clone();
    }
    for option in &mut fresh.options {
        if let Some(known) = fallback
            .options
            .iter()
            .find(|k| k.name.iter().any(|n| option.name.contains(n)))
        {
            if !known.description.is_empty() {
                option.description = known.description.clone();
            }
            if !option.hint.is_empty() && !known.hint.is_empty() {
                option.hint = known.hint.clone();
            }
            if option.provider.is_empty() {
                option.provider = known.provider.clone();
            }
            option.persistent |= known.persistent;
        }
    }
    if fresh.options.is_empty() {
        fresh.options = fallback.options.clone();
    }
    if fresh.subcommands.is_empty() {
        fresh.subcommands = fallback.subcommands.clone();
    } else {
        for child in &mut fresh.subcommands {
            if let Some(known) = fallback.subcommands.iter().find(|s| s.name == child.name) {
                let mut merged = known.clone();
                enrich(&mut merged, child.clone());
                *child = merged;
            }
        }
        // Common commands keep their familiar ordering; new installed commands
        // still appear without maintaining a per-version list in Kiln.
        fresh.subcommands.sort_by_key(|s| {
            fallback
                .subcommands
                .iter()
                .position(|k| k.name == s.name)
                .unwrap_or(usize::MAX)
        });
    }
    *fallback = fresh;
}
fn expand_help(root: &mut Spec, executable: &Path, name: &str, tokens: &[(usize, String)]) {
    if let Some(spec) = super::discovery::discover(executable, name, &[]) {
        enrich(root, spec);
    }
    let mut current = root;
    let mut inherited = current
        .options
        .iter()
        .filter(|o| o.persistent)
        .cloned()
        .collect::<Vec<_>>();
    let mut chain = Vec::new();
    let mut pending = false;
    for (_, word) in &tokens[1..tokens.len() - 1] {
        if pending {
            pending = false;
            continue;
        }
        if word == "--" {
            break;
        }
        if word.starts_with('-') {
            let key = word.split('=').next().unwrap();
            let Some(option) = current
                .options
                .iter()
                .chain(inherited.iter())
                .find(|o| o.name.iter().any(|n| n == key))
            else {
                break;
            };
            pending = !option.hint.is_empty() && !word.contains('=');
            continue;
        }
        let Some(index) = current.subcommands.iter().position(|s| s.name == *word) else {
            break;
        };
        if chain.len() == 4 {
            break;
        }
        chain.push(word.clone());
        current = &mut current.subcommands[index];
        if let Some(spec) = super::discovery::discover(executable, name, &chain) {
            enrich(current, spec);
        }
        inherited.extend(current.options.iter().filter(|o| o.persistent).cloned());
    }
}

fn from_spec(root: &Spec, buffer: &str, tokens: &[(usize, String)], cwd: &Path) -> Vec<Choice> {
    let (start, prefix) = tokens.last().unwrap();
    let mut chain = vec![root];
    let mut current = root;
    let mut used = HashSet::<String>::new();
    let mut pending: Option<&OptionDef> = None;
    let mut positionals = 0;
    let mut end_options = false;
    for (_, word) in &tokens[1..tokens.len() - 1] {
        if pending.take().is_some() {
            continue;
        }
        if !end_options && word == "--" {
            end_options = true;
            continue;
        }
        if !end_options && word.starts_with('-') {
            let key = word.split('=').next().unwrap();
            let Some(option) = options(&chain)
                .into_iter()
                .find(|o| o.name.iter().any(|n| n == key))
            else {
                return vec![];
            };
            used.extend(option.name.iter().cloned());
            if !option.hint.is_empty() && !word.contains('=') {
                pending = Some(option);
            }
        } else if !end_options
            && positionals == 0
            && let Some(child) = current.subcommands.iter().find(|s| s.name == *word)
        {
            current = child;
            chain.push(child);
        } else {
            positionals += 1;
        }
    }
    let mut rows = vec![];
    let mut value_prefix = prefix.as_str();
    let mut inline_key = None;
    // Recognize assignments only in an option position. Literal '=' in a
    // positional path or an already pending option value belongs to the path.
    if pending.is_none()
        && !end_options
        && prefix.starts_with('-')
        && let Some((key, value)) = prefix.split_once('=')
    {
        let Some(option) = options(&chain)
            .into_iter()
            .find(|o| o.name.iter().any(|n| n == key))
        else {
            return vec![];
        };
        pending = Some(option);
        value_prefix = value;
        inline_key = Some(key);
    }
    if let Some(option) = pending {
        rows.extend(
            option
                .values
                .iter()
                .map(|v| Item::new(v, &option.description, "", ChoiceKind::Value)),
        );
        rows.extend(arguments(&option.provider, cwd, value_prefix));
    } else if !end_options && prefix.starts_with('-') {
        for option in options(&chain) {
            if option.name.iter().any(|n| used.contains(n)) {
                continue;
            }
            rows.extend(
                option
                    .name
                    .iter()
                    .map(|n| Item::new(n, &option.description, &option.hint, ChoiceKind::Option)),
            );
        }
    } else {
        if positionals == 0 && !end_options {
            rows.extend(
                current
                    .subcommands
                    .iter()
                    .map(|s| Item::spec(s, ChoiceKind::Subcommand)),
            );
        }
        if matches!(current.provider.as_str(), "files" | "directories") || positionals == 0 {
            rows.extend(arguments(&current.provider, cwd, prefix));
        }
        if rows.is_empty() && prefix.is_empty() && current.subcommands.is_empty() && !end_options {
            for option in options(&chain) {
                if option.name.iter().any(|n| used.contains(n)) {
                    continue;
                }
                rows.extend(
                    option.name.iter().map(|n| {
                        Item::new(n, &option.description, &option.hint, ChoiceKind::Option)
                    }),
                );
            }
        }
    }
    let mut choices = finish(rows, buffer, *start, value_prefix);
    if let Some(key) = inline_key {
        for choice in &mut choices {
            // Replace the entire raw shell word. Its decoded key length is
            // not an offset into a token containing quotes or escapes.
            choice.buffer = format!("{}{}={}", &buffer[..*start], key, quoted(&choice.label));
            choice.cursor = choice.buffer.chars().count();
        }
    }
    choices
}
fn options<'a>(chain: &[&'a Spec]) -> Vec<&'a OptionDef> {
    chain
        .iter()
        .enumerate()
        .flat_map(|(i, s)| {
            s.options
                .iter()
                .filter(move |o| i + 1 == chain.len() || o.persistent)
        })
        .collect()
}
struct Item {
    label: String,
    description: String,
    hint: String,
    kind: ChoiceKind,
}
impl Item {
    fn new(label: &str, description: &str, hint: &str, kind: ChoiceKind) -> Self {
        Self {
            label: label.into(),
            description: description.into(),
            hint: hint.into(),
            kind,
        }
    }
    fn spec(spec: &Spec, kind: ChoiceKind) -> Self {
        Self::new(&spec.name, &spec.description, &spec.hint, kind)
    }
}
fn valid(word: &str) -> bool {
    !word.is_empty() && !word.chars().any(char::is_control) && word.len() <= 1024
}
fn finish(mut rows: Vec<Item>, buffer: &str, start: usize, prefix: &str) -> Vec<Choice> {
    rows.retain(|r| valid(&r.label) && r.label.starts_with(prefix) && r.label != prefix);
    let mut seen = HashSet::new();
    rows.retain(|r| seen.insert(r.label.clone()));
    rows.into_iter()
        .take(80)
        .map(|r| {
            let buffer = format!("{}{}", &buffer[..start], quoted(&r.label));
            let cursor = buffer.chars().count();
            Choice {
                label: r.label,
                description: r.description,
                hint: r.hint,
                kind: r.kind,
                buffer,
                cursor,
            }
        })
        .collect()
}
fn arguments(provider: &str, cwd: &Path, prefix: &str) -> Vec<Item> {
    match provider {
        "files" => paths(cwd, prefix, false),
        "directories" => paths(cwd, prefix, true),
        "scripts" => package_scripts(cwd),
        "branches" => branches(cwd),
        _ => vec![],
    }
}
fn paths(cwd: &Path, prefix: &str, directories_only: bool) -> Vec<Item> {
    let (base, leaf) = prefix
        .rfind('/')
        .map_or(("", prefix), |i| (&prefix[..=i], &prefix[i + 1..]));
    let dir = if Path::new(base).is_absolute() {
        PathBuf::from(base)
    } else {
        cwd.join(base)
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut rows = vec![];
    for entry in entries.take(2048).flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !valid(name)
            || !name.starts_with(leaf)
            || (name.starts_with('.') && !leaf.starts_with('.'))
        {
            continue;
        }
        let is_dir = entry.path().is_dir();
        if directories_only && !is_dir {
            continue;
        }
        rows.push(Item::new(
            &format!("{base}{name}{}", if is_dir { "/" } else { "" }),
            if is_dir { "디렉터리" } else { "파일" },
            "",
            if is_dir {
                ChoiceKind::Directory
            } else {
                ChoiceKind::File
            },
        ));
    }
    rows.sort_by(|a, b| {
        (a.kind != ChoiceKind::Directory, &a.label)
            .cmp(&(b.kind != ChoiceKind::Directory, &b.label))
    });
    if directories_only && base.is_empty() && (leaf.is_empty() || "../".starts_with(leaf)) {
        rows.push(Item::new("../", "상위 디렉터리", "", ChoiceKind::Directory));
    }
    rows
}
fn read(path: &Path, max: usize) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    file.take((max + 1) as u64).read_to_string(&mut text).ok()?;
    (text.len() <= max).then_some(text)
}
fn package_scripts(cwd: &Path) -> Vec<Item> {
    for path in cwd.ancestors().take(16) {
        let file = path.join("package.json");
        if !file.is_file() {
            continue;
        }
        let Some(text) = read(&file, 512 * 1024) else {
            return vec![];
        };
        let Some(json) = serde_json::from_str::<serde_json::Value>(&text).ok() else {
            return vec![];
        };
        let Some(scripts) = json.get("scripts").and_then(|s| s.as_object()) else {
            return vec![];
        };
        return scripts
            .iter()
            .filter(|(_, v)| v.is_string())
            .take(2048)
            .map(|(name, _)| Item::new(name, "프로젝트 패키지 스크립트", "", ChoiceKind::Script))
            .collect();
    }
    vec![]
}
fn branches(cwd: &Path) -> Vec<Item> {
    let Some(git) = cwd
        .ancestors()
        .take(32)
        .map(|p| p.join(".git"))
        .find(|p| p.exists())
    else {
        return vec![];
    };
    let git = if git.is_dir() {
        git
    } else {
        let Some(text) = read(&git, 8192) else {
            return vec![];
        };
        let Some(path) = text.trim().strip_prefix("gitdir: ") else {
            return vec![];
        };
        git.parent().unwrap().join(path)
    };
    let common = read(&git.join("commondir"), 8192)
        .map(|s| git.join(s.trim()))
        .unwrap_or(git);
    let mut names = BTreeSet::new();
    let base = common.join("refs/heads");
    let mut stack = vec![(base.clone(), 0)];
    let mut seen = 0;
    while let Some((dir, depth)) = stack.pop() {
        if depth >= 16 || seen >= 4096 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.take(4096 - seen).flatten() {
            seen += 1;
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                stack.push((entry.path(), depth + 1));
            } else if kind.is_file()
                && let Ok(path) = entry.path().strip_prefix(&base)
            {
                names.insert(path.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    if let Some(text) = read(&common.join("packed-refs"), 2 * 1024 * 1024) {
        for line in text.lines().take(8192) {
            if let Some((_, name)) = line.split_once(' ')
                && let Some(name) = name.strip_prefix("refs/heads/")
            {
                names.insert(name.to_owned());
            }
        }
    }
    names
        .into_iter()
        .map(|n| Item::new(&n, "로컬 Git 브랜치", "", ChoiceKind::Branch))
        .collect()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn newly_installed_cli_is_discovered_and_typed_values_never_reach_help() {
        let Some(root) = std::env::var_os("KILN_COMPLETION_DISCOVERY_ROOT") else {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().canonicalize().unwrap();
            let output=std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact","app::terminal_completion::suggestions::tests::newly_installed_cli_is_discovered_and_typed_values_never_reach_help","--nocapture","--test-threads=1"])
                .env("HOME",&root).env("KILN_COMPLETION_DISCOVERY_ROOT",&root).output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        };
        let root = PathBuf::from(root);
        let bin = root.join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let project = root.join("project");
        std::fs::create_dir_all(project.join("source folder")).unwrap();
        std::fs::write(project.join("source.txt"), "fixture").unwrap();
        let executable = bin.join("fresh-cli");
        let calls = root.join("help-calls");
        let script = format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> '{}'
case "$*" in
  --help) printf 'Usage: fresh-cli [COMMAND]\n\nCommands:\n  serve*  Start the fixture service\n\nOptions:\n  --help  Show help\n' ;;
  'serve --help') printf 'Usage: fresh-cli serve [OPTIONS]\n\nOptions:\n  --directory <DIRECTORY>  Project directory\n  --file <FILE>            Input file\n  --message <MESSAGE>      Literal message\n  --help                   Show help\n' ;;
  *) exit 9 ;;
esac
"#,
            calls.display()
        );
        std::fs::write(&executable, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let native = vec![executable.to_string_lossy().into_owned()];
        for (input, expected) in [
            ("fresh-cli se", "fresh-cli serve"),
            ("fresh-cli serve --d", "fresh-cli serve --directory"),
            (
                "fresh-cli serve --directory so",
                "fresh-cli serve --directory 'source folder/'",
            ),
            (
                "fresh-cli serve --directory=so",
                "fresh-cli serve --directory='source folder/'",
            ),
            (
                "fresh-cli serve --file source.t",
                "fresh-cli serve --file source.txt",
            ),
        ] {
            let rows = candidates(input, input.chars().count(), &project, &native);
            assert!(
                rows.iter().any(|r| r.buffer == expected),
                "{input:?}: {rows:?}"
            );
        }
        let secret = "fresh-cli serve --message PRIVATE_VALUE";
        assert!(candidates(secret, secret.len(), &project, &native).is_empty());
        assert_eq!(
            std::fs::read_to_string(calls).unwrap(),
            "--help\nserve --help\n",
            "only recognized help invocations may run; cached help should be reused"
        );
    }
}
