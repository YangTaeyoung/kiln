#!/usr/bin/env python3
"""Reports user-facing changes the product site has not reflected yet, and checks that every
translated docs page keeps the English page's structure.

Usage: python3 site/_build/check-site.py [--ref origin/main]
Exit status 1 when a translated page is out of structure; listed changes are informational."""
import argparse
import json
import pathlib
import re
import subprocess
import sys

BUILD = pathlib.Path(__file__).resolve().parent
REPO = BUILD.parent.parent
SRC = BUILD / "docs-src"
LANGS = ["ko", "ja", "zh-CN"]
WATCHED = ["README.md", "docs", "crates/kiln-common/locales/messages.json"]
IGNORED = ("docs/maintainers/", "docs/plans/", "docs/superpowers/", "docs/prototypes/", "docs/reviews/")


def git(*args):
    return subprocess.run(["git", "-C", str(REPO), *args], capture_output=True, text=True, check=True).stdout


def structure(text):
    ids = re.findall(r"\{#([\w-]+)\}", text)
    links = re.findall(r"\]\(([^)]+)\)", text)
    code = re.findall(r"```.*?```", text, re.S)
    return ids, links, code


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ref", default="HEAD")
    args = ap.parse_args()
    src = json.loads((BUILD / "source.json").read_text())
    base = src["source_commit"]
    changed = [p for p in git("diff", "--name-only", f"{base}..{args.ref}", "--", *WATCHED).split() if not p.startswith(IGNORED)]
    print(f"Site reflects Kiln {src['kiln_version']} ({base}).")
    if changed:
        print(f"User-facing files changed since then ({len(changed)}):")
        for p in changed:
            print("  " + p)
        print("Reflect these in site/ (docs-src, landing tiles, screenshots), then update source.json.")
    else:
        print("No user-facing changes since the recorded commit.")

    problems = []
    for en in sorted((SRC / "en").glob("*.md")):
        ref = structure(en.read_text())
        for lang in LANGS:
            tr = SRC / lang / en.name
            if not tr.exists():
                problems.append(f"{lang}/{en.name}: missing")
                continue
            got = structure(tr.read_text())
            for label, a, b in zip(("heading ids", "link targets", "code blocks"), ref, got):
                if a != b:
                    problems.append(f"{lang}/{en.name}: {label} differ from en")
    for p in problems:
        print("ERROR " + p)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
