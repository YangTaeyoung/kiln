import html
import json
import pathlib
import re
import sys

import markdown

SRC = pathlib.Path(__file__).resolve().parent / "docs-src"
OUT = pathlib.Path(__file__).resolve().parent.parent / "docs"
LANGS = ["en", "ko", "ja", "zh-CN"]

GROUPS = [
    ("g_start", ["index", "install"]),
    ("g_use", ["workspaces", "terminal", "agents", "accounts", "conversations"]),
    ("g_tools", ["git", "editor", "databases", "remote"]),
]
ORDER = [s for _, slugs in GROUPS for s in slugs]

SHOTS = {"index": "agents", "workspaces": "agents", "agents": "agents", "terminal": "files", "git": "git", "databases": "db", "editor": "editor", "remote": "remote", "install": "settings"}
GLAZE = ["tenmoku", "celadon", "shino", "kaki"]
GROUP_OF = {s: g for g, slugs in GROUPS for s in slugs}

UI = {
    "en": {"g_start": "Get started", "g_use": "Using Kiln", "g_tools": "Tools", "docs": "Documentation", "toc": "On this page", "prev": "Previous", "next": "Next",
           "features": "Features", "download": "Download", "menu": "Pages", "shot": "Kiln running a demo workspace.", "home": "Home", "issue": "Report an issue", "releases": "Releases"},
    "ko": {"g_start": "시작하기", "g_use": "Kiln 사용하기", "g_tools": "도구", "docs": "문서", "toc": "이 페이지에서", "prev": "이전", "next": "다음",
           "features": "기능", "download": "다운로드", "menu": "목차", "shot": "데모 워크스페이스로 실행한 Kiln", "home": "홈", "issue": "문제 신고", "releases": "릴리스"},
    "ja": {"g_start": "はじめに", "g_use": "Kiln を使う", "g_tools": "ツール", "docs": "ドキュメント", "toc": "このページの内容", "prev": "前へ", "next": "次へ",
           "features": "機能", "download": "ダウンロード", "menu": "ページ一覧", "shot": "デモ用ワークスペースで動かした Kiln", "home": "ホーム", "issue": "問題を報告", "releases": "リリース"},
    "zh-CN": {"g_start": "入门", "g_use": "使用 Kiln", "g_tools": "工具", "docs": "文档", "toc": "本页内容", "prev": "上一页", "next": "下一页",
              "features": "功能", "download": "下载", "menu": "页面目录", "shot": "运行演示工作区的 Kiln", "home": "首页", "issue": "报告问题", "releases": "发布"},
}


def read(lang, slug):
    path = SRC / lang / f"{slug}.md"
    if not path.exists():
        path = SRC / "en" / f"{slug}.md"
    text = path.read_text()
    meta = {}
    m = re.match(r"---\n(.*?)\n---\n", text, re.S)
    if m:
        for line in m.group(1).splitlines():
            k, _, v = line.partition(":")
            meta[k.strip()] = v.strip()
        text = text[m.end():]
    return meta, text


def slugify(value, separator):
    value = re.sub(r"[^\w\s-]", "", value, flags=re.U).strip().lower()
    return re.sub(r"[\s_-]+", separator, value)


def render(text):
    md = markdown.Markdown(extensions=["tables", "fenced_code", "attr_list", "toc", "sane_lists"],
                           extension_configs={"toc": {"slugify": slugify}})
    body = md.convert(text)
    body = re.sub(r"<blockquote>\s*<p><strong>(Note|Tip|Warning|참고|팁|주의|注意|ヒント|警告|提示)[:：]?</strong>[:：]?\s*",
                  lambda m: f'<blockquote class="callout"><p><strong>{m.group(1)}</strong> ', body)
    body = body.replace("<table>", '<div class="table"><table>').replace("</table>", "</table></div>")
    return body


def nav_html(lang, current):
    out = []
    for gkey, slugs in GROUPS:
        out.append(f'<p class="nav-group">{UI[lang][gkey]}</p><ul>')
        for s in slugs:
            title = read(lang, s)[0].get("title", s)
            cls = ' class="on" aria-current="page"' if s == current else ""
            out.append(f'<li><a href="{s}.html"{cls}>{html.escape(title)}</a></li>')
        out.append("</ul>")
    return "".join(out)


def page(slug):
    i = ORDER.index(slug)
    prev_slug = ORDER[i - 1] if i > 0 else None
    next_slug = ORDER[i + 1] if i < len(ORDER) - 1 else None
    articles, navs, titles, descs = [], [], {}, {}
    for lang in LANGS:
        meta, text = read(lang, slug)
        titles[lang] = meta.get("title", slug)
        descs[lang] = meta.get("description", "")
        body = render(text)
        if slug in SHOTS:
            gz = GLAZE[(ORDER.index(slug) + 1) % 4]
            img = (f'<figure class="shot"><div class="plate" data-glaze="{gz}" data-seed="{ORDER.index(slug) * 1.7:.1f}"><img src="../assets/shots/{SHOTS[slug]}-{lang}.webp" alt="" loading="lazy" decoding="async"></div>'
                   f'<figcaption>{UI[lang]["shot"]}</figcaption></figure>')
            body = re.sub(r"(</p>)", r"\1" + img.replace("\\", "\\\\"), body, count=1)
        pn = []
        if prev_slug:
            pn.append(f'<a class="pn prev" href="{prev_slug}.html"><small>{UI[lang]["prev"]}</small>{html.escape(read(lang, prev_slug)[0].get("title", prev_slug))}</a>')
        else:
            pn.append("<span></span>")
        if next_slug:
            pn.append(f'<a class="pn next" href="{next_slug}.html"><small>{UI[lang]["next"]}</small>{html.escape(read(lang, next_slug)[0].get("title", next_slug))}</a>')
        hidden = "" if lang == "en" else " hidden"
        articles.append(
            f'<article class="doc" data-lang="{lang}" lang="{lang}"{hidden}>'
            f'<div class="doc-head"><div class="chip-tile" data-glaze="{GLAZE[ORDER.index(slug) % 4]}" data-seed="{ORDER.index(slug) * 2.3:.1f}"></div><p class="crumb">{UI[lang]["docs"]}<b>{UI[lang][GROUP_OF[slug]]}</b></p></div><h1>{html.escape(titles[lang])}</h1>'
            f'<p class="desc">{html.escape(descs[lang])}</p>{body}'
            f'<nav class="pager" aria-label="{UI[lang]["prev"]} / {UI[lang]["next"]}">{"".join(pn)}</nav></article>')
        navs.append(f'<div class="side-nav" data-lang="{lang}"{hidden}>{nav_html(lang, slug)}</div>')
    ui = json.dumps({l: {"title": titles[l], "toc": UI[l]["toc"], "features": UI[l]["features"], "docs": UI[l]["docs"], "download": UI[l]["download"],
                         "menu": UI[l]["menu"], "issue": UI[l]["issue"], "releases": UI[l]["releases"]} for l in LANGS}, ensure_ascii=False)
    tpl = (pathlib.Path(__file__).resolve().parent / "docs-page.tpl.html").read_text()
    return (tpl.replace("{{TITLE}}", html.escape(titles["en"]))
               .replace("{{DESC}}", html.escape(descs["en"]))
               .replace("{{NAV}}", "".join(navs))
               .replace("{{ARTICLES}}", "".join(articles))
               .replace("{{UI}}", ui))


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    for slug in ORDER:
        (OUT / f"{slug}.html").write_text(page(slug))
    print("built", len(ORDER), "pages")


if __name__ == "__main__":
    main()
