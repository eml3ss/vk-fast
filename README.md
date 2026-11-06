# VK Fast

<img src="assets/vk-fast-icon.png" width="80" height="80" align="right" alt="VK Fast">

Неофициальный плеер VK Music для **Linux и Windows** на Rust и egui. Русский и английский интерфейс, тёмная тема, PipeWire на Linux и WASAPI на Windows.

Декодирование и обработка звука выполняются на Rust: **Symphonia**, **Rubato** и **ebur128**. [Устройство аудиотракта](docs/native-audio.md).

Готовые выпуски: [GitHub Releases](https://github.com/eml3ss/vk-fast/releases). Текущая ветка разработки может отличаться от опубликованного выпуска.

## Возможности

- Библиотека, VK Mix, рекомендации, история и поиск музыки.
- Страницы исполнителей, релизов, плейлистов и их владельцев.
- Создание и редактирование плейлистов, закрепление и изменение порядка.
- Загрузка своих MP3 с названием трека и исполнителем.
- Очередь, Shuffle, Loop, нормализация громкости и горячие клавиши.
- Тексты VK, Musixmatch и Genius: боковая панель и полноэкранный режим.
- Постоянный аудиокэш до 5 ГиБ; отдельные кэши библиотеки и изображений.
- Сохранение входа, громкости, истории, позиции трека и размеров окна.
- Системные медиакнопки, MPRIS/SMTC, трей и Discord Rich Presence.
- Один экземпляр приложения и обновления Windows из приложения.

## Вход

«Войти через VK» открывает нативную форму. Tauri работает со своим профилем в фоне; капча показывается отдельной карточкой. Доступен QR-вход. Сохранённая сессия восстанавливается при запуске. [Подробнее](docs/tauri-login.md).

## Сборка

Rust 1.95+ и Cargo; зависимости закреплены в Cargo.lock.

```sh
cargo build --release --locked
cargo run --release --locked
```

Для Linux нужны pkg-config, Clang/libclang, GTK 3, WebKitGTK 4.1, X11/Wayland, OpenGL/EGL и PipeWire. Пакеты для сборки:

| Система | Пакеты |
| --- | --- |
| Arch Linux | `base-devel rust pkgconf clang libx11 libxcursor libxi libxrandr libxkbcommon wayland mesa libpipewire gtk3 webkit2gtk-4.1` |
| Debian / Ubuntu | `build-essential pkg-config libclang-dev libgtk-3-dev libwebkit2gtk-4.1-dev libxkbcommon-dev libx11-dev libxcursor-dev libxi-dev libxrandr-dev libwayland-dev libegl1-mesa-dev libgl1-mesa-dev libpipewire-0.3-dev libspa-0.2-dev` |

Для запуска Linux-пакетов используются библиотеки дистрибутива. [Linux: запуск и упаковка](docs/linux.md).

Windows x86-64 использует WASAPI; для входа нужен Microsoft Edge WebView2 Runtime. Настройки и кэш можно хранить в AppData или рядом с программой. [Windows: сборка, установщик и обновления](docs/windows.md).

## Данные и лицензия

Linux использует стандартные XDG-каталоги VK Fast. Windows — выбранные при установке `config` и `cache`. Лимит аудиокэша учитывается отдельно от библиотеки и обложек.

GPL-3.0-only. Лицензии используемых компонентов сохраняются: [подробнее](docs/licensing.md). Проект не связан с VK.
