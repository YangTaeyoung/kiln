//! Product approval/tool projections. Protocol params stay in the transcript.
use super::{Request, ToolCall};
use kiln_accounts::Tool;
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Command,
    Files,
    Permission,
    Network,
    Question,
    Information,
    Tool,
}
impl Kind {
    pub fn title(self) -> &'static str {
        kiln_common::i18n::tr(match self {
            Self::Command => "명령 실행 승인",
            Self::Files => "파일 변경 승인",
            Self::Permission => "권한 요청",
            Self::Network => "네트워크 접근 승인",
            Self::Question => "질문",
            Self::Information => "추가 정보 요청",
            Self::Tool => "도구 실행 승인",
        })
    }
}
#[derive(Clone, Debug)]
pub(super) struct Summary {
    pub kind: Kind,
    pub fields: Vec<(&'static str, String)>,
}
fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(bounded(s)),
        Value::Array(values) => {
            let parts = values
                .iter()
                .take(16)
                .filter_map(|v| v.as_str().map(bounded))
                .collect::<Vec<_>>();
            (!parts.is_empty()).then(|| bounded(&parts.join("\n")))
        }
        _ => None,
    }
}
fn bounded(s: &str) -> String {
    let mut end = s.len().min(16 * 1024);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].into()
}
fn rpc_title(s: &str) -> bool {
    s.contains('/') || matches!(s, "execCommandApproval" | "applyPatchApproval")
}
pub(super) fn kind(r: &Request, provider: Tool) -> Kind {
    if provider == Tool::Claude {
        // Claude emits Request only for can_use_tool; details is actual input,
        // so domain keys such as permissions/requestedSchema are not envelopes.
        return match r.title.as_str() {
            "Bash" => Kind::Command,
            "Edit" | "Write" | "MultiEdit" => Kind::Files,
            "AskUserQuestion" => Kind::Question,
            _ => Kind::Tool,
        };
    }
    let p = &r.details;
    if p.get("networkApprovalContext").is_some() {
        return Kind::Network;
    }
    if p.get("requestedSchema").is_some() || r.title == "mcpServer/elicitation/request" {
        return Kind::Information;
    }
    match r.title.as_str() {
        "item/commandExecution/requestApproval" | "execCommandApproval" => Kind::Command,
        "item/fileChange/requestApproval"
        | "applyPatchApproval"
        | "Edit"
        | "Write"
        | "MultiEdit" => Kind::Files,
        "item/permissions/requestApproval" => Kind::Permission,
        "item/tool/requestUserInput" | "AskUserQuestion" => Kind::Question,
        _ if !r.questions.is_empty() => Kind::Question,
        _ if p.get("command").is_some() || p.get("cmd").is_some() => Kind::Command,
        _ if p.get("permissions").is_some() => Kind::Permission,
        _ => Kind::Tool,
    }
}
impl Summary {
    fn push(&mut self, label: &'static str, value: Option<String>) {
        if self.fields.len() < 24
            && let Some(value) = value
            && !value.is_empty()
            && !self.fields.iter().any(|(l, v)| *l == label && *v == value)
        {
            self.fields.push((label, value));
        }
    }
    fn paths(&mut self, p: &Value, label: &'static str) {
        for key in ["path", "paths", "file_path", "filePath"] {
            self.push(label, text(&p[key]));
        }
    }
    fn permissions(&mut self, p: &Value) {
        if let Some(enabled) = p["network"]["enabled"].as_bool() {
            self.push(
                "네트워크",
                Some(
                    kiln_common::i18n::tr(if enabled {
                        "접근 허용"
                    } else {
                        "접근 차단"
                    })
                    .into(),
                ),
            );
        }
        for filesystem in ["fileSystem", "filesystem"] {
            self.push("읽기 경로", text(&p[filesystem]["read"]));
            self.push("쓰기 경로", text(&p[filesystem]["write"]));
        }
        self.push("읽기 경로", text(&p["additionalReadRoots"]));
        self.push("쓰기 경로", text(&p["additionalWriteRoots"]));
    }
}
pub(super) fn project(r: &Request, provider: Tool) -> Summary {
    let kind = kind(r, provider);
    let domain_input = match provider {
        Tool::Claude => Some(&r.details),
        // ACP wraps actual arguments in the explicitly specified rawInput field.
        Tool::Antigravity => r.details.get("rawInput").filter(|input| !input.is_null()),
        Tool::Codex => None,
    };
    if let Some(domain_input) = domain_input {
        let mut summary = Summary {
            kind,
            fields: vec![],
        };
        summary.push("작업", Some(bounded(&r.title)));
        let (input, truncated) = input_preview(domain_input);
        summary.push("도구 입력", Some(input));
        if truncated {
            summary.push(
                "안내",
                Some(kiln_common::i18n::tr("도구 입력이 길어 일부만 표시합니다.").into()),
            );
        }
        if provider == Tool::Antigravity {
            for location in r.details["locations"]
                .as_array()
                .into_iter()
                .flatten()
                .take(16)
            {
                summary.paths(location, "대상 경로");
            }
        }
        return summary;
    }
    project_details(&r.title, &r.details, kind)
}
// Bound serialization itself, not just the final label. This runs on the display
// preparation worker and keeps arbitrary MCP/domain keys and values intact.
fn input_preview(input: &Value) -> (String, bool) {
    use std::io::Write;
    struct Preview {
        bytes: Vec<u8>,
        truncated: bool,
    }
    impl Write for Preview {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let remaining = (16 * 1024usize).saturating_sub(self.bytes.len());
            let count = remaining.min(bytes.len());
            self.bytes.extend_from_slice(&bytes[..count]);
            if count < bytes.len() {
                self.truncated = true;
                return Err(std::io::ErrorKind::WriteZero.into());
            }
            Ok(count)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut preview = Preview {
        bytes: Vec::with_capacity(16 * 1024),
        truncated: false,
    };
    let _ = serde_json::to_writer_pretty(&mut preview, input);
    let valid = match std::str::from_utf8(&preview.bytes) {
        Ok(_) => preview.bytes.len(),
        Err(error) => error.valid_up_to(),
    };
    preview.bytes.truncate(valid);
    (
        String::from_utf8(preview.bytes).expect("valid bounded UTF-8"),
        preview.truncated,
    )
}
fn project_details(title: &str, p: &Value, kind: Kind) -> Summary {
    let mut s = Summary {
        kind,
        fields: vec![],
    };
    if !rpc_title(title) && p.get("reason").is_none() && p.get("message").is_none() {
        s.push("작업", Some(bounded(title)));
    }
    s.push("명령", text(&p["command"]).or_else(|| text(&p["cmd"])));
    for key in ["cwd", "directory", "workingDirectory"] {
        s.push("작업 폴더", text(&p[key]));
    }
    s.paths(p, "대상 경로");
    for key in ["changes", "fileChanges", "locations"] {
        if let Some(changes) = p[key].as_array() {
            for change in changes.iter().take(16) {
                s.paths(change, "대상 경로");
            }
        }
        if let Some(changes) = p[key].as_object() {
            for path in changes.keys().take(16) {
                s.push("대상 경로", Some(bounded(path)));
            }
        }
    }
    s.push("허용 범위", text(&p["grantRoot"]));
    s.push(
        "요청 범위",
        match p["scope"].as_str() {
            Some("turn") => Some(kiln_common::i18n::tr("현재 작업").into()),
            Some("session") => Some(kiln_common::i18n::tr("현재 세션").into()),
            _ => text(&p["scope"]),
        },
    );
    for key in ["reason", "message", "description"] {
        s.push("요청 이유", text(&p[key]));
    }
    s.push("서버", text(&p["serverName"]));
    s.push("주소", text(&p["url"]));
    s.permissions(&p["permissions"]);
    s.permissions(&p["additionalPermissions"]);
    let network = &p["networkApprovalContext"];
    s.push("호스트", text(&network["host"]));
    s.push("프로토콜", text(&network["protocol"]));
    if let Some(port) = network["port"].as_u64() {
        s.push("포트", Some(port.to_string()));
    }
    for (key, label) in [
        ("old_string", "변경 전"),
        ("new_string", "변경 후"),
        ("diff", "변경 내용"),
        ("patch", "변경 내용"),
    ] {
        s.push(label, text(&p[key]));
    }
    if s.kind == Kind::Files {
        s.push("작성 내용", text(&p["content"]));
    }
    s
}

// Codex::item assigns the complete native item to ToolCall.input. Identify that
// exact envelope by provider + matching native item ID/type. Never strip keys
// from ordinary arguments, particularly user/domain `id`, `type`, or `status`.
fn native_item<'a>(tool: &'a ToolCall, provider: Tool) -> Option<&'a str> {
    (provider == Tool::Codex && tool.input["id"].as_str() == Some(tool.id.as_str()))
        .then(|| tool.input["type"].as_str())
        .flatten()
}
pub(super) fn tool_input(tool: &ToolCall, provider: Tool) -> String {
    let Some(kind) = native_item(tool, provider) else {
        return if tool.input.is_null() {
            String::new()
        } else {
            serde_json::to_string_pretty(&tool.input).unwrap_or_default()
        };
    };
    if matches!(kind, "mcpToolCall" | "dynamicToolCall") {
        return tool
            .input
            .get("arguments")
            .map(|args| serde_json::to_string_pretty(args).unwrap_or_default())
            .unwrap_or_default();
    }
    let category = match kind {
        "commandExecution" => Kind::Command,
        "fileChange" => Kind::Files,
        _ => Kind::Tool,
    };
    let mut summary = project_details("", &tool.input, category);
    for key in ["query", "queries", "searchQuery"] {
        summary.push("검색어", text(&tool.input[key]));
    }
    summary.push("대상 경로", text(&tool.input["action"]["path"]));
    summary.push("검색어", text(&tool.input["action"]["query"]));
    for key in ["prompt", "revisedPrompt"] {
        summary.push("요청", text(&tool.input[key]));
    }
    summary
        .fields
        .iter()
        .map(|(label, value)| format!("{}: {value}", kiln_common::i18n::tr(label)))
        .collect::<Vec<_>>()
        .join("\n")
}
pub(super) fn tool_title(tool: &ToolCall, provider: Tool) -> String {
    super::tool_presentation::title(tool, provider, native_item(tool, provider))
}

#[cfg(test)]
mod round2_tests {
    use super::*;
    #[test]
    fn review_round2_claude_domain_keys_are_not_misclassified_as_rpc_schema() {
        let request = Request {
            id: "transport-id".into(),
            title: "mcp__fixture__save".into(),
            questions: vec![],
            options: vec![],
            details: serde_json::json!({"requestedSchema":{"id":"business-schema"},"permissions":"business-policy","id":"invoice-42","type":"invoice","status":"draft"}),
        };
        let summary = project(&request, Tool::Claude);
        assert_eq!(summary.kind, Kind::Tool);
        let input = &summary
            .fields
            .iter()
            .find(|(label, _)| *label == "도구 입력")
            .unwrap()
            .1;
        for value in [
            "business-schema",
            "business-policy",
            "invoice-42",
            "invoice",
            "draft",
        ] {
            assert!(input.contains(value));
        }
        assert!(!input.contains("transport-id"));
    }
    #[test]
    fn review_round2_domain_input_preview_is_bounded_and_explicitly_truncated() {
        let request = Request {
            id: "7".into(),
            title: "mcp__fixture__long_input".into(),
            questions: vec![],
            options: vec![],
            details: serde_json::json!({"body":"한글".repeat(20_000)}),
        };
        let summary = project(&request, Tool::Claude);
        let input = &summary
            .fields
            .iter()
            .find(|(label, _)| *label == "도구 입력")
            .unwrap()
            .1;
        assert!(input.len() <= 16 * 1024);
        assert!(input.is_char_boundary(input.len()));
        assert!(summary.fields.iter().any(|(label, _)| *label == "안내"));
        assert_eq!(
            request.details["body"].as_str().unwrap().chars().count(),
            40_000
        );
    }
}

#[cfg(test)]
mod acp_projection_test {
    use super::*;
    #[test]
    fn review_round2_acp_raw_input_preserves_domain_values_without_tool_envelope() {
        let request = Request {
            id: "rpc-id".into(),
            title: "Run fixture tool".into(),
            questions: vec![],
            options: vec![],
            details: serde_json::json!({"toolCallId":"transport-tool-id","status":"pending","rawInput":{"id":"domain-42","type":"record","status":"draft","body":"actual body"},"locations":[{"path":"/fixture/record.json"}]}),
        };
        let summary = project(&request, Tool::Antigravity);
        let rendered = format!("{:?}", summary.fields);
        for value in [
            "domain-42",
            "record",
            "draft",
            "actual body",
            "/fixture/record.json",
        ] {
            assert!(rendered.contains(value));
        }
        for hidden in ["rpc-id", "transport-tool-id", "pending", "rawInput"] {
            assert!(!rendered.contains(hidden));
        }
    }
}
