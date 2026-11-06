#!/usr/bin/env python3
"""Package the single-executable portable app and optional NSIS installer."""
import argparse
from contextlib import contextmanager
import os
from pathlib import Path
import shutil
import tempfile
import tomllib
import zipfile
import windows_installer

ROOT = Path(__file__).resolve().parent.parent


@contextmanager
def atomic_archive(destination):
    with tempfile.NamedTemporaryFile(prefix='.' + destination.stem + '-', suffix='.tmp', dir=destination.parent, delete=False) as temporary:
        path = Path(temporary.name)
    try:
        with zipfile.ZipFile(path, 'w', zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
            yield archive
        with path.open('r+b') as complete:
            os.fsync(complete.fileno())
        os.replace(path, destination)
    finally:
        path.unlink(missing_ok=True)


def package(output, with_installer=False):
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['package']['version']
    name = f'VK-Fast-{version}-windows-x86_64'
    target = Path(os.environ.get('CARGO_TARGET_DIR', str(ROOT / 'target')))
    if not target.is_absolute():
        target = ROOT / target
    executable = target / 'x86_64-pc-windows-msvc/release/vk-fast.exe'
    if not executable.is_file():
        raise SystemExit(f'Build output missing: {executable}')
    windows_installer.require_x64(executable)
    output.mkdir(parents=True, exist_ok=True)
    destination = output / (name + '.zip')
    installer_destination = output / (name + '-setup.exe')
    with tempfile.TemporaryDirectory(prefix='vk-fast-package-') as temporary:
        installer = Path(temporary) / installer_destination.name
        if with_installer:
            windows_installer.build(installer, executable, version)
        with atomic_archive(destination) as archive:
            archive.write(executable, 'vk-fast.exe')
        if with_installer:
            handle, temporary_name = tempfile.mkstemp(prefix='.vk-fast-setup-', suffix='.tmp', dir=output)
            published = Path(temporary_name)
            try:
                with os.fdopen(handle, 'wb') as file:
                    with installer.open('rb') as built:
                        shutil.copyfileobj(built, file)
                    file.flush()
                    os.fsync(file.fileno())
                os.replace(published, installer_destination)
            finally:
                published.unlink(missing_ok=True)
    for path in [destination] + ([installer_destination] if with_installer else []):
        print(path)
        print(f'{path.stat().st_size / 1024 / 1024:.1f} MiB')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=ROOT / 'dist')
    parser.add_argument('--installer', action='store_true')
    args = parser.parse_args()
    package(args.output.resolve(), args.installer)
