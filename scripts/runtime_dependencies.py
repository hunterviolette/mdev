import json
import shutil
import subprocess
import tempfile
from pathlib import Path

root = Path(__file__).resolve().parent.parent


def stage_runtime_dependencies(destination):
    manifest = json.loads((root / 'package.json').read_text(encoding='utf-8'))
    workspaces = manifest['workspaces']

    with tempfile.TemporaryDirectory() as directory:
        directory = Path(directory)
        shutil.copy2(root / 'package.json', directory / 'package.json')
        shutil.copy2(root / 'package-lock.json', directory / 'package-lock.json')

        for workspace in workspaces:
            source = root / workspace / 'package.json'
            target = directory / workspace / 'package.json'
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, target)

        subprocess.run([
            'npm', 'ci',
            '--omit=dev',
            '--include=optional',
            '--ignore-scripts',
            '--no-audit',
            '--no-fund',
            '--workspace=bridge',
            '--workspace=adt-bridge'
        ], cwd=directory, check=True)

        source_modules = directory / 'node_modules'
        if not source_modules.is_dir():
            raise RuntimeError('Production dependency installation did not produce node_modules')

        workspace_paths = {(directory / workspace).resolve() for workspace in workspaces}

        def ignore_workspace_links(path, names):
            return [
                name for name in names
                if (Path(path) / name).is_symlink()
                and (Path(path) / name).resolve() in workspace_paths
            ]

        if destination.exists():
            shutil.rmtree(destination)

        shutil.copytree(source_modules, destination, symlinks=False, ignore=ignore_workspace_links)
