use egui::Color32;
use kiln_common::{Theme, theme};
use kiln_editor::{highlight::Highlighter, syntax};
use std::path::Path;

#[test]
fn custom_palette_reload_invalidates_existing_syntax_spans_in_dark_and_light_modes() {
    let root = tempfile::tempdir().unwrap();
    struct Restore(std::path::PathBuf);
    impl Drop for Restore {
        fn drop(&mut self) {
            theme::load_custom_themes(&self.0);
            Theme::set_current("kiln-dark");
        }
    }
    let _restore = Restore(root.path().join("missing"));
    let path = root.path().join("custom.json");
    let lines = ["let value = \"user text\";"];
    let mut highlighter =
        Highlighter::new(syntax::detect(Path::new("example.rs"), ""), lines.len());
    for base in ["kiln-dark", "kiln-light"] {
        for (hex, expected) in [
            ("#12AB34", Color32::from_rgb(0x12, 0xab, 0x34)),
            ("#AB34CD", Color32::from_rgb(0xab, 0x34, 0xcd)),
        ] {
            std::fs::write(&path, serde_json::json!({"name":"custom", "label":"테마", "base":base, "colors":{"green":hex}}).to_string()).unwrap();
            assert!(theme::load_custom_themes(root.path()).is_empty());
            Theme::set_current("custom:custom");
            assert!(highlighter.ensure(&lines, lines.len(), None));
            assert_eq!(
                highlighter.last_work, 1,
                "palette edits must invalidate previously completed lines"
            );
            let offset = lines[0].find("user text").unwrap() as u32;
            assert_eq!(
                highlighter
                    .line_spans(0)
                    .unwrap()
                    .iter()
                    .find(|span| span.start <= offset && offset < span.end)
                    .unwrap()
                    .color,
                expected
            );
            kiln_common::i18n::with_language(kiln_common::i18n::Language::English, || {
                let choice = Theme::choices()
                    .into_iter()
                    .find(|choice| choice.name == "custom:custom")
                    .unwrap();
                assert_eq!(
                    choice.display_label(),
                    "테마",
                    "user labels must never be translated"
                );
            });
        }
    }
}
