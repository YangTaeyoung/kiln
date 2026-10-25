# Kiln product site

Static site for Kiln: a product landing page and an 11-page user guide in
English, Korean, Japanese and Simplified Chinese. The publishing build renders each
language to its own URL; the guide is generated from Markdown.

| Path | Contents |
| --- | --- |
| `index.html` | Landing page. Tile copy lives in `assets/site-i18n.js`. |
| `docs/*.html` | Generated guide pages. Do not edit by hand. |
| `assets/` | Fonts, glaze renderer, styles, screenshots and hero recordings. |
| `_build/docs-src/<lang>/*.md` | Guide sources. English is the source of truth. |
| `_build/build-docs.py` | Builds `docs/` from the sources. |
| `_build/check-site.py` | Lists user-facing changes since `_build/source.json` and checks translations. |
| `_build/capture/` | Scripts that capture screenshots and recordings from a real, isolated Kiln. |

## When a user-facing feature changes

Update the site in the same change as the feature.

1. **Guide.** Edit the English page in `_build/docs-src/en/`, then the same page in `ko`, `ja`
   and `zh-CN`. Keep every `{#id}`, link and code block identical across languages, and name
   UI elements exactly as `crates/kiln-common/locales/messages.json` does in each language.
2. **Landing.** If the feature deserves a place in the atlas, add or edit its tile in
   `index.html` and its strings for all four languages in `assets/site-i18n.js`. Keep titles to
   a few words and descriptions to one short line.
3. **Screens.** When the change is visible in a captured screen, recapture (below). Never use
   real workspaces, accounts or terminal output.
4. **Build and check.**

   ```sh
   python3 site/_build/build-docs.py
   python3 site/_build/check-site.py --ref origin/main
   ```

5. **Record.** When the site reflects a release, set `kiln_version` and `source_commit` in
   `_build/source.json`.

## Publishing

`.github/workflows/pages.yml` builds and publishes `public/` to GitHub Pages
whenever `site/` changes on `main`. The workflow fails if `docs/` was not rebuilt after a
source change or if a translation drifts from the English structure.

## Capturing screens

Captures run a second, isolated Kiln (its own home, config, daemon socket and demo
repositories) and photograph only its window. Screen Recording permission is required for
the terminal that runs the scripts. The running Kiln and its sessions are not touched.

```sh
KILN_EXE=/path/to/Kiln.app/Contents/MacOS/kiln site/_build/capture/setup-demo.sh
site/_build/capture/capture.sh
```

Terminal content comes from scene files in `_build/capture/scenes/`; everything else on screen
is the real app. Edit `compose.py` to change the window layouts.

## Search / localized publishing

The published artifact is now generated (the source landing remains editable HTML):

```sh
pip install -r site/_build/requirements.txt
python3 site/_build/check-site.py
python3 site/_build/build-docs.py
python3 site/_build/build-site.py
python3 site/_build/check-seo.py
python3 -m http.server 8767 --directory public
```

`build-site.py` emits 48 indexable documents: English `/`, Korean `/ko/`, Japanese `/ja/`,
Simplified Chinese `/zh-cn/`, each with 11 guide pages. The published HTML contains the
selected language before JavaScript runs. Language controls navigate to reciprocal URLs;
stored preferences do not override a URL's language. Existing `/docs/index.html` remains
available with `/docs/` as canonical. `public/` is ignored and never edited by hand.

`_build/seo-content.json` owns localized search titles/descriptions and the workflow/FAQ
section. Keep its headline/subtitle aligned with `assets/site-i18n.js`. Guide frontmatter
owns guide search titles/descriptions. Generated output includes canonical, hreflang,
Open Graph/Twitter, JSON-LD, robots.txt, sitemap.xml and a noindex 404 page.
CI checks all canonical URLs, reciprocal alternates, internal links/fragments, assets,
unique headings/IDs/titles and JSON-LD syntax before deployment. See `_build/SEO-AUDIT.md`.

## Platform-aware downloads

`assets/downloads.js` recommends macOS or Windows across the landing page and guide
headers, using available browser platform hints with a legacy fallback. Both desktop
platforms remain linked. Mobile devices (including iPads using a desktop Mac identity),
Linux and unknown platforms get a neutral choice. Download links continue to open the
latest Releases page; the browser does not guess CPU architecture or start a download.

Localized labels live in `assets/site-i18n.js` and are included in every published
page by `build-site.py`. The controls work independently of the animation libraries.
Run `node --test site/_build/downloads.test.cjs` for platform detection fixtures.

## Crawler access and Search Console

`_build/robots.txt` is the published robots policy. Its wildcard permits every crawler,
including search engines and unlisted AI systems; explicit OpenAI, Anthropic, Perplexity
and Google-Extended entries make AI search/training/user-fetch permission clear. The
public site is served directly by GitHub Pages through a DNS-only CNAME, so Cloudflare
proxy bot controls do not apply to its requests. Recheck that before any DNS/proxy change.

The Search Console property is scoped to `https://kiln.melona.cloud/`. The public
`google-site-verification` meta tag in `index.html` proves ownership; preserve it across
rebuilds. This is an intentionally public ownership proof, not a private API credential.
Submit `https://kiln.melona.cloud/sitemap.xml` in that property's Sitemaps report.
A successful submission confirms discovery, not that all 48 URLs are indexed.

`check-seo.py` checks the ownership proof on every localized landing, sitemap discovery,
and crawl permission for search bots, named AI bots and an unlisted crawler. Robots
permissions do not guarantee a crawler will visit, train on or cite the site.

Official bot references:
- https://developers.openai.com/api/docs/bots
- https://support.claude.com/en/articles/8896518-does-anthropic-crawl-data-from-the-web-and-how-can-site-owners-block-the-crawler
- https://docs.perplexity.ai/docs/resources/perplexity-crawlers
- https://developers.google.com/crawling/docs/crawlers-fetchers/google-common-crawlers#google-extended
