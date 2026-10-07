#!/usr/bin/env python3
"""Configure/verify native privacy resources and distinct app executable UUIDs.

Run before signing; --verify is read-only and safe on final distribution bundles.
Apple TN3179 and TN3178 document the usage description and identity requirements.
"""
import argparse
import json
from pathlib import Path
import plistlib
import re
import subprocess

KEY = 'NSLocalNetworkUsageDescription'
DESCRIPTIONS = {
    'en': 'Kiln connects to local servers for terminal commands, Kubernetes, SSH, databases and remote files.',
    'ko': 'Kiln에서 터미널 명령, Kubernetes, SSH, 데이터베이스 및 원격 파일을 사용할 때 로컬 서버에 연결합니다.',
    'ja': 'Kiln はターミナルコマンド、Kubernetes、SSH、データベース、リモートファイルの利用時にローカルサーバーに接続します。',
    'zh-Hans': 'Kiln 在使用终端命令、Kubernetes、SSH、数据库和远程文件时连接本地服务器。',
}


def bundles(app):
    return [app, app / 'Contents/Library/Kiln Status.app']


def uuids(binary):
    output = subprocess.check_output(['dwarfdump', '--uuid', str(binary)], text=True)
    result = {}
    for value, arch in re.findall(r'^UUID: ([0-9A-Fa-f-]{36}) \(([^)]+)\)', output, re.M):
        if arch in result:
            raise ValueError('Duplicate UUID architecture in executable')
        result[arch] = value.upper()
    if not result:
        raise ValueError('App executable has no Mach-O build UUID')
    return result


def check_identity(app):
    identities = []
    executables = []
    for bundle in bundles(app):
        info = plistlib.loads((bundle / 'Contents/Info.plist').read_bytes())
        identities.append(info['CFBundleIdentifier'])
        name = info['CFBundleExecutable']
        if Path(name).name != name:
            raise ValueError('Bundle executable must be a file name')
        executables.append(uuids(bundle / 'Contents/MacOS' / name))
    if identities != ['dev.kiln.app', 'dev.kiln.statusbar']:
        raise ValueError('Production app identities do not match the release contract')
    if executables[0].keys() != executables[1].keys():
        raise ValueError('GUI and companion executable architectures differ')
    if set(executables[0].values()) & set(executables[1].values()):
        raise ValueError('GUI and companion must have distinct Mach-O build UUIDs (TN3178)')
    return dict(zip(identities, executables))


def strings(description):
    return f'"{KEY}" = {json.dumps(description, ensure_ascii=False)};\n'


def configure(app):
    # Refuse an ambiguous identity before modifying resources or signing.
    check_identity(app)
    for bundle in bundles(app):
        path = bundle / 'Contents/Info.plist'
        info = plistlib.loads(path.read_bytes())
        info.update({KEY: DESCRIPTIONS['en'], 'CFBundleDevelopmentRegion': 'en',
                     'CFBundleLocalizations': list(DESCRIPTIONS)})
        path.write_bytes(plistlib.dumps(info))
        for language, description in DESCRIPTIONS.items():
            directory = bundle / 'Contents/Resources' / f'{language}.lproj'
            directory.mkdir(parents=True, exist_ok=True)
            (directory / 'InfoPlist.strings').write_text(strings(description), encoding='utf-8')


def verify(app):
    identity = check_identity(app)
    for bundle in bundles(app):
        info = plistlib.loads((bundle / 'Contents/Info.plist').read_bytes())
        if info.get(KEY) != DESCRIPTIONS['en']:
            raise ValueError('Missing or mismatched local-network usage description')
        if info.get('CFBundleDevelopmentRegion') != 'en' or info.get('CFBundleLocalizations') != list(DESCRIPTIONS):
            raise ValueError('Native privacy localization metadata does not match')
        if 'NSBonjourServices' in info:
            raise ValueError('Do not declare unused Bonjour discovery services')
        for language, description in DESCRIPTIONS.items():
            path = bundle / 'Contents/Resources' / f'{language}.lproj/InfoPlist.strings'
            if path.read_text(encoding='utf-8') != strings(description):
                raise ValueError(f'Missing or mismatched native privacy translation: {language}')
    return identity


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('app', type=Path)
    parser.add_argument('--verify', action='store_true')
    args = parser.parse_args()
    if not args.verify:
        configure(args.app)
    print(json.dumps(verify(args.app), sort_keys=True))


if __name__ == '__main__':
    main()
