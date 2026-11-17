//! Human-facing setting copy. IDs and values sent to providers are never translated.
//! Semantics: Claude SDK flagSettings / permissions, Codex config reference and ACP v1.
use super::*;

pub(super) fn semantic_id(option: &ControlOption) -> &str {
    match option.category.as_deref() {
        Some("model" | "mode" | "thought_level") => {
            if label(&option.id) == option.id {
                option.category.as_deref().unwrap()
            } else {
                &option.id
            }
        }
        _ => &option.id,
    }
}
pub(crate) fn option_title(option: &ControlOption) -> String {
    let id = semantic_id(option);
    if let Some(name) = id.strip_prefix("skill:") {
        return format!("{} · {name}", tr("스킬"));
    }
    if label(id) != id {
        return label(id).into();
    }
    format!(
        "{} · {}",
        tr("추가 설정"),
        if option.label.is_empty() {
            &option.id
        } else {
            &option.label
        }
    )
}
pub(crate) fn control_help(id: &str) -> &'static str {
    tr(match id {
        "model" => "응답을 생성할 AI 모델을 선택합니다.",
        "advisorModel" => "주 에이전트가 자문을 구할 때 사용할 모델을 선택합니다.",
        "effort" | "effortLevel" | "thought_level" => {
            "답변을 준비하는 추론 강도입니다. 높은 단계는 더 많은 시간과 사용량이 필요할 수 있습니다."
        }
        "permissionMode" => "파일 수정과 도구 실행 전에 승인을 받는 방식을 선택합니다.",
        "collaborationMode" => "바로 작업을 수행할지, 먼저 계획을 세울지 선택합니다.",
        "mode" => "AI가 작업을 진행하는 방식을 선택합니다.",
        "ultracode" => {
            "현재 추론 강도를 유지하면서 여러 에이전트를 활용하는 동적 워크플로를 사용합니다."
        }
        "serviceTier" => {
            "요청 처리 등급을 선택합니다. 지원 여부와 요금은 계정 및 모델에 따라 다릅니다."
        }
        "fastMode" => {
            "지원하는 모델의 빠른 응답 모드를 사용합니다. 별도 사용량 요금이 적용될 수 있습니다."
        }
        "thinkingBudget" => "생각 과정에 사용할 토큰의 최대 수를 지정합니다.",
        "thinkingDisplay" => "생각 과정을 화면에 얼마나 표시할지 선택합니다.",
        "enableWorkflows" => {
            "AI가 여러 에이전트의 작업을 구성하는 워크플로 기능을 사용할 수 있게 합니다."
        }
        "workflowSizeGuideline" => {
            "워크플로의 권장 에이전트 규모입니다. 강제 실행 제한은 아닙니다."
        }
        "workflowKeywordTriggerEnabled" => {
            "메시지에 ultracode를 포함하면 해당 턴에서 워크플로를 사용하도록 합니다."
        }
        "outputStyle" => "답변의 구성과 설명 방식을 선택합니다.",
        "agent" => "대화를 맡을 주 에이전트를 선택합니다. 이름은 설치된 에이전트의 이름입니다.",
        "autoCompactEnabled" => {
            "대화가 길어지면 이전 내용을 자동으로 압축해 컨텍스트 공간을 확보합니다."
        }
        "language" => "AI가 답변할 언어를 지정합니다.",
        "fallbackModel" => "기본 모델을 사용할 수 없을 때 시도할 모델을 한 줄에 하나씩 입력합니다.",
        "promptCacheTtl" | "subagentPromptCacheTtl" => {
            "재사용할 프롬프트 캐시의 유지 시간을 선택합니다. 요금은 제공자의 정책을 따릅니다."
        }
        "promptSuggestionEnabled" => "대화에 이어 보낼 메시지 제안을 표시합니다.",
        "permissions" => "파일·네트워크 접근과 승인을 함께 정의하는 권한 프로필을 선택합니다.",
        "approvalPolicy" => "허용 범위를 넘는 작업에 대해 언제 승인을 요청할지 선택합니다.",
        "approvalsReviewer" => {
            "승인이 필요한 요청을 누가 검토할지 선택합니다. 샌드박스의 허용 범위는 바뀌지 않습니다."
        }
        "summary" => "추론 과정의 요약을 얼마나 자세히 표시할지 선택합니다.",
        "sandboxPolicy" => "도구가 파일을 읽고 쓰거나 네트워크에 접근할 수 있는 범위를 정합니다.",
        "granularApproval" => {
            "체크한 종류의 승인 요청을 표시합니다. 체크 해제한 요청은 자동 거절됩니다."
        }
        id if id.starts_with("skill:") => {
            "AI와 사용자가 이 스킬을 발견하고 실행할 수 있는 범위를 선택합니다."
        }
        _ => "제공자가 이 대화에 제공한 추가 설정입니다. 고유한 이름은 그대로 표시합니다.",
    })
}
/// A translated title and concrete effect for every built-in enum.
pub(crate) fn choice_copy(control: &str, value: &str) -> Option<(&'static str, &'static str)> {
    let pair = match (control, value) {
        ("approvalPolicy", "untrusted") => (
            "신뢰되지 않은 작업 확인",
            "신뢰된 읽기 작업 외에는 승인을 요구하는 정책입니다. 설치된 CLI가 제공하는 경우에만 적용됩니다.",
        ),
        ("approvalPolicy", "on-request") => (
            "필요할 때 승인 요청",
            "AI가 허용 범위를 넘는 작업에 대해 승인을 요청할 수 있습니다.",
        ),
        ("approvalPolicy", "never") => (
            "승인 요청 안 함",
            "승인을 요청하지 않습니다. 현재 권한으로 실행할 수 없는 작업은 실패합니다.",
        ),
        ("approvalPolicy", "on-failure") => (
            "실패 후 승인 요청",
            "제한된 실행이 실패하면 다시 실행하기 위한 승인을 요청하는 이전 정책입니다.",
        ),
        ("approvalsReviewer", "user") => (
            "내가 검토",
            "승인 요청을 직접 확인하고 허용 또는 거절합니다.",
        ),
        ("approvalsReviewer", "auto_review" | "guardian_subagent") => (
            "AI가 검토",
            "검토용 AI가 승인 대상 요청을 평가합니다. 모든 작업을 자동 허용하는 설정은 아닙니다.",
        ),
        ("permissionMode", "default") => (
            "직접 승인",
            "기존 권한 규칙을 따르며, 승인이 필요한 도구 실행은 사용자에게 묻습니다.",
        ),
        ("permissionMode", "auto") => (
            "자동 검토",
            "Claude의 검토기가 도구 실행을 평가하고 승인 여부를 결정합니다.",
        ),
        ("permissionMode", "acceptEdits") => (
            "편집 자동 승인",
            "파일 편집을 자동 승인합니다. 그 밖의 도구 실행은 기존 권한 규칙을 따릅니다.",
        ),
        ("permissionMode", "dontAsk") => (
            "승인 요청 자동 거절",
            "이미 허용된 작업은 실행하고, 추가 승인이 필요한 작업은 묻지 않고 거절합니다.",
        ),
        ("permissionMode", "bypassPermissions") => {
            ("권한 확인 생략", "도구 실행에 대한 권한 확인을 생략합니다.")
        }
        ("permissionMode", "plan") => (
            "계획",
            "코드를 살펴보고 계획을 세웁니다. 일반적인 파일 편집과 실행을 제한합니다.",
        ),
        ("collaborationMode", "default") => (
            "작업",
            "요청에 따라 코드를 수정하고 도구를 실행합니다. 권한 정책은 별도로 적용됩니다.",
        ),
        ("collaborationMode", "plan") => (
            "계획",
            "구현에 앞서 작업 계획을 만들고 필요한 사항을 확인합니다.",
        ),
        ("mode", "ask") => ("변경 전 확인", "변경을 수행하기 전에 승인을 요청합니다."),
        ("mode", "code") => ("코딩", "코드를 작성하고 수정하는 작업 모드입니다."),
        ("mode", "plan" | "planning") => ("계획", "작업을 수행하기 전에 계획을 세웁니다."),
        ("mode", "auto") => ("자동", "제공자가 정한 자동 작업 모드를 사용합니다."),
        ("mode", "default") => ("기본", "제공자가 정한 기본 작업 모드를 사용합니다."),
        ("effort" | "effortLevel" | "thought_level", "none") => {
            ("없음", "추가 추론을 사용하지 않는 단계입니다.")
        }
        ("effort" | "effortLevel" | "thought_level", "minimal") => {
            ("최소", "추론을 최소화하는 단계입니다.")
        }
        ("effort" | "effortLevel" | "thought_level", "low") => (
            "낮음",
            "간단한 작업에 맞춰 추론을 적게 사용하는 단계입니다.",
        ),
        ("effort" | "effortLevel" | "thought_level", "medium") => {
            ("보통", "응답 시간과 추론 깊이의 균형을 맞추는 단계입니다.")
        }
        ("effort" | "effortLevel" | "thought_level", "high") => {
            ("높음", "복잡한 작업을 위해 더 깊게 추론하는 단계입니다.")
        }
        ("effort" | "effortLevel" | "thought_level", "xhigh") => {
            ("매우 높음", "높음보다 더 많은 추론을 사용하는 단계입니다.")
        }
        ("effort" | "effortLevel" | "thought_level", "max") => {
            ("최대", "제공자가 정의한 최대 수준의 추론 단계입니다.")
        }
        ("effort" | "effortLevel" | "thought_level", "ultra") => {
            ("최상", "제공자가 정의한 최상위 추론 단계입니다.")
        }
        ("serviceTier", "default" | "standard") => ("표준", "표준 처리 등급을 사용합니다."),
        ("serviceTier", "auto") => (
            "자동",
            "계정과 요청에 맞는 처리 등급을 제공자가 선택합니다.",
        ),
        ("serviceTier", "priority" | "fast") => (
            "우선 처리",
            "우선 처리 등급으로 요청합니다. 지원 여부와 요금은 제공자의 정책을 따릅니다.",
        ),
        ("serviceTier", "flex") => (
            "유연한 처리",
            "처리 시간과 가용성이 달라질 수 있는 유연한 처리 등급을 사용합니다.",
        ),
        ("summary", "auto") => ("자동", "모델이 적절한 추론 요약 방식을 선택합니다."),
        ("summary", "concise") => ("간결하게", "추론 요약을 짧게 표시합니다."),
        ("summary", "detailed") => ("자세하게", "추론 요약을 자세히 표시합니다."),
        ("summary", "none") => (
            "표시 안 함",
            "추론 요약을 표시하지 않습니다. 추론 강도는 바뀌지 않습니다.",
        ),
        ("thinkingDisplay", "summarized") => ("요약", "생각 과정을 요약해서 표시합니다."),
        ("thinkingDisplay", "omitted") => (
            "숨김",
            "생각 과정 표시를 생략합니다. 추론 자체를 끄는 설정은 아닙니다.",
        ),
        ("thinkingDisplay", "highlights") => ("핵심만", "생각 과정의 주요 내용만 표시합니다."),
        ("workflowSizeGuideline", "small") => (
            "작게",
            "에이전트 5개 미만을 목표로 합니다. 강제 제한은 아닙니다.",
        ),
        ("workflowSizeGuideline", "medium") => (
            "보통",
            "에이전트 10개 미만을 목표로 합니다. 강제 제한은 아닙니다.",
        ),
        ("workflowSizeGuideline", "large") => (
            "크게",
            "에이전트 50개 미만을 목표로 합니다. 강제 제한은 아닙니다.",
        ),
        ("workflowSizeGuideline", "unrestricted") => (
            "규모 지침 없음",
            "워크플로에 권장 에이전트 규모를 지정하지 않습니다.",
        ),
        ("promptCacheTtl" | "subagentPromptCacheTtl", "5m") => {
            ("5분", "프롬프트 캐시를 5분 동안 유지하도록 요청합니다.")
        }
        ("promptCacheTtl" | "subagentPromptCacheTtl", "1h") => {
            ("1시간", "프롬프트 캐시를 1시간 동안 유지하도록 요청합니다.")
        }
        ("outputStyle", "Default" | "default") => {
            ("기본", "소프트웨어 개발에 맞춘 기본 응답 스타일입니다.")
        }
        ("outputStyle", "Proactive" | "proactive") => (
            "주도적으로",
            "합리적인 가정으로 일상적인 작업을 시작하는 스타일입니다.",
        ),
        ("outputStyle", "Concise" | "concise") => {
            ("간결하게", "결과를 먼저 전달하고 불필요한 서두를 줄입니다.")
        }
        ("outputStyle", "Explanatory" | "explanatory") => {
            ("설명 중심", "구현 이유와 관련 개념을 함께 설명합니다.")
        }
        ("outputStyle", "Learning" | "learning") => (
            "학습 중심",
            "설명을 제공하고 일부 코드를 직접 작성하며 배울 수 있게 합니다.",
        ),
        ("sandboxPolicy" | "permissions", "read-only" | ":read-only") => {
            ("읽기 전용", "파일을 읽을 수 있지만 변경할 수 없습니다.")
        }
        ("sandboxPolicy" | "permissions", "workspace-write" | ":workspace") => (
            "작업 폴더에 쓰기",
            "작업 폴더와 추가로 허용한 경로에 파일을 쓸 수 있습니다.",
        ),
        ("sandboxPolicy", "external-sandbox") => {
            ("외부 샌드박스", "파일 실행 격리를 외부 환경이 담당합니다.")
        }
        ("sandboxPolicy" | "permissions", "danger-full-access" | "full-access" | ":danger-full-access") => (
            "전체 접근",
            "파일 접근에 대한 샌드박스 제한 없이 실행합니다.",
        ),
        (id, "on") if id.starts_with("skill:") => (
            "모두 사용",
            "AI에 스킬 이름과 설명을 제공하고 사용자도 실행할 수 있게 합니다.",
        ),
        (id, "name-only") if id.starts_with("skill:") => {
            ("이름만 공개", "AI에 스킬 이름만 제공하고 설명은 숨깁니다.")
        }
        (id, "user-invocable-only") if id.starts_with("skill:") => (
            "직접 실행만",
            "AI에서 숨기고 사용자의 슬래시 명령 실행만 허용합니다.",
        ),
        (id, "off") if id.starts_with("skill:") => {
            ("사용 안 함", "AI와 사용자 모두에게서 이 스킬을 숨깁니다.")
        }
        (_, "true") => ("켜짐", "이 기능을 사용합니다."),
        (_, "false") => ("꺼짐", "이 기능을 사용하지 않습니다."),
        _ => return None,
    };
    Some((tr(pair.0), tr(pair.1)))
}
pub(crate) fn choice_help(option: &ControlOption, choice: &ControlChoice) -> String {
    let id = semantic_id(option);
    if let Some((_, help)) = choice_copy(id, &choice.value) {
        return help.into();
    }
    if matches!(id, "model" | "advisorModel") {
        return model_summary(&choice.description);
    }
    if choice
        .description
        .chars()
        .any(|c| ('가'..='힣').contains(&c))
    {
        return choice.description.clone();
    }
    tr(match id {
        "agent" => "이 이름으로 설치된 에이전트의 지시와 도구 구성을 사용합니다.",
        "outputStyle" => "이 이름으로 설치된 사용자 응답 스타일을 사용합니다.",
        "permissions" => "이 이름으로 정의된 권한 구성을 적용합니다. 정확한 접근 범위는 제공자 설명을 확인하세요.",
        _ => "제공자가 정의한 선택지입니다. 세부 동작은 제공자 설명을 확인하세요.",
    }).into()
}
pub(super) fn model_summary(description: &str) -> String {
    // Exact catalog text, not inferred abilities from a model name.
    for (segment, summary) in [
        (
            "Best for everyday, complex tasks",
            "일상적인 작업과 복잡한 작업을 위한 모델",
        ),
        (
            "Most capable for your hardest and longest-running tasks",
            "가장 어려운 작업과 장시간 작업을 위한 모델",
        ),
        (
            "Efficient for routine tasks",
            "반복적인 일상 작업에 효율적인 모델",
        ),
        ("Fastest for quick answers", "빠른 답변을 위한 모델"),
    ] {
        if description.split(" · ").any(|part| part == segment) {
            return tr(summary).into();
        }
    }
    if description.starts_with("Use the default model (currently ") {
        return tr("제공자가 현재 권장하는 기본 모델").into();
    }
    let translated = match description.trim() {
        "Latest workhorse model for coding and everyday work." => {
            "코딩과 일상적인 작업을 위한 최신 범용 모델"
        }
        "Frontier intelligence for the most demanding work." => "고난도 작업을 위한 고성능 모델",
        "GPT-6.1 Terra Codex model." => "GPT-6.1 Terra 계열의 Codex 모델",
        "Previous generation workhorse model." => "이전 세대 범용 모델",
        "Fast and affordable model for easier tasks." => "간단한 작업을 위한 빠르고 경제적인 모델",
        "" => "이 계정에서 제공하는 AI 모델",
        text if text.chars().any(|c| ('가'..='힣').contains(&c)) => text,
        _ => "이 계정에서 제공하는 AI 모델 · 상세 정보에서 제공자 설명 확인",
    };
    tr(translated).into()
}
pub(crate) fn help_text(ui: &mut Ui, text: &str) {
    ui.add(
        egui::Label::new(
            RichText::new(text)
                .size(12.)
                .color(Theme::current().text_dim),
        )
        .wrap(),
    );
}
