#!/usr/bin/env python3
"""Generate a minimal appcast from Sparkle's signature output, without secrets.

macOS: make-appcast.py VERSION ARCHIVE SIGNATURE writes appcast.xml next to ARCHIVE.
Windows: make-appcast.py --windows VERSION X64_ARCHIVE X64_SIGNATURE ARM64_ARCHIVE ARM64_SIGNATURE
writes appcast-windows.xml next to the archives, one enclosure per architecture.
"""
import datetime, email.utils, json, pathlib, re, sys, xml.etree.ElementTree as ET

SPARKLE = 'http://www.andymatuschak.org/xml-namespaces/sparkle'
KILN = 'https://github.com/YangTaeyoung/kiln/appcast'


def signed(archive_name, signature_name):
    """The archive, its Ed25519 signature and its length, checked against the archive."""
    archive = pathlib.Path(archive_name)
    signature = pathlib.Path(signature_name).read_text()
    match = re.search(r'sparkle:edSignature="([A-Za-z0-9+/=]+)"', signature)
    length = re.search(r'length="(\d+)"', signature)
    if not match or not length or int(length[1]) != archive.stat().st_size:
        raise SystemExit(f'Missing signature or archive length mismatch: {archive.name}')
    return archive, match[1], length[1]


def channel(config, version):
    ET.register_namespace('sparkle', SPARKLE)
    ET.register_namespace('kiln', KILN)
    rss = ET.Element('rss', {'version': '2.0'})
    chan = ET.SubElement(rss, 'channel')
    ET.SubElement(chan, 'title').text = 'Kiln updates'
    ET.SubElement(chan, 'link').text = f'https://github.com/{config["repository"]}'
    item = ET.SubElement(chan, 'item')
    ET.SubElement(item, 'title').text = f'Kiln {version}'
    ET.SubElement(item, 'pubDate').text = email.utils.format_datetime(datetime.datetime.now(datetime.timezone.utc))
    ET.SubElement(item, f'{{{SPARKLE}}}version').text = version
    ET.SubElement(item, f'{{{SPARKLE}}}shortVersionString').text = version
    return rss, item


def enclosure(item, config, version, archive, signature, length, extra=None):
    attributes = {
        'url': f'https://github.com/{config["repository"]}/releases/download/v{version}/{archive.name}',
        'length': length,
        'type': 'application/octet-stream',
        f'{{{SPARKLE}}}edSignature': signature,
    }
    attributes.update(extra or {})
    ET.SubElement(item, 'enclosure', attributes)


def write(rss, path):
    ET.indent(rss)
    ET.ElementTree(rss).write(path, encoding='utf-8', xml_declaration=True)


def main(args):
    config = json.loads(pathlib.Path('scripts/release-config.json').read_text())
    if args[:1] == ['--windows']:
        version, x64, x64_signature, arm64, arm64_signature = args[1:]
        archives = [('x64', *signed(x64, x64_signature)), ('arm64', *signed(arm64, arm64_signature))]
        rss, item = channel(config, version)
        ET.SubElement(item, 'description').text = f'Release notes: https://github.com/{config["repository"]}/releases/tag/v{version}'
        for arch, archive, signature, length in archives:
            enclosure(item, config, version, archive, signature, length, {f'{{{SPARKLE}}}os': 'windows', f'{{{KILN}}}arch': arch})
        write(rss, archives[0][1].parent / 'appcast-windows.xml')
        return
    version, archive_name, signature_name = args
    archive, signature, length = signed(archive_name, signature_name)
    rss, item = channel(config, version)
    ET.SubElement(item, f'{{{SPARKLE}}}minimumSystemVersion').text = '11.0'
    ET.SubElement(item, 'description').text = f'Release notes: https://github.com/{config["repository"]}/releases/tag/v{version}'
    enclosure(item, config, version, archive, signature, length)
    write(rss, archive.parent / 'appcast.xml')


if __name__ == '__main__':
    main(sys.argv[1:])
