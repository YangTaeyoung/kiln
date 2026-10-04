#!/usr/bin/env python3
"""Sign Sparkle's executables and nested bundles from the inside out."""
import pathlib, subprocess, sys
framework = pathlib.Path(sys.argv[1])
identity = sys.argv[2]
def sign(path):
    subprocess.run(['codesign', '--force', '--options', 'runtime', '--timestamp',
        '--preserve-metadata=identifier,entitlements', '--sign', identity, str(path)], check=True)
magic = {b'\xfe\xed\xfa\xce', b'\xce\xfa\xed\xfe', b'\xfe\xed\xfa\xcf',
         b'\xcf\xfa\xed\xfe', b'\xca\xfe\xba\xbe', b'\xbe\xba\xfe\xca'}
paths = [p for p in framework.rglob('*') if not p.is_symlink()]
for p in paths:
    if p.is_file():
        with p.open('rb') as stream:
            if stream.read(4) in magic: sign(p)
for p in sorted(paths, key=lambda p:len(p.parts), reverse=True):
    if p.is_dir() and p.suffix in {'.app', '.xpc', '.framework'}: sign(p)
sign(framework)
