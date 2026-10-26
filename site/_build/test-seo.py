"""Exercise published artifacts, including failures the publishing gate must catch.

Run build-docs.py and build-site.py first. No source-text assertions or network calls.
"""
import json
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest

from bs4 import BeautifulSoup

REPO = Path(__file__).resolve().parents[2]
PUBLIC = REPO / 'public'
CHECKER = Path(__file__).with_name('check-seo.py')
SOCIAL = 'https://kiln.melona.cloud/assets/kiln-social-card-v2.png'


class PublishedSEOTests(unittest.TestCase):
    def test_all_48_pages_describe_the_actual_social_image(self):
        image = (PUBLIC / 'assets/kiln-social-card-v2.png').read_bytes()
        self.assertEqual(image[:8], b'\x89PNG\r\n\x1a\n')
        width, height = struct.unpack('>II', image[16:24])
        self.assertEqual((width, height), (1733, 907))
        pages = [p for p in PUBLIC.rglob('*.html') if p.name != '404.html']
        self.assertEqual(len(pages), 48)
        for page in pages:
            with self.subTest(page=page.relative_to(PUBLIC)):
                soup = BeautifulSoup(page.read_text(), 'html.parser')
                for key, value in {'og:image': SOCIAL, 'og:image:secure_url': SOCIAL,
                                   'og:image:type': 'image/png', 'og:image:width': str(width),
                                   'og:image:height': str(height)}.items():
                    tags = soup.find_all('meta', property=key)
                    self.assertEqual([t.get('content') for t in tags], [value])
                self.assertEqual(soup.find('meta', attrs={'name': 'twitter:image'})['content'], SOCIAL)
                alt = soup.find('meta', property='og:image:alt')['content']
                self.assertIn('Kiln', alt)
                self.assertEqual(soup.find('meta', attrs={'name': 'twitter:image:alt'})['content'], alt)
                self.assertEqual(len(soup.find_all('meta', property='og:locale:alternate')), 3)

    def run_checker(self, root):
        return subprocess.run([sys.executable, str(CHECKER), '--root', str(root)],
                              text=True, capture_output=True)

    def test_checker_accepts_published_artifacts(self):
        result = self.run_checker(PUBLIC)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def assert_page_mutation_rejected(self, mutate, diagnostic):
        with tempfile.TemporaryDirectory(prefix='kiln-seo-metadata-test-') as temp:
            root = Path(temp)
            shutil.copytree(PUBLIC, root, dirs_exist_ok=True, ignore=shutil.ignore_patterns('assets'))
            (root / 'assets').symlink_to(PUBLIC / 'assets', target_is_directory=True)
            home = root / 'index.html'
            soup = BeautifulSoup(home.read_text(), 'html.parser')
            mutate(soup)
            home.write_text(str(soup))
            result = self.run_checker(root)
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn(diagnostic, result.stdout + result.stderr)

    def test_checker_requires_alternate_relation_on_head_hreflang_links(self):
        for relation in (None, 'stylesheet', 'alternate stylesheet'):
            with self.subTest(rel=relation):
                def mutate(soup):
                    link = soup.find('link', hreflang='ja')
                    if relation is None:
                        del link['rel']
                    else:
                        link['rel'] = relation
                self.assert_page_mutation_rejected(mutate, 'hreflang')

    def test_checker_validates_both_structured_image_objects(self):
        invalid_fields = {
            '@type': 'VideoObject',
            'url': '/assets/kiln-social-card-v2.png',
            'contentUrl': 'https://example.invalid/missing.png',
            'width': 1200,
            'height': 630,
            'encodingFormat': 'image/jpeg',
            'caption': 'Unrelated image',
        }
        for owner, property_name in [('WebPage', 'primaryImageOfPage'), ('SoftwareApplication', 'image')]:
            cases = [*invalid_fields.items(), ('null object', None), ('string object', SOCIAL)]
            for field, value in cases:
                with self.subTest(owner=owner, property=property_name, field=field):
                    def mutate(soup):
                        script = soup.find('script', type='application/ld+json')
                        data = json.loads(script.string)
                        node = next(n for n in data['@graph'] if n['@type'] == owner)
                        if field in invalid_fields:
                            node[property_name][field] = value
                        else:
                            node[property_name] = value
                        script.string = json.dumps(data)
                    self.assert_page_mutation_rejected(mutate, 'ImageObject')

    def test_checker_rejects_regressions_in_rendered_documents(self):
        def drop_meta(soup):
            soup.find('meta', property='og:image:width').decompose()

        def duplicate_meta(soup):
            soup.head.append(soup.find('meta', property='og:image').__copy__())

        def set_meta(key, value):
            def mutate(soup):
                soup.find('meta', attrs={'property' if key.startswith('og:') else 'name': key})['content'] = value
            return mutate

        def schema_change(key, value):
            def mutate(soup):
                script = soup.find('script', type='application/ld+json')
                graph = json.loads(script.string)
                next(n for n in graph['@graph'] if n['@type'] == 'SoftwareApplication')[key] = value
                script.string = json.dumps(graph)
            return mutate

        def bad_hreflang(soup):
            soup.find('link', hreflang='ja')['href'] = 'https://kiln.melona.cloud/ko/'

        def broken_link(soup):
            soup.find('a', href='/docs/')['href'] = '/docs/terminal.html#missing-anchor'

        cases = [
            ('missing width', drop_meta, 'og:image:width'),
            ('duplicate image', duplicate_meta, 'og:image'),
            ('wrong MIME', set_meta('og:image:type', 'image/jpeg'), 'og:image:type'),
            ('wrong dimensions', set_meta('og:image:width', '1200'), 'og:image:width'),
            ('relative image URL', set_meta('og:image', '/assets/kiln-social-card-v2.png'), 'og:image'),
            ('empty Twitter alt', set_meta('twitter:image:alt', ''), 'twitter:image:alt'),
            ('wrong locale', set_meta('og:locale', 'ko_KR'), 'og:locale'),
            ('wrong alternate target', bad_hreflang, 'hreflang'),
            ('missing fragment', broken_link, 'missing anchor'),
            ('fake rating', schema_change('aggregateRating', {'ratingValue': 5, 'ratingCount': 42}), 'rating'),
            ('stale version', schema_change('softwareVersion', '0.0.0'), 'softwareVersion'),
            ('social card used as screenshot', schema_change('screenshot', SOCIAL), 'screenshot'),
            ('false OS', schema_change('operatingSystem', 'Linux'), 'operatingSystem'),
            ('paid offer', schema_change('offers', {'@type': 'Offer', 'price': '9', 'priceCurrency': 'USD'}), 'offers'),
        ]
        for name, mutate, diagnostic in cases:
            with self.subTest(case=name), tempfile.TemporaryDirectory(prefix='kiln-seo-test-') as temp:
                root = Path(temp)
                for path in PUBLIC.rglob('*'):
                    if path.is_file() and 'assets' not in path.relative_to(PUBLIC).parts:
                        target = root / path.relative_to(PUBLIC)
                        target.parent.mkdir(parents=True, exist_ok=True)
                        shutil.copyfile(path, target)
                (root / 'assets').symlink_to(PUBLIC / 'assets', target_is_directory=True)
                home = root / 'index.html'
                soup = BeautifulSoup(home.read_text(), 'html.parser')
                mutate(soup)
                home.write_text(str(soup))
                result = self.run_checker(root)
                self.assertNotEqual(result.returncode, 0, name)
                self.assertIn(diagnostic, result.stdout + result.stderr)

    def test_checker_rejects_crawl_and_error_page_regressions(self):
        cases = [
            ('robots.txt', lambda text: text + '\nUser-agent: Twitterbot\nDisallow: /assets/\n', 'robots blocks Twitterbot'),
            ('404.html', lambda text: text.replace('noindex', 'index'), '404: noindex'),
            ('sitemap.xml', lambda text: text.replace('/ko/docs/terminal.html', '/ko/docs/agents.html'), 'sitemap: 48 unique'),
            ('index.html', lambda text: text.replace('"@context":', '"@context"'), 'malformed JSON-LD'),
            ('index.html', lambda text: text.replace('<head>', '<head><meta name="Googlebot" content="noindex">'), 'indexable for Googlebot'),
        ]
        for path, mutate, diagnostic in cases:
            with self.subTest(path=path, diagnostic=diagnostic), tempfile.TemporaryDirectory(prefix='kiln-seo-crawl-test-') as temp:
                root = Path(temp)
                shutil.copytree(PUBLIC, root, dirs_exist_ok=True, ignore=shutil.ignore_patterns('assets'))
                (root / 'assets').symlink_to(PUBLIC / 'assets', target_is_directory=True)
                target = root / path
                target.write_text(mutate(target.read_text()))
                result = self.run_checker(root)
                self.assertNotEqual(result.returncode, 0, diagnostic)
                self.assertIn(diagnostic, result.stdout + result.stderr)


if __name__ == '__main__':
    unittest.main()
