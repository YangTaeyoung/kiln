#!/usr/bin/env python3
"""Regression checks for production localization coverage; no extra dependencies."""
import importlib.util
import pathlib
import unittest

spec = importlib.util.spec_from_file_location(
    "localizations", pathlib.Path(__file__).with_name("check-localizations.py")
)
check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)


class CoverageTests(unittest.TestCase):
    def literals(self, source):
        return [value for _, value in check.untranslated_ui_literals(source)]

    def test_production_after_test_module_is_checked(self):
        source = '''#[cfg(test)] mod tests { ui.label("Deliberate user fixture"); }
impl App { fn ui() { ui.label("Untranslated after tests"); } }'''
        self.assertEqual(self.literals(source), ["Untranslated after tests"])
        masked = check.production_source(source)
        self.assertEqual(len(masked), len(source))
        self.assertEqual(masked.count("\n"), source.count("\n"))

    def test_braces_in_strings_and_comments_cannot_end_test_module(self):
        source = '''#[cfg(test)] mod tests {
    let a = "} escaped \\\" {";
    let b = r###"} #[cfg(test)] mod fake {"###;
    let c = '}';
    // } fake closing brace
    /* } /* nested { comment */ more } */
    ui.label("Deliberate fixture");
}
impl App { fn ui() { ui.button("Production after lexical noise"); } }'''
        self.assertEqual(self.literals(source), ["Production after lexical noise"])

    def test_multiple_inline_test_items_preserve_production_between_them(self):
        source = '''#[cfg(test)] fn fixture() { ui.label("Test only"); }
fn production() { ui.label("첫 번째 누락"); }
#[cfg(test)] const FIXTURE: &str = "{ unmatched brace";
fn other() { ui.label("Second missing label"); }
#[cfg(test)] mod end { ui.label("Test only"); }'''
        self.assertEqual(self.literals(source), ["첫 번째 누락", "Second missing label"])

    def test_compound_test_guards_exclude_only_test_required_items(self):
        for predicate in ["all(test, unix)", "all(unix, test)", "any(test, all(test, unix))"]:
            with self.subTest(predicate=predicate):
                source = f'#[cfg({predicate})] mod tests {{ ui.label("Test only"); }} ui.label("Production");'
                self.assertEqual(self.literals(source), ["Production"])
        for predicate in ["any(test, unix)", "not(test)", 'feature = "updater-test"']:
            with self.subTest(predicate=predicate):
                self.assertEqual(self.literals(f'#[cfg({predicate})] fn ui() {{ ui.label("Production"); }}'), ["Production"])

    def test_comment_and_raw_string_cfg_text_are_not_attributes(self):
        source = '''// #[cfg(test)] mod fake {
let data = r#"#[cfg(test)] mod fake { }"#;
ui.label("Still production");'''
        self.assertEqual(self.literals(source), ["Still production"])

    def test_unclosed_test_item_fails_instead_of_hiding_production(self):
        with self.assertRaisesRegex(ValueError, "unclosed"):
            check.production_source('#[cfg(test)] mod tests { ui.label("} string");')

    def test_catalog_keys_are_excluded_by_test_origin_not_magic_content(self):
        sentinel = "user/project: not an application message"
        source = f'#[cfg(test)] mod tests {{ tr("{sentinel}"); }} fn ui() {{ tr("{sentinel}"); }}'
        strings = list(check.strings(check.production_source(source)))
        self.assertEqual([value for _, value in strings], [sentinel])

    def test_ui_text_is_checked_while_translation_calls_and_protocols_are_preserved(self):
        self.assertEqual(self.literals('ui.label(RichText::new("Missing translation"));'), ["Missing translation"])
        self.assertEqual(self.literals('edit.hint_text("입력 안내 누락");'), ["입력 안내 누락"])
        self.assertEqual(self.literals('ui.checkbox(&mut setting, "체크박스 누락");'), ["체크박스 누락"])
        self.assertEqual(self.literals('icon_button(ui, Icon::Close, 24.0, false, "Missing icon tooltip");'), ["Missing icon tooltip"])
        self.assertEqual(self.literals('tool_button(ui, Some(Icon::Close), "Nested parameter label");'), ["Nested parameter label"])
        for source in ['ui.label(tr("번역 호출"));', 'ui.label(trf!("응답 {value}"));', 'ui.label("GitHub");', 'button.on_hover_text("git pull");', 'self.keymap.label("sidebar", "⌘B");', 'let user_text = "Unchanged user data";']:
            with self.subTest(source=source):
                self.assertEqual(self.literals(source), [])


if __name__ == "__main__":
    unittest.main()
