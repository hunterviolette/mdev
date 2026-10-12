import os
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request
from pathlib import Path

root = Path(__file__).resolve().parent.parent
version = os.environ['GITHUB_REF_NAME'].removeprefix('v')
node_version = os.environ['NODE_VERSION']
output = root / 'release-output'
output.mkdir(exist_ok=True)

with tempfile.TemporaryDirectory() as temporary:
    temporary = Path(temporary)
    package = temporary / 'package'
    application = package / 'opt' / 'mdev-server'
    runtime = application / 'runtime'
    control = package / 'DEBIAN'
    (application / 'bridge').mkdir(parents=True)
    (application / 'adt-bridge').mkdir(parents=True)
    (application / 'web').mkdir(parents=True)
    runtime.mkdir(parents=True)
    control.mkdir(parents=True)
    (package / 'etc' / 'mdev').mkdir(parents=True)
    (package / 'lib' / 'systemd' / 'system').mkdir(parents=True)

    shutil.copy2(root / 'api' / 'target' / 'release' / 'workflow-api', runtime / 'mdev-api')
    shutil.copytree(root / 'bridge' / 'dist', application / 'bridge' / 'dist')
    shutil.copytree(root / 'adt-bridge' / 'dist', application / 'adt-bridge' / 'dist')
    shutil.copytree(root / 'web' / 'dist', application / 'web' / 'dist')
    shutil.copytree(root / 'node_modules', application / 'node_modules', symlinks=True)

    archive = temporary / 'node.tar.gz'
    url = f'https://nodejs.org/dist/v{node_version}/node-v{node_version}-linux-x64.tar.gz'
    urllib.request.urlretrieve(url, archive)
    with tarfile.open(archive, 'r:gz') as package_archive:
        source = package_archive.extractfile(f'node-v{node_version}-linux-x64/bin/node')
        if source is None:
            raise RuntimeError('Node executable missing')
        with source, (runtime / 'node').open('wb') as destination:
            shutil.copyfileobj(source, destination)
    for executable in (runtime / 'node', runtime / 'mdev-api'):
        executable.chmod(0o755)

    (control / 'control').write_text(f'Package: mdev-server\nVersion: {version}\nSection: utils\nPriority: optional\nArchitecture: amd64\nMaintainer: mdev release automation\nDepends: libc6, libstdc++6, ca-certificates, adduser, systemd\nDescription: mdev headless server with web interface\n', encoding='utf-8')
    (control / 'conffiles').write_text('/etc/mdev/mdev.env\n', encoding='utf-8')
    (package / 'etc' / 'mdev' / 'mdev.env').write_text('WORKFLOW_API_HOST=127.0.0.1\nWORKFLOW_API_PORT=8788\nMDEV_DATA_DIR=/var/lib/mdev\nMDEV_BROWSER_USER_DATA_DIR=/var/lib/mdev/browser-profile\nMDEV_NO_BROWSER_OPEN=1\n', encoding='utf-8')
    (package / 'lib' / 'systemd' / 'system' / 'mdev.service').write_text('[Unit]\nDescription=mdev headless server\nWants=network-online.target\nAfter=network-online.target\n\n[Service]\nType=simple\nUser=mdev\nGroup=mdev\nWorkingDirectory=/opt/mdev-server\nEnvironmentFile=/etc/mdev/mdev.env\nEnvironment=PATH=/opt/mdev-server/runtime:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin\nExecStart=/opt/mdev-server/runtime/mdev-api\nRestart=on-failure\nRestartSec=5\nKillSignal=SIGINT\nTimeoutStopSec=60\nStateDirectory=mdev\nUMask=0077\nNoNewPrivileges=true\nProtectSystem=strict\nProtectHome=true\nReadWritePaths=/var/lib/mdev\n\n[Install]\nWantedBy=multi-user.target\n', encoding='utf-8')
    (control / 'postinst').write_text('#!/bin/sh\nset -e\nif ! getent group mdev >/dev/null; then addgroup --system mdev; fi\nif ! id -u mdev >/dev/null 2>&1; then adduser --system --ingroup mdev --home /var/lib/mdev --no-create-home mdev; fi\ninstall -d -o mdev -g mdev -m 0700 /var/lib/mdev\nsystemctl daemon-reload || true\nif [ "$1" = configure ]; then systemctl enable mdev.service || true; systemctl restart mdev.service || true; fi\n', encoding='utf-8')
    (control / 'prerm').write_text('#!/bin/sh\nset -e\nif [ "$1" = remove ] || [ "$1" = deconfigure ]; then systemctl stop mdev.service || true; systemctl disable mdev.service || true; fi\n', encoding='utf-8')
    (control / 'postrm').write_text('#!/bin/sh\nset -e\nsystemctl daemon-reload || true\n', encoding='utf-8')
    for filename in ('postinst', 'prerm', 'postrm'):
        (control / filename).chmod(0o755)

    artifact = output / 'mdev-server-linux-amd64.deb'
    subprocess.run(['dpkg-deb', '--build', '--root-owner-group', str(package), str(artifact)], check=True)
    subprocess.run(['dpkg-deb', '--info', str(artifact)], check=True)
