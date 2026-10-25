"""Fail the publish job on crawlability, metadata or internal-link regressions."""
import json
from urllib.robotparser import RobotFileParser
from pathlib import Path
from urllib.parse import urlsplit, unquote
from xml.etree import ElementTree as ET
from bs4 import BeautifulSoup
ROOT = Path(__file__).resolve().parents[2] / 'public'
BASE = 'https://kiln.melona.cloud'
urls = [el.text for el in ET.parse(ROOT/'sitemap.xml').iter() if el.tag.endswith('}loc')]
errors = []
cache = {}
def file_for(path):
    p = ROOT / unquote(path).lstrip('/')
    return p/'index.html' if path.endswith('/') else p
def document(path):
    p = file_for(path)
    if p not in cache: cache[p] = BeautifulSoup(p.read_text(), 'html.parser')
    return cache[p]
def check(ok, message):
    if not ok: errors.append(message)
verification = BeautifulSoup((ROOT.parent/'site/index.html').read_text(), 'html.parser').select_one('meta[name=google-site-verification]')['content']
search_bots = ['Googlebot', 'Googlebot-Image', 'Googlebot-Video', 'bingbot', 'Yeti', 'Daum', 'DuckDuckBot', 'Slurp', 'Baiduspider', 'YandexBot', 'YandexImages', 'Applebot', 'MojeekBot', 'Sogou web spider', 'Sogou inst spider']
titles=set()
for url in urls:
    path = urlsplit(url).path; s = document(path)
    if not s.select_one('article.doc'):
        proof = s.select_one('meta[name=google-site-verification]')
        check(proof and proof.get('content') == verification, f'{path}: Search Console verification preserved')
    check(len(s.select('h1'))==1, f'{path}: one h1')
    check(s.title and s.title.text not in titles, f'{path}: unique title'); titles.add(s.title.text)
    check(s.select_one('meta[name=description]')['content'], f'{path}: description')
    check(s.select_one('link[rel=canonical]')['href']==url, f'{path}: canonical')
    for meta in s.select('meta[name][content]'):
        if meta['name'].lower() in {'robots', 'yandex', *(bot.lower() for bot in search_bots)}:
            directives = set(meta['content'].lower().replace(',', ' ').split())
            check(not directives.intersection({'noindex', 'none', 'nofollow'}), f'{path}: indexable for {meta["name"]}')
    alts=s.select('link[hreflang]'); check(len(alts)==5,f'{path}: five language alternates')
    for a in alts:
        check(a['href'] in urls, f'{path}: alternate in sitemap')
        other=document(urlsplit(a['href']).path)
        check(any(x['href']==url for x in other.select('link[hreflang]')),f'{path}: reciprocal hreflang')
    for script in s.select('script[type="application/ld+json"]'): json.loads(script.string)
    check(bool(s.select('script[type="application/ld+json"]')),f'{path}: structured data')
    for el in s.select('[href], [src], [poster]'):
        for attr in ('href','src','poster'):
            value=el.get(attr,'')
            if not value.startswith(('/','#')): continue
            parsed=urlsplit(value); target=parsed.path or path
            p=file_for(target)
            check(p.exists(),f'{path}: missing {value}')
            if parsed.fragment and p.exists() and (p.suffix=='.html'):
                check(document(target).find(id=unquote(parsed.fragment)) is not None,f'{path}: missing anchor {value}')
    for img in s.select('img'): check(img.has_attr('alt'),f'{path}: img alt')
    ids=[el['id'] for el in s.select('[id]')];check(len(ids)==len(set(ids)),f'{path}: duplicate IDs')
check(len(urls)==48,'48 canonical URLs')
check(not (ROOT/'_build').exists(),'private build sources excluded')
check('Sitemap: '+BASE+'/sitemap.xml' in (ROOT/'robots.txt').read_text(),'robots sitemap')
robots = RobotFileParser()
robots.parse((ROOT/'robots.txt').read_text().splitlines())
for bot in search_bots + ['Unknown-Search-Crawler', 'Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)', 'GPTBot', 'OAI-SearchBot', 'ChatGPT-User', 'ClaudeBot', 'Claude-SearchBot', 'Claude-User', 'PerplexityBot', 'Perplexity-User', 'Google-Extended', 'Unknown-AI-Crawler']:
    for url in urls + [BASE+'/robots.txt', BASE+'/sitemap.xml', BASE+'/assets/hero-poster-en.webp', BASE+'/assets/downloads.js']:
        check(robots.can_fetch(bot, url), f'robots blocks {bot}: {url}')
check(robots.site_maps() == [BASE+'/sitemap.xml'], 'robots canonical sitemap')
if errors: raise SystemExit('\n'.join(errors))
print(f'SEO checks passed: {len(urls)} pages, reciprocal locales, metadata, JSON-LD, assets, links and anchors')
