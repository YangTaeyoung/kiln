"""Validate the published artifact, not generator source or JavaScript state."""
import argparse
import json
from pathlib import Path
import struct
from urllib.parse import unquote, urljoin, urlsplit
from urllib.robotparser import RobotFileParser
from xml.etree import ElementTree as ET

from bs4 import BeautifulSoup

SITE = Path(__file__).resolve().parent.parent
BASE = 'https://kiln.melona.cloud'
LANGS = {'en': '/', 'ko': '/ko/', 'ja': '/ja/', 'zh-CN': '/zh-cn/'}
LOCALES = {'en': 'en_US', 'ko': 'ko_KR', 'ja': 'ja_JP', 'zh-CN': 'zh_CN'}
SLUGS = ['index', 'install', 'workspaces', 'terminal', 'agents', 'accounts', 'conversations', 'git', 'editor', 'databases', 'remote']
SOCIAL = BASE + '/assets/kiln-social-card-v2.png'
SEARCH_BOTS = ['Googlebot', 'Googlebot-Image', 'Googlebot-Video', 'bingbot', 'Yeti', 'Daum', 'DuckDuckBot', 'Slurp', 'Baiduspider', 'YandexBot', 'YandexImages', 'Applebot', 'MojeekBot', 'Sogou web spider', 'Sogou inst spider']
BOTS = SEARCH_BOTS + ['Unknown-Search-Crawler', 'Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)', 'GPTBot', 'OAI-SearchBot', 'ChatGPT-User', 'ClaudeBot', 'Claude-SearchBot', 'Claude-User', 'PerplexityBot', 'Perplexity-User', 'Google-Extended', 'Unknown-AI-Crawler', 'Twitterbot', 'facebookexternalhit']


def routes(suffix):
    return {lang: BASE + prefix + suffix for lang, prefix in LANGS.items()}


def audit(root):
    root = Path(root)
    errors, cache, assets = [], {}, set()
    content = json.loads((SITE / '_build/seo-content.json').read_text())
    version = json.loads((SITE / '_build/source.json').read_text())['kiln_version']

    def check(ok, message):
        if not ok:
            errors.append(message)

    def file_for(url):
        path = unquote(urlsplit(url).path)
        return root / path.lstrip('/') / 'index.html' if path.endswith('/') else root / path.lstrip('/')

    def document(url):
        path = file_for(url)
        if path not in cache:
            check(path.is_file(), f'{url}: missing document')
            cache[path] = BeautifulSoup(path.read_text() if path.is_file() else '', 'html.parser')
        return cache[path]

    def one(soup, selector, label, attr=None):
        found = soup.select(selector)
        check(len(found) == 1, f'{label}: expected exactly one tag')
        value = (found[0].get(attr, '') if attr else found[0].get_text()) if found else ''
        check(bool(value.strip()), f'{label}: empty value')
        return value

    try:
        tree = ET.parse(root / 'sitemap.xml')
        urls = [el.text for el in tree.iter('{http://www.sitemaps.org/schemas/sitemap/0.9}loc')]
    except (OSError, ET.ParseError) as exc:
        return [f'sitemap: {exc}']
    suffixes = [''] + ['docs/' + ('' if s == 'index' else s + '.html') for s in SLUGS]
    expected = {url for suffix in suffixes for url in routes(suffix).values()}
    check(len(urls) == 48 and len(set(urls)) == 48, 'sitemap: 48 unique canonical URLs')
    check(set(urls) == expected, 'sitemap: exact canonical page coverage')
    png = root / 'assets/kiln-social-card-v2.png'
    try:
        data = png.read_bytes()
        check(data[:16] == b'\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR', 'social image: PNG signature and IHDR')
        width, height = struct.unpack('>II', data[16:24])
        check(width > 0 and height > 0 and len(data) < 5_000_000, 'social image: valid dimensions and under 5 MB')
    except (OSError, struct.error) as exc:
        errors.append(f'social image: {exc}')
        width, height = 0, 0

    def check_image_object(image, lang, label):
        expected_image = {'@type': 'ImageObject', 'url': SOCIAL, 'contentUrl': SOCIAL,
                          'width': width, 'height': height, 'encodingFormat': 'image/png',
                          'caption': content[lang]['social_image_alt']}
        check(isinstance(image, dict) and all(image.get(key) == value for key, value in expected_image.items()),
              f'{label}: social ImageObject must match the delivered image and localized caption')

    source = BeautifulSoup((SITE / 'index.html').read_text(), 'html.parser')
    titles, descriptions = set(), set()
    bot_names = {'robots', 'yandex', *(b.lower() for b in BOTS)}
    for url in urls:
        if url not in expected:
            continue
        soup = document(url)
        path = urlsplit(url).path
        lang = next((l for l in ('ko', 'ja', 'zh-CN') if path.startswith(LANGS[l])), 'en')
        suffix = path[len(LANGS[lang]):]
        guide = suffix.startswith('docs/')
        check(soup.html and soup.html.get('lang') == lang, f'{path}: html language')
        check(len(soup.select('h1')) == 1, f'{path}: one h1')
        title = one(soup, 'head title', f'{path}: title')
        check(title not in titles, f'{path}: unique title'); titles.add(title)
        description = one(soup, 'head meta[name="description"]', f'{path}: description', 'content')
        check(description not in descriptions, f'{path}: unique description'); descriptions.add(description)
        check(one(soup, 'head link[rel="canonical"]', f'{path}: canonical', 'href') == url, f'{path}: self canonical')
        one(soup, 'head meta[name="viewport"]', f'{path}: viewport', 'content')
        for meta in soup.select('meta[name][content]'):
            if meta['name'].lower() in bot_names:
                directives = set(meta['content'].lower().replace(',', ' ').split())
                check(not directives.intersection({'noindex', 'none', 'nofollow'}), f'{path}: indexable for {meta["name"]}')
        if not guide:
            for name in ['google-site-verification', 'naver-site-verification', 'msvalidate.01']:
                proof = source.find('meta', attrs={'name': name})
                value = one(soup, f'head meta[name="{name}"]', f'{path}: {name}', 'content')
                check(proof and value == proof.get('content'), f'{path}: {name} ownership proof preserved')
        alternates = {**routes(suffix), 'x-default': routes(suffix)['en']}
        tags = soup.select('head link[hreflang]')
        check(len(tags) == 5 and all(a.get('rel') == ['alternate'] for a in tags)
              and {a.get('hreflang'): a.get('href') for a in tags} == alternates,
              f'{path}: exact reciprocal hreflang cluster with rel=alternate in head')
        for target in routes(suffix).values():
            check(soup.find('a', href=urlsplit(target).path), f'{path}: crawlable language link {target}')
        og = {'og:type': 'website', 'og:site_name': 'Kiln', 'og:title': title,
              'og:description': description, 'og:url': url, 'og:image': SOCIAL,
              'og:image:secure_url': SOCIAL, 'og:image:type': 'image/png',
              'og:image:width': str(width), 'og:image:height': str(height),
              'og:image:alt': content[lang]['social_image_alt'], 'og:locale': LOCALES[lang]}
        for key, value in og.items():
            actual = one(soup, f'head meta[property="{key}"]', f'{path}: {key}', 'content')
            check(actual == value, f'{path}: {key} does not match page/image')
        alt_locales = [m.get('content') for m in soup.select('head meta[property="og:locale:alternate"]')]
        check(len(alt_locales) == 3 and set(alt_locales) == set(LOCALES.values()) - {LOCALES[lang]}, f'{path}: og:locale:alternate')
        for key, value in {'twitter:card': 'summary_large_image', 'twitter:title': title,
                           'twitter:description': description, 'twitter:image': SOCIAL,
                           'twitter:image:alt': content[lang]['social_image_alt']}.items():
            check(one(soup, f'head meta[name="{key}"]', f'{path}: {key}', 'content') == value, f'{path}: {key} does not match page/image')
        graph = []
        scripts = soup.select('script[type="application/ld+json"]')
        check(bool(scripts), f'{path}: structured data')
        for script in scripts:
            try:
                data = json.loads(script.string or '')
                check(data.get('@context') == 'https://schema.org', f'{path}: schema context')
                graph.extend(data.get('@graph', [data]))
            except (ValueError, TypeError) as exc:
                errors.append(f'{path}: malformed JSON-LD: {exc}')
        pages = [n for n in graph if n.get('@type') == 'WebPage']
        check(len(pages) == 1 and pages[0].get('url') == url and pages[0].get('inLanguage') == lang, f'{path}: WebPage URL/language')
        if pages:
            check_image_object(pages[0].get('primaryImageOfPage'), lang, f'{path}: WebPage.primaryImageOfPage')
        for node in graph:
            check('aggregateRating' not in node and 'review' not in node, f'{path}: no invented rating/review')
        apps = [n for n in graph if n.get('@type') == 'SoftwareApplication']
        check(len(apps) == (0 if guide else 1), f'{path}: SoftwareApplication on landing only')
        for app in apps:
            check_image_object(app.get('image'), lang, f'{path}: SoftwareApplication.image')
            check(app.get('softwareVersion') == version, f'{path}: softwareVersion matches site source version')
            check(app.get('operatingSystem') == 'macOS 11 or later; Windows 11', f'{path}: operatingSystem')
            check(app.get('offers') == {'@type': 'Offer', 'price': '0', 'priceCurrency': 'USD'}, f'{path}: free offers with real currency')
            check(app.get('isAccessibleForFree') is True and app.get('applicationCategory') == 'DeveloperApplication', f'{path}: app category and access')
            check(app.get('screenshot') == BASE + f'/assets/hero-poster-{lang}.webp', f'{path}: screenshot must remain an actual localized product capture')
            assets.add(app.get('screenshot', ''))
        if guide:
            crumbs = [n for n in graph if n.get('@type') == 'BreadcrumbList']
            check(len(crumbs) == 1, f'{path}: BreadcrumbList')
            if crumbs:
                items = crumbs[0].get('itemListElement', [])
                check([i.get('position') for i in items] == list(range(1, len(items) + 1)) and items and items[-1].get('item') == url, f'{path}: breadcrumb order and URL')
        for el in soup.select('[href], [src], [poster]'):
            for attr in ('href', 'src', 'poster'):
                value = el.get(attr)
                if not value:
                    continue
                target = urljoin(url, value)
                parsed = urlsplit(target)
                if parsed.netloc != urlsplit(BASE).netloc:
                    continue
                check(parsed.scheme == 'https', f'{path}: HTTPS local resource {value}')
                p = file_for(target)
                check(p.is_file(), f'{path}: missing {value}')
                if parsed.path.startswith('/assets/'):
                    assets.add(BASE + parsed.path)
                if parsed.fragment and p.is_file() and p.suffix == '.html':
                    check(document(target).find(id=unquote(parsed.fragment)), f'{path}: missing anchor {value}')
        for img in soup.select('img'):
            check(img.has_attr('alt'), f'{path}: img alt')
        ids = [el['id'] for el in soup.select('[id]')]
        check(len(ids) == len(set(ids)), f'{path}: duplicate IDs')
    check(not (root / '_build').exists(), 'private build sources excluded')
    for page in root.rglob('*.html'):
        relative = page.relative_to(root).as_posix()
        if relative != '404.html':
            canonical = BASE + '/' + (relative[:-10] if relative.endswith('index.html') else relative)
            check(canonical in expected, f'unsitemapped HTML page: {relative}')
    not_found = document(BASE + '/404.html')
    check(any('noindex' in m.get('content', '').lower() for m in not_found.select('meta[name="robots"]')), '404: noindex')
    check(not not_found.select('link[rel="canonical"]'), '404: no canonical to a valid page')
    robots = RobotFileParser()
    robots.parse((root / 'robots.txt').read_text().splitlines())
    check(robots.site_maps() == [BASE + '/sitemap.xml'], 'robots: canonical sitemap discovery')
    for bot in BOTS:
        for url in urls + sorted(assets | {SOCIAL, BASE + '/robots.txt', BASE + '/sitemap.xml'}):
            check(robots.can_fetch(bot, url), f'robots blocks {bot}: {url}')
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=SITE.parent / 'public', help='published artifact directory')
    args = parser.parse_args()
    errors = audit(args.root)
    if errors:
        raise SystemExit('\n'.join(errors))
    print('SEO checks passed: 48 pages, exact reciprocal locales, OG/Twitter image bytes, app schema, crawl permissions, 404, assets, links and anchors')


if __name__ == '__main__':
    main()
