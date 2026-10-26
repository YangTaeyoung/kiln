//! Native display adapters for the read/edit/command records observed in agy 1.4.7.
//! Canonical Agy inputs and results stay unchanged in the transcript annotations.
use super::*;

pub(crate) struct AgyTool {
    id: String,
    name: String,
    input: Value,
    output: String,
    error: bool,
    kind: Kind,
}

enum Kind {
    Read { path: String },
    Edit { path: String, old: String, new: String, line: Option<u64> },
    Command { command: String, cwd: Option<String>, exit: Option<i64>, shown: String },
}

pub(super) fn failed(output: &str) -> bool {
    output.starts_with("Encountered error in step execution:")
        || output.starts_with("[Tool error]\n")
}

pub(super) fn parse(name: &str, body: &str, id: &str, output: &str) -> Option<AgyTool> {
    if !id.starts_with("agy-") { return None; }
    let input: Value = serde_json::from_str(body).ok()?;
    let fields = input.as_object()?;
    let only = |keys: &[&str]| fields.keys().all(|k| keys.contains(&k.as_str()) || k == "toolSummary");
    let field = |key: &str| fields.get(key)?.as_str().map(str::to_owned);
    let error = failed(output);
    let output = output.strip_prefix("[Tool error]\n").unwrap_or(output);
    let kind = match name {
        "read_file" if only(&["Path"]) => Kind::Read { path: field("Path")? },
        "edit_file" if only(&["Path", "Target", "Replacement"]) => {
            let (path, old, new) = (field("Path")?, field("Target")?, field("Replacement")?);
            if old.is_empty() { return None; }
            // agy reports the affected starting line after a successful single replacement.
            // Do not invent line numbers or a successful diff for failed/unknown results.
            let line = output.strip_prefix(&format!("Made 1 replacement in {path} starting at line "))
                .and_then(|rest| rest.split_once(' ')).and_then(|(line, _)| line.parse::<u64>().ok())
                .filter(|line| *line > 0);
            Kind::Edit { path, old, new, line }
        }
        "run_command" if only(&["CommandLine", "Cwd", "WaitMsBeforeAsync"]) => {
            let command = field("CommandLine")?;
            let cwd = match fields.get("Cwd") { None => None, Some(v) => Some(v.as_str()?.to_owned()) };
            let (exit, shown) = if error {
                (None, output.to_owned())
            } else {
                let rest = output.strip_prefix("Command exited with code ")?;
                let (exit, shown) = rest.split_once(".\n\nCommand output:\n")?;
                (Some(exit.parse::<i64>().ok()?), shown.to_owned())
            };
            Kind::Command { command, cwd, exit, shown }
        }
        _ => return None,
    };
    let error = error || matches!(&kind, Kind::Command { exit: Some(code), .. } if *code != 0);
    Some(AgyTool { id: id.into(), name: name.into(), input, output: output.into(), error, kind })
}

impl AgyTool {
    fn diff(old: &str, new: &str, line: u64) -> (String, Value) {
        let old: Vec<_> = old.lines().collect();
        let new: Vec<_> = new.lines().collect();
        let lines: Vec<_> = old.iter().map(|s| format!("-{s}")).chain(new.iter().map(|s| format!("+{s}"))).collect();
        let diff = format!("@@ -{line},{} +{line},{} @@\n{}\n", old.len(), new.len(), lines.join("\n"));
        let hunk = json!({"oldStart":line,"oldLines":old.len(),"newStart":line,"newLines":new.len(),"lines":lines});
        (diff, hunk)
    }

    pub(super) fn claude(&self, seed: &str, cwd: &str) -> Record {
        let id = format!("toolu_{}", &super::super::digest(seed.as_bytes())[..24]);
        let (name, input, content, structured) = match &self.kind {
            Kind::Read { path } => ("Read", json!({"file_path":path}), self.output.clone(), None),
            Kind::Edit { path, old, new, line } => {
                let structured = (!self.error).then(|| line.map(|line| {
                    let (_, hunk) = Self::diff(old, new, line);
                    json!({"filePath":path,"oldString":old,"newString":new,"originalFile":null,
                        "structuredPatch":[hunk],"userModified":false,"replaceAll":false})
                })).flatten();
                ("Edit", json!({"file_path":path,"old_string":old,"new_string":new,"replace_all":false}), self.output.clone(), structured)
            }
            Kind::Command { command, cwd: dir, exit, shown: output } => (
                "Bash", json!({"command":shown(command, dir.as_deref(), cwd)}),
                match exit { Some(code) => claude_content(output, *code, None), None => output.clone() },
                Some(json!({"stdout":if exit.is_some() {output.as_str()} else {""},
                    "stderr":if exit.is_none() {output.as_str()} else {""},"interrupted":exit.is_none(),"isImage":false,"noOutputExpected":false})),
            ),
        };
        (json!({"type":"tool_use","id":id,"name":name,"input":input}),
         json!({"type":"tool_result","tool_use_id":id,"content":content,"is_error":self.error}), structured)
    }

    pub(super) fn codex_items(&self) -> Option<(Value, Value)> {
        let Kind::Command { command, cwd, exit, shown } = &self.kind else { return None };
        let mut arguments = json!({"cmd":command});
        if let Some(cwd) = cwd { arguments["workdir"] = json!(cwd); }
        let output = match exit {
            Some(code) => format!("Process exited with code {code}\nOutput:\n{shown}"),
            None => shown.clone(),
        };
        Some((
            json!({"type":"function_call","status":"completed","call_id":self.id,"name":"exec_command","arguments":arguments.to_string()}),
            json!({"type":"function_call_output","call_id":self.id,"output":output}),
        ))
    }

    pub(super) fn codex_cell(&self, seed: &str, cwd: &str) -> Value {
        let id = format!("agy-{}", super::super::stable_id(seed));
        match &self.kind {
            Kind::Command { command, cwd: dir, exit, shown } => {
                let mut cell = command_cell(&id, command, dir.as_deref(), cwd, shown, exit.unwrap_or(0));
                cell["status"] = json!(if self.error { "failed" } else { "completed" });
                cell["exit_code"] = json!(exit);
                cell
            }
            Kind::Edit { path, old, new, line: Some(line) } if !self.error => {
                let (diff, _) = Self::diff(old, new, *line);
                json!({"type":"FileChange","id":id,"status":"completed","stdout":self.output,"stderr":"",
                    "changes":{path:{"type":"update","unified_diff":diff,"move_path":null}}})
            }
            _ => mcp_cell(&self.id, "antigravity", &self.name, &self.input, &self.output, self.error),
        }
    }
}
