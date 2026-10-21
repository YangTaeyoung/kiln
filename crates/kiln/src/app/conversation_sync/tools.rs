//! Tool calls recognised in the readable tool records of the canonical history, and the
//! native records that show them in each CLI.
use serde_json::{Map, Value, json};

/// How the command was recorded by the CLI it came from.
pub(super) enum Native {
    /// Claude Code `Bash`: its input and tool result text.
    Claude { input: Value, content: String },
    /// Codex code mode: the `exec` program, the result header and the printed output.
    Codex { program: String, header: String, result: String },
    /// Codex `exec_command` function call: its arguments and output text.
    Function { arguments: String, output: String },
}

/// A shell command and its result.
pub(super) struct Shell {
    /// Tool-use or call ID of the original record.
    pub id: String,
    pub command: String,
    pub cwd: Option<String>,
    pub output: String,
    pub exit_code: i64,
    /// Codex: the process session of a command still running when its output was recorded.
    pub running: Option<i64>,
    pub native: Native,
}

fn record<'a>(text: &'a str, kind: &str) -> Option<(&'a str, &'a str)> {
    text.strip_prefix(kind)?.split_once("]\n")
}

/// Claude Code's own texts for a tool use that was refused or failed before it ran.
const CLAUDE_REFUSALS: [&str; 4] = ["Permission for", "The user doesn't want", "<tool_use_error>", "The server-side"];

/// Arguments of a Codex `exec_command` call.
const EXEC_FIELDS: [&str; 7] = ["cmd", "workdir", "max_output_tokens", "yield_time_ms", "login", "shell", "tty"];

/// The output, exit code and running session of one recorded command.
struct Outcome {
    output: String,
    exit_code: i64,
    running: Option<i64>,
}

/// The shell command recorded by `call` and its result recorded by `result`, from Claude
/// Code `Bash`, a Codex code-mode program that runs one `tools.exec_command` call, or a Codex
/// `exec_command` function call.
pub(super) fn shell(call: &str, result: &str) -> Option<Shell> {
    let (name, body) = record(call, "[Tool call: ")?;
    let (id, output) = record(result, "[Tool result: ")?;
    match name {
        "Bash" => {
            let input: Value = serde_json::from_str(body).ok()?;
            let fields = input.as_object()?;
            if !fields.keys().all(|k| matches!(k.as_str(), "command" | "description" | "timeout"))
                || CLAUDE_REFUSALS.iter().any(|refusal| output.starts_with(refusal))
            {
                return None;
            }
            let command = fields.get("command")?.as_str()?.to_owned();
            let (exit_code, shown) = claude_exit(output);
            Some(Shell {
                id: id.into(),
                command,
                cwd: None,
                output: shown.into(),
                exit_code,
                running: None,
                native: Native::Claude { input, content: output.into() },
            })
        }
        "exec" => {
            let commands = exec_commands(body)?;
            let [(command, cwd)] = <[_; 1]>::try_from(commands).ok()?;
            let (header, rest) = exec_output(output)?;
            let [outcome] = <[_; 1]>::try_from(exec_results(body, rest, 1)?).ok()?;
            Some(Shell {
                id: id.into(),
                command,
                cwd,
                output: outcome.output,
                exit_code: outcome.exit_code,
                running: outcome.running,
                native: Native::Codex { program: body.into(), header: header.into(), result: rest.into() },
            })
        }
        "exec_command" => {
            let fields: Map<String, Value> = serde_json::from_str(body).ok()?;
            let (command, cwd) = exec_arguments(&fields)?;
            let outcome = function_output(output)?;
            Some(Shell {
                id: id.into(),
                command,
                cwd,
                output: outcome.output,
                exit_code: outcome.exit_code,
                running: outcome.running,
                native: Native::Function { arguments: body.into(), output: output.into() },
            })
        }
        _ => None,
    }
}

/// A failed Claude command result starts with `Exit code N`.
fn claude_exit(content: &str) -> (i64, &str) {
    if let Some(rest) = content.strip_prefix("Exit code ") {
        let (code, after) = rest.split_once('\n').unwrap_or((rest, ""));
        if let Ok(code) = code.parse() {
            return (code, after);
        }
    }
    (0, content)
}

/// `cmd` and `workdir` of `exec_command` arguments that hold only known fields.
fn exec_arguments(fields: &Map<String, Value>) -> Option<(String, Option<String>)> {
    if !fields.keys().all(|k| EXEC_FIELDS.contains(&k.as_str())) {
        return None;
    }
    let cmd = fields.get("cmd")?.as_str()?.to_owned();
    let workdir = match fields.get("workdir") {
        None => None,
        Some(value) => Some(value.as_str()?.to_owned()),
    };
    Some((cmd, workdir))
}

/// Whether `text` at `at` starts the identifier `tools`.
fn tools_at(text: &str, at: usize) -> bool {
    let before = text[..at].chars().next_back();
    let after = text[at + "tools".len()..].chars().next();
    !before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$')
        && !after.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

/// `cmd` and `workdir` of each `tools.exec_command` call of a code-mode program that uses no
/// other tool, in program order, and the program with each call replaced by `EXEC`.
fn exec_calls(program: &str) -> Option<(Vec<(String, Option<String>)>, String)> {
    let mut commands = Vec::new();
    let mut shape = String::new();
    let mut rest = program;
    while let Some(at) = rest.match_indices("tools").map(|(i, _)| i).find(|&i| tools_at(rest, i)) {
        shape.push_str(&rest[..at]);
        let (fields, after) = object(rest[at..].strip_prefix("tools.exec_command(")?)?;
        commands.push(exec_arguments(&fields)?);
        shape.push_str("EXEC");
        rest = after.trim_start().strip_prefix(')')?;
    }
    shape.push_str(rest);
    (!commands.is_empty()).then_some((commands, shape))
}

/// `cmd` and `workdir` of each call of a code-mode program that only runs
/// `tools.exec_command` calls, in program order.
fn exec_commands(program: &str) -> Option<Vec<(String, Option<String>)>> {
    exec_calls(program).map(|(commands, _)| commands)
}

/// The header of a code-mode program result and the output the program printed.
fn exec_output(output: &str) -> Option<(&str, &str)> {
    let split = output.find("Output:\n")? + "Output:\n".len();
    let (header, rest) = output.split_at(split);
    header.starts_with("Script ").then_some((header, rest))
}

/// `shape` without whitespace matches `pattern`, where `$` stands for one identifier that
/// is the same wherever it appears.
fn shaped(shape: &str, pattern: &str) -> bool {
    let text: String = shape.chars().filter(|c| !c.is_whitespace()).collect();
    let text = text.trim_end_matches(';');
    let mut name: Option<&str> = None;
    let mut rest = text;
    for (k, part) in pattern.split('$').enumerate() {
        if k > 0 {
            let end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(rest.len());
            if end == 0 || name.is_some_and(|n| n != &rest[..end]) {
                return false;
            }
            name = Some(&rest[..end]);
            rest = &rest[end..];
        }
        let Some(after) = rest.strip_prefix(part) else { return false };
        rest = after;
    }
    rest.is_empty()
}

/// Programs that print each command's whole result in program order.
fn prints_results(shape: &str, count: usize) -> bool {
    let sequential = vec!["text(awaitEXEC)"; count].join(";");
    let calls = vec!["EXEC"; count].join(",");
    shaped(shape, &sequential)
        || (count == 1 && shaped(shape, "const$=awaitEXEC;text($)"))
        || ["all", "allSettled"].iter().any(|kind| {
            let start = format!("const$=awaitPromise.{kind}([{calls}]);");
            shaped(shape, &format!("{start}$.forEach(text)"))
                || shaped(shape, &format!("{start}for(const$of$)text($)"))
        })
}

/// Programs that print only the output of their single command.
fn prints_output(shape: &str) -> bool {
    shaped(shape, "text((awaitEXEC).output)") || shaped(shape, "const$=awaitEXEC;text($.output)")
}

/// The outcome of each of `count` commands from the output of `program`: printed results
/// numbered by their position, whole results printed in program order, or the printed
/// output of a single command.
fn exec_results(program: &str, output: &str, count: usize) -> Option<Vec<Outcome>> {
    let (_, shape) = exec_calls(program)?;
    let values = serde_json::Deserializer::from_str(output)
        .into_iter::<Value>()
        .collect::<Result<Vec<_>, _>>()
        .ok()
        .filter(|values| values.len() == count);
    if let Some(values) = values {
        let numbered = values.iter().all(|v| v.get("i").is_some());
        if numbered || prints_results(&shape, count) {
            if let Some(outcomes) = values.iter().enumerate().map(|(k, v)| exec_value(v, k)).collect() {
                return Some(outcomes);
            }
        }
    }
    (count == 1 && prints_output(&shape)).then(|| vec![Outcome { output: output.into(), exit_code: 0, running: None }])
}

/// The outcome printed for the command at `position`: an `exec_command` result, optionally
/// settled by `Promise.allSettled` and numbered with `i`.
fn exec_value(value: &Value, position: usize) -> Option<Outcome> {
    let mut fields = value.as_object()?.clone();
    if let Some(i) = fields.remove("i") {
        if i.as_u64()? != position as u64 {
            return None;
        }
        if fields.len() == 1 && fields.contains_key("result") {
            return settled(fields.get("result")?);
        }
    }
    settled(&Value::Object(fields))
}

/// An `exec_command` result, or a fulfilled settlement of one or of its output text.
fn settled(value: &Value) -> Option<Outcome> {
    let fields = value.as_object()?;
    let output = fields.get("output").and_then(Value::as_str);
    if let (Some(output), Some(exit_code)) = (output, fields.get("exit_code").and_then(Value::as_i64)) {
        return Some(Outcome { output: output.into(), exit_code, running: None });
    }
    if let (Some(output), Some(session)) = (output, fields.get("session_id").and_then(Value::as_i64)) {
        return Some(Outcome { output: output.into(), exit_code: 0, running: Some(session) });
    }
    if fields.get("status").and_then(Value::as_str) != Some("fulfilled") || fields.len() != 2 {
        return None;
    }
    match (fields.get("value"), fields.get("result")) {
        (Some(value), None) => settled(value),
        (None, Some(Value::String(output))) => Some(Outcome { output: output.clone(), exit_code: 0, running: None }),
        (None, Some(result)) => settled(result),
        _ => None,
    }
}

/// The outcome of a Codex `exec_command` function call: its header lines and the output
/// after them.
fn function_output(text: &str) -> Option<Outcome> {
    let (header, output) = text.split_once("\nOutput:\n")?;
    let (mut exit_code, mut running) = (None, None);
    for line in header.lines() {
        if let Some(code) = line.strip_prefix("Process exited with code ") {
            exit_code = Some(code.parse().ok()?);
        } else if let Some(session) = line.strip_prefix("Process running with session ID ") {
            running = Some(session.parse().ok()?);
        } else if !["Chunk ID: ", "Wall time: ", "Original token count: "].iter().any(|p| line.starts_with(p)) {
            return None;
        }
    }
    match (exit_code, running) {
        (Some(exit_code), None) => Some(Outcome { output: output.into(), exit_code, running: None }),
        (None, Some(session)) => Some(Outcome { output: output.into(), exit_code: 0, running: Some(session) }),
        _ => None,
    }
}

/// The output text of one printed `exec_command` result, for readable tool records.
pub(super) fn printed_output(value: &Value) -> Option<String> {
    let mut fields = value.as_object()?.clone();
    fields.remove("i");
    let value = match fields.remove("result") {
        Some(result) if fields.is_empty() => result,
        Some(result) => {
            fields.insert("result".into(), result);
            Value::Object(fields)
        }
        None => Value::Object(fields),
    };
    settled(&value).map(|o| claude_content(&o.output, o.exit_code, o.running))
}

/// A flat JavaScript object literal whose keys are identifiers or strings and whose values
/// are JSON scalars, at the start of `text`, and the text after it.
fn object(text: &str) -> Option<(Map<String, Value>, &str)> {
    let mut rest = text.trim_start().strip_prefix('{')?.trim_start();
    let mut fields = Map::new();
    if let Some(after) = rest.strip_prefix('}') {
        return Some((fields, after));
    }
    loop {
        let (key, after) = if rest.starts_with('"') {
            let (key, after) = scalar(rest)?;
            (key.as_str()?.to_owned(), after)
        } else {
            let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))?;
            if end == 0 {
                return None;
            }
            (rest[..end].to_owned(), &rest[end..])
        };
        rest = after.trim_start().strip_prefix(':')?.trim_start();
        let (value, after) = scalar(rest)?;
        if fields.insert(key, value).is_some() {
            return None;
        }
        rest = after.trim_start();
        if let Some(after) = rest.strip_prefix(',') {
            rest = after.trim_start();
            continue;
        }
        return Some((fields, rest.strip_prefix('}')?));
    }
}

/// One JSON string, number, boolean or null at the start of `text`, and the text after it.
fn scalar(text: &str) -> Option<(Value, &str)> {
    let end = if text.starts_with('"') {
        let mut escaped = false;
        let mut end = None;
        for (i, c) in text.char_indices().skip(1) {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                end = Some(i + 1);
                break;
            }
        }
        end?
    } else {
        text.find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '+' | '.')))
            .unwrap_or(text.len())
    };
    let value: Value = serde_json::from_str(&text[..end]).ok()?;
    (!value.is_object() && !value.is_array()).then_some((value, &text[end..]))
}

/// The working directory and command of a shell call record: Claude Code's leading
/// `cd DIR && ` counts as the directory.
fn call_command(text: &str) -> Option<(Option<String>, String)> {
    let (name, body) = record(text, "[Tool call: ")?;
    match name {
        "Bash" => {
            let input: Value = serde_json::from_str(body).ok()?;
            let fields = input.as_object()?;
            if !fields.keys().all(|k| matches!(k.as_str(), "command" | "description" | "timeout")) {
                return None;
            }
            let command = fields.get("command")?.as_str()?;
            Some(match changed_directory(command) {
                Some((dir, rest)) => (Some(dir), rest.to_owned()),
                None => (None, command.to_owned()),
            })
        }
        "exec" => {
            let [(command, dir)] = <[_; 1]>::try_from(exec_commands(body)?).ok()?;
            Some((dir, command))
        }
        "exec_command" => {
            let (command, dir) = exec_arguments(&serde_json::from_str(body).ok()?)?;
            Some((dir, command))
        }
        _ => None,
    }
}

/// The directory and the rest of a command that starts with `cd DIR && `.
fn changed_directory(command: &str) -> Option<(String, &str)> {
    let rest = command.strip_prefix("cd ")?;
    if rest.starts_with('\'') {
        let mut dir = String::new();
        let mut rest = &rest[1..];
        loop {
            let end = rest.find('\'')?;
            dir.push_str(&rest[..end]);
            rest = &rest[end + 1..];
            match rest.strip_prefix("\\''") {
                Some(after) => {
                    dir.push('\'');
                    rest = after;
                }
                None => return Some((dir, rest.strip_prefix(" && ")?)),
            }
        }
    }
    let (dir, rest) = rest.split_once(" && ")?;
    (!dir.is_empty() && !dir.contains(char::is_whitespace)).then(|| (dir.to_owned(), rest))
}

/// The Claude Code tool result text of a command result record, in either CLI's form.
fn result_output(text: &str) -> Option<String> {
    let (_, output) = record(text, "[Tool result: ")?;
    if let Some((_, rest)) = exec_output(output)
        && let Ok(value) = serde_json::from_str::<Value>(rest)
        && let Some(outcome) = settled(&value)
    {
        return Some(claude_content(&outcome.output, outcome.exit_code, outcome.running));
    }
    if let Some(outcome) = function_output(output) {
        return Some(claude_content(&outcome.output, outcome.exit_code, outcome.running));
    }
    Some(output.to_owned())
}

/// Whether two tool records show the same shell command, or the same result, in the forms
/// of either CLI.
pub(super) fn same(a: &str, b: &str) -> bool {
    if let (Some((x_dir, x)), Some((y_dir, y))) = (call_command(a), call_command(b)) {
        let within = |a: &str, b: &str| std::path::Path::new(a).is_absolute() && std::path::Path::new(a).ends_with(b);
        let dirs = match (x_dir.as_deref(), y_dir.as_deref()) {
            (Some(x), Some(y)) => x == y || within(x, y) || within(y, x),
            _ => true,
        };
        return x == y && dirs;
    }
    matches!((result_output(a), result_output(b)), (Some(x), Some(y)) if x == y)
}

/// Claude Code tools other than Bash that every Claude Code session defines.
const CLAUDE_TOOLS: [&str; 10] = ["Read", "Write", "Edit", "Glob", "Grep", "WebFetch", "WebSearch", "TodoWrite", "Agent", "NotebookEdit"];

/// One command of a Codex code-mode program, with its result.
pub(super) struct Command {
    pub command: String,
    pub cwd: Option<String>,
    pub output: String,
    pub exit_code: i64,
    pub running: Option<i64>,
}

/// A tool call and its result.
pub(super) enum Pair {
    Shell(Shell),
    /// A Codex code-mode program running several `tools.exec_command` calls.
    Commands { id: String, commands: Vec<Command>, program: String, header: String, result: String },
    /// A Claude Code tool other than Bash, as Claude Code recorded it.
    Claude { id: String, name: String, input: Value, content: String },
    /// Any other tool, by name, input and output.
    Other { id: String, name: String, input: Value, output: String, error: bool },
}

/// The tool call recorded by `call` and its result recorded by `result`.
pub(super) fn pair(call: &str, result: &str) -> Option<Pair> {
    if let Some(shell) = shell(call, result) {
        return Some(Pair::Shell(shell));
    }
    let (name, body) = record(call, "[Tool call: ")?;
    let (id, output) = record(result, "[Tool result: ")?;
    if name == "exec"
        && let Some(calls) = exec_commands(body)
        && let Some((header, rest)) = exec_output(output)
        && let Some(outcomes) = exec_results(body, rest, calls.len())
    {
        let commands = calls
            .into_iter()
            .zip(outcomes)
            .map(|((command, cwd), o)| Command { command, cwd, output: o.output, exit_code: o.exit_code, running: o.running })
            .collect();
        return Some(Pair::Commands { id: id.into(), commands, program: body.into(), header: header.into(), result: rest.into() });
    }
    let refused = CLAUDE_REFUSALS.iter().any(|refusal| output.starts_with(refusal));
    let input = serde_json::from_str::<Value>(body).ok().filter(Value::is_object);
    Some(match input {
        Some(input) if id.starts_with("toolu_") && !refused && CLAUDE_TOOLS.contains(&name) => {
            Pair::Claude { id: id.into(), name: name.into(), input, content: output.into() }
        }
        input => Pair::Other {
            id: id.into(),
            name: name.into(),
            input: input.unwrap_or_else(|| json!({"input": body})),
            output: super::portable::result_output(output),
            error: refused,
        },
    })
}

/// The command as Claude Code shows it in `cwd`: preceded by `cd DIR && ` when it ran in
/// another directory.
fn shown(command: &str, dir: Option<&str>, cwd: &str) -> String {
    let Some(dir) = dir else { return command.to_owned() };
    let dir = std::path::Path::new(cwd).join(dir);
    if dir == std::path::Path::new(cwd) {
        return command.to_owned();
    }
    let dir = dir.to_string_lossy();
    let plain = !dir.is_empty() && dir.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+@%:,=~".contains(c));
    let quoted = if plain { dir.into_owned() } else { format!("'{}'", dir.replace('\'', "'\\''")) };
    format!("cd {quoted} && {command}")
}

/// Claude Code tool result text for a command's output, exit code and running session.
fn claude_content(output: &str, exit_code: i64, running: Option<i64>) -> String {
    if let Some(session) = running {
        let line = format!("Process running with session ID {session}");
        return match output {
            "" => line,
            _ if output.ends_with('\n') => format!("{output}{line}"),
            _ => format!("{output}\n{line}"),
        };
    }
    if exit_code == 0 { output.to_owned() } else { format!("Exit code {exit_code}\n{output}") }
}

/// The Codex history item that shows a command and its output, with the working directory
/// as a file URL.
fn command_cell(id: &str, command: &str, dir: Option<&str>, cwd: &str, output: &str, exit_code: i64) -> Value {
    let dir = match dir {
        Some(dir) => std::path::Path::new(cwd).join(dir),
        None => std::path::PathBuf::from(cwd),
    };
    let dir = url::Url::from_file_path(&dir)
        .map(String::from)
        .unwrap_or_else(|()| format!("file://{}", dir.display()));
    json!({"type": "CommandExecution", "id": id, "command": ["/bin/zsh", "-lc", command],
        "cwd": dir, "process_id": null, "source": "unified_exec_startup",
        "status": if exit_code == 0 { "completed" } else { "failed" }, "exit_code": exit_code,
        "aggregated_output": output, "duration": {"secs": 0, "nanos": 0},
        "parsed_cmd": [{"type": "unknown", "cmd": command}]})
}

/// A Codex tool call history item.
fn mcp_cell(id: &str, server: &str, tool: &str, arguments: &Value, output: &str, error: bool) -> Value {
    json!({"type": "McpToolCall", "id": id, "server": server, "tool": tool, "arguments": arguments,
        "result": {"content": [{"type": "text", "text": output}], "isError": error},
        "status": if error { "failed" } else { "completed" }, "duration": {"secs": 0, "nanos": 0}, "readOnlyHint": false})
}

/// Codex code-mode call and output response items.
fn exec_items(id: &str, program: &str, header: &str, result: &str) -> (Value, Value) {
    (
        json!({"type": "custom_tool_call", "status": "completed", "call_id": id, "name": "exec", "input": program}),
        json!({"type": "custom_tool_call_output", "call_id": id,
            "output": [{"type": "input_text", "text": header}, {"type": "input_text", "text": result}]}),
    )
}

impl Pair {
    /// Claude Code tool use and tool result blocks, and the structured result beside them,
    /// when Claude Code shows this call natively in `cwd`.
    pub fn claude(&self, seed: &str, cwd: &str) -> Option<(Vec<Value>, Vec<Value>, Option<Value>)> {
        match self {
            Pair::Shell(shell) => {
                let (call, result, structured) = shell.claude(&shell.claude_id(seed), cwd);
                Some((vec![call], vec![result], Some(structured)))
            }
            Pair::Commands { commands, .. } => {
                let ids: Vec<String> = (0..commands.len())
                    .map(|k| format!("toolu_{}", &super::digest(format!("{seed}:{k}").as_bytes())[..24]))
                    .collect();
                Some((
                    commands
                        .iter()
                        .zip(&ids)
                        .map(|(c, id)| json!({"type": "tool_use", "id": id, "name": "Bash",
                            "input": {"command": shown(&c.command, c.cwd.as_deref(), cwd)}}))
                        .collect(),
                    commands
                        .iter()
                        .zip(&ids)
                        .map(|(c, id)| json!({"tool_use_id": id, "type": "tool_result",
                            "content": claude_content(&c.output, c.exit_code, c.running), "is_error": c.exit_code != 0}))
                        .collect(),
                    None,
                ))
            }
            Pair::Claude { id, name, input, content } => Some((
                vec![json!({"type": "tool_use", "id": id, "name": name, "input": input})],
                vec![json!({"tool_use_id": id, "type": "tool_result", "content": content, "is_error": false})],
                None,
            )),
            _ => None,
        }
    }
    /// Codex call and output response items, for commands Codex recorded.
    pub fn codex_items(&self) -> Option<(Value, Value)> {
        match self {
            Pair::Shell(shell) => shell.codex_items(),
            Pair::Commands { id, program, header, result, .. } => Some(exec_items(id, program, header, result)),
            _ => None,
        }
    }
    /// The Codex history items that show the call and its result.
    pub fn codex_cells(&self, seed: &str, cwd: &str) -> Vec<Value> {
        match self {
            Pair::Shell(shell) => vec![shell.command_execution(&format!("exec-{}", super::stable_id(seed)), cwd)],
            Pair::Commands { commands, .. } => commands
                .iter()
                .enumerate()
                .map(|(k, c)| {
                    let id = format!("exec-{}", super::stable_id(&format!("{seed}:{k}")));
                    command_cell(&id, &c.command, c.cwd.as_deref(), cwd, &c.output, c.exit_code)
                })
                .collect(),
            Pair::Claude { id, name, input, content } => {
                vec![mcp_cell(id, "claude_code", name, input, &super::portable::result_output(content), false)]
            }
            Pair::Other { id, name, input, output, error } => {
                let server = if id.starts_with("toolu_") { "claude_code" } else { "codex" };
                vec![mcp_cell(id, server, name, input, output, *error)]
            }
        }
    }
}

impl Shell {
    /// The Claude tool-use ID: the original one for a Claude command.
    pub fn claude_id(&self, seed: &str) -> String {
        match self.native {
            Native::Claude { .. } => self.id.clone(),
            Native::Codex { .. } | Native::Function { .. } => format!("toolu_{}", &super::digest(seed.as_bytes())[..24]),
        }
    }
    /// Claude Code `Bash` tool use and tool result content blocks shown in `cwd`, and the
    /// structured result Claude Code keeps beside the tool result.
    pub fn claude(&self, tool_id: &str, cwd: &str) -> (Value, Value, Value) {
        let (input, content) = match &self.native {
            Native::Claude { input, content } => (input.clone(), content.clone()),
            Native::Codex { .. } | Native::Function { .. } => (
                json!({"command": shown(&self.command, self.cwd.as_deref(), cwd)}),
                claude_content(&self.output, self.exit_code, self.running),
            ),
        };
        (
            json!({"type": "tool_use", "id": tool_id, "name": "Bash", "input": input}),
            json!({"tool_use_id": tool_id, "type": "tool_result", "content": content, "is_error": self.exit_code != 0}),
            json!({"stdout": self.output, "stderr": "", "interrupted": false, "isImage": false, "noOutputExpected": false}),
        )
    }
    /// Codex call and output response items, for a command Codex recorded.
    pub fn codex_items(&self) -> Option<(Value, Value)> {
        match &self.native {
            Native::Claude { .. } => None,
            Native::Codex { program, header, result } => Some(exec_items(&self.id, program, header, result)),
            Native::Function { arguments, output } => Some((
                json!({"type": "function_call", "status": "completed", "call_id": self.id, "name": "exec_command", "arguments": arguments}),
                json!({"type": "function_call_output", "call_id": self.id, "output": output}),
            )),
        }
    }
    /// The Codex history item that shows the command and its output.
    pub fn command_execution(&self, id: &str, cwd: &str) -> Value {
        command_cell(id, &self.command, self.cwd.as_deref(), cwd, &self.output, self.exit_code)
    }
}
