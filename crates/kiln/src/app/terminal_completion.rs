//! Bounded, local shell suggestions. No subprocess, history file, or network lookup.
use std::path::Path;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub label: String,
    pub description: String,
    pub buffer: String,
    pub cursor: usize,
}
const COMMANDS: &[(&str, &str)] = &[
    ("cd", "디렉터리 이동"),
    ("ls", "파일 목록 보기"),
    ("git", "버전 관리"),
    ("npm", "Node 패키지 관리"),
    ("docker", "컨테이너 관리"),
    ("cargo", "Rust 패키지 관리"),
    ("mkdir", "디렉터리 만들기"),
    ("pwd", "현재 경로 출력"),
];
fn options(command: &str) -> &'static [(&'static str, &'static str)] {
    match command {
        "git" => &[
            ("status", "작업 트리 변경 사항 보기"),
            ("diff", "변경 사항 비교"),
            ("log", "커밋 기록 보기"),
            ("switch", "브랜치 전환"),
            ("add", "파일 변경 사항 스테이징"),
            ("commit", "스테이징한 변경 사항 커밋"),
            ("--help", "Git 도움말 열기"),
        ],
        "docker" => &[
            ("ps", "컨테이너 목록 보기"),
            ("images", "이미지 목록 보기"),
            ("build", "이미지 빌드"),
            ("run", "컨테이너 실행"),
            ("compose", "Compose 서비스 관리"),
            ("--help", "Docker 도움말 열기"),
        ],
        "npm" => &[
            ("install", "의존성 설치"),
            ("run", "패키지 스크립트 실행"),
            ("test", "테스트 스크립트 실행"),
            ("--help", "npm 도움말 열기"),
        ],
        "cargo" => &[
            ("check", "실행 파일 생성 없이 검사"),
            ("build", "패키지 빌드"),
            ("test", "패키지 테스트 실행"),
            ("run", "빌드 후 실행"),
            ("--help", "Cargo 도움말 열기"),
        ],
        "ls" => &[
            ("-a", "숨김 항목 포함"),
            ("-l", "자세한 목록 형식 사용"),
            ("-h", "읽기 쉬운 크기 표시"),
        ],
        "cd" => &[("..", "상위 디렉터리"), ("-", "이전 디렉터리")],
        _ => &[],
    }
}
/// Simple words only. Shell expressions are deliberately left to native completion.
fn words(input: &str) -> Option<Vec<(usize, String)>> {
    let mut result = Vec::new();
    let mut word = String::new();
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut active = false;
    for (i, ch) in input.char_indices() {
        if !active && !ch.is_whitespace() {
            start = i;
            active = true;
        }
        if escaped {
            word.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if q == '"' && matches!(ch, '$' | '`') {
                return None;
            }
            if ch == q {
                quote = None;
            } else {
                word.push(ch);
            }
            continue;
        }
        if matches!(ch, '\'' | '"') {
            quote = Some(ch);
            continue;
        }
        if matches!(ch, ';' | '|' | '&' | '$' | '`' | '(' | ')' | '<' | '>') {
            return None;
        }
        if ch.is_whitespace() {
            if active {
                result.push((start, std::mem::take(&mut word)));
                active = false;
            }
        } else {
            word.push(ch);
        }
    }
    if escaped {
        return None;
    }
    if active {
        result.push((start, word));
    } else {
        result.push((input.len(), String::new()));
    }
    Some(result)
}
fn quoted(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_alphanumeric() || "/_-.".contains(c))
    {
        value.into()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}
pub fn candidates(
    buffer: &str,
    cursor: usize,
    cwd: &Path,
    native_commands: &[String],
) -> Vec<Choice> {
    if buffer.len() > 4096 || buffer.contains(['\n', '\r']) {
        return vec![];
    }
    let Some(byte) = buffer
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(buffer.len()))
        .nth(cursor)
    else {
        return vec![];
    };
    // Replacing a token before unconsumed text needs a full shell parser. Fail closed.
    if byte != buffer.len() {
        return vec![];
    }
    let Some(tokens) = words(buffer) else {
        return vec![];
    };
    let Some((start, prefix)) = tokens.last() else {
        return vec![];
    };
    let command = &tokens[0].1;
    let mut raw = Vec::<(String, String)>::new();
    if tokens.len() == 1 {
        raw.extend(COMMANDS.iter().map(|(a, b)| (a.to_string(), b.to_string())));
        raw.extend(
            native_commands
                .iter()
                .filter(|n| !n.contains(['\n', '\r', '\0']))
                .map(|n| (n.clone(), "설치된 명령".into())),
        );
    } else if command == "cd" || prefix.contains('/') {
        let slash = prefix.rfind('/');
        let (base, leaf) = slash
            .map(|i| (&prefix[..=i], &prefix[i + 1..]))
            .unwrap_or(("", prefix.as_str()));
        // Tilde expansion is not guessed from the GUI's HOME; use the shell's native Tab.
        if !base.starts_with('~') {
            let dir = if Path::new(base).is_absolute() {
                Path::new(base).to_path_buf()
            } else {
                cwd.join(base)
            };
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.take(2048).flatten() {
                    let name = entry.file_name();
                    let Some(name) = name.to_str() else { continue };
                    if !name.starts_with(leaf) || (!leaf.starts_with('.') && name.starts_with('.'))
                    {
                        continue;
                    }
                    let is_dir = entry.path().is_dir();
                    if command == "cd" && !is_dir {
                        continue;
                    }
                    if name.chars().any(char::is_control) {
                        continue;
                    }
                    raw.push((
                        format!("{base}{name}{}", if is_dir { "/" } else { "" }),
                        if is_dir { "디렉터리" } else { "파일" }.into(),
                    ));
                }
            }
        }
    } else {
        raw.extend(
            options(command)
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string())),
        );
    }
    raw.retain(|(label, _)| label.starts_with(prefix));
    raw.sort_by(|a, b| a.0.cmp(&b.0));
    raw.dedup_by(|a, b| a.0 == b.0);
    raw.into_iter()
        .take(80)
        .map(|(label, description)| {
            let replacement = quoted(&label);
            let buffer = format!("{}{}", &buffer[..*start], replacement);
            let cursor = buffer.chars().count();
            Choice {
                label,
                description,
                buffer,
                cursor,
            }
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_quotes_unicode_and_no_shell_execution() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("한 글")).unwrap();
        std::fs::create_dir(d.path().join("O'Reilly")).unwrap();
        let rows = candidates("cd 한", 4, d.path(), &[]);
        assert_eq!(rows[0].buffer, "cd '한 글/'");
        assert!(!rows[0].buffer.contains('\n'));
        assert_eq!(
            candidates("cd 'O", 5, d.path(), &[])[0].buffer,
            "cd 'O'\\''Reilly/'"
        );
        assert!(candidates("cd $(evil)", 10, d.path(), &[]).is_empty());
        assert!(candidates("cd \"$HOME", 9, d.path(), &[]).is_empty());
        std::fs::create_dir(d.path().join("~literal")).unwrap();
        assert_eq!(
            candidates("cd '~", 5, d.path(), &[])[0].buffer,
            "cd '~literal/'"
        );
        assert!(candidates("git status", 3, d.path(), &[]).is_empty());
        assert_eq!(
            candidates("git st", 6, d.path(), &[])[0].buffer,
            "git status"
        );
    }
}

type Work = (
    kiln_proto::ShellCompletion,
    std::sync::mpsc::Sender<Vec<Choice>>,
);
fn enqueue(request: kiln_proto::ShellCompletion, tx: std::sync::mpsc::Sender<Vec<Choice>>) {
    static WORKER: std::sync::OnceLock<
        std::sync::Arc<(std::sync::Mutex<Option<Work>>, std::sync::Condvar)>,
    > = std::sync::OnceLock::new();
    let worker = WORKER.get_or_init(|| {
        let shared = std::sync::Arc::new((
            std::sync::Mutex::new(None::<Work>),
            std::sync::Condvar::new(),
        ));
        let c = shared.clone();
        std::thread::spawn(move || {
            loop {
                let job = {
                    let (work, signal) = (&c.0, &c.1);
                    let mut slot = work.lock().unwrap();
                    while slot.is_none() {
                        slot = signal.wait(slot).unwrap();
                    }
                    slot.take().unwrap()
                };
                let (r, tx) = job;
                let _ = tx.send(candidates(
                    &r.buffer,
                    r.cursor,
                    Path::new(&r.cwd),
                    &r.commands,
                ));
            }
        });
        shared
    });
    *worker.0.lock().unwrap() = Some((request, tx));
    worker.1.notify_one();
}
#[derive(Default)]
pub struct Popup {
    request: Option<kiln_proto::ShellCompletion>,
    rows: Vec<Choice>,
    rx: Option<std::sync::mpsc::Receiver<Vec<Choice>>>,
    selected: usize,
    closed_revision: Option<u64>,
    #[cfg(test)]
    last_rect: Option<egui::Rect>,
}
impl Popup {
    pub fn close(&mut self) {
        if let Some(request) = &self.request {
            self.closed_revision = Some(request.revision);
        }
        self.request = None;
        self.rows.clear();
        self.rx = None;
    }
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        conn: &mut super::conn::Conn,
        session: kiln_proto::SessionId,
        allowed: bool,
        preview_enabled: bool,
        rect: egui::Rect,
        anchor: egui::Pos2,
    ) {
        let snapshot = conn.shell_completions.get(&session).cloned();
        if !allowed || snapshot.is_none() {
            self.close();
            return;
        }
        let snapshot = snapshot.unwrap();
        if !preview_enabled && !snapshot.explicit {
            self.close();
            return;
        }
        if self.closed_revision == Some(snapshot.revision) {
            return;
        }
        if self
            .request
            .as_ref()
            .is_none_or(|r| r.revision != snapshot.revision)
        {
            let (tx, rx) = std::sync::mpsc::channel();
            enqueue(snapshot.clone(), tx);
            self.request = Some(snapshot);
            self.rx = Some(rx);
            self.rows.clear();
            self.selected = 0;
        }
        if let Some(rx) = &self.rx {
            match rx.try_recv() {
                Ok(rows) => {
                    self.rows = rows;
                    self.rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.close();
                    return;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        let engaged = self.request.as_ref().is_some_and(|r| r.explicit);
        let loading = engaged && self.rx.is_some();
        let ready = engaged && !self.rows.is_empty() && self.rx.is_none();
        let mut moved = false;
        let mut accept = false;
        let mut dismiss = false;
        ui.input_mut(|input| {
            input.events.retain(|e| match e {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.is_none()
                    && loading
                    && matches!(
                        key,
                        egui::Key::Enter
                            | egui::Key::Tab
                            | egui::Key::ArrowUp
                            | egui::Key::ArrowDown
                    ) =>
                {
                    false
                }
                egui::Event::Key {
                    key: egui::Key::Escape,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.is_none() && engaged => {
                    dismiss = true;
                    false
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.is_none() && ready => match key {
                    egui::Key::ArrowDown => {
                        moved = true;
                        self.selected = (self.selected + 1).min(self.rows.len().saturating_sub(1));
                        false
                    }
                    egui::Key::ArrowUp => {
                        moved = true;
                        self.selected = self.selected.saturating_sub(1);
                        false
                    }
                    egui::Key::Tab | egui::Key::Enter => {
                        accept = true;
                        false
                    }
                    egui::Key::Escape => {
                        dismiss = true;
                        false
                    }
                    _ => {
                        dismiss = true;
                        true
                    }
                },
                egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Ime(_) => {
                    dismiss = true;
                    true
                }
                egui::Event::Key { pressed: true, .. } => {
                    dismiss = true;
                    true
                }
                _ => true,
            });
        });
        if dismiss {
            self.close();
            return;
        }
        let theme = kiln_common::Theme::current();
        let frame = egui::Frame::popup(ui.style()).fill(theme.bg_elevated);
        let margin = frame.total_margin().sum();
        let outer_width = (rect.width() - 8.0).clamp(1.0, 436.0);
        let outer_height = (rect.height() - 8.0).clamp(1.0, 260.0);
        let width = (outer_width - margin.x).max(1.0);
        let content_height = (outer_height - margin.y).max(1.0);
        if !engaged && self.rows.is_empty() {
            return;
        }
        let height = outer_height;
        let pos = egui::pos2(
            anchor.x.clamp(
                rect.left() + 4.0,
                (rect.right() - outer_width - 4.0).max(rect.left() + 4.0),
            ),
            if anchor.y + height < rect.bottom() {
                anchor.y
            } else {
                (anchor.y - height - 16.0).max(rect.top() + 4.0)
            },
        );
        let popup_response=egui::Area::new(egui::Id::new(("shell-completion", session)))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .constrain_to(rect)
            .show(ui.ctx(), |ui| {
                frame.show(ui, |ui| {
                    ui.set_width(width);
                    egui::ScrollArea::vertical().id_salt("completion-body").max_height(content_height).min_scrolled_height(0.0).show(ui,|ui|{
                        ui.set_width(width);
                        ui.label(egui::RichText::new(kiln_common::i18n::tr("명령 자동완성")).strong());
                        if self.rx.is_some() {
                            ui.label(kiln_common::i18n::tr("추천 항목을 찾는 중…"));
                            ui.ctx()
                                .request_repaint_after(std::time::Duration::from_millis(30));
                        } else if self.rows.is_empty() {
                            ui.label(kiln_common::i18n::tr("추천 항목이 없습니다. Esc로 닫거나 Tab으로 셸 자동완성을 사용하세요."));
                        }
                        ui.scope(|ui| {
                                for (i, row) in self.rows.iter().enumerate() {
                                    let response = ui
                                        .selectable_label(self.selected == i, &row.label)
                                        .on_hover_text(kiln_common::i18n::tr(&row.description));
                                    if self.selected == i && moved {
                                        response.scroll_to_me(Some(egui::Align::Center));
                                    }
                                    if response.clicked() {
                                        self.selected = i;
                                        accept = true;
                                    }
                                }
                            });
                        if let Some(row) = self.rows.get(self.selected) {
                            ui.label(
                                egui::RichText::new(kiln_common::i18n::tr(&row.description))
                                    .small()
                                    .color(theme.text_dim),
                            );
                        }
                        ui.label(
                            egui::RichText::new(if engaged {
                                kiln_common::i18n::tr("↑↓ 선택 · Tab 삽입 · Esc 닫기")
                            } else {
                                kiln_common::i18n::tr("Ctrl+Space로 선택 · Tab은 셸 자동완성")
                            })
                            .small()
                            .color(theme.text_dim),
                        );
                    });
                });
            });
        #[cfg(test)]
        {
            self.last_rect = Some(popup_response.response.rect);
        }
        #[cfg(not(test))]
        let _ = popup_response;
        if accept {
            if let (Some(request), Some(row)) = (&self.request, self.rows.get(self.selected)) {
                conn.send(kiln_proto::ClientMsg::ApplyShellCompletion {
                    session,
                    revision: request.revision,
                    buffer: row.buffer.clone(),
                    cursor: row.cursor,
                });
            }
            self.close();
        }
    }
}

#[cfg(test)]
mod popup_tests {
    use super::*;
    use egui_kittest::Harness;
    #[test]
    fn preview_and_empty_results_preserve_native_keys_and_themes_fit() {
        for theme_name in ["kiln-dark", "kiln-light"] {
            kiln_common::Theme::set_current(theme_name);
            for (explicit, with_rows, loading, consumed) in [
                (false, true, false, false),
                (true, false, false, false),
                (true, true, false, true),
                (true, false, true, true),
            ] {
                let request = kiln_proto::ShellCompletion {
                    explicit,
                    revision: 1,
                    buffer: "git st".into(),
                    cursor: 6,
                    cwd: "/".into(),
                    request_file: String::new(),
                    commands: vec![],
                };
                let mut conn = super::super::conn::Conn::offline(egui::Context::default());
                conn.shell_completions.insert(1, request.clone());
                let rows = if with_rows {
                    candidates("git st", 6, Path::new("/"), &[])
                } else {
                    vec![]
                };
                let (_keep_alive, rx) = std::sync::mpsc::channel();
                let popup = Popup {
                    rx: loading.then_some(rx),
                    request: Some(request),
                    rows,
                    ..Default::default()
                };
                let theme = kiln_common::Theme::by_name(theme_name);
                let mut fonts = false;
                let mut h = Harness::builder().with_size([360.0, 240.0]).build_ui_state(
                    move |ui, state: &mut (Popup, super::super::conn::Conn, bool)| {
                        if !fonts {
                            kiln_common::fonts::install(ui.ctx());
                            fonts = true;
                            return;
                        }
                        theme.apply(ui.ctx());
                        let rect = ui.available_rect_before_wrap();
                        state.0.ui(
                            ui,
                            &mut state.1,
                            1,
                            true,
                            true,
                            rect,
                            rect.left_top() + egui::vec2(12.0, 24.0),
                        );
                        state.2 = ui.input(|i| {
                            i.events.iter().any(|e| {
                                matches!(
                                    e,
                                    egui::Event::Key {
                                        key: egui::Key::Tab,
                                        pressed: true,
                                        ..
                                    }
                                )
                            })
                        });
                    },
                    (popup, conn, false),
                );
                h.run_steps(3);
                if explicit && with_rows {
                    h.render()
                        .unwrap()
                        .save(format!("/tmp/kiln-shell-completion-{theme_name}.png"))
                        .unwrap();
                }
                h.event(egui::Event::Key {
                    key: egui::Key::Tab,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                });
                h.step();
                assert_eq!(
                    h.state().2,
                    !consumed,
                    "explicit={explicit}, rows={with_rows}"
                );
            }
        }
    }
}

#[cfg(test)]
mod short_popup_tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};
    #[test]
    fn short_pane_scrolls_whole_popup_and_keyboard_selection_stays_visible() {
        for theme_name in ["kiln-dark", "kiln-light"] {
            kiln_common::Theme::set_current(theme_name);
            for height in [100.0, 136.0] {
                let r = kiln_proto::ShellCompletion {
                    explicit: true,
                    revision: 10,
                    buffer: "git ".into(),
                    cursor: 4,
                    cwd: "/".into(),
                    request_file: String::new(),
                    commands: vec![],
                };
                let rows = (0..12)
                    .map(|i| Choice {
                        label: format!("fixture-option-{i:02}"),
                        description: "작업 트리 변경 사항 보기".into(),
                        buffer: format!("git fixture-{i}"),
                        cursor: 13,
                    })
                    .collect();
                let popup = Popup {
                    request: Some(r.clone()),
                    rows,
                    ..Default::default()
                };
                let mut conn = super::super::conn::Conn::offline(egui::Context::default());
                conn.shell_completions.insert(1, r);
                let theme = kiln_common::Theme::by_name(theme_name);
                let mut fonts = false;
                let mut h = Harness::builder()
                    .with_size([320.0, height])
                    .build_ui_state(
                        move |ui, s: &mut (Popup, super::super::conn::Conn, egui::Rect)| {
                            if !fonts {
                                kiln_common::fonts::install(ui.ctx());
                                fonts = true;
                                return;
                            }
                            theme.apply(ui.ctx());
                            s.2 = ui.available_rect_before_wrap();
                            s.0.ui(
                                ui,
                                &mut s.1,
                                1,
                                true,
                                true,
                                s.2,
                                s.2.left_top() + egui::vec2(8.0, 18.0),
                            );
                        },
                        (popup, conn, egui::Rect::NOTHING),
                    );
                h.run_steps(4);
                let rect = h.state().0.last_rect.unwrap();
                assert!(
                    h.state().2.expand(0.5).contains_rect(rect),
                    "popup {rect:?} exceeds {:?}",
                    h.state().2
                );
                for _ in 0..7 {
                    h.event(egui::Event::Key {
                        key: egui::Key::ArrowDown,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    });
                    h.step();
                }
                h.run_steps(3);
                assert_eq!(h.state().0.selected, 7);
                assert!(
                    h.state()
                        .0
                        .last_rect
                        .unwrap()
                        .contains_rect(h.get_by_label("fixture-option-07").rect()),
                    "keyboard-selected row must be visible before accepting"
                );
                h.render()
                    .unwrap()
                    .save(format!(
                        "/tmp/kiln-shell-completion-short-{theme_name}-{}.png",
                        height as u32
                    ))
                    .unwrap();
            }
        }
    }
}

#[cfg(test)]
mod dismissal_tests {
    use super::*;
    use egui_kittest::Harness;
    #[test]
    fn escape_then_blur_does_not_reopen_the_same_request() {
        let request = kiln_proto::ShellCompletion {
            explicit: true,
            revision: 77,
            buffer: "git st".into(),
            cursor: 6,
            cwd: "/".into(),
            request_file: String::new(),
            commands: vec![],
        };
        let popup = Popup {
            request: Some(request.clone()),
            rows: candidates("git st", 6, Path::new("/"), &[]),
            ..Default::default()
        };
        let mut conn = super::super::conn::Conn::offline(egui::Context::default());
        conn.shell_completions.insert(1, request);
        let mut fonts = false;
        let mut h = Harness::builder().with_size([360.0, 240.0]).build_ui_state(
            move |ui, s: &mut (Popup, super::super::conn::Conn, bool)| {
                if !fonts {
                    kiln_common::fonts::install(ui.ctx());
                    fonts = true;
                    return;
                }
                kiln_common::Theme::current().apply(ui.ctx());
                let rect = ui.available_rect_before_wrap();
                s.0.ui(ui, &mut s.1, 1, s.2, true, rect, rect.left_top());
            },
            (popup, conn, true),
        );
        h.run_steps(3);
        h.event(egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        h.step();
        assert_eq!(h.state().0.closed_revision, Some(77));
        h.state_mut().2 = false;
        h.run_steps(3);
        h.state_mut().2 = true;
        h.run_steps(3);
        assert!(h.state().0.request.is_none());
        assert_eq!(h.state().0.closed_revision, Some(77));
        h.state_mut()
            .1
            .shell_completions
            .get_mut(&1)
            .unwrap()
            .revision = 78;
        h.step();
        assert_eq!(
            h.state().0.request.as_ref().map(|r| r.revision),
            Some(78),
            "a fresh explicit request must still open"
        );
    }
}
