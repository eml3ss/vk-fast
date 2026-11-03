#!/usr/bin/env python3
"""Package the portable runtime, optional NSIS installer and source/license companion."""
import argparse
from contextlib import contextmanager
import hashlib
import os
from pathlib import Path
import shutil
import tempfile
import tomllib
import zipfile
import windows_installer

ROOT = Path(__file__).resolve().parent.parent


def add_directory(archive, folder, prefix):
    for path in sorted(folder.rglob("*")):
        relative = path.relative_to(folder)
        if path.is_file() and not {"__pycache__", ".git", "target"}.intersection(relative.parts):
            archive.write(path, (Path(prefix) / path.relative_to(folder)).as_posix())


@contextmanager
def atomic_archive(destination):
    """Replace a completed archive on the same filesystem; preserve it on error."""
    with tempfile.NamedTemporaryFile(
        prefix="." + destination.stem + "-", suffix=".tmp", dir=destination.parent, delete=False,
    ) as temporary:
        path = Path(temporary.name)
    try:
        with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
            yield archive
        with path.open("r+b") as complete:
            os.fsync(complete.fileno())
        os.replace(path, destination)
    finally:
        path.unlink(missing_ok=True)


def package(output, with_installer=False):
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    name = f"VK-Fast-{version}-windows-x86_64"
    executable = ROOT / "target/x86_64-pc-windows-msvc/release/vk-fast.exe"
    decoder = ROOT / "target/windows-ffmpeg"
    notices = ROOT / "target/windows-third-party-licenses.txt"
    for required in [executable, decoder / "bin/ffmpeg.exe", decoder / "source/ffmpeg-9.0.2.tar.xz", notices]:
        if not required.is_file():
            raise SystemExit(f"Build output missing: {required}")
    output.mkdir(parents=True, exist_ok=True)
    destination = output / f"{name}.zip"
    companion_name = name + "-source-and-licenses"
    companion_destination = output / f"{companion_name}.zip"
    installer_destination = output / f"{name}-setup.exe"
    with tempfile.TemporaryDirectory(prefix="vk-fast-package-") as temporary:
        installer = Path(temporary) / installer_destination.name
        installer_notices = None
        if with_installer:
            installer_notices = windows_installer.build(installer, executable, decoder / "bin/ffmpeg.exe", version)
        folder = Path(temporary) / companion_name
        folder.mkdir()
        for directory in ["licenses", "source"]:
            shutil.copytree(decoder / directory, folder / directory)
        shutil.copy2(ROOT / "LICENSE", folder / "licenses/VK-Fast-GPL-3.0.txt")
        shutil.copy2(ROOT / "assets/fonts/OFL.txt", folder / "licenses/Inter-OFL.txt")
        shutil.copy2(notices, folder / "licenses/Rust-dependencies.txt")
        if installer_notices:
            license_path, compiler_source = installer_notices
            shutil.copy2(license_path, folder / "licenses/NSIS.txt")
            shutil.copy2(compiler_source, folder / "source" / compiler_source.name)
        source = folder / "source/vk-fast-source.zip"
        with zipfile.ZipFile(source, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
            for filename in ["Cargo.toml", "Cargo.lock", "build.rs", "tauri.conf.json", "LICENSE", "README.md", "AGENTS.md", ".gitignore", ".gitattributes"]:
                archive.write(ROOT / filename, filename)
            for directory in ["src", "assets", "webview-assets", "docs", "scripts", ".cargo", "vendor"]:
                add_directory(archive, ROOT / directory, directory)
        (folder / "README.txt").write_text(
            "VK Fast — Windows x86-64\n\n"
            + (f"Установщик: {installer_destination.name}.\n"
               "Выбор папки приложения и раздельный выбор хранения config/cache:\n"
               "папка установки либо %LOCALAPPDATA%\\VK Fast.\n"
               "Выбор хранится в vk-fast-storage.ini рядом с приложением.\n"
               "При обновлении он сохраняется. Удаление программы не удаляет пользовательские данные.\n"
               "При смене расположения существующие данные автоматически не переносятся.\n\n" if with_installer else "") +
            f"Рабочий архив приложения: {name}.zip.\n"
            "Распакуйте его целиком и запустите vk-fast.exe.\n"
            "Рабочий архив содержит только vk-fast.exe, bin/ffmpeg.exe и пустые config/cache.\n"
            "config — настройки, вход, история и сохранённая позиция.\n"
            "cache — аудио, обложки, аватары и библиотека.\n"
            "В portable-архиве данные по умолчанию хранятся рядом с приложением независимо от рабочего каталога.\n"
            "bin/ffmpeg.exe — минимальный аудиодекодер со встроенным Mbed TLS; полный FFmpeg устанавливать не нужно.\n\n"
            "Для окна входа VK нужен Microsoft Edge WebView2 Runtime. Он обычно уже установлен;\n"
            "если отсутствует: https://developer.microsoft.com/microsoft-edge/webview2/\n"
            "QR-вход не использует WebView2.\n"
            "Храните приложение в доступной для записи папке.\n\n"
            "Этот отдельный архив содержит исходники и рецепты в source/, лицензии в licenses/.\n"
            "SHA256SUMS.txt включает контрольные суммы обоих исполняемых файлов и сопровождающих файлов.\n"
            "Версия для Windows собрана на Linux; через Wine проверены командный запуск, декодирование и переносимые настройки.\n"
            "Графическая проверка в этой среде Wine ограничена недоступным OpenGL-контекстом.\n"
            "Полный интерфейс, вход, трей, SMTC и WASAPI требуют проверки на настоящей Windows.\n",
            encoding="utf-8-sig",
        )
        hashes = [
            f"{hashlib.sha256(executable.read_bytes()).hexdigest()}  runtime/vk-fast.exe",
            f"{hashlib.sha256((decoder / 'bin/ffmpeg.exe').read_bytes()).hexdigest()}  runtime/bin/ffmpeg.exe",
        ]
        if with_installer:
            hashes.append(f"{hashlib.sha256(installer.read_bytes()).hexdigest()}  runtime/{installer_destination.name}")
        for path in sorted(folder.rglob("*")):
            if path.is_file():
                hashes.append(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.relative_to(folder).as_posix()}")
        (folder / "SHA256SUMS.txt").write_text("\n".join(hashes) + "\n", encoding="utf-8")
        # Publish the companion first, then replace the runtime only after all
        # source/notice work succeeded. Each individual archive is atomic.
        with atomic_archive(companion_destination) as archive:
            add_directory(archive, folder, companion_name)
        with atomic_archive(destination) as archive:
            archive.write(executable, name + "/vk-fast.exe")
            archive.write(decoder / "bin/ffmpeg.exe", name + "/bin/ffmpeg.exe")
            for directory in ["config", "cache"]:
                archive.writestr(name + "/" + directory + "/", "")
        if with_installer:
            handle, temporary_name = tempfile.mkstemp(prefix=".vk-fast-setup-", suffix=".tmp", dir=output)
            published = Path(temporary_name)
            try:
                with os.fdopen(handle, "wb") as file:
                    with installer.open("rb") as built:
                        shutil.copyfileobj(built, file)
                    file.flush()
                    os.fsync(file.fileno())
                os.replace(published, installer_destination)
            finally:
                published.unlink(missing_ok=True)
    for path in [destination, companion_destination] + ([installer_destination] if with_installer else []):
        print(path)
        print(f"{path.stat().st_size / (1024 * 1024):.1f} MiB")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=ROOT / "dist")
    parser.add_argument("--installer", action="store_true", help="Also build the NSIS installer")
    args = parser.parse_args()
    package(args.output.resolve(), args.installer)
