//! Synthetic configuration fixtures rendered by the real editor and wgpu.
//! Captures are review artifacts in /tmp, never repository snapshot baselines.

mod common;

use egui::{Color32, Shape};
use egui_kittest::Harness;
use kiln_editor::Editor;

fn painted_token(shape: &Shape, line: &str, literal: &str) -> Option<Color32> {
    match shape {
        Shape::Vec(shapes) => shapes.iter().find_map(|s| painted_token(s, line, literal)),
        Shape::Text(text) if text.galley.job.text == line => {
            let col = line.find(literal).expect(literal);
            let section = text
                .galley
                .job
                .sections
                .iter()
                .find(|s| s.byte_range.contains(&col.into()))?;
            Some(text.override_text_color.unwrap_or(section.format.color))
        }
        _ => None,
    }
}

fn color(h: &Harness<'_, Editor>, line: &str, literal: &str) -> Color32 {
    h.output()
        .shapes
        .iter()
        .find_map(|s| painted_token(&s.shape, line, literal))
        .unwrap_or_else(|| panic!("editor did not paint {literal:?} in {line:?}"))
}

#[test]
fn config_languages_render_with_distinct_tokens_in_light_dark_narrow_and_normal_views() {
    let original = kiln_common::Theme::current();
    let directory = std::env::var_os("KILN_EDITOR_CONFIG_CAPTURE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("kiln-editor-config-review"));
    std::fs::create_dir_all(&directory).unwrap();

    for theme in ["kiln-dark", "kiln-light"] {
        kiln_common::Theme::set_current(theme);
        for width in [380.0, 900.0] {
            for (name, path, language, text, key_line, key, value_line, value) in [
                (
                    "dotenv",
                    ".env.local",
                    "DotENV",
                    concat!(
                        "# Local development\n",
                        "export PORT=\"3000\"\n",
                        "API_URL=https://example.test\n",
                        "GREETING=\"hello \\\"Kiln\\\"\"\n",
                        "MULTILINE=\"first line\n",
                        "second # still a value\"\n",
                        "MODE=development # trailing\n",
                        "SPACED   =  \"ready\"\n",
                    ),
                    "export PORT=\"3000\"",
                    "PORT",
                    "export PORT=\"3000\"",
                    "3000",
                ),
                (
                    "ssh",
                    "/fixture/home/.ssh/config",
                    "SSH Config",
                    concat!(
                        "# Local development\n",
                        "Host dev-* !production\n",
                        "  HostName example.test\n",
                        "  User developer\n",
                        "  Port 2222\n",
                        "  IdentityFile \"${HOME}/key-%h\"\n",
                        "Match host *.test user developer\n",
                        "  ForwardAgent yes\n",
                        "Include ~/.ssh/config.d/*.conf\n",
                    ),
                    "  HostName example.test",
                    "HostName",
                    "  HostName example.test",
                    "example.test",
                ),
                (
                    "sshd",
                    "/fixture/etc/ssh/sshd_config.d/ports.conf",
                    "SSHD Config",
                    concat!(
                        "# Local development\n",
                        "Port 2222\n",
                        "PermitRootLogin no\n",
                        "Match User deploy\n",
                        "  ForceCommand \"echo # literal\"\n",
                        "  AuthorizedKeysFile .ssh/keys-%u\n",
                        "Include /etc/ssh/sshd_config.d/*\n",
                    ),
                    "Port 2222",
                    "Port",
                    "Port 2222",
                    "2222",
                ),
            ] {
                let mut h = Harness::builder()
                    .with_size(egui::vec2(width, 420.0))
                    .wgpu()
                    .build_ui_state(
                        |ui, editor: &mut Editor| {
                            if common::fonts_ready(ui.ctx()) {
                                editor.ui(ui);
                            }
                        },
                        Editor::from_text(path, text),
                    );
                common::apply_theme(&h.ctx);
                // Font installation and lazy syntax highlighting may need more
                // than one frame; bound the wait to a small, explicit frame count.
                for _ in 0..20 {
                    h.step();
                    if h.state().highlighted_lines() >= text.lines().count() {
                        break;
                    }
                }
                assert!(h.state().highlighted_lines() >= text.lines().count());
                assert_eq!(h.state().status().language, language);
                let keyword = color(&h, key_line, key);
                let painted_value = color(&h, value_line, value);
                let comment = color(&h, "# Local development", "Local");
                assert_ne!(
                    keyword, painted_value,
                    "{name} {theme} {width}: key/value paint colors"
                );
                assert_ne!(
                    keyword, comment,
                    "{name} {theme} {width}: key/comment paint colors"
                );
                assert_ne!(
                    painted_value, comment,
                    "{name} {theme} {width}: value/comment paint colors"
                );
                if name == "dotenv" {
                    assert_ne!(color(&h, "export PORT=\"3000\"", "="), keyword);
                    assert_ne!(color(&h, "export PORT=\"3000\"", "="), painted_value);
                    assert_ne!(
                        color(&h, "API_URL=https://example.test", "API_URL"),
                        color(&h, "API_URL=https://example.test", "https")
                    );
                    assert_eq!(color(&h, "second # still a value\"", "#"), painted_value);
                    assert_ne!(
                        color(&h, "SPACED   =  \"ready\"", "SPACED"),
                        color(&h, "SPACED   =  \"ready\"", "ready")
                    );
                }
                let viewport = h.ctx.content_rect();
                let (_, gutter, document) = h.state().layout_info();
                assert!(viewport.contains_rect(gutter));
                assert!(viewport.contains_rect(document));
                h.render()
                    .unwrap()
                    .save(directory.join(format!("{name}-{theme}-{width:.0}.png")))
                    .unwrap();

                // Explicit Plain Text must also reach the actual painted text,
                // and returning to Auto must invalidate the previous layout.
                h.state_mut()
                    .set_language_override(Some("Plain Text".into()));
                h.run_steps(3);
                assert_eq!(h.state().status().language, "Plain Text");
                assert_eq!(color(&h, key_line, key), color(&h, value_line, value));
                h.state_mut().set_language_override(None);
                h.run_steps(3);
                assert_eq!(h.state().status().language, language);
                assert_ne!(color(&h, key_line, key), color(&h, value_line, value));
            }
        }
    }
    kiln_common::Theme::set_current(original.name);
    eprintln!("Synthetic editor review captures: {}", directory.display());
}
