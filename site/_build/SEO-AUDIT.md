# Kiln search readiness: 2026-10-27

## Current public HTTP evidence

Audited `https://kiln.melona.cloud/` using curl with certificate verification enabled
on 2026-10-26. Web-fetch access was unavailable; that did not prevent the HTTP audit.
The snapshot is the currently published site, **before** this social-card change:

- All 48 distinct canonical URLs from the public sitemap returned HTTPS 200 without
  redirects. Titles/descriptions, one H1, unique IDs, self canonicals, reciprocal
  language clusters and crawlable language links passed across all four languages.
- All 52 referenced public assets returned 200. `robots.txt` returned 200 as text/plain;
  `sitemap.xml` returned 200 as application/xml. HTTP `/` redirected to HTTPS `/`.
- An intentionally nonexistent path returned HTTP 404 with a noindex error document.
  `/404.html` itself returned 200, as expected for the hosted error document; it is not
  in the sitemap. `/docs/index.html` remains available with `/docs/` as canonical.
- Public robots rules allow search, AI search/training/user fetches, and unlisted
  crawlers. The artifact audit checks every canonical URL and referenced asset against
  those rules, including the social image and social preview crawlers.
  Separate homepage HTTP requests using Googlebot, bingbot, OAI-SearchBot,
  Claude-SearchBot, PerplexityBot, Twitterbot and facebookexternalhit user-agent
  strings all returned 200. These probes do not prove that those actual bots visited.
- The public pages still use localized hero-poster WebP captures as social images.
  All 48 lack image dimensions, MIME, secure_url, Twitter image alt and OG alternate
  locales. Their OG alt is a feature-section heading rather than an image description.
  The four landing SoftwareApplication nodes lack softwareVersion. These are the
  concrete metadata gaps addressed here, not evidence of failed Google indexing.

No remaining broken canonical/locale/link/anchor, asset-status or unintended noindex
failure was found in the downloaded 48-page snapshot. Asset availability was checked
separately over HTTP; link/fragment checks used the downloaded HTML.

## Changes prepared for publication

- A single branded social image on **all 48 pages**, including every guide:
  `/assets/kiln-social-card-v2.png`. The original PNG was fully decoded locally:
  **1733 × 907 pixels, image/png, 2,103,619 bytes**. Its aspect ratio and original
  fidelity are preserved. The build reads its dimensions from the PNG IHDR; the gate
  compares rendered dimensions and MIME with the actual PNG file, not an assumed size.
- OG title/description remain localized and page-specific. OG url is the HTTPS
  canonical; image and secure_url are absolute HTTPS URLs. OG locale uses en_US,
  ko_KR, ja_JP or zh_CN, with the other three as alternates. OG/Twitter image alt
  describes the icon, English headline and agent-terminal product workspace in each
  page's language. The image's English lettering is not described as localized.
- WebPage.primaryImageOfPage is an ImageObject with the same URL, dimensions, MIME
  and localized caption. SoftwareApplication.image may use the social composition,
  while **screenshot remains the real localized hero-poster-{lang}.webp capture**.
- SoftwareApplication.softwareVersion follows `_build/source.json.kiln_version`.
  The release owner updates that manifest; there is no hardcoded release version.
  OS requirements remain macOS 11 or later / Windows 11, matching the installation
  guide. The real free offer remains price 0, priceCurrency USD. No ratings or reviews
  are invented. This is descriptive Schema.org markup, **not a claim of eligibility
  for Google's software-app rich result**, which also requires genuine rating/review data.
- Google, Naver and Bing ownership meta tags are preserved on localized landing pages.
  No crawler policy, Search Console property, account, DNS or publication setting changes.

## Verification

```sh
python3 site/_build/build-docs.py  # guide owner maintains the generated source pages
python3 site/_build/build-site.py
python3 site/_build/check-seo.py
python3 site/_build/test-seo.py
```

The first new artifact test failed on all 48 pages against the old social-image URL
before the generator change. The corrected artifact passes. Regression tests mutate
rendered HTML, sitemap, robots and 404 fixtures, then run the real publishing checker.
They reject missing/duplicate tags, wrong MIME/dimensions/URL/alt/locale, wrong hreflang,
broken fragments, fabricated rating data, stale version, marketing-card screenshot,
false OS/price, malformed JSON-LD, bot noindex, blocked social assets, duplicate sitemap
URLs and an indexable error page. They do not grep implementation source. The tests use
isolated temporary directories and never edit the real published artifact.

The checker also rejects extra unsitemapped HTML, missing build assets and leaked build
sources. It parses all JSON-LD and validates application/page/breadcrumb meaning rather
than treating valid JSON alone as sufficient. It is deterministic and runs without HTTP.
Full image decoding was an additional local check; PNG header/size validation runs in CI
without introducing an image-library dependency.

Review follow-up on 2026-10-27 added 21 artifact mutations before changing the checker.
The RED run reproduced 13 false-green results: hreflang links with missing, stylesheet
or mixed rel values, an invalid primaryImageOfPage.contentUrl, and unchecked
SoftwareApplication.image fields or shapes. The gate now requires exactly the five
expected rel=alternate links in head. Both WebPage.primaryImageOfPage and
SoftwareApplication.image must be ImageObject objects with the delivered image's
absolute url/contentUrl, actual dimensions, image/png MIME and localized caption.
The new cases independently corrupt each of those fields or replace the object with
null/a string. The existing generated pages already satisfy these rules; the generator,
content, image and published site were not changed by this follow-up.

Final local run: 48-page checker passed; six artifact tests passed in 33.857 seconds,
including 40 deliberate regression cases. `git diff --check` passed. The title-only
source-landing edit changes no visual styling. Design-hook reports about existing CSS
font sizes, colors and radii are outside this metadata diff; they were reviewed as
unchanged context, with no design waivers or ignore configuration added.

## Search intent and content scope

The four localized homepages describe the terminal-first agent IDE. The terminal,
agents, installation and workspace guides own their relevant workflow questions.
Existing workflow explanations, limits, FAQ and contextual guide links remain intact.
No keyword-stuffed pages, invented testimonials or unsupported feature claims were added.

## Verified scope and outstanding boundaries

The parent performed a fresh, read-only authenticated GSC MCP `sites.list` lookup:
19 visible entries, cache miss, no site_url containing kiln or melona. This establishes
only the connected MCP account's current property visibility. It does **not** establish
that a property is absent in the user's browser account, that ownership is invalid, or
that the site is unindexed. Existing public ownership proofs are retained.

Current URL Inspection, indexed-page counts, query performance, authenticated sitemap
submission and Naver indexing/submission status were not verified in this audit. No
browser authentication, property creation or external submission was performed.

The new social card and metadata still need deployment by the parent, followed by a
fresh HTTP check of all 48 pages, the PNG's status/Content-Type/dimensions and robots.
A local pass is not deployment evidence. Social platforms may retain cached previews;
no platform-specific cache refresh or preview rendering is claimed. Rankings, rich-result
appearance and field Core Web Vitals are not proved by these technical checks.

## Official references

- Canonical URLs: https://developers.google.com/search/docs/crawling-indexing/consolidate-duplicate-urls
- Reciprocal localized versions: https://developers.google.com/search/docs/specialty/international/localized-versions
- Canonical sitemap scope: https://developers.google.com/search/docs/crawling-indexing/sitemaps/build-sitemap
- Robots scope and limits: https://developers.google.com/search/docs/crawling-indexing/robots/intro
- App price, OS and rich-result requirements: https://developers.google.com/search/docs/appearance/structured-data/software-app
- Software version and screenshot semantics: https://schema.org/SoftwareApplication
- Image bytes URL, dimensions, MIME and caption: https://schema.org/ImageObject
- OG image properties and locale arrays: https://ogp.me/

Implementation: `build-site.py`, `check-seo.py`, `test-seo.py`, `seo-content.json`,
`../index.html`. Image and prompt placement belong to the parent; guide copy and generated
`site/docs/` belong to the documentation agent. Release version/source pin remain with
the parent. No app, daemon, signing, release artifact or installed application changes.

## Search preview policy

All canonical pages explicitly allow indexing, following links and large image
previews. Text and video preview limits are left unrestricted. The missing-page
document retains its separate noindex policy. These settings permit previews;
the search engine still decides how a result appears. See the official
[robots meta documentation](https://developers.google.com/search/docs/crawling-indexing/robots-meta-tag).
