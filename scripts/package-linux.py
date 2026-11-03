#!/usr/bin/env python3
"""Finish one relocatable AppDir and produce AppImage + tar.gz, without user data."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
APPIMAGETOOL_SHA256 = "62aebc77c2149e96b593b94da9bbd1881e0984a56399ff6d7a03b7ddd7c9ce68"


def third_party(appdir):
    destination = appdir / "usr/share/doc/vk-fast"
    destination.mkdir(parents=True, exist_ok=True)
    shutil.copy2(ROOT / "LICENSE", destination / "LICENSE")
    shutil.copy2(ROOT / "assets/fonts/OFL.txt", destination / "Inter-OFL.txt")
    for notice in (ROOT / "docs/third-party/linux").glob("AppImage*-LICENSE.txt"):
        shutil.copy2(notice, destination / notice.name)
    subprocess.run(["python3", str(ROOT / "scripts/collect-licenses.py"), "--target", "x86_64-unknown-linux-gnu",
                    "--output", str(destination / "Rust-notices.txt")], cwd=ROOT, check=True)
    basenames = {path.name for path in appdir.rglob("*") if path.is_file()
                 and not path.relative_to(appdir).as_posix().startswith(("usr/share/doc/", "usr/share/licenses/"))}
    owned = subprocess.run(["dpkg-query", "-S", "*"], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, check=True).stdout
    packages = set()
    for line in owned.splitlines():
        if ": /" not in line:
            continue
        package, path = line.split(": /", 1)
        if Path(path).name in basenames:
            packages.update(value.strip() for value in package.split(","))
    records = ["Package\tBinary version\tSource\tSource version\tCorresponding source"]
    licenses = destination / "ubuntu-copyright"
    licenses.mkdir(exist_ok=True)
    for package in sorted(packages):
        record = subprocess.check_output(["dpkg-query", "-W", "-f=${Package}\t${Version}\t${source:Package}\t${source:Version}", package], text=True)
        name, version, source, source_version = record.split("\t")
        link = "https://launchpad.net/ubuntu/+source/" + urllib.parse.quote(source, safe="") + "/" + urllib.parse.quote(source_version, safe="")
        records.append(record + "\t" + link)
        copyright_file = Path("/usr/share/doc") / name / "copyright"
        if copyright_file.is_file():
            shutil.copyfile(copyright_file, licenses / (package.replace(":", "_") + ".txt"))
    (destination / "Ubuntu-sources.tsv").write_text("\n".join(records) + "\n")
    shutil.copytree("/usr/share/common-licenses", destination / "common-licenses", dirs_exist_ok=True)
    (destination / "BUILDINFO.txt").write_text(
        "VK Fast 0.1.0 — Linux x86-64\nBaseline: Ubuntu 24.04, glibc 2.39\n"
        "Main source: https://github.com/eml3ss/vk-fast/tree/v0.1.0\n"
        "FFmpeg recipe: scripts/build-ffmpeg-linux.sh; Mozilla trust patch: scripts/ffmpeg-tls\n"
        "GTK deployment: tauri-cli 2.12.4, MIT/Apache-2.0\n"
        "AppRun: https://github.com/tauri-apps/binary-releases/releases/tag/apprun-old\n"
        "AppImage runtime/tool sources: https://github.com/AppImage/type2-runtime and https://github.com/AppImage/appimagetool\n"
    )


def appimagetool():
    cache = Path.home() / ".cache/vk-fast-linux"
    cache.mkdir(parents=True, exist_ok=True)
    path = cache / ("appimagetool-" + APPIMAGETOOL_SHA256[:12] + ".AppImage")
    if not path.is_file():
        with urllib.request.urlopen("https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage", timeout=60) as response:
            data = response.read(32 * 1024 * 1024)
        if hashlib.sha256(data).hexdigest() != APPIMAGETOOL_SHA256:
            raise SystemExit("AppImage tool checksum mismatch")
        path.write_bytes(data)
    if hashlib.sha256(path.read_bytes()).hexdigest() != APPIMAGETOOL_SHA256:
        raise SystemExit("Cached AppImage tool checksum mismatch")
    path.chmod(0o755)
    return path


def package(appdir, output):
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    name = f"VK-Fast-{version}-linux-x86_64"
    output.mkdir(parents=True, exist_ok=True)
    if not (appdir / "AppRun.runtime").exists():
        (appdir / "AppRun").rename(appdir / "AppRun.runtime")
    shutil.copy2(ROOT / "scripts/linux/AppRun", appdir / "AppRun")
    (appdir / "AppRun").chmod(0o755)
    (appdir / "vk-fast").unlink(missing_ok=True)
    (appdir / "vk-fast").symlink_to("AppRun")
    third_party(appdir)
    for required in ["usr/bin/vk-fast", "usr/bin/ffmpeg", "usr/lib/libpipewire-0.3.so.0", "usr/share/pipewire/client.conf",
                     "usr/lib/pipewire-0.3/libpipewire-module-protocol-native.so"]:
        if not (appdir / required).is_file():
            raise SystemExit(f"Missing bundle resource: {required}")
    with tempfile.TemporaryDirectory(prefix=".vk-fast-linux-", dir=output) as temporary:
        temporary = Path(temporary)
        image = temporary / (name + ".AppImage")
        env = os.environ.copy()
        env.update(ARCH="x86_64", VERSION=version, APPIMAGE_EXTRACT_AND_RUN="1")
        subprocess.run([str(appimagetool()), "--appimage-extract-and-run", str(appdir), str(image)], env=env, check=True)
        image.chmod(0o755)
        archive = temporary / (name + ".tar.gz")
        def normalize(info):
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            if info.isfile(): info.mode = 0o755 if info.mode & 0o111 else 0o644
            elif info.isdir(): info.mode = 0o755
            return info
        with tarfile.open(archive, "w:gz", compresslevel=9) as tar:
            tar.add(appdir, arcname=name, filter=normalize)
        for path in [image, archive]:
            os.replace(path, output / path.name)
            print(output / path.name)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--appdir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    package(args.appdir.resolve(), args.output.resolve())
