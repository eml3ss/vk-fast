#!/usr/bin/env python3
"""Small Linux packages: application, using distribution GUI and audio libraries."""
import argparse
import gzip
import hashlib
import os
import re
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
APPIMAGETOOL_SHA256 = '62aebc77c2149e96b593b94da9bbd1881e0984a56399ff6d7a03b7ddd7c9ce68'


def appimagetool():
    cache = Path.home() / '.cache/vk-fast-linux'
    cache.mkdir(parents=True, exist_ok=True)
    path = cache / ('appimagetool-' + APPIMAGETOOL_SHA256[:12] + '.AppImage')
    if not path.is_file():
        with urllib.request.urlopen('https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage', timeout=60) as response:
            data = response.read(32 * 1024 * 1024)
        if hashlib.sha256(data).hexdigest() != APPIMAGETOOL_SHA256:
            raise SystemExit('AppImage tool checksum mismatch')
        path.write_bytes(data)
    if hashlib.sha256(path.read_bytes()).hexdigest() != APPIMAGETOOL_SHA256:
        raise SystemExit('Cached AppImage tool checksum mismatch')
    path.chmod(0o755)
    return path


def notices(appdir, work, version):
    rust = work / 'Rust-notices.txt'
    subprocess.run(['python3', str(ROOT / 'scripts/collect-licenses.py'), '--target', 'x86_64-unknown-linux-gnu', '--output', str(rust)], cwd=ROOT, check=True)
    content = [f'VK Fast {version}\nCorresponding source and build recipes: https://github.com/eml3ss/vk-fast\n',
               'WebKitGTK, GTK, PipeWire, fontconfig and their dependencies are provided by the distribution, not redistributed in this package.\n']
    files = [ROOT / 'LICENSE', ROOT / 'assets/fonts/OFL.txt', rust]
    files += sorted((ROOT / 'docs/third-party/linux').glob('AppImage*-LICENSE.txt'))
    for path in files:
        if path.is_file():
            content += ['\n--- ' + path.name + ' ---\n', path.read_text(errors='replace')]
    destination = appdir / 'usr/share/doc/vk-fast'
    destination.mkdir(parents=True)
    with (destination / 'LICENSES.txt.gz').open('wb') as raw:
        with gzip.GzipFile(fileobj=raw, mode='wb', mtime=0) as archive:
            archive.write(''.join(content).encode())
    shutil.copy2(rust, ROOT / 'docs/third-party/linux/Rust-notices.txt')


def package(executable, output):
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['package']['version']
    name = f'VK-Fast-{version}-linux-x86_64'
    for path in [executable]:
        if not path.is_file():
            raise SystemExit(f'Build output missing: {path}')
    symbols = subprocess.check_output(['readelf', '--version-info', str(executable)], text=True)
    baseline = max([(2, 39)] + [tuple(map(int, item)) for item in re.findall(r'GLIBC_(\d+)\.(\d+)', symbols)])
    glibc = '.'.join(map(str, baseline))
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.vk-fast-linux-', dir=output) as directory:
        work = Path(directory)
        appdir = work / name
        binary_dir = appdir / 'usr/bin'
        binary_dir.mkdir(parents=True)
        for path in [executable]:
            target = binary_dir / path.name
            shutil.copy2(path, target)
            subprocess.run(['strip', '--strip-unneeded', str(target)], check=True)
            target.chmod(0o755)
        shutil.copy2(ROOT / 'scripts/linux/AppRun', appdir / 'AppRun')
        (appdir / 'AppRun').write_text((appdir / 'AppRun').read_text().replace('glibc 2.39+', f'glibc {glibc}+'))
        (appdir / 'AppRun').chmod(0o755)
        (appdir / 'vk-fast').symlink_to('AppRun')
        shutil.copy2(ROOT / 'assets/vk-fast-icon-512.png', appdir / 'vk-fast.png')
        (appdir / '.DirIcon').symlink_to('vk-fast.png')
        (appdir / 'vk-fast.desktop').write_text('[Desktop Entry]\nType=Application\nName=VK Fast\nExec=vk-fast\nIcon=vk-fast\nCategories=AudioVideo;Audio;Music;\nTerminal=false\n')
        (appdir / 'README.txt').write_text('VK Fast ' + version + '\n\nRun ./vk-fast (or AppRun).\nRequires distribution WebKitGTK 4.1 / GTK 3, PipeWire, fontconfig and glibc ' + glibc + '+.\nNo GUI libraries, WebKit engine, system modules or source archives are bundled.\nUser data follows XDG paths and is never stored inside this package.\nNotices: usr/share/doc/vk-fast/LICENSES.txt.gz\nhttps://github.com/eml3ss/vk-fast\n')
        notices(appdir, work, version)
        image = work / (name + '.AppImage')
        env = os.environ.copy()
        env.pop('LD_PRELOAD', None)
        env.update(ARCH='x86_64', VERSION=version, APPIMAGE_EXTRACT_AND_RUN='1')
        subprocess.run([str(appimagetool()), '--appimage-extract-and-run', str(appdir), str(image)], env=env, check=True)
        image.chmod(0o755)
        archive_path = work / (name + '.tar.gz')
        with tarfile.open(archive_path, 'w:gz', compresslevel=9) as archive:
            def normalized(info):
                info.uid = info.gid = 0
                info.uname = info.gname = ''
                return info
            archive.add(appdir, arcname=name, filter=normalized)
        for path in [image, archive_path]:
            os.replace(path, output / path.name)
            print(output / path.name)
            print(f'{(output / path.name).stat().st_size / 1024 / 1024:.1f} MiB')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--executable', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=ROOT / 'dist')
    args = parser.parse_args()
    package(args.executable.resolve(), args.output.resolve())
