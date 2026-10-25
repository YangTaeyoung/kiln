#!/usr/bin/env python3
"""Exercise generated plist output, never source text or Launch Services defaults."""
import plistlib
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HELPER = Path(__file__).with_name("configure-document-types.py")


class DocumentTypesTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name) / "Info.plist"
        self.original = {
            "CFBundleIdentifier": "dev.kiln.app",
            "CFBundleVersion": "unchanged",
            "CFBundleExecutable": "kiln",
            "SUPublicEDKey": "existing-public-key",
            "SUFeedURL": "https://example.test/appcast.xml",
            "NSLocalNetworkUsageDescription": "existing network description",
        }
        self.write(self.original)

    def write(self, info):
        self.path.write_bytes(plistlib.dumps(info))

    def run_helper(self, *args, success=True):
        result = subprocess.run([sys.executable, str(HELPER), *args, str(self.path)], capture_output=True, text=True)
        self.assertEqual(result.returncode == 0, success, result.stdout + result.stderr)

    def test_text_unknown_extensionless_and_folder_types_are_alternate(self):
        self.run_helper()
        info = plistlib.loads(self.path.read_bytes())
        types = info["CFBundleDocumentTypes"]
        by_uti = {uti: entry for entry in types for uti in entry["LSItemContentTypes"]}
        self.assertEqual(set(by_uti), {"public.text", "public.data", "public.folder"})
        for uti, entry in by_uti.items():
            self.assertEqual(entry["CFBundleTypeRole"], "Editor", uti)
            self.assertEqual(entry["LSHandlerRank"], "Alternate", uti)
            self.assertNotIn("NSDocumentClass", entry)
        self.assertEqual({key: info[key] for key in self.original}, self.original)
        self.assertNotIn("UTExportedTypeDeclarations", info)
        self.assertNotIn("UTImportedTypeDeclarations", info)
        before = self.path.read_bytes()
        self.run_helper()
        self.assertEqual(self.path.read_bytes(), before)
        self.run_helper("--verify")
        self.assertEqual(self.path.read_bytes(), before)

    def test_verify_rejects_missing_or_default_associations_without_writing(self):
        before = self.path.read_bytes()
        self.run_helper("--verify", success=False)
        self.assertEqual(self.path.read_bytes(), before)
        self.run_helper()
        info = plistlib.loads(self.path.read_bytes())
        info["CFBundleDocumentTypes"][0]["LSHandlerRank"] = "Default"
        self.write(info)
        before = self.path.read_bytes()
        self.run_helper("--verify", success=False)
        self.assertEqual(self.path.read_bytes(), before)

    def test_companion_identity_is_rejected_unchanged(self):
        self.write(dict(self.original, CFBundleIdentifier="dev.kiln.statusbar"))
        before = self.path.read_bytes()
        self.run_helper(success=False)
        self.assertEqual(self.path.read_bytes(), before)


if __name__ == "__main__":
    unittest.main()
