<div align="center">

<img src="assets/vk-fast-icon.png" width="96" height="96" alt="">

# VK Fast

**Неофициальный плеер VK Музыки для Linux и Windows на Rust и egui.**

[![релиз](https://img.shields.io/github/v/release/eml3ss/vk-fast?label=%D1%80%D0%B5%D0%BB%D0%B8%D0%B7&color=3670c4&labelColor=171b21)](https://github.com/eml3ss/vk-fast/releases/latest)
![Linux Wayland | X11](https://img.shields.io/badge/Linux-Wayland%20%7C%20X11-20262f?labelColor=171b21)
![Windows 10 | 11](https://img.shields.io/badge/Windows-10%20%7C%2011-20262f?labelColor=171b21)
![сделано на Rust + egui](https://img.shields.io/badge/%D1%81%D0%B4%D0%B5%D0%BB%D0%B0%D0%BD%D0%BE%20%D0%BD%D0%B0-Rust%20%2B%20egui-20262f?labelColor=171b21)
[![лицензия GPL-3.0](https://img.shields.io/badge/%D0%BB%D0%B8%D1%86%D0%B5%D0%BD%D0%B7%D0%B8%D1%8F-GPL--3.0-20262f?labelColor=171b21)](LICENSE)

[Скачать](https://github.com/eml3ss/vk-fast/releases/latest) ·
[Возможности](#возможности) ·
[Установка](#установка) ·
[Вход](#вход) ·
[Сборка](#сборка) ·
[Изменения](CHANGELOG.md) ·
[Лицензия](#лицензия)

</div>

<br>

VK Fast — отдельное приложение для VK Музыки: без браузера внутри, с быстрым откликом, своим
звуковым движком и экономным расходом памяти и процессора. Вход по QR-коду из приложения VK на телефоне.

> [!IMPORTANT]
> Это неофициальный проект. Он не связан с VK, и VK его не поддерживает. Нужен ваш аккаунт VK.

## Возможности

- **Музыка.** «Моя музыка», плейлисты, альбомы и подписанные исполнители с быстрым поиском;
  VK Микс, рекомендации, история и общий поиск.
- **Страницы.** Исполнители с дискографией и участием в релизах, альбомы, плейлисты, пользователи
  и сообщества.
- **Плейлисты.** Создание и редактирование, добавление треков, перетаскивание, закрепление.
- **Свои треки.** Загрузка MP3 с названием и исполнителем, скачивание треков с обложкой и тегами.
- **Тексты песен.** Синхронный текст из VK, Musixmatch, LRCLIB и NetEase, обычный — из Genius;
  полноэкранный режим с живым фоном из обложки.
- **Звук.** Десятиполосный эквалайзер с пресетами, плавный переход между треками, аудиокэш до 5 ГБ.
- **Оформление.** Девять тем (в том числе Kanagawa, Catppuccin, Tokyo Night и Rosé Pine),
  [свои темы из JSON](docs/themes.md) и любой цвет акцента.
- **Удобства.** Очередь с «Воспроизвести следующим», блокировка исполнителей, горячие клавиши,
  медиаклавиши и управление из системы (MPRIS, SMTC), статус в Discord.
- **Система.** Открытие ссылок VK Музыки прямо в плеере, работа в трее, обновления на Windows
  из приложения.

## Установка

Файлы — на странице [последнего релиза](https://github.com/eml3ss/vk-fast/releases/latest).

| Система | Файл | Что нужно |
| --- | --- | --- |
| Linux | `VK-Fast-…-linux-x86_64.AppImage` или `.tar.gz` | glibc 2.39+ (Ubuntu 24.04+, Arch, Fedora), PipeWire, fontconfig |
| Windows 10 / 11 | `VK-Fast-…-windows-x86_64-setup.exe` — установщик | — |
| Windows 10 / 11 | `VK-Fast-…-windows-x86_64.zip` — без установки | — |

Подробнее: [Linux](docs/linux.md), [Windows](docs/windows.md). На Windows приложение само
предлагает обновиться, когда выходит новая версия.

## Вход

Только по QR-коду: отсканируйте его приложением VK на телефоне и подтвердите вход. Пароль
и браузер не нужны. Сохранённая сессия восстанавливается при запуске, «Выйти из аккаунта»
удаляет её с компьютера.

## Сборка

Rust 1.98+, Cargo и CMake (из него собирается BoringSSL, через который клиент говорит с VK как
Chrome); зависимости закреплены в Cargo.lock.

```sh
cargo build --release --locked
cargo run --release --locked
```

Для Linux нужны pkg-config, CMake, Clang/libclang, X11/Wayland, OpenGL/EGL и PipeWire. Пакеты для сборки:

| Система | Пакеты |
| --- | --- |
| Arch Linux | `base-devel rust cmake pkgconf clang libx11 libxcursor libxi libxrandr libxkbcommon wayland mesa libpipewire` |
| Debian / Ubuntu | `build-essential cmake pkg-config libclang-dev libxkbcommon-dev libx11-dev libxcursor-dev libxi-dev libxrandr-dev libwayland-dev libegl1-mesa-dev libgl1-mesa-dev libpipewire-0.3-dev libspa-0.2-dev` |

Для запуска Linux-пакетов используются библиотеки дистрибутива. [Linux: запуск и упаковка](docs/linux.md).
Пакеты выпусков собирает GitHub Actions: [выпуск](docs/release.md).

## Лицензия

VK Fast — бесплатный проект с открытым исходным кодом под GPL-3.0-only. Лицензии используемых
компонентов сохраняются: [подробнее](docs/licensing.md).
