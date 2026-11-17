//! Prepared, selectable tool content. Highlighting runs on the transcript worker.
use super::super::ToolCall;
use egui::{Color32, FontId, RichText, TextFormat, Ui, text::LayoutJob};
use kiln_accounts::Tool;
use kiln_common::Theme;
use kiln_editor::{
    highlight::{Highlighter, Span},
    syntax,
};
use serde_json::Value;
use std::{
    hash::{Hash, Hasher},
    ops::Range,
    path::Path,
    sync::Arc,
};

// Highlighting is bounded, but the complete text remains available to scrolling
// and copying. The renderer lays out only visible lines, including plain tails.
const HIGHLIGHT_BYTES: usize = 256 * 1024;
const HIGHLIGHT_LINES: usize = 2_000;

#[derive(Clone)]
pub(super) struct PreparedTool {
    fingerprint: u64,
    pub(super) theme: Theme,
    blocks: Vec<Arc<Block>>,
    rows: usize,
}
#[derive(Clone)]
struct Block {
    title: String,
    text: Arc<str>,
    language: Option<String>,
    lines: Vec<Range<usize>>,
    spans: Vec<Vec<Span>>,
    diff: bool,
}
fn fingerprint(tool: &ToolCall) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    tool.name.hash(&mut h);
    tool.input.to_string().hash(&mut h);
    tool.output.hash(&mut h);
    tool.diff.hash(&mut h);
    h.finish()
}
impl PreparedTool {
    pub(super) fn reuse_or_new(previous: Option<&Self>, tool: &ToolCall, provider: Tool) -> Self {
        let fingerprint = fingerprint(tool);
        if let Some(previous) =
            previous.filter(|p| p.fingerprint == fingerprint && p.theme == Theme::current())
        {
            return previous.clone();
        }
        Self::new(tool, provider)
    }
    pub(super) fn new(tool: &ToolCall, provider: Tool) -> Self {
        let mut result = Self {
            fingerprint: fingerprint(tool),
            theme: Theme::current(),
            blocks: Vec::new(),
            rows: 0,
        };
        // Only unwrap known native Codex envelopes. Domain id/type fields stay
        // ordinary input, following approval::tool_input's provider contract.
        let native = provider == Tool::Codex
            && tool.input["id"].as_str() == Some(&tool.id)
            && matches!(
                tool.input["type"].as_str(),
                Some("commandExecution" | "fileChange" | "mcpToolCall" | "dynamicToolCall")
            );
        let native_arguments = native
            && matches!(
                tool.input["type"].as_str(),
                Some("mcpToolCall" | "dynamicToolCall")
            );
        let args = if native_arguments {
            tool.input.get("arguments").unwrap_or(&tool.input)
        } else {
            &tool.input
        };
        let parsed = args
            .as_str()
            .and_then(|s| serde_json::from_str::<Value>(s).ok());
        let args = parsed.as_ref().unwrap_or(args);
        let path_field = ["file_path", "path", "filePath", "filename"]
            .iter()
            .find_map(|k| args[*k].as_str().map(|value| (*k, value)));
        let path = path_field.map(|(_, value)| value);
        let command_field = ["command", "cmd"]
            .iter()
            .find_map(|k| args[*k].as_str().map(|value| (*k, value)));
        let mut consumed = std::collections::HashSet::new();
        if let Some((key, path)) = path_field {
            result.push("대상 경로", path, None, false);
            consumed.insert(key);
        }
        if let Some((key, command)) = command_field {
            result.push("실행 명령", command, Some("sh"), false);
            consumed.insert(key);
        }
        for (key, label) in [
            ("content", "작성 내용"),
            ("source", "작성 내용"),
            ("code", "작성 내용"),
            ("old_string", "변경 전"),
            ("new_string", "변경 후"),
            ("patch", "변경 내용"),
            ("diff", "변경 내용"),
        ] {
            if let Some(text) = args[key].as_str() {
                result.push(label, text, path, matches!(key, "patch" | "diff"));
                consumed.insert(key);
            }
        }
        if !consumed.is_empty() {
            // Keep remaining arguments visible; omit native output/status fields
            // already rendered below rather than repeating the whole envelope.
            if let Some(object) = args.as_object() {
                let remaining: serde_json::Map<String, Value> = object
                    .iter()
                    .filter(|(key, _)| {
                        !consumed.contains(key.as_str())
                            && (!native
                                || native_arguments
                                || matches!(
                                    key.as_str(),
                                    "cwd"
                                        | "workdir"
                                        | "timeout"
                                        | "timeout_ms"
                                        | "description"
                                        | "yield_time_ms"
                                        | "durationMs"
                                        | "exitCode"
                                ))
                    })
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                if !remaining.is_empty() {
                    result.push(
                        "입력",
                        &serde_json::to_string_pretty(&remaining).unwrap_or_default(),
                        Some("json"),
                        false,
                    );
                }
            }
        } else {
            let input = super::super::approval::tool_input(tool, provider);
            let json = serde_json::from_str::<Value>(&input).is_ok();
            if !input.is_empty() {
                result.push("입력", &input, json.then_some("json"), false);
            }
        }
        let read = tool.name.to_ascii_lowercase().contains("read");
        if !tool.output.is_empty() {
            result.push("출력", &tool.output, if read { path } else { None }, false);
        }
        if !tool.diff.is_empty() {
            result.push("변경 내용", &tool.diff, Some("diff"), true);
        }
        result.rows = result.blocks.iter().map(|b| 1 + b.lines.len()).sum();
        result
    }
    fn push(&mut self, title: &str, text: &str, hint: Option<&str>, diff: bool) {
        // Preserve fence markers and prose exactly, but highlight the enclosed
        // source with its declared language, even inside mixed tool output.
        let mut start = 0;
        let mut offset = 0;
        let mut fence: Option<(char, usize)> = None;
        let mut language = hint;
        for line in text.split_inclusive('\n') {
            let trimmed = line.trim_start();
            let marker = trimmed.chars().next();
            let count = marker.map_or(0, |c| trimmed.chars().take_while(|m| *m == c).count());
            if matches!(marker, Some('`' | '~')) && count >= 3 {
                if let Some((open, length)) = fence {
                    if marker == Some(open) && count >= length {
                        self.block(title, &text[start..offset], language, diff);
                        self.block(title, line, None, false);
                        start = offset + line.len();
                        fence = None;
                        language = hint;
                    }
                } else {
                    self.block(title, &text[start..offset], hint, diff);
                    self.block(title, line, None, false);
                    start = offset + line.len();
                    language = trimmed[count..].trim().split_whitespace().next();
                    fence = Some((marker.unwrap(), count));
                }
            }
            offset += line.len();
        }
        self.block(title, &text[start..], language, diff);
    }
    fn block(&mut self, title: &str, text: &str, hint: Option<&str>, diff: bool) {
        if text.is_empty() {
            return;
        }
        let syntax = hint.and_then(|hint| {
            syntax::assets()
                .set
                .find_syntax_by_token(hint)
                .or_else(|| syntax::detect(Path::new(hint), ""))
                .or_else(|| syntax::assets().set.find_syntax_by_extension(hint))
        });
        let mut start = 0;
        let lines: Vec<_> = text
            .split_inclusive('\n')
            .map(|line| {
                let range = start..start + line.len();
                start += line.len();
                range
            })
            .collect();
        let n = lines
            .iter()
            .take(HIGHLIGHT_LINES)
            .take_while(|r| r.end <= HIGHLIGHT_BYTES)
            .count();
        let content: Vec<_> = lines[..n]
            .iter()
            .map(|r| {
                let line = text[r.clone()].trim_end_matches('\n');
                // Claude Read prefixes are presentation, not source. Highlight only
                // the source while retaining every prefix byte in the displayed line.
                let prefix = line
                    .find('→')
                    .filter(|i| line[..*i].trim().chars().all(|c| c.is_ascii_digit()))
                    .map_or(0, |i| i + '→'.len_utf8());
                (prefix, &line[prefix..])
            })
            .collect();
        let source: Vec<_> = content.iter().map(|(_, s)| *s).collect();
        let mut highlighter = Highlighter::new(syntax, n);
        highlighter.ensure(&source, n, None);
        let spans = content
            .iter()
            .enumerate()
            .map(|(i, (prefix, _))| {
                highlighter
                    .line_spans(i)
                    .unwrap_or_default()
                    .iter()
                    .map(|span| Span {
                        start: span.start + *prefix as u32,
                        end: span.end + *prefix as u32,
                        ..*span
                    })
                    .collect()
            })
            .collect();
        self.blocks.push(Arc::new(Block {
            title: title.into(),
            text: Arc::from(text),
            language: syntax.map(|s| s.name.clone()),
            lines,
            spans,
            diff,
        }));
    }
    pub(super) fn show(&self, ui: &mut Ui, id: &str) -> egui::Rect {
        let height = viewport_height(ui.clip_rect().height());
        // min_scrolled_height is intentional: available_height in the outer
        // transcript is merely its remaining cursor space, sometimes a sliver.
        let spacing = ui.spacing().item_spacing.y;
        ui.spacing_mut().item_spacing.y = 0.;
        let response = egui::ScrollArea::both()
            .id_salt((id, "tool-code"))
            .min_scrolled_height(height)
            .max_height(height)
            .auto_shrink([false, true])
            .show_rows(ui, 24., self.rows, |ui, rows| {
                let mut base = 0;
                for (block_index, block) in self.blocks.iter().enumerate() {
                    let end = base + 1 + block.lines.len();
                    if rows.start < end && rows.end > base {
                        for row in rows.start.max(base)..rows.end.min(end) {
                            let row_start = ui.cursor().top();
                            ui.push_id((block_index, row), |ui| {
                                if row == base {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(kiln_common::i18n::tr(&block.title))
                                                .color(self.theme.text_dim)
                                                .small(),
                                        );
                                        if let Some(language) = &block.language {
                                            ui.label(
                                                RichText::new(language)
                                                    .color(self.theme.text_dim)
                                                    .small(),
                                            );
                                        }
                                        if ui.small_button(kiln_common::i18n::tr("복사")).clicked()
                                        {
                                            ui.ctx().copy_text(block.text.to_string());
                                        }
                                    });
                                } else {
                                    let line = row - base - 1;
                                    let text = block.text[block.lines[line].clone()]
                                        .trim_end_matches('\n');
                                    let job = line_job(
                                        text,
                                        block
                                            .spans
                                            .get(line)
                                            .map(Vec::as_slice)
                                            .unwrap_or_default(),
                                        self.theme,
                                        block.diff,
                                    );
                                    ui.add(egui::Label::new(job).selectable(true).extend());
                                }
                            });
                            let used = ui.cursor().top() - row_start;
                            ui.add_space((24. - used).max(0.));
                        }
                    }
                    base = end;
                }
            });
        ui.spacing_mut().item_spacing.y = spacing;
        response.inner_rect
    }
}
pub(super) fn viewport_height(clip_height: f32) -> f32 {
    (clip_height * 0.6)
        .clamp(200., 360.)
        .min((clip_height - 24.).max(64.))
}
fn line_job(text: &str, spans: &[Span], theme: Theme, diff: bool) -> LayoutJob {
    let mut job = LayoutJob::default();
    let base = if diff && text.starts_with('+') {
        theme.green
    } else if diff && text.starts_with('-') {
        theme.red
    } else {
        theme.text
    };
    let format = |color: Color32| TextFormat {
        font_id: FontId::monospace(12.),
        color,
        ..Default::default()
    };
    let mut cursor = 0;
    for span in spans {
        let start = span.start as usize;
        let end = (span.end as usize).min(text.len());
        if start < cursor
            || start >= end
            || !text.is_char_boundary(start)
            || !text.is_char_boundary(end)
        {
            continue;
        }
        job.append(&text[cursor..start], 0., format(base));
        let mut f = format(if diff { base } else { span.color });
        f.italics = span.italic;
        if span.underline {
            f.underline = egui::Stroke::new(1., f.color);
        }
        job.append(&text[start..end], 0., f);
        cursor = end;
    }
    job.append(&text[cursor..], 0., format(base));
    job
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::agent_gui::ToolCall;
    use serde_json::json;

    #[test]
    fn tool_code_semantic_source_keeps_exact_unicode_and_selects_language() {
        let source = "pub fn 인사() { println!(\"안녕\"); }\n";
        let tool = ToolCall {
            name: "Write".into(),
            input: json!({"file_path":"src/main.rs", "content": source}),
            ..Default::default()
        };
        let prepared = PreparedTool::new(&tool, kiln_accounts::Tool::Claude);
        let block = prepared
            .blocks
            .iter()
            .find(|b| b.text.as_ref() == source)
            .unwrap();
        assert_eq!(block.language.as_deref(), Some("Rust"));
        assert!(
            block
                .spans
                .iter()
                .flatten()
                .any(|s| s.color != prepared.theme.text)
        );
        assert!(
            prepared
                .blocks
                .iter()
                .all(|b| !b.text.contains("\\\"content\\\""))
        );
    }

    #[test]
    fn tool_code_shell_fences_plain_logs_and_complete_large_output() {
        let output = format!("{}\nTAIL 한글", "ordinary log\n".repeat(10_000));
        let tool = ToolCall {
            name: "Bash".into(),
            input: json!({"command":"printf '%s\\n' \"안녕\""}),
            output: output.clone(),
            ..Default::default()
        };
        let prepared = PreparedTool::new(&tool, kiln_accounts::Tool::Claude);
        assert!(
            prepared
                .blocks
                .iter()
                .any(|b| b.language.as_deref() == Some("Bourne Again Shell (bash)"))
        );
        let logs = prepared
            .blocks
            .iter()
            .find(|b| b.text.as_ref() == output)
            .unwrap();
        assert_eq!(logs.language, None);
        assert!(logs.text.ends_with("TAIL 한글"));
        assert!(logs.lines.len() > 10_000);
        let tool = ToolCall {
            output: "before\n```python\nprint('안녕')\n```\nafter".into(),
            ..Default::default()
        };
        let prepared = PreparedTool::new(&tool, kiln_accounts::Tool::Claude);
        let output: String = prepared.blocks.iter().map(|b| b.text.as_ref()).collect();
        assert_eq!(output, tool.output);
        assert!(
            prepared
                .blocks
                .iter()
                .any(|b| b.language.as_deref() == Some("Python"))
        );
    }

    #[test]
    fn tool_code_semantic_projection_keeps_unconsumed_values_and_path_aliases() {
        let tool = ToolCall {
            name: "Write".into(),
            input: json!({"file_path":"first.rs", "path":"second.rs", "content":[{"text":"exact content"}], "command":["bash", "-lc", "echo exact"], "cwd":"/fixture", "timeout_ms":1234}),
            ..Default::default()
        };
        let prepared = PreparedTool::new(&tool, Tool::Claude);
        let text = prepared
            .blocks
            .iter()
            .map(|b| b.text.as_ref())
            .collect::<String>();
        for expected in [
            "first.rs",
            "second.rs",
            "exact content",
            "bash",
            "echo exact",
            "/fixture",
            "1234",
        ] {
            assert!(
                text.contains(expected),
                "unconsumed argument must survive: {expected}"
            );
        }
        assert!(text.contains("\"content\"") && text.contains("\"command\""));
    }

    #[test]
    fn tool_code_cache_tracks_content_and_full_palette() {
        let original = Theme::current();
        Theme::set_current("kiln-dark");
        let mut tool = ToolCall {
            name: "Write".into(),
            input: json!({"file_path":"file.rs", "content":"fn main() { let n = 42; }"}),
            ..Default::default()
        };
        let first = PreparedTool::new(&tool, Tool::Claude);
        let same = PreparedTool::reuse_or_new(Some(&first), &tool, Tool::Claude);
        assert!(Arc::ptr_eq(&first.blocks[0], &same.blocks[0]));
        Theme::set_current("kiln-light");
        let light = PreparedTool::reuse_or_new(Some(&first), &tool, Tool::Claude);
        assert!(!Arc::ptr_eq(&first.blocks[0], &light.blocks[0]));
        assert_eq!(light.theme, Theme::KILN_LIGHT);
        tool.output = "new output".into();
        let changed = PreparedTool::reuse_or_new(Some(&light), &tool, Tool::Claude);
        assert!(!Arc::ptr_eq(&light.blocks[0], &changed.blocks[0]));
        assert!(
            changed
                .blocks
                .iter()
                .any(|b| b.text.as_ref() == "new output")
        );
        Theme::set_current(original.name);
    }

    #[test]
    fn tool_code_native_envelopes_hide_transport_fields_but_keep_domain_arguments() {
        let native = ToolCall {
            id: "native".into(),
            input: json!({"id":"native", "type":"commandExecution", "command":"cargo test", "cwd":"/fixture", "processId":"private-process", "status":"running"}),
            ..Default::default()
        };
        let prepared = PreparedTool::new(&native, Tool::Codex);
        let text = prepared
            .blocks
            .iter()
            .map(|b| b.text.as_ref())
            .collect::<String>();
        assert!(text.contains("cargo test") && text.contains("/fixture"));
        assert!(!text.contains("private-process") && !text.contains("commandExecution"));
        let domain = ToolCall {
            id: "native".into(),
            input: json!({"id":"native", "type":"mcpToolCall", "arguments":{"command":"run", "id":"invoice-42", "type":"invoice", "status":"draft"}}),
            ..Default::default()
        };
        let prepared = PreparedTool::new(&domain, Tool::Codex);
        let text = prepared
            .blocks
            .iter()
            .map(|b| b.text.as_ref())
            .collect::<String>();
        assert!(text.contains("invoice-42") && text.contains("invoice") && text.contains("draft"));
    }

    #[test]
    fn tool_code_read_prefixes_and_diff_preserve_exact_text_and_sign_colors() {
        let output = "     1→pub fn 인사() {}\n";
        let diff = "--- a/main.rs\n+++ b/main.rs\n-old\n+new\n";
        let tool = ToolCall {
            name: "Read".into(),
            input: json!({"file_path":"main.rs"}),
            output: output.into(),
            diff: diff.into(),
            ..Default::default()
        };
        let prepared = PreparedTool::new(&tool, Tool::Claude);
        let block = prepared
            .blocks
            .iter()
            .find(|b| b.text.as_ref() == output)
            .unwrap();
        let line = output.trim_end_matches('\n');
        let job = line_job(line, &block.spans[0], prepared.theme, false);
        assert_eq!(job.text, line);
        assert!(
            block.spans[0]
                .iter()
                .all(|s| s.start >= "     1→".len() as u32)
        );
        for (text, color) in [("+new", prepared.theme.green), ("-old", prepared.theme.red)] {
            let job = line_job(text, &[], prepared.theme, true);
            assert_eq!(job.text, text);
            assert!(job.sections.iter().all(|s| s.format.color == color));
        }
        assert!(prepared.blocks.iter().any(|b| b.text.as_ref() == diff));
    }

    #[test]
    fn tool_code_viewport_reserves_readable_height_independent_of_remaining_cursor() {
        assert_eq!(viewport_height(700.), 360.);
        assert!(viewport_height(420.) >= 200.);
        assert!(viewport_height(150.) <= 150.);
    }
}

#[cfg(test)]
mod visual_tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    #[test]
    fn tool_code_nested_viewport_highlights_selects_and_copies_in_dark_and_light() {
        for dark in [true, false] {
            for width in [320., 900.] {
                Theme::set_current(if dark { "kiln-dark" } else { "kiln-light" });
                let source = "// Exact source · 한글\npub fn greet(name: &str) -> String {\n    format!(\"Hello, {name}!\")\n}\n".repeat(30);
                let tool = ToolCall {
                    id: "source".into(),
                    name: "Read".into(),
                    input: serde_json::json!({"file_path":"src/main.rs"}),
                    output: source.clone(),
                    ..Default::default()
                };
                let prepared = PreparedTool::new(&tool, Tool::Claude);
                let mut initialized = false;
                let mut h = Harness::builder().with_size([width, 700.]).build_ui_state(
                    |ui, (prepared, viewport): &mut (PreparedTool, Option<egui::Rect>)| {
                        Theme::set_current(if dark { "kiln-dark" } else { "kiln-light" });
                        Theme::current().apply(ui.ctx());
                        if !initialized {
                            kiln_common::fonts::install(ui.ctx());
                            initialized = true;
                            return;
                        }
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            // The enclosing transcript has only a tiny remainder
                            // even though its actual clip is a healthy window.
                            ui.add_space(620.);
                            *viewport = Some(prepared.show(ui, "nested-source"));
                        });
                    },
                    (prepared, None),
                );
                h.run();
                let viewport = h.state().1.unwrap();
                assert!(
                    viewport.height() >= 300.,
                    "reserved readable viewport despite outer cursor: {viewport:?}"
                );
                assert!(
                    viewport.width() <= width,
                    "horizontal overflow stays inside viewport"
                );
                // Scroll the outer transcript to expose the nested viewport.
                h.event(egui::Event::PointerMoved(egui::pos2(width / 2., 100.)));
                h.event(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    phase: egui::TouchPhase::Move,
                    delta: egui::vec2(0., -600.),
                    modifiers: egui::Modifiers::NONE,
                });
                h.run();
                assert!(h.query_all_by_label_contains("pub fn greet").count() > 0);
                let copied = h.query_all_by_label("복사").nth(1).unwrap();
                copied.click();
                h.step();
                assert!(h.output().platform_output.commands.iter().any(|command| matches!(command, egui::OutputCommand::CopyText(text) if text == &source)));
                if let Some(dir) = std::env::var_os("KILN_VISUAL_CAPTURE_DIR") {
                    let dir = std::path::PathBuf::from(dir);
                    std::fs::create_dir_all(&dir).unwrap();
                    h.render()
                        .unwrap()
                        .save(dir.join(format!(
                            "tool-code-{}-{width:.0}.png",
                            if dark { "dark" } else { "light" }
                        )))
                        .unwrap();
                }
            }
        }
    }
}
