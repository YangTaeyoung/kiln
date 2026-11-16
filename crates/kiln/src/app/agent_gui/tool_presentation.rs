//! Locale-dependent presentation only. Wire names, arguments and history remain intact.
use super::*;
use kiln_common::i18n::tr;
fn operation(name: &str) -> Option<&'static str> {
    Some(match name {
        "AskUserQuestion" | "request_user_input" | "item/tool/requestUserInput" => {
            "사용자에게 질문"
        }
        "Bash"
        | "exec_command"
        | "commandExecution"
        | "execute"
        | "item/commandExecution/requestApproval"
        | "execCommandApproval" => "명령 실행",
        "Read" | "read_file" => "파일 읽기",
        "Write"
        | "Edit"
        | "MultiEdit"
        | "apply_patch"
        | "fileChange"
        | "edit"
        | "item/fileChange/requestApproval"
        | "applyPatchApproval" => "파일 변경",
        "Glob" | "Grep" => "파일 검색",
        "WebSearch" | "webSearch" => "웹 검색",
        "WebFetch" => "웹 페이지 가져오기",
        "imageView" => "이미지 보기",
        "imageGeneration" => "이미지 생성",
        "TodoWrite" | "update_plan" => "계획 업데이트",
        "think" => "생각하기",
        "Agent" | "Task" => "에이전트 작업",
        "delete" => "파일 삭제",
        "move" => "파일 이동",
        "switch_mode" => "모드 변경",
        "echo" => "메시지 반환",
        "item/permissions/requestApproval" => "권한 요청",
        "mcpServer/elicitation/request" => "추가 정보 요청",
        _ => return None,
    })
}
fn server_label(server: &str) -> String {
    server
        .split(['_', '-'])
        .filter(|s| !s.is_empty())
        .map(|s| {
            if s.eq_ignore_ascii_case("qa") {
                "QA".into()
            } else {
                let mut chars = s.chars();
                chars
                    .next()
                    .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
                    .unwrap_or_default()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
pub(super) fn named(name: &str) -> String {
    if let Some(parts) = name.strip_prefix("mcp__") {
        if let Some((server, tool)) = parts.split_once("__") {
            return format!(
                "{} · {}",
                server_label(server),
                tr(operation(tool).unwrap_or("외부 도구"))
            );
        }
    }
    tr(operation(name).unwrap_or("외부 도구")).into()
}
pub(super) fn title(tool: &ToolCall, provider: Tool, native_item: Option<&str>) -> String {
    if native_item == Some("mcpToolCall") {
        return format!(
            "{} · {}",
            server_label(tool.input["server"].as_str().unwrap_or_default()),
            tr(operation(tool.input["tool"].as_str().unwrap_or_default()).unwrap_or("외부 도구"))
        );
    }
    if let Some(kind) = native_item.and_then(operation) {
        return tr(kind).into();
    }
    if provider == Tool::Antigravity {
        if let Some(meta) = &tool.presentation {
            // A known exact name can refine ACP's deliberately coarse category.
            if let Some(name) = meta.native_name.as_deref().and_then(operation) {
                return tr(name).into();
            }
            if let Some(kind) = meta.kind.as_deref().and_then(|kind| match kind {
                "read" => Some("데이터 읽기"),
                "search" => Some("정보 검색"),
                "fetch" => Some("외부 데이터 가져오기"),
                _ => operation(kind),
            }) {
                return tr(kind).into();
            }
            if let Some(name) = &meta.native_name {
                return named(name);
            }
        }
        // Old ACP journals only have generated English titles: do not guess an operation.
        return tr("외부 도구").into();
    }
    named(&tool.name)
}

pub(super) fn request_title(request: &Request, provider: Tool) -> String {
    if provider == Tool::Antigravity {
        let tool = ToolCall {
            name: request.title.clone(),
            presentation: Some(ToolPresentation {
                kind: request.details["kind"].as_str().map(str::to_owned),
                native_name: request.details["name"].as_str().map(str::to_owned),
                title: Some(request.title.clone()),
            }),
            ..Default::default()
        };
        return title(&tool, provider, None);
    }
    named(&request.title)
}
