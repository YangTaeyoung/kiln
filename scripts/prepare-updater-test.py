#!/usr/bin/env python3
"""Prepare two isolated, signed native Sparkle fixtures; never publish these apps."""
import json
import pathlib
import plistlib
import re
import shutil
import socket
import subprocess
import sys
import xml.etree.ElementTree as ET

root = pathlib.Path(sys.argv[1]).resolve()
if root.parent != pathlib.Path('/private/tmp') or not root.name.startswith('kiln-updater-'):
    raise SystemExit('Use a dedicated /tmp/kiln-updater-* directory')
identity = sys.argv[2]
binary = pathlib.Path('target/release/kiln').resolve()
version = subprocess.check_output([binary, '--version'], text=True).strip()
if not version.endswith('-updater-test'):
    raise SystemExit('Build the updater-test feature with this root first')
compiled_root = pathlib.Path(subprocess.check_output([binary, 'updater-test-root'], text=True).strip())
if compiled_root != root:
    raise SystemExit('The compiled fixture root does not match the requested root; rebuild first')
config = json.loads(pathlib.Path('scripts/release-config.json').read_text())
with socket.socket() as listener:
    listener.bind(('127.0.0.1', 0))
    port = listener.getsockname()[1]
feed = f'http://127.0.0.1:{port}/appcast.xml'
for folder, number in [('installed', '0.0.1'), ('download', '0.0.2')]:
    app = root / folder / 'Kiln Updater Test.app'
    if app.exists():
        raise SystemExit(f'Fixture already exists: {app}; create a fresh test root')
    app.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(['ditto', 'target/release/Kiln.app', app], check=True)
    shutil.copy2(binary, app / 'Contents/MacOS/kiln')
    helper = app / 'Contents/Library/Kiln Status.app'
    shutil.copy2(binary, helper / 'Contents/MacOS/kiln-status')
    for bundle, bundle_id in [(app, 'dev.kiln.updater-test'), (helper, 'dev.kiln.updater-test.statusbar')]:
        info_file = bundle / 'Contents/Info.plist'
        info = plistlib.loads(info_file.read_bytes())
        info.update(CFBundleIdentifier=bundle_id, CFBundleVersion=number, CFBundleShortVersionString=number)
        if bundle == app:
            info.update(CFBundleName='Kiln Updater Test', CFBundleDisplayName='Kiln Updater Test',
                        SUFeedURL=feed, SUEnableAutomaticChecks=False,
                        NSAppTransportSecurity={'NSAllowsArbitraryLoads': True})
        info_file.write_bytes(plistlib.dumps(info))
    for bundle in [helper, app]:
        subprocess.run(['codesign', '--force', '--options', 'runtime', '--timestamp', '--sign', identity, bundle], check=True)
    subprocess.run(['codesign', '--verify', '--deep', '--strict', app], check=True)
archive = root / 'download/Kiln-updater-test-0.0.2.zip'
subprocess.run(['ditto', '-c', '-k', '--keepParent', root / 'download/Kiln Updater Test.app', archive], check=True)
signer = 'target/vendor/sparkle/bin/sign_update'
metadata = subprocess.check_output([signer, '--account', config['sparkle_key_account'], archive], text=True)
signature = re.search(r'edSignature="([^"]+)"', metadata)[1]
subprocess.run([signer, '--account', config['sparkle_key_account'], '--verify', archive, signature], check=True)
ns = 'http://www.andymatuschak.org/xml-namespaces/sparkle'
ET.register_namespace('sparkle', ns)
rss = ET.Element('rss', {'version': '2.0'})
channel = ET.SubElement(rss, 'channel')
ET.SubElement(channel, 'title').text = 'Kiln isolated update test'
item = ET.SubElement(channel, 'item')
ET.SubElement(item, 'title').text = 'Kiln isolated update 0.0.2'
ET.SubElement(item, f'{{{ns}}}version').text = '0.0.2'
ET.SubElement(item, f'{{{ns}}}shortVersionString').text = '0.0.2'
ET.SubElement(item, 'description').text = 'Local fixture only. Keep the shell and unsaved draft through the update flow.'
ET.SubElement(item, 'enclosure', {'url': f'http://127.0.0.1:{port}/{archive.name}',
    'length': str(archive.stat().st_size), 'type': 'application/octet-stream', f'{{{ns}}}edSignature': signature})
ET.ElementTree(rss).write(root / 'download/appcast.xml', encoding='utf-8', xml_declaration=True)
(root / 'port').write_text(str(port))
print(f'Fixtures ready at {root}. Serve download/ on 127.0.0.1:{port}; open installed/Kiln Updater Test.app.')
