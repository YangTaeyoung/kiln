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


def strings(source):
    """Rust string tokens; skip line/block comments and character literals."""
    pattern = re.compile(r"//[^\n]*|/\*|(?:br|r)(?P<h>\#*)\"|(?:b)?\"|'([^'\\]|\\.)'")
    pos = 0
    while match := pattern.search(source, pos):
        start, token, pos = match.start(), match.group(), match.end()
        if token.startswith("//") or token.startswith("'"):
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
                continue
            # UI keys use JSON-compatible escapes. Rust-specific escape strings
            # are protocol/code data, not localization call sites.
            try:
                value = json.loads(raw)
            except ValueError:
                if CALL.search(source[max(0, start - 100):start]):
                    raise ValueError(f"unsupported escape in localization key: {raw}")
                continue
        yield start, value


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
        for start, value in strings(source):
            if CALL.search(source[max(0, start - 100):start]):
                used.add(value)
                if value not in catalog and re.search("[가-힣]", value):
                    line = source.count("\n", 0, start) + 1
                    errors.append(f"{path.relative_to(ROOT)}:{line}: missing message {value!r}")
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print(f"Validated {len(catalog)} messages in 4 languages; {len(used)} literal UI keys.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
