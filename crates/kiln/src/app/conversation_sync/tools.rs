//! Tool calls recognised in the readable tool records of the canonical history, and the
//! native records that show them in each CLI.
use serde_json::{Map, Value, json};

/// How the command was recorded by the CLI it came from.
pub(super) enum Native {
    /// Claude Code `Bash`: its input and tool result text.
    Claude { input: Value, content: String },
    /// Codex code mode: the `exec` program and its two output texts.
    Codex { program: String, header: String, result: String },
}

/// A shell command and its result.
pub(super) struct Shell {
    /// Tool-use or call ID of the original record.
    pub id: String,
    pub command: String,
    pub cwd: Option<String>,
    pub output: String,
    pub exit_code: i64,
    pub native: Native,
}

fn record<'a>(text: &'a str, kind: &str) -> Option<(&'a str, &'a str)> {
    text.strip_prefix(kind)?.split_once("]\n")
}

/// Claude Code's own texts for a tool use that was refused or failed before it ran.
const CLAUDE_REFUSALS: [&str; 4] = ["Permission for", "The user doesn't want", "<tool_use_error>", "The server-side"];

/// The shell command recorded by `call` and its result recorded by `result`, from Claude
/// Code `Bash` or a Codex code-mode program that prints one `tools.exec_command` call.
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
                native: Native::Claude { input, content: output.into() },
            })
        }
        "exec" => {
            let (command, cwd) = exec_command(body)?;
            let (header, results) = exec_results(output)?;
            let [(rest, exit_code, shown)] = <[_; 1]>::try_from(results).ok()?;
            Some(Shell {
                id: id.into(),
                command,
                cwd,
                output: shown,
                exit_code,
                native: Native::Codex { program: body.into(), header: header.into(), result: rest.into() },
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

/// `cmd` and `workdir` of a code-mode program that only prints one `tools.exec_command` call.
fn exec_command(program: &str) -> Option<(String, Option<String>)> {
    let mut commands = exec_commands(program)?;
    (commands.len() == 1).then(|| commands.remove(0))
}

/// `cmd` and `workdir` of each call of a code-mode program that only prints
/// `tools.exec_command` calls.
fn exec_commands(program: &str) -> Option<Vec<(String, Option<String>)>> {
    let mut rest = program.trim();
    let mut commands = Vec::new();
    while !rest.is_empty() {
        let (fields, after) = object(rest.strip_prefix("text(await tools.exec_command(")?)?;
        rest = after.strip_prefix("));")?.trim_start();
        if !fields
            .keys()
            .all(|k| matches!(k.as_str(), "cmd" | "workdir" | "max_output_tokens" | "yield_time_ms"))
        {
            return None;
        }
        let cmd = fields.get("cmd")?.as_str()?.to_owned();
        let workdir = match fields.get("workdir") {
            None => None,
            Some(value) => Some(value.as_str()?.to_owned()),
        };
        commands.push((cmd, workdir));
    }
    (!commands.is_empty()).then_some(commands)
}

/// The header and each command result of a code-mode program result: the result JSON text,
/// exit code and output.
fn exec_results(output: &str) -> Option<(&str, Vec<(&str, i64, String)>)> {
    let split = output.find("Output:\n")? + "Output:\n".len();
    let (header, rest) = output.split_at(split);
    if !header.starts_with("Script ") {
        return None;
    }
    let mut results = Vec::new();
    let mut values = serde_json::Deserializer::from_str(rest).into_iter::<Value>();
    let mut start = 0;
    while let Some(value) = values.next() {
        let value = value.ok()?;
        let end = values.byte_offset();
        let code = value.get("exit_code")?.as_i64()?;
        let shown = value.get("output")?.as_str()?.to_owned();
        results.push((&rest[start..end], code, shown));
        start = end;
    }
    if !rest[start..].trim().is_empty() || results.is_empty() {
        return None;
    }
    // Whitespace after the last result belongs to its text.
    let last = results.len() - 1;
    let begin = rest.len() - rest[start..].len() - results[last].0.len();
    results[last].0 = &rest[begin..];
    Some((header, results))
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

/// The command of a shell call record.
fn call_command(text: &str) -> Option<String> {
    let (name, body) = record(text, "[Tool call: ")?;
    match name {
        "Bash" => {
            let input: Value = serde_json::from_str(body).ok()?;
            let fields = input.as_object()?;
            fields
                .keys()
                .all(|k| matches!(k.as_str(), "command" | "description" | "timeout"))
                .then(|| fields.get("command")?.as_str().map(str::to_owned))?
        }
        "exec" => exec_command(body).map(|(command, _)| command),
        _ => None,
    }
}

/// The exit code and output of a command result record, in either CLI's form.
fn result_output(text: &str) -> Option<(i64, String)> {
    let (_, output) = record(text, "[Tool result: ")?;
    if let Some(split) = output.find("Output:\n").map(|i| i + "Output:\n".len())
        && output.starts_with("Script ")
        && let Ok(value) = serde_json::from_str::<Value>(&output[split..])
        && let (Some(code), Some(shown)) = (value.get("exit_code").and_then(Value::as_i64), value.get("output").and_then(Value::as_str))
    {
        return Some((code, shown.to_owned()));
    }
    let (code, shown) = claude_exit(output);
    Some((code, shown.to_owned()))
}

/// Whether two tool records show the same shell command, or the same result, in the forms
/// of either CLI.
pub(super) fn same(a: &str, b: &str) -> bool {
    if let (Some(x), Some(y)) = (call_command(a), call_command(b)) {
        return x == y;
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
}

/// A tool call and its result.
pub(super) enum Pair {
    Shell(Shell),
    /// A Codex code-mode program printing several `tools.exec_command` calls.
    Commands { id: String, commands: Vec<Command>, program: String, header: String, results: Vec<String> },
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
        && let Some((header, results)) = exec_results(output)
        && calls.len() == results.len()
    {
        let commands = calls
            .into_iter()
            .zip(&results)
            .map(|((command, cwd), (_, exit_code, output))| Command { command, cwd, output: output.clone(), exit_code: *exit_code })
            .collect();
        return Some(Pair::Commands {
            id: id.into(),
            commands,
            program: body.into(),
            header: header.into(),
            results: results.iter().map(|(raw, _, _)| (*raw).to_owned()).collect(),
        });
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

/// Whether a command with working directory `dir` ran in `cwd`.
fn runs_in(dir: Option<&str>, cwd: &str) -> bool {
    dir.is_none_or(|dir| std::path::Path::new(cwd).join(dir) == std::path::Path::new(cwd))
}

/// Claude Code tool result text for a command's output and exit code.
fn claude_content(output: &str, exit_code: i64) -> String {
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

impl Pair {
    /// Claude Code tool use and tool result blocks, and the structured result beside them,
    /// when Claude Code shows this call natively in `cwd`.
    pub fn claude(&self, seed: &str, cwd: &str) -> Option<(Vec<Value>, Vec<Value>, Option<Value>)> {
        match self {
            Pair::Shell(shell) if shell.runs_in(cwd) => {
                let (call, result, structured) = shell.claude(&shell.claude_id(seed));
                Some((vec![call], vec![result], Some(structured)))
            }
            Pair::Commands { commands, .. } if commands.iter().all(|c| runs_in(c.cwd.as_deref(), cwd)) => {
                let ids: Vec<String> = (0..commands.len())
                    .map(|k| format!("toolu_{}", &super::digest(format!("{seed}:{k}").as_bytes())[..24]))
                    .collect();
                Some((
                    commands
                        .iter()
                        .zip(&ids)
                        .map(|(c, id)| json!({"type": "tool_use", "id": id, "name": "Bash", "input": {"command": c.command}}))
                        .collect(),
                    commands
                        .iter()
                        .zip(&ids)
                        .map(|(c, id)| json!({"tool_use_id": id, "type": "tool_result",
                            "content": claude_content(&c.output, c.exit_code), "is_error": c.exit_code != 0}))
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
            Pair::Commands { id, program, header, results, .. } => {
                let mut output = vec![json!({"type": "input_text", "text": header})];
                output.extend(results.iter().map(|text| json!({"type": "input_text", "text": text})));
                Some((
                    json!({"type": "custom_tool_call", "status": "completed", "call_id": id, "name": "exec", "input": program}),
                    json!({"type": "custom_tool_call_output", "call_id": id, "output": output}),
                ))
            }
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
    /// Whether the command ran in `cwd`.
    pub fn runs_in(&self, cwd: &str) -> bool {
        runs_in(self.cwd.as_deref(), cwd)
    }
    /// The Claude tool-use ID: the original one for a Claude command.
    pub fn claude_id(&self, seed: &str) -> String {
        match self.native {
            Native::Claude { .. } => self.id.clone(),
            Native::Codex { .. } => format!("toolu_{}", &super::digest(seed.as_bytes())[..24]),
        }
    }
    /// Claude Code `Bash` tool use and tool result content blocks, and the structured result
    /// Claude Code keeps beside the tool result.
    pub fn claude(&self, tool_id: &str) -> (Value, Value, Value) {
        let (input, content) = match &self.native {
            Native::Claude { input, content } => (input.clone(), content.clone()),
            Native::Codex { .. } => (json!({"command": self.command}), claude_content(&self.output, self.exit_code)),
        };
        (
            json!({"type": "tool_use", "id": tool_id, "name": "Bash", "input": input}),
            json!({"tool_use_id": tool_id, "type": "tool_result", "content": content, "is_error": self.exit_code != 0}),
            json!({"stdout": self.output, "stderr": "", "interrupted": false, "isImage": false, "noOutputExpected": false}),
        )
    }
    /// Codex `exec` call and output response items, for a command Codex recorded.
    pub fn codex_items(&self) -> Option<(Value, Value)> {
        let Native::Codex { program, header, result } = &self.native else {
            return None;
        };
        Some((
            json!({"type": "custom_tool_call", "status": "completed", "call_id": self.id, "name": "exec", "input": program}),
            json!({"type": "custom_tool_call_output", "call_id": self.id,
                "output": [{"type": "input_text", "text": header}, {"type": "input_text", "text": result}]}),
        ))
    }
    /// The Codex history item that shows the command and its output.
    pub fn command_execution(&self, id: &str, cwd: &str) -> Value {
        command_cell(id, &self.command, self.cwd.as_deref(), cwd, &self.output, self.exit_code)
    }
}
