# VK Fast

<img src="assets/vk-fast-icon.png" width="80" height="80" align="right" alt="VK Fast">

Неофициальный плеер VK Music для **Linux и Windows** на Rust и egui.

Готовые выпуски: [GitHub Releases](https://github.com/eml3ss/vk-fast/releases). [Список изменений](CHANGELOG.md). Текущая ветка разработки может отличаться от опубликованного выпуска.

## Возможности

- Библиотека плейлистов, альбомов и подписанных исполнителей с быстрым поиском.
- VK Mix, рекомендации, история и общий поиск музыки.
- Страницы исполнителей, релизов, плейлистов и их владельцев.
- Создание и редактирование плейлистов, закрепление и изменение порядка.
- Загрузка своих MP3 с названием трека и исполнителем.
- Текста с Musixmatch, Genius и VK.
- Десятиполосный эквалайзер со своими пресетами.
- Девять штатных тем (в том числе Kanagawa, Catppuccin, Tokyo Night и Rosé Pine), [свои темы из JSON](docs/themes.md) и любой цвет акцента вместо синего.
- Плавный переход между треками с настраиваемой длительностью.
- Постоянный аудиокэш до 5 ГБ.
- Сохранение входа, громкости, истории, позиции трека и размеров окна.
- Возможность включить отображение активности в Discord (RPC).
- Открытие ссылок VK Музыки из других приложений прямо в плеере.
- Обновления в Windows из приложения.

## Вход

Вход выполняется только по QR-коду: отсканируйте его приложением VK на телефоне и подтвердите вход. Пароль и браузер не нужны. Сохранённая сессия восстанавливается при запуске, «Выйти из аккаунта» удаляет её с компьютера.

## Сборка

Rust 1.95+ и Cargo; зависимости закреплены в Cargo.lock.

```sh
cargo build --release --locked
cargo run --release --locked
```

Для Linux нужны pkg-config, Clang/libclang, X11/Wayland, OpenGL/EGL и PipeWire. Пакеты для сборки:

| Система | Пакеты |
| --- | --- |
| Arch Linux | `base-devel rust pkgconf clang libx11 libxcursor libxi libxrandr libxkbcommon wayland mesa libpipewire` |
| Debian / Ubuntu | `build-essential pkg-config libclang-dev libxkbcommon-dev libx11-dev libxcursor-dev libxi-dev libxrandr-dev libwayland-dev libegl1-mesa-dev libgl1-mesa-dev libpipewire-0.3-dev libspa-0.2-dev` |

Для запуска Linux-пакетов используются библиотеки дистрибутива. [Linux: запуск и упаковка](docs/linux.md). Пакеты выпусков собирает GitHub Actions: [выпуск](docs/release.md).

## Лицензия
VK Fast - бесплатный проект с открытым исходным кодом.

GPL-3.0-only. Лицензии используемых компонентов сохраняются: [подробнее](docs/licensing.md). Проект не связан с VK.
