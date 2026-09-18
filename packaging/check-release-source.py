#!/usr/bin/env python3
"""Fail closed unless a release uses merged main code and consistent versions."""
import json
import os
from pathlib import Path
import re
import subprocess
import tomllib


def validate(root: Path, ref: str) -> None:
    def git(*args):
        return subprocess.check_output(['git', '-C', str(root), *args], text=True).strip()

    head = git('rev-parse', 'HEAD')
    main = git('rev-parse', 'origin/main')
    subprocess.run(['git', '-C', str(root), 'merge-base', '--is-ancestor', head, main], check=True)
    version = tomllib.loads((root / 'Cargo.toml').read_text())['workspace']['package']['version']
    if not re.fullmatch(r'\d+\.\d+\.\d+', version):
        raise ValueError('Expected a stable workspace version')
    for file in ['desktop/package.json', 'desktop/src-tauri/tauri.conf.json']:
        if json.loads((root / file).read_text())['version'] != version:
            raise ValueError(f'Version mismatch: {file}')
    if tomllib.loads((root / 'desktop/src-tauri/Cargo.toml').read_text())['package']['version'] != version:
        raise ValueError('Desktop Cargo version differs from workspace')
    if ref.startswith('refs/tags/'):
        tag = ref.removeprefix('refs/tags/')
        if tag != f'v{version}' or git('rev-parse', f'{ref}^{{commit}}') != head:
            raise ValueError('Release tag must match the checked-out commit and version')
    elif ref != 'refs/heads/main':
        raise ValueError('Manual release validation may only run from main')
    print(f'Validated main source {head}, version {version}')


if __name__ == '__main__':
    validate(Path.cwd(), os.environ['GITHUB_REF'])
