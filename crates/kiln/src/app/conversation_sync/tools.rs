//! Shell commands recognised in the readable tool records of the canonical history, and the
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
            let split = output.find("Output:\n")? + "Output:\n".len();
            let (header, rest) = output.split_at(split);
            if !header.starts_with("Script ") {
                return None;
            }
            let value: Value = serde_json::from_str(rest).ok()?;
            let exit_code = value.get("exit_code")?.as_i64()?;
            let shown = value.get("output")?.as_str()?.to_owned();
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
    let inner = program
        .trim()
        .strip_prefix("text(await tools.exec_command(")?
        .strip_suffix("));")?;
    let fields = object(inner)?;
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
    Some((cmd, workdir))
}

/// A flat JavaScript object literal whose keys are identifiers or strings and whose values
/// are JSON scalars.
fn object(text: &str) -> Option<Map<String, Value>> {
    let mut rest = text.trim().strip_prefix('{')?.trim_start();
    let mut fields = Map::new();
    if let Some(after) = rest.strip_prefix('}') {
        return after.trim().is_empty().then_some(fields);
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
        return rest.strip_prefix('}')?.trim().is_empty().then_some(fields);
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

/// A tool call and its result.
pub(super) enum Pair {
    Shell(Shell),
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

/// A Codex tool call history item.
fn mcp_cell(id: &str, server: &str, tool: &str, arguments: &Value, output: &str, error: bool) -> Value {
    json!({"type": "McpToolCall", "id": id, "server": server, "tool": tool, "arguments": arguments,
        "result": {"content": [{"type": "text", "text": output}], "isError": error},
        "status": if error { "failed" } else { "completed" }, "duration": {"secs": 0, "nanos": 0}, "readOnlyHint": false})
}

impl Pair {
    /// Claude Code tool use and tool result blocks, and the structured result beside them,
    /// when Claude Code shows this call natively in `cwd`.
    pub fn claude(&self, seed: &str, cwd: &str) -> Option<(Value, Value, Option<Value>)> {
        match self {
            Pair::Shell(shell) if shell.runs_in(cwd) => {
                let (call, result, structured) = shell.claude(&shell.claude_id(seed));
                Some((call, result, Some(structured)))
            }
            Pair::Claude { id, name, input, content } => Some((
                json!({"type": "tool_use", "id": id, "name": name, "input": input}),
                json!({"tool_use_id": id, "type": "tool_result", "content": content, "is_error": false}),
                None,
            )),
            _ => None,
        }
    }
    /// Codex call and output response items, for a command Codex recorded.
    pub fn codex_items(&self) -> Option<(Value, Value)> {
        match self {
            Pair::Shell(shell) => shell.codex_items(),
            _ => None,
        }
    }
    /// The Codex history item that shows the call and its result.
    pub fn codex_cell(&self, exec: &str, cwd: &str) -> Value {
        match self {
            Pair::Shell(shell) => shell.command_execution(exec, cwd),
            Pair::Claude { id, name, input, content } => {
                mcp_cell(id, "claude_code", name, input, &super::portable::result_output(content), false)
            }
            Pair::Other { id, name, input, output, error } => {
                let server = if id.starts_with("toolu_") { "claude_code" } else { "codex" };
                mcp_cell(id, server, name, input, output, *error)
            }
        }
    }
}

impl Shell {
    /// Whether the command ran in `cwd`.
    pub fn runs_in(&self, cwd: &str) -> bool {
        self.cwd.as_deref().is_none_or(|dir| std::path::Path::new(cwd).join(dir) == std::path::Path::new(cwd))
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
            Native::Codex { .. } => (
                json!({"command": self.command}),
                if self.exit_code == 0 {
                    self.output.clone()
                } else {
                    format!("Exit code {}\n{}", self.exit_code, self.output)
                },
            ),
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
    /// The Codex history item that shows the command and its output, with the working directory
    /// as a file URL.
    pub fn command_execution(&self, id: &str, cwd: &str) -> Value {
        let dir = match &self.cwd {
            Some(dir) => std::path::Path::new(cwd).join(dir),
            None => std::path::PathBuf::from(cwd),
        };
        let dir = url::Url::from_file_path(&dir)
            .map(String::from)
            .unwrap_or_else(|()| format!("file://{}", dir.display()));
        json!({"type": "CommandExecution", "id": id, "command": ["/bin/zsh", "-lc", self.command],
            "cwd": dir, "process_id": null, "source": "unified_exec_startup",
            "status": if self.exit_code == 0 { "completed" } else { "failed" }, "exit_code": self.exit_code,
            "aggregated_output": self.output, "duration": {"secs": 0, "nanos": 0},
            "parsed_cmd": [{"type": "unknown", "cmd": self.command}]})
    }
}
