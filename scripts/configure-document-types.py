#!/usr/bin/env python3
"""Configure Finder document associations in a generated Kiln Info.plist."""
import argparse
import plistlib
from pathlib import Path


def document_types() -> list[dict]:
    # public.data covers text/config files with unknown or absent extensions.
    # Kiln opens these as text; its editor keeps binary/lossy content read-only.
    # Alternate advertises Open With without claiming ownership or setting defaults.
    # https://developer.apple.com/library/archive/documentation/Miscellaneous/Reference/UTIRef/Articles/System-DeclaredUniformTypeIdentifiers.html
    # https://developer.apple.com/documentation/bundleresources/information-property-list/cfbundledocumenttypes/lshandlerrank
    return [
        {
            "CFBundleTypeName": name,
            "CFBundleTypeRole": "Editor",
            "LSHandlerRank": "Alternate",
            "LSItemContentTypes": [uti],
        }
        for name, uti in [
            ("Text and Source Files", "public.text"),
            ("Other Files", "public.data"),
            ("Workspace Folders", "public.folder"),
        ]
    ]


def configure(path: Path, verify: bool = False) -> None:
    info = plistlib.loads(path.read_bytes())
    if info.get("CFBundleIdentifier") != "dev.kiln.app":
        raise ValueError("document associations belong only to the Kiln GUI bundle")
    expected = document_types()
    if verify:
        if info.get("CFBundleDocumentTypes") != expected:
            raise ValueError("Kiln document associations are missing or differ from Editor/Alternate")
        return
    info["CFBundleDocumentTypes"] = expected
    path.write_bytes(plistlib.dumps(info))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--verify", action="store_true")
    parser.add_argument("plist", type=Path)
    args = parser.parse_args()
    configure(args.plist, args.verify)
