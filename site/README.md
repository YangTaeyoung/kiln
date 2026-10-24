# Kiln product site

Static site for Kiln: a landing page with a feature atlas and an 11-page user guide in
English, Korean, Japanese and Simplified Chinese. No build step for the landing page; the
guide is generated from Markdown.

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

`.github/workflows/pages.yml` publishes `index.html`, `assets/` and `docs/` to GitHub Pages
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
