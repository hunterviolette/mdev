import json
import os
import shutil
from datetime import datetime, timezone
from pathlib import Path

root = Path('release-artifacts')
output = Path('release-publish')
output.mkdir(exist_ok=True)
repository = os.environ['GITHUB_REPOSITORY']
tag = os.environ['GITHUB_REF_NAME']
version = tag.removeprefix('v')
base_url = f'https://github.com/{repository}/releases/download/{tag}'
platforms = {}

suffixes = {
    'windows-x64': ('windows-x86_64', ('.nsis.zip', '.exe')),
    'linux-x64': ('linux-x86_64', ('.AppImage',)),
    # 'macos-x64': ('darwin-x86_64', ('.app.tar.gz',)),
    'macos-arm64': ('darwin-aarch64', ('.app.tar.gz',))
}

for job, (platform, suffix) in suffixes.items():
    directory = root / job
    if not directory.is_dir():
        raise SystemExit(f'Missing release artifact directory: {directory}')
    files = list(directory.rglob('*'))
    signed = []
    for extension in suffix:
        candidates = [p for p in files if p.is_file() and p.name.endswith(extension)]
        matches = [p for p in candidates if Path(str(p) + '.sig').is_file()]
        if matches:
            signed = matches
            break
    if len(signed) != 1:
        raise SystemExit(f'Expected exactly one signed updater artifact for {platform}; found {len(signed)}')
    artifact = signed[0]
    signature = Path(str(artifact) + '.sig')
    published_name = f'{job}-{artifact.name}'
    shutil.copy2(artifact, output / published_name)
    shutil.copy2(signature, output / f'{published_name}.sig')
    platforms[platform] = {
        'url': f'{base_url}/{published_name}',
        'signature': signature.read_text(encoding='utf-8').strip()
    }
    if not platforms[platform]['signature']:
        raise SystemExit(f'Empty updater signature for {platform}')
    for file in files:
        if not file.is_file() or file == artifact or file == signature or file.name.endswith('.sig'):
            continue
        if file.name.endswith(('.dmg', '.deb', '.AppImage', '.exe', '.msi', '.tar.gz', '.nsis.zip')):
            shutil.copy2(file, output / f'{job}-{file.name}')

manifest = {
    'version': version,
    'pub_date': datetime.now(timezone.utc).isoformat(),
    'platforms': platforms
}
(output / 'latest.json').write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
