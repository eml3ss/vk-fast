"""Build the NSIS installer with a pinned, user-local compiler (Wine on Linux)."""
import hashlib
import os
from pathlib import Path
import shutil
import struct
import subprocess
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parent.parent
NSIS_VERSION = "3.13"
ARCHIVE_SHA256 = "ba63dffc4410ee89193e1cb5a41989991bd77c61068da17e3156d136b7b0b3d8"
SOURCE_SHA256 = "a8ffe024602d46b6d766f9e1ce30c324ad2a24daeacd3efc2642d436a0c157ac"


def download(cache, name, expected):
    path = cache / name
    if not path.is_file():
        url = f"https://downloads.sourceforge.net/project/nsis/NSIS%203/{NSIS_VERSION}/{name}"
        with urllib.request.urlopen(url, timeout=60) as response:
            data = response.read(16 * 1024 * 1024)
        if hashlib.sha256(data).hexdigest() != expected:
            raise SystemExit(f"NSIS download checksum mismatch: {name}")
        temporary = path.with_suffix(path.suffix + ".tmp")
        temporary.write_bytes(data)
        os.replace(temporary, path)
    if hashlib.sha256(path.read_bytes()).hexdigest() != expected:
        raise SystemExit(f"NSIS cache checksum mismatch: {path}")
    return path


def compiler():
    override = os.environ.get("VK_FAST_BUILD_TOOLS")
    cache = Path(override).resolve() if override else Path.home() / ".cache/vk-fast-build-tools"
    cache.mkdir(parents=True, exist_ok=True)
    archive = download(cache, f"nsis-{NSIS_VERSION}.zip", ARCHIVE_SHA256)
    source = download(cache, f"nsis-{NSIS_VERSION}-src.tar.bz2", SOURCE_SHA256)
    directory = cache / f"nsis-{NSIS_VERSION}"
    with zipfile.ZipFile(archive) as files:
        for item in files.infolist():
            target = cache / item.filename
            if not target.resolve().is_relative_to(directory.resolve()) or (item.external_attr >> 16) & 0o170000 == 0o120000:
                raise SystemExit("Unsafe path in NSIS compiler archive")
        files.extractall(cache)
    executable = directory / "makensis.exe"
    env = os.environ.copy()
    if os.name == "nt":
        command = [str(executable)]
        encode_path = lambda path: str(path.resolve())
    else:
        wine = shutil.which("wine")
        if not wine:
            raise SystemExit("Wine is required to run the pinned NSIS compiler on Linux.")
        prefix = Path(env.get("VK_FAST_NSIS_WINEPREFIX", str(cache / "nsis-wine"))).resolve()
        prefix.parent.mkdir(parents=True, exist_ok=True)
        env.update(WINEPREFIX=str(prefix), WINEARCH="win64", WINEDEBUG="-all", WINEDLLOVERRIDES="winemenubuilder.exe=d", DISPLAY="")
        command = [wine, str(executable)]
        encode_path = lambda path: "Z:" + str(path.resolve()).replace("/", "\\")
    return command, env, encode_path, directory / "COPYING", source


def require_x64(path):
    data = path.read_bytes()
    if data[:2] != b"MZ" or len(data) < 64:
        raise SystemExit(f"Not a Windows executable: {path}")
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    if data[pe:pe + 4] != b"PE\0\0" or struct.unpack_from("<H", data, pe + 4)[0] != 0x8664:
        raise SystemExit(f"Expected Windows x86-64: {path}")


def build(destination, executable, version):
    for path in [executable]:
        require_x64(path)
    command, env, encode, license_path, source = compiler()
    defines = {
        "OUTPUT_FILE": encode(destination), "APP_EXE": encode(executable),
        "APP_ICON": encode(ROOT / "assets/vk-fast-icon.ico"),
        "APP_VERSION": version, "APP_SIZE_KB": str(executable.stat().st_size // 1024 + 256),
    }
    if any(any(character in value for character in ['"', '\n', '\r', '$']) for value in defines.values()):
        raise SystemExit("Unsupported character in NSIS compiler input")
    subprocess.run(command + ["/V3", "/WX", "/NOCONFIG", "/NOCD", "/INPUTCHARSET", "UTF8"] + [f"/D{key}={value}" for key, value in defines.items()]
        + [encode(ROOT / "scripts/windows-installer/vk-fast.nsi")], env=env, cwd=ROOT, check=True)
    if not destination.is_file() or destination.stat().st_size < 1024:
        raise SystemExit("NSIS did not produce the installer")
    return license_path, source
