#!/usr/bin/env python3
"""Collect dependency license texts for the binary distribution, not credentials."""
import json, pathlib, subprocess, sys
root=pathlib.Path(__file__).resolve().parent.parent
metadata=json.loads(subprocess.check_output(['cargo','metadata','--locked','--format-version','1'],cwd=root))
sections=[]
for package in sorted(metadata['packages'],key=lambda p:(p['name'],p['version'])):
    base=pathlib.Path(package['manifest_path']).parent
    if package['source'] is None and root/'vendor' not in base.parents: continue
    texts=[]
    for pattern in ('LICENSE*','LICENCE*','COPYING*','NOTICE*'):
        for path in sorted(base.glob(pattern)):
            if path.is_file():texts.append(f'{path.name}\n{path.read_text(errors="replace")}')
    declared=package.get('license_file')
    if declared and (base/declared).is_file():texts.append((base/declared).read_text(errors='replace'))
    sections.append(f'{package["name"]} {package["version"]}\nLicense: {package.get("license") or "See upstream"}\nSource: {package.get("repository") or package["source"]}\n\n'+ '\n\n'.join(texts))
for path in [root/'LICENSE',root/'THIRD_PARTY_NOTICES.md',
             root/'crates/kiln-common/assets/REMOTE-MARKS.md',
             *sorted((root/'crates/kiln-common/fonts').glob('*LICENSE*')),
             root/'crates/kiln-common/fonts/JetBrainsMono-OFL.txt',
             root/'crates/kiln-common/fonts/NotoSansCJK-OFL.txt',
             root/'crates/kiln-db/assets/logos/LICENSE-simple-icons.md',
             *sorted((root/'crates/kiln-common/assets').glob('*LICENSE*')),
             *sorted((root/'target/vendor/sparkle').glob('LICENSE*'))]:
    if path.is_file():sections.append(path.name+'\n'+path.read_text(errors='replace'))
destination=pathlib.Path(sys.argv[1]);destination.parent.mkdir(parents=True,exist_ok=True)
destination.write_text('\n\n'+'\n\n'+'\n\n'.join('\n'+'='*72+'\n'+s for s in sections))
