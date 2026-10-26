"""Build crawlable, language-specific Pages output; sources remain in site/."""
import importlib.util
import json
import pathlib
import shutil
import struct
from urllib.parse import urljoin, urlsplit
from xml.etree import ElementTree as ET
from bs4 import BeautifulSoup
from fontTools import subset
from fontTools.ttLib import TTFont

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT.parent / 'public'
BASE = 'https://kiln.melona.cloud'
LANGS = {'en': '/', 'ko': '/ko/', 'ja': '/ja/', 'zh-CN': '/zh-cn/'}
NAMES = {'en': 'English', 'ko': '한국어', 'ja': '日本語', 'zh-CN': '简体中文'}
LOCALES = {'en': 'en_US', 'ko': 'ko_KR', 'ja': 'ja_JP', 'zh-CN': 'zh_CN'}
SOCIAL_IMAGE = '/assets/kiln-social-card-v2.png'
CONTENT = json.loads((ROOT / '_build/seo-content.json').read_text())
I18N = json.loads((ROOT / 'assets/site-i18n.js').read_text().split('=', 1)[1].strip().rstrip(';'))
spec = importlib.util.spec_from_file_location('docs', ROOT / '_build/build-docs.py')
docs = importlib.util.module_from_spec(spec)
spec.loader.exec_module(docs)


def route(lang, slug=None):
    return LANGS[lang] + (('docs/' + ('' if slug == 'index' else slug + '.html')) if slug else '')


def build(lang, slug=None):
    c = CONTENT[lang]
    soup = BeautifulSoup((ROOT / ('docs/' + slug + '.html' if slug else 'index.html')).read_text(), 'html.parser')
    def tag(tag_name, text=None, **attrs):
        el = soup.new_tag(tag_name, attrs=attrs)
        if text is not None: el.string = text
        return el
    soup.html['lang'] = lang
    for platform, key in [('mac', 'dl_mac'), ('windows', 'dl_windows'), ('generic', 'nav_download')]:
        soup.html['data-download-' + platform + '-text'] = I18N[lang][key]
    routes = {l: route(l, slug) for l in LANGS}
    if slug:
        meta, _ = docs.read(lang, slug)
        title, description = meta['title'] + ' · Kiln Docs', meta['description']
        for el in soup.select('[data-lang]'):
            if el['data-lang'] != lang: el.decompose()
            else: el.attrs.pop('hidden', None)
        for el in soup.select('[data-ui]'):
            el.string = docs.UI[lang].get(el['data-ui'], el.get_text())
        soup.select_one('#toc-h').string = docs.UI[lang]['toc']
    else:
        title, description = c['title'], c['description']
        for el in soup.select('[data-i18n]'):
            el.string = I18N[lang].get(el['data-i18n'], el.get_text())
        section = tag('section', **{'class': 'seo-guide', 'aria-labelledby': 'workflow-heading'})
        section.append(tag('h2', c['section'], id='workflow-heading'))
        section.append(tag('p', c['intro']))
        for heading, body, target in c['items']:
            section.append(tag('h3', heading)); section.append(tag('p', body))
            section.append(tag('a', c['guide'] + ' →', href='docs/' + target + '.html'))
        section.append(tag('h2', c['faq_title']))
        for question, answer in c['faq']:
            detail = tag('details'); detail.append(tag('summary', question)); detail.append(tag('p', answer)); section.append(detail)
        soup.main.append(section)
        for el in soup.select('.shot[data-crop]'):
            x,y,w,h = map(float, el['data-crop'].split())
            el['style'] = f'aspect-ratio:{w*2000}/{h*1257}'
            el.append(tag('img', src=f"/assets/shots/{el['data-shot']}-{lang}.webp", alt=I18N[lang].get({'agents':'f_agents_t','git':'f_review_t','db':'f_db_t'}.get(el['data-shot'], ''), c['section']), loading='lazy', decoding='async', width='2000', height='1257', style=f'width:{100/w}%;left:{-x/w*100}%;top:{-y/h*100}%'))
        video = soup.select_one('#hero-video')
        video['poster'] = f'/assets/hero-poster-{lang}.webp'
        video['preload'] = 'none'
        for source in video.select('source'):
            source['src'] = source['src'].replace('-en.', f'-{lang}.')
        soup.head.append(tag('link', rel='preload', href=video['poster'], **{'as':'image', 'fetchpriority':'high'}))
    # Rewrite local URLs against the source layout, then preserve locale for documents.
    for el in soup.find_all(True):
        for attr in ('href', 'src', 'poster'):
            value = el.get(attr)
            if not value or value.startswith(('#', 'http:', 'https:', 'data:', 'mailto:')): continue
            parsed = urlsplit(urljoin('/docs/' if slug else '/', value))
            path = parsed.path
            if path == '/index.html': path = LANGS[lang]
            elif path.startswith('/docs/'):
                path = LANGS[lang] + path.lstrip('/')
                if path.endswith('/index.html'): path = path[:-10]
            el[attr] = path + ('#' + parsed.fragment if parsed.fragment else '')
    for img in soup.select('.shot img'):
        if not img.get('alt'): img['alt'] = docs.UI[lang]['shot']
        img['width'], img['height'] = '2000', '1257'
    for option in soup.select('#lang option'):
        option.attrs.pop('selected', None)
        if option['value'] == lang: option['selected'] = ''
    soup.title.string = title
    soup.select_one('meta[name="description"]')['content'] = description
    soup.head.append(tag('meta', name='robots', content='index, follow, max-image-preview:large, max-snippet:-1, max-video-preview:-1'))
    canonical = BASE + routes[lang]
    soup.head.append(tag('link', rel='canonical', href=canonical))
    for l, path in {**routes, 'x-default': routes['en']}.items():
        soup.head.append(tag('link', rel='alternate', hreflang=l, href=BASE+path))
    image = BASE + SOCIAL_IMAGE
    # Read the delivered asset rather than assuming a standard social-card size.
    with (ROOT / SOCIAL_IMAGE.lstrip('/')).open('rb') as image_file:
        header = image_file.read(24)
    if header[:16] != b'\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR':
        raise ValueError('The social image must be the supplied PNG')
    width, height = struct.unpack('>II', header[16:24])
    image_object = {'@type':'ImageObject', 'url':image, 'contentUrl':image,
                    'width':width, 'height':height, 'encodingFormat':'image/png',
                    'caption':c['social_image_alt']}
    for key, value in {'og:type':'website', 'og:site_name':'Kiln', 'og:title':title, 'og:description':description, 'og:url':canonical, 'og:image':image, 'og:image:secure_url':image, 'og:image:type':'image/png', 'og:image:width':str(width), 'og:image:height':str(height), 'og:image:alt':c['social_image_alt'], 'og:locale':LOCALES[lang]}.items():
        soup.head.append(tag('meta', property=key, content=value))
    for other, locale in LOCALES.items():
        if other != lang: soup.head.append(tag('meta', property='og:locale:alternate', content=locale))
    for key,value in {'twitter:card':'summary_large_image','twitter:title':title,'twitter:description':description,'twitter:image':image,'twitter:image:alt':c['social_image_alt']}.items():
        soup.head.append(tag('meta', name=key, content=value))
    graph = [{'@type':'WebSite','@id':BASE+'/#website','name':'Kiln','url':BASE+'/'}, {'@type':'WebPage','@id':canonical+'#page','url':canonical,'name':title,'description':description,'inLanguage':lang,'isPartOf':{'@id':BASE+'/#website'},'primaryImageOfPage':image_object}]
    if not slug:
        version = json.loads((ROOT / '_build/source.json').read_text())['kiln_version']
        graph.append({'@type':'SoftwareApplication','@id':BASE+'/#app','name':'Kiln','url':BASE+'/','description':description,'applicationCategory':'DeveloperApplication','operatingSystem':'macOS 11 or later; Windows 11','softwareVersion':version,'license':'https://opensource.org/license/mit','isAccessibleForFree':True,'offers':{'@type':'Offer','price':'0','priceCurrency':'USD'},'downloadUrl':'https://github.com/YangTaeyoung/kiln/releases/latest','image':image_object,'screenshot':BASE+f'/assets/hero-poster-{lang}.webp','sameAs':'https://github.com/YangTaeyoung/kiln'})
    else:
        crumbs = [(c['home'],BASE+LANGS[lang]),(c['docs'],BASE+route(lang,'index'))]
        if slug != 'index': crumbs.append((meta['title'],canonical))
        graph.append({'@type':'BreadcrumbList','itemListElement':[{'@type':'ListItem','position':i+1,'name':name,'item':url} for i,(name,url) in enumerate(crumbs)]})
    soup.head.append(tag('script', json.dumps({'@context':'https://schema.org','@graph':graph},ensure_ascii=False).replace('<','\\u003c'), type='application/ld+json'))
    soup.head.append(tag('script', 'window.KILN_ROUTES = '+json.dumps(routes)+';'))
    soup.head.append(tag('link', rel='stylesheet', href='/assets/seo.css'))
    nav = tag('nav', **{'class':'locale-links','aria-label':c['languages']})
    for l,path in routes.items(): nav.append(tag('a', NAMES[l], href=path, hreflang=l, lang=l))
    soup.footer.append(nav)
    path = OUT / routes[lang].lstrip('/')
    if routes[lang].endswith('/'): path = path / 'index.html'
    path.parent.mkdir(parents=True,exist_ok=True); path.write_text(str(soup))
    return canonical


def main():
    if OUT.exists(): shutil.rmtree(OUT)
    OUT.mkdir(); shutil.copytree(ROOT/'assets',OUT/'assets')
    urls = [build(lang,slug) for lang in LANGS for slug in [None]+docs.ORDER]
    # Keep original font sources; rebuild published subsets whenever copy changes.
    text = ''.join(BeautifulSoup(p.read_text(), 'html.parser').get_text() for p in OUT.rglob('*.html'))
    for path in sorted((OUT/'assets/fonts').glob('*.woff2')):
        font = TTFont(path)
        sub = subset.Subsetter()
        sub.populate(text=text)
        sub.subset(font)
        font.save(path)
    ET.register_namespace('',  'http://www.sitemaps.org/schemas/sitemap/0.9')
    sitemap = ET.Element('urlset', xmlns='http://www.sitemaps.org/schemas/sitemap/0.9')
    for url in urls: ET.SubElement(ET.SubElement(sitemap,'url'),'loc').text=url
    ET.ElementTree(sitemap).write(OUT/'sitemap.xml',encoding='utf-8',xml_declaration=True)
    shutil.copyfile(ROOT/'_build/robots.txt', OUT/'robots.txt')
    (OUT/'.nojekyll').touch()
    (OUT/'404.html').write_text('<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width"><meta name="robots" content="noindex"><title>Page not found · Kiln</title><h1>Page not found</h1><p>This address does not exist.</p><a href="/">Kiln</a> · <a href="/docs/">Documentation</a></html>')
    print(f'Built {len(urls)} canonical pages in {OUT}')

if __name__ == '__main__': main()
