#!/usr/bin/env python3
"""Generate a minimal appcast from Sparkle's signature output, without secrets."""
import datetime, email.utils, json, pathlib, re, sys, xml.etree.ElementTree as ET
version, archive_name, signature_name = sys.argv[1:]
archive = pathlib.Path(archive_name)
config = json.loads(pathlib.Path('scripts/release-config.json').read_text())
signature = pathlib.Path(signature_name).read_text()
match = re.search(r'sparkle:edSignature="([A-Za-z0-9+/=]+)"', signature)
length = re.search(r'length="(\d+)"', signature)
if not match or not length or int(length[1]) != archive.stat().st_size:
    raise SystemExit('Missing signature or archive length mismatch')
ns = 'http://www.andymatuschak.org/xml-namespaces/sparkle'
ET.register_namespace('sparkle', ns)
rss = ET.Element('rss', {'version':'2.0'})
channel=ET.SubElement(rss,'channel')
ET.SubElement(channel,'title').text='Kiln updates'
ET.SubElement(channel,'link').text=f'https://github.com/{config["repository"]}'
item=ET.SubElement(channel,'item')
ET.SubElement(item,'title').text=f'Kiln {version}'
ET.SubElement(item,'pubDate').text=email.utils.format_datetime(datetime.datetime.now(datetime.timezone.utc))
ET.SubElement(item,f'{{{ns}}}version').text=version
ET.SubElement(item,f'{{{ns}}}shortVersionString').text=version
ET.SubElement(item,f'{{{ns}}}minimumSystemVersion').text='11.0'
ET.SubElement(item,'description').text=f'Release notes: https://github.com/{config["repository"]}/releases/tag/v{version}'
ET.SubElement(item,'enclosure', {'url':f'https://github.com/{config["repository"]}/releases/download/v{version}/{archive.name}',
    'length':length[1], 'type':'application/octet-stream', f'{{{ns}}}edSignature':match[1]})
ET.indent(rss)
ET.ElementTree(rss).write(archive.parent/'appcast.xml', encoding='utf-8', xml_declaration=True)
