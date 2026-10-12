import json
import os
import platform
import shutil
import tarfile
import urllib.request
import zipfile
from pathlib import Path

root = Path(__file__).resolve().parent.parent
workspace_targets = {
    (root / workspace).resolve()
    for workspace in json.loads((root / 'package.json').read_text(encoding='utf-8'))['workspaces']
}

def ignore_workspace_links(directory, names):
    return [name for name in names if (Path(directory) / name).resolve() in workspace_targets]

stage = root / 'launcher' / 'src-tauri'
version = os.environ['GITHUB_REF_NAME'].removeprefix('v')
repository = os.environ['GITHUB_REPOSITORY']
public_key = os.environ['TAURI_SIGNING_PUBLIC_KEY'].strip()
node_version = os.environ['NODE_VERSION']
system = platform.system().lower()
machine = platform.machine().lower()
architecture = 'arm64' if machine in ('arm64', 'aarch64') else 'x64'
node_platform = {'windows': 'win', 'darwin': 'darwin', 'linux': 'linux'}[system]
node_name = f'node-v{node_version}-{node_platform}-{architecture}'
archive_ext = 'zip' if system == 'windows' else 'tar.gz'
archive = stage / f'{node_name}.{archive_ext}'
url = f'https://nodejs.org/dist/v{node_version}/{archive.name}'

if not public_key or 'REPLACE_WITH' in public_key:
    raise SystemExit('Missing valid TAURI_SIGNING_PUBLIC_KEY')

for name in ('runtime', 'bridge', 'adt-bridge', 'web', 'node_modules'):
    path = stage / name
    if path.exists():
        shutil.rmtree(path)

(stage / 'runtime').mkdir(parents=True)
api_name = 'workflow-api.exe' if system == 'windows' else 'workflow-api'
api_target = 'mdev-api.exe' if system == 'windows' else 'mdev-api'
shutil.copy2(root / 'api' / 'target' / 'release' / api_name, stage / 'runtime' / api_target)
shutil.copytree(root / 'bridge' / 'dist', stage / 'bridge' / 'dist')
shutil.copytree(root / 'adt-bridge' / 'dist', stage / 'adt-bridge' / 'dist')
shutil.copytree(root / 'web' / 'dist', stage / 'web' / 'dist')
shutil.copytree(root / 'node_modules', stage / 'node_modules', symlinks=False, ignore=ignore_workspace_links)

urllib.request.urlretrieve(url, archive)
if system == 'windows':
    with zipfile.ZipFile(archive) as package:
        with package.open(f'{node_name}/node.exe') as source:
            with (stage / 'runtime' / 'node.exe').open('wb') as target:
                shutil.copyfileobj(source, target)
else:
    with tarfile.open(archive, 'r:gz') as package:
        source = package.extractfile(f'{node_name}/bin/node')
        if source is None:
            raise SystemExit('Node executable missing from runtime archive')
        with source, (stage / 'runtime' / 'node').open('wb') as target:
            shutil.copyfileobj(source, target)
    for name in ('mdev-api', 'node'):
        (stage / 'runtime' / name).chmod(0o755)
archive.unlink()

config = {
    'version': version,
    'plugins': {
        'updater': {
            'pubkey': public_key,
            'endpoints': [f'https://github.com/{repository}/releases/latest/download/latest.json']
        }
    }
}
(stage / 'tauri.release.conf.json').write_text(json.dumps(config, indent=2), encoding='utf-8')
