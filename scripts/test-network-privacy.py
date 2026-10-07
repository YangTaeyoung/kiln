#!/usr/bin/env python3
"""Isolated regressions for signed-bundle privacy/identity requirements."""
import importlib.util
from pathlib import Path
import plistlib
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('privacy', Path(__file__).with_name('configure-network-privacy.py'))
privacy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(privacy)
MAIN = 'AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA'
STATUS = 'BBBBBBBB-BBBB-BBBB-BBBB-BBBBBBBBBBBB'


class PrivacyTests(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.TemporaryDirectory(prefix='kiln-privacy-test-')
        self.addCleanup(self.root.cleanup)
        self.app = Path(self.root.name) / 'Kiln.app'
        for bundle, name, identity in zip(privacy.bundles(self.app), ['kiln', 'kiln-status'], ['dev.kiln.app', 'dev.kiln.statusbar']):
            (bundle / 'Contents/MacOS').mkdir(parents=True)
            (bundle / 'Contents/MacOS' / name).write_bytes(b'fixture')
            (bundle / 'Contents/Info.plist').write_bytes(plistlib.dumps({'CFBundleExecutable': name, 'CFBundleIdentifier': identity}))

    def output(self, command, **kwargs):
        value = STATUS if Path(command[-1]).name == 'kiln-status' else MAIN
        return f'UUID: {value} (arm64) {command[-1]}\n'

    def test_both_apps_include_four_native_translations_without_changing_identity(self):
        with patch.object(privacy.subprocess, 'check_output', side_effect=self.output):
            privacy.configure(self.app)
            self.assertEqual(privacy.verify(self.app), {'dev.kiln.app': {'arm64': MAIN}, 'dev.kiln.statusbar': {'arm64': STATUS}})
            before = {p: p.read_bytes() for p in self.app.rglob('*') if p.is_file()}
            privacy.verify(self.app)
            self.assertEqual(before, {p: p.read_bytes() for p in self.app.rglob('*') if p.is_file()})
            for bundle in privacy.bundles(self.app):
                info = plistlib.loads((bundle / 'Contents/Info.plist').read_bytes())
                self.assertNotIn('NSBonjourServices', info)
                for lang in privacy.DESCRIPTIONS:
                    self.assertTrue((bundle / f'Contents/Resources/{lang}.lproj/InfoPlist.strings').is_file())

    def test_collision_or_missing_uuid_blocks_bundle_before_mutation(self):
        for output in [f'UUID: {MAIN} (arm64) fixture\n', '']:
            before = (self.app / 'Contents/Info.plist').read_bytes()
            with patch.object(privacy.subprocess, 'check_output', return_value=output), self.assertRaises(ValueError):
                privacy.configure(self.app)
            self.assertEqual(before, (self.app / 'Contents/Info.plist').read_bytes())

    def test_architecture_mismatch_blocks_bundle(self):
        with patch.object(privacy.subprocess, 'check_output', side_effect=[f'UUID: {MAIN} (arm64) a\n', f'UUID: {STATUS} (x86_64) b\n']), self.assertRaises(ValueError):
            privacy.configure(self.app)

    def test_missing_description_translation_or_unused_service_fails_verification(self):
        with patch.object(privacy.subprocess, 'check_output', side_effect=self.output):
            privacy.configure(self.app)
            path = self.app / 'Contents/Info.plist'
            original = path.read_bytes()
            for change in [{privacy.KEY: ''}, {'NSBonjourServices': ['_dummy._tcp']}]:
                info = plistlib.loads(original); info.update(change)
                path.write_bytes(plistlib.dumps(info))
                with self.assertRaises(ValueError): privacy.verify(self.app)
            path.write_bytes(original)
            (self.app / 'Contents/Resources/ko.lproj/InfoPlist.strings').write_text('broken', encoding='utf-8')
            with self.assertRaises(ValueError): privacy.verify(self.app)


if __name__ == '__main__':
    unittest.main()
