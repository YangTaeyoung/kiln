#!/usr/bin/env python3
"""Checks the Windows appcast that make-appcast.py writes; no extra dependencies."""
import pathlib
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET

SPARKLE = 'http://www.andymatuschak.org/xml-namespaces/sparkle'
KILN = 'https://github.com/YangTaeyoung/kiln/appcast'
ROOT = pathlib.Path(__file__).resolve().parent.parent


class WindowsAppcastTests(unittest.TestCase):
    def run_script(self, *args):
        return subprocess.run([sys.executable, 'scripts/make-appcast.py', *map(str, args)], cwd=ROOT, capture_output=True, text=True)

    def fixture(self, folder, arch, payload):
        archive = folder / f'Kiln-0.2.0-windows-{arch}.zip'
        archive.write_bytes(payload)
        signature = folder / f'{arch}.sig'
        signature.write_text(f'sparkle:edSignature="{arch.upper()}sig==" length="{len(payload)}"\n')
        return archive, signature

    def test_one_item_offers_a_signed_archive_per_architecture(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = pathlib.Path(tmp)
            x64 = self.fixture(folder, 'x64', b'x64 archive')
            arm64 = self.fixture(folder, 'arm64', b'arm64 archive!')
            result = self.run_script('--windows', '0.2.0', *x64, *arm64)
            self.assertEqual(result.returncode, 0, result.stderr)
            item = ET.parse(folder / 'appcast-windows.xml').getroot().find('channel/item')
            self.assertEqual(item.find(f'{{{SPARKLE}}}version').text, '0.2.0')
            enclosures = {e.get(f'{{{KILN}}}arch'): e for e in item.findall('enclosure')}
            self.assertEqual(sorted(enclosures), ['arm64', 'x64'])
            for arch, payload in (('x64', b'x64 archive'), ('arm64', b'arm64 archive!')):
                e = enclosures[arch]
                self.assertEqual(e.get(f'{{{SPARKLE}}}os'), 'windows')
                self.assertEqual(e.get('length'), str(len(payload)))
                self.assertEqual(e.get(f'{{{SPARKLE}}}edSignature'), f'{arch.upper()}sig==')
                self.assertTrue(e.get('url').endswith(f'/releases/download/v0.2.0/Kiln-0.2.0-windows-{arch}.zip'))

    def test_a_length_mismatch_writes_nothing(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = pathlib.Path(tmp)
            x64 = self.fixture(folder, 'x64', b'x64 archive')
            arm64 = self.fixture(folder, 'arm64', b'arm64 archive!')
            arm64[0].write_bytes(b'changed after signing')
            result = self.run_script('--windows', '0.2.0', *x64, *arm64)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse((folder / 'appcast-windows.xml').exists())


if __name__ == '__main__':
    unittest.main()
