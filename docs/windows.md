# Windows x86-64 portable

Архив нужно распаковать целиком, затем запустить `vk-fast.exe`. Вариант выпуска — только Windows x86-64 (MSVC). Установщик не нужен. Вывод звука — WASAPI через CPAL; системный микшер Windows преобразует 48 кГц float PCM под устройство.

Папки определяются по расположению `.exe`, независимо от рабочего каталога:

| Папка | Содержимое |
| --- | --- |
| `config` | настройки, вход VK, собственный профиль webview, история, последний трек и позиция |
| `cache/audio` | аудио, отдельный лимит до 5 ГиБ |
| `cache/library`, `cache/covers`, `cache/avatars` | данные библиотеки и маленькие изображения |
| `bin/ffmpeg.exe` | минимальный статический аудиодекодер с Mbed TLS и встроенными корневыми сертификатами |


Папка приложения должна быть доступна для записи. При входе через окно VK используется установленный [Microsoft Edge WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/). QR-вход не использует webview. Обычные профили браузеров приложение не читает.

## Сборка на Linux

Для основного приложения нужны официальный Rust 1.95+ с целью `x86_64-pc-windows-msvc`, Cargo, LLVM (`clang-cl`, `lld-link`, `llvm-lib`, `llvm-rc`), Python 3.11+ и [cargo-xwin](https://github.com/rust-cross/cargo-xwin) для загрузки MSVC CRT/Windows SDK. Используйте компилятор и стандартную библиотеку из одного официального toolchain.

Для первой сборки минимального FFmpeg нужны дополнительно Bash, curl, tar с xz/bzip2, make, patch и GCC хоста. Скрипт скачивает проверенные исходники и переносимый LLVM-MinGW без установки в систему. Готовый `target/windows-ffmpeg/bin/ffmpeg.exe` переиспользуется при совпадении маркера TLS-бэкенда; старый Schannel-кэш обновляется один раз. Принудительная пересборка включается через `VK_FAST_REBUILD_FFMPEG=1`. Другие архитектуры не собираются.

Пример подготовки с rustup:

```sh
rustup toolchain install stable --profile minimal --target x86_64-pc-windows-msvc
cargo install --locked cargo-xwin --version 0.23.1
export XWIN_ARCH=x86_64
export XWIN_CACHE_DIR="$HOME/.cache/vk-fast-xwin"
cargo xwin cache xwin
export VK_FAST_XWIN_ROOT="$XWIN_CACHE_DIR/xwin"
rustup run stable ./scripts/build-windows.sh --output dist
```

`scripts/windows-msvc.py` передаёт Cargo точные пути CRT/SDK и использует статический MSVC runtime. Он также работает, когда парсер конфигурации конкретной версии cargo-xwin несовместим с установленным Cargo. Если SDK уже подготовлен, повторно скачивать его не нужно. `VK_FAST_WINDOWS_SYSROOT` позволяет указать отдельный официальный Rust sysroot; `RUSTC`/`PATH` при этом должны выбирать соответствующий официальный компилятор.

Результат — два отдельных архива:

- `VK-Fast-0.1.0-windows-x86_64.zip` — только `vk-fast.exe`, `bin/ffmpeg.exe` и пустые `config`/`cache`.
- `VK-Fast-0.1.0-windows-x86_64-source-and-licenses.zip` — исходники VK Fast и выбранного FFmpeg, рецепты сборки, лицензии, README и контрольные суммы файлов.

Упаковщик использует явный список файлов; пользовательские `config`/`cache` не копируются. Каждый архив сначала полностью записывается во временный файл рядом с результатом, затем атомарно заменяет предыдущий. Подробнее: [компоненты FFmpeg](windows-ffmpeg.md).

Проверки кода и ограничения автоматической проверки описаны в [AGENTS.md](../AGENTS.md#проверки). Проверка декодера — в [документации FFmpeg](windows-ffmpeg.md#проверка-декодера).
