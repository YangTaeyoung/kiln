#!/usr/bin/env python3
"""Fetch a pinned official Sparkle binary; private signing keys are never files."""
import hashlib, json, pathlib, subprocess

root = pathlib.Path(__file__).resolve().parent.parent
config = json.loads((root / 'scripts/release-config.json').read_text())
vendor = root / 'target/vendor'
vendor.mkdir(parents=True, exist_ok=True)
archive = vendor / f'Sparkle-{config["sparkle_version"]}.tar.xz'
if not archive.exists():
    subprocess.run(['curl', '-fsSL', '--retry', '3',
        f'https://github.com/sparkle-project/Sparkle/releases/download/{config["sparkle_version"]}/{archive.name}',
        '-o', str(archive)], check=True)
if hashlib.sha256(archive.read_bytes()).hexdigest() != config['sparkle_sha256']:
    raise SystemExit('Sparkle archive checksum mismatch; refusing to use it')
destination = vendor / 'sparkle'
destination.mkdir(exist_ok=True)
# The pinned digest authenticates the archive; preserve its framework symlinks.
subprocess.run(['tar', '-xf', str(archive), '-C', str(destination)], check=True)
print(destination)
