# Kiln search readiness — 2026-10-25

## Search intent and page ownership

| Intent | English target | Korean target | Destination |
| --- | --- | --- | --- |
| Product discovery | AI agent IDE, agent IDE, AI IDE | AI 에이전트 IDE, 에이전트 IDE, AI IDE | `/`, `/ko/` |
| Terminal workflow | AI terminal, persistent terminal sessions | AI 터미널, AI 에이전트 터미널 | `/docs/terminal.html`, `/ko/docs/terminal.html` |
| Agent orchestration | Claude Code and Codex terminal, Antigravity | Claude Code·Codex 함께 실행 | `/docs/agents.html`, `/ko/docs/agents.html` |
| Installation | Kiln macOS, Kiln Windows | Kiln 설치 | localized installation guide |

Japanese and Simplified Chinese have equivalent, fully translated URLs. These targets
reflect actual product capabilities and user intent, not measured search volume or an
observed ranking. No keyword-stuffed doorway pages or invented testimonials were added.

## Implemented coverage

- 48 static pages containing their language's text without JavaScript; one H1 and no
  duplicate article IDs. Existing English documentation URLs are preserved.
- Unique titles and descriptions, self canonical, reciprocal hreflang including x-default,
  localized crawlable navigation and cross-language links. No preference-based redirects.
- XML sitemap and robots.txt; real missing paths use GitHub Pages' 404 response and a
  noindex error document. Source/build folders are excluded from deployment.
- WebSite/WebPage/SoftwareApplication metadata on landing pages; BreadcrumbList on guides.
  No fictional ratings. SoftwareApplication is descriptive Schema.org markup, **not a
  claim of Google app-rich-result eligibility**: genuine review/rating data is absent.
- Open Graph and Twitter large-image cards with real product captures and localized copy.
- Useful workflow explanations, scope limits, FAQ and contextual guide links.
- Deferred landing scripts, poster preload, below-fold lazy screenshots, image dimensions,
  mobile opt-in video playback, and reduced decorative canvas resolution.
- Font subsets generated from published text at every build, preserving original fonts
  and licenses. Language names use system fonts to avoid loading unrelated writing systems.
- Improved text contrast, footer heading hierarchy and long inline-code wrapping on mobile.
- CI blocks broken assets/anchors, malformed JSON-LD, duplicate IDs/titles, missing canonical
  or alternate URLs, unexpected noindex and sitemap omissions.

## Verification and limits

Baseline production Lighthouse 12.8.2 mobile (single lab run): performance 56,
accessibility 94, best practices 100, SEO 100. This shows why Lighthouse SEO alone is not
an indexability/international SEO audit. Lab scores vary; they are not field Core Web
Vitals, proof of indexing, search rankings or Google Rich Results Test results.

Local Chrome checks cover all four languages at 320/390/1024/1440px for both homepage and
agent guide (32 combinations), JavaScript-disabled Korean content and language navigation.
The static audit checks all 48 documents, assets and internal links/fragments.

HTTPS certificate verification and HTTP-to-HTTPS redirection passed before this change.
The available Search Console account lists no Kiln/melona.cloud property. Search Console
URL Inspection, live indexing/performance data and authenticated sitemap submission remain
unverified until the owner connects the matching property. Naver Search Advisor ownership
and submission are also unverified. No indexing API/ping workaround was used.

After ownership is connected: submit `https://kiln.melona.cloud/sitemap.xml` to Google and
Naver, inspect the homepage plus Korean homepage/terminal/agents guides, then track query
impressions, clicks, CTR and position by page/country over time. Use real user questions
to expand guides and earn relevant external references; ranking is not guaranteed.

## Official references

- https://developers.google.com/search/docs/fundamentals/seo-starter-guide
- https://developers.google.com/search/docs/specialty/international/localized-versions
- https://developers.google.com/search/docs/crawling-indexing/sitemaps/build-sitemap
- https://developers.google.com/search/docs/appearance/structured-data/software-app
- https://developers.google.com/search/docs/appearance/page-experience
- https://searchadvisor.naver.com/guide/seo-basic-intro
- https://fonttools.readthedocs.io/en/latest/subset/index.html

Implementation: `build-site.py`, `check-seo.py`, `seo-content.json`, `../assets/seo.css`,
`../../.github/workflows/pages.yml`. Product claims remain grounded in `docs-src/en/` and
the native release/source snapshot recorded in `source.json`.
