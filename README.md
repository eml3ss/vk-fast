# VK Fast

<img src="assets/vk-fast-icon.png" width="80" height="80" align="right" alt="Логотип VK Fast">

Неофициальный настольный плеер VK Music для **Linux и Windows** на Rust и egui/eframe. Русский и английский интерфейс, тёмная тема и нативный вывод звука. Проект не связан с VK.

## Возможности

- Личная библиотека, VK Mix, недавние прослушивания и рекомендации.
- Поиск треков, исполнителей, альбомов и плейлистов с подсказками.
- Страницы исполнителей и релизов, создание и редактирование плейлистов, закрепление и изменение их порядка.
- Добавление треков в «Мою музыку» и плейлисты, миксы по треку и плейлисту.
- Перемотка, Shuffle, Loop, нормализация громкости и горячие клавиши.
- Постоянный аудиокэш до 5 ГиБ; отдельные кэши библиотеки, обложек и аватарки.
- Сохранение входа, настроек, размера окна, истории и позиции текущего трека.
- Системный трей и медиакнопки: MPRIS на Linux, SMTC на Windows.
- Один экземпляр приложения: повторный запуск поднимает открытое окно, в том числе из трея.
- [Discord Rich Presence](docs/discord-rpc.md): текущий трек, обложка и кнопка GitHub VK Fast; включается в настройках.

## Вход

Кнопка «Войти через VK» открывает официальную страницу VK в отдельном окне Tauri с собственным профилем приложения. Также доступен вход по QR-коду с подтверждением на телефоне. Сохранённая сессия восстанавливается при следующем запуске; профили обычных браузеров не читаются.

Подробнее: [вход и хранение сессии](docs/tauri-login.md). Музыкальные запросы используют внутренний вебинтерфейс VK и могут потребовать обновления клиента при изменениях сервиса.

## Что установить

Для сборки нужны **Rust 1.95+**, Cargo и компилятор C. Rust-зависимости закреплены в `Cargo.lock` и загружаются Cargo автоматически.

### Linux

Нужны `pkg-config`, Clang/libclang, GTK 3, WebKitGTK 4.1, библиотеки X11/Wayland, OpenGL/EGL и PipeWire. Для воспроизведения должны работать PipeWire и его менеджер сессии, а `ffmpeg` должен быть доступен в `PATH`.

Примеры системных пакетов:

| Дистрибутив | Пакеты |
| --- | --- |
| Arch Linux | `base-devel rust pkgconf clang libx11 libxcursor libxi libxrandr libxkbcommon wayland mesa libpipewire ffmpeg gtk3 webkit2gtk-4.1` |
| Debian / Ubuntu | `build-essential pkg-config libclang-dev libgtk-3-dev libwebkit2gtk-4.1-dev libxkbcommon-dev libx11-dev libxcursor-dev libxi-dev libxrandr-dev libwayland-dev libegl1-mesa-dev libgl1-mesa-dev libpipewire-0.3-dev libspa-0.2-dev ffmpeg` |

Версия Rust в репозитории дистрибутива может быть ниже требуемой; в таком случае используйте rustup. Для выбора обложки нужен `xdg-desktop-portal` с backend вашей среды. Для MPRIS нужен пользовательский D-Bus.

Node.js, npm и Tauri CLI для сборки приложения не нужны. FFmpeg запускается отдельным процессом; заголовки `libavcodec` и `libavformat` не требуются.

### Windows

Готовую portable-сборку нужно распаковать целиком и запустить `vk-fast.exe`. Для окна входа нужен **Microsoft Edge WebView2 Runtime**. Звук выводится через WASAPI; минимальный FFmpeg с Mbed TLS и встроенными корневыми сертификатами включён в архив.

Кросс-сборка из Linux требует официального Rust toolchain с целью `x86_64-pc-windows-msvc`, LLVM, Python 3.11+ и `cargo-xwin`. Для первой сборки декодера также нужны Bash, curl, tar/xz/bzip2, make, patch и GCC. Готовый декодер переиспользуется при следующих сборках. [Подготовка toolchain и упаковка Windows](docs/windows.md), [состав FFmpeg](docs/windows-ffmpeg.md).

## Сборка и запуск

Из каталога проекта на Linux:

```sh
cargo run --locked
```

Оптимизированная сборка:

```sh
cargo build --release --locked
./target/release/vk-fast
```

После подготовки Windows toolchain по инструкции выше:

```sh
./scripts/build-windows.sh --output dist
```

Правила работы с кодом и команды проверок находятся в [AGENTS.md](AGENTS.md).

## Хранение данных

| Данные | Linux | Windows portable |
| --- | --- | --- |
| Настройки и логи | `$XDG_CONFIG_HOME/vk-fast` или `~/.config/vk-fast` | `config` рядом с `.exe` |
| Сессия и история | `$XDG_STATE_HOME/vk-fast` или `~/.local/state/vk-fast` | `config` рядом с `.exe` |
| Кэши | `$XDG_CACHE_HOME/vk-fast` или `~/.cache/vk-fast` | `cache` рядом с `.exe` |

Лимит аудиокэша не включает библиотеку и изображения. Логи включаются в настройках и по умолчанию выключены. Файлы сессии и профиль webview содержат данные входа и не предназначены для публикации.

## Лицензия

Код VK Fast — **GPL-3.0-only**, полный текст в [LICENSE](LICENSE). Inter распространяется по [SIL OFL 1.1](assets/fonts/OFL.txt); модуль прокрутки содержит код fastframe-scroll под MIT с сохранённым уведомлением в [исходнике](src/scrolling.rs).

Условия сторонних компонентов сохраняются отдельно. Windows-выпуск сопровождается отдельным архивом исходников и лицензий FFmpeg, Mbed TLS и зависимостей: [подробности лицензирования](docs/licensing.md).
