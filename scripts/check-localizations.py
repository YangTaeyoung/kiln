#!/usr/bin/env python3
"""Validate UI catalogs, format arguments and literal translation call sites.

Run from any directory. This never translates arbitrary runtime/user text.
Rust's compiler additionally validates every trf! branch and its argument types.
"""
import collections
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
CATALOG = ROOT / "crates/kiln-common/locales/messages.json"
LOCALES = ("en", "ja", "zh-CN")
FIELD = re.compile(r"(?<!\{)\{([^{}]*)\}(?!\})")
CALL = re.compile(r"(?:\btr\s*\(|\btrf\s*!\s*\()\s*$")
# Direct text sinks catch missing translation calls, not only missing catalog rows.
# Product names, commands, paths and key chords are intentionally invariant.
TEXT_SINK = re.compile(r"(?:\.\s*(?:label|button|heading|hint_text|on_hover_text|collapsing|checkbox|radio_value|selectable_label|selectable_value|menu_button)|(?:RichText|Label|Button|CollapsingHeader)::new|\b(?:icon_button|tool_button|setting_row|setting_toggle|group|pill))\s*$")
INVARIANT_UI = {
    # Product names, SQL terms, key chords and literal executable examples.
    "Actions", "GitHub", "Git", "Kiln", "URL", "SQLite", "NOT NULL",
    "⌘ Enter", "⌘Enter", "Ctrl+Enter", "Ctrl+C", "Alt+Z", "Shift+F12",
    "Esc", "Enter", "Del", "/bin/zsh", "postgres://user:pass@host:5432/db",
    "gh pr checkout", "git fetch --all --prune", "git pull", "git push",
}


def untranslated_ui_literals(source):
    """Yield app-owned literals rendered without a translation call.

    Tests may deliberately display untranslated user data. Their module is not
    a production text surface and is excluded from this guard.
    """
    source = production_source(source)
    code = lexical_code(source)
    for start, value in strings(source):
        if at_ui_text_sink(code, start) and value not in INVARIANT_UI:
            if re.search("[가-힣]", value) or re.search(r"[A-Za-z]{3}", value):
                yield start, value


def at_ui_text_sink(code, start):
    """Find the enclosing call even after nested earlier arguments such as Some()."""
    stack = []
    pairs = {"(": ")", "[": "]", "{": "}"}
    for pos in range(start - 1, -1, -1):
        char = code[pos]
        if char in ")]}":
            stack.append(char)
        elif char in "([{":
            if stack:
                if pairs[char] != stack.pop():
                    return False
            elif char == "(":
                before = code[max(0, pos - 100):pos]
                return bool(TEXT_SINK.search(before)) and not re.search(r"\bkeymap\s*\.\s*label\s*$", before)
            else:
                return False  # A block/array literal is not a direct text argument.
        elif char == ";" and not stack:
            return False
    return False


def lexical_code(source):
    code = list(source)
    for start, end, _ in lexical_regions(source):
        code[start:end] = blank_region(source[start:end])
    return "".join(code)


def lexical_regions(source):
    """Non-code Rust token spans and decoded strings; respect nested comments."""
    pattern = re.compile(r"//[^\n]*|/\*|(?:br|r)(?P<h>\#*)\"|(?:b)?\"|'([^'\\]|\\.)'")
    pos = 0
    while match := pattern.search(source, pos):
        start, token, pos = match.start(), match.group(), match.end()
        if token.startswith("//") or token.startswith("'"):
            yield start, pos, None
            continue
        if token == "/*":
            depth = 1
            while depth and pos < len(source):
                nested = re.search(r"/\*|\*/", source[pos:])
                if not nested:
                    pos = len(source)
                    break
                depth += 1 if nested[0] == "/*" else -1
                pos += nested.end()
            yield start, pos, None
            continue
        if token.startswith(("r", "br")):
            ending = '"' + match.group("h")
            end = source.find(ending, pos)
            if end < 0:
                raise ValueError("unclosed raw string")
            value, pos = source[pos:end], end + len(ending)
        else:
            end = pos
            while end < len(source):
                if source[end] == "\\":
                    end += 2
                elif source[end] == '"':
                    break
                else:
                    end += 1
            raw, pos = source[start:end + 1], end + 1
            if raw.startswith("b"):
                yield start, pos, None
                continue
            # UI keys use JSON-compatible escapes. Rust-specific escape strings
            # are protocol/code data, not localization call sites.
            try:
                value = json.loads(raw)
            except ValueError:
                if CALL.search(source[max(0, start - 100):start]):
                    raise ValueError(f"unsupported escape in localization key: {raw}")
                yield start, pos, None
                continue
        yield start, pos, value


def strings(source):
    for start, _, value in lexical_regions(source):
        if value is not None:
            yield start, value


def blank_region(text):
    # Keep offsets and line numbers identical to the original source.
    return "".join("\n" if char == "\n" else " " for char in text)


def test_only_cfg(expression):
    expression = expression.strip()
    if expression == "test":
        return True
    predicate = re.fullmatch(r"(all|any)\s*\((.*)\)", expression, re.S)
    if not predicate:
        return False  # Retain unknown/negated expressions rather than hiding UI.
    arguments, start, depth = [], 0, 0
    for index, char in enumerate(predicate[2]):
        depth += (char == "(") - (char == ")")
        if char == "," and depth == 0:
            arguments.append(predicate[2][start:index])
            start = index + 1
    arguments.append(predicate[2][start:])
    arguments = [argument for argument in arguments if argument.strip()]
    required = [test_only_cfg(argument) for argument in arguments]
    return any(required) if predicate[1] == "all" else bool(required) and all(required)


def production_source(source):
    """Mask only cfg(test) items; production items after a test module stay visible.

    Brace matching uses a lexical mask, so strings, raw strings, character
    literals, line comments and nested block comments cannot end an item.
    """
    code = lexical_code(source)
    output = list(source)
    attribute = re.compile(r"#\s*\[\s*cfg\s*\((.*?)\)\s*\]", re.S)
    for match in attribute.finditer(code):
        if not test_only_cfg(match[1]):
            continue
        # A test-only constant/use may terminate with a semicolon; block items
        # terminate at their matching top-level closing brace.
        after = code[match.end():]
        kind = re.search(r"\b(mod|fn|impl|struct|enum|trait|const|static|use)\b", after)
        semicolon_item = kind and kind[1] in {"const", "static", "use"}
        stack = []
        end = None
        for pos in range(match.end(), len(source)):
            char = code[pos]
            if char in "([{":
                stack.append(char)
            elif char in ")]}":
                if stack:
                    opening = stack.pop()
                    if {"(": ")", "[": "]", "{": "}"}[opening] != char:
                        raise ValueError("unbalanced cfg(test) item")
                    if char == "}" and not stack and not semicolon_item:
                        end = pos + 1
                        break
            elif char == ";" and not stack:
                end = pos + 1
                break
        if end is None:
            raise ValueError("unclosed cfg(test) item")
        output[match.start():end] = blank_region(source[match.start():end])
    return "".join(output)


def main():
    catalog = json.loads(CATALOG.read_text())
    errors, used = [], set()
    for source, translations in catalog.items():
        fields = collections.Counter(FIELD.findall(source))
        for locale in LOCALES:
            value = translations.get(locale)
            if not isinstance(value, str) or not value.strip():
                errors.append(f"{locale}: missing translation for {source!r}")
            elif re.search("[가-힣]", value):
                errors.append(f"{locale}: Korean text remains in {source!r}")
            elif fields != collections.Counter(FIELD.findall(value)):
                errors.append(f"{locale}: changed format arguments in {source!r}")
    for path in ROOT.glob("crates/*/src/**/*.rs"):
        source = path.read_text()
        production = production_source(source)
        for start, value in strings(production):
            if CALL.search(source[max(0, start - 100):start]):
                used.add(value)
                if value not in catalog:
                    line = source.count("\n", 0, start) + 1
                    errors.append(f"{path.relative_to(ROOT)}:{line}: missing message {value!r}")
        for start, value in untranslated_ui_literals(source):
            line = source.count("\n", 0, start) + 1
            errors.append(f"{path.relative_to(ROOT)}:{line}: untranslated UI literal {value!r}")
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print(f"Validated {len(catalog)} messages in 4 languages; {len(used)} literal UI keys.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
