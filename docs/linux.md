# Linux x86-64

AppImage и tar.gz содержат приложение со встроенным Rust-декодером. Начиная с 0.1.2 WebKitGTK, GTK, PipeWire, их библиотеки и модули используются из дистрибутива. Они больше не копируются в пакеты. Требуется glibc 2.39+ и работающий PipeWire.

Установите системные зависимости, если их ещё нет:

| Дистрибутив | Пакеты |
| --- | --- |
| Ubuntu 24.04+ / Debian с glibc 2.39+ | `libwebkit2gtk-4.1-0`, `libpipewire-0.3-0t64`, `fontconfig` |
| Arch Linux и производные | `webkit2gtk-4.1`, `pipewire`, `fontconfig` |
| Fedora | `webkit2gtk4.1`, `pipewire-libs`, `fontconfig` |

GTK 3 и остальные зависимости устанавливаются вместе с WebKitGTK. Графические драйверы и шрифты также берутся из системы. При отсутствии библиотек загрузчик покажет сообщение со списком недостающих компонентов.

```sh
chmod +x VK-Fast-0.1.2-linux-x86_64.AppImage
./VK-Fast-0.1.2-linux-x86_64.AppImage
```

Если FUSE недоступен, используйте `--appimage-extract-and-run` или tar.gz:

```sh
tar -xzf VK-Fast-0.1.2-linux-x86_64.tar.gz
./VK-Fast-0.1.2-linux-x86_64/vk-fast
```

Настройки и кэши находятся в обычных XDG-каталогах. Пакеты не содержат пользовательские данные. Звук декодируется внутри приложения.

## Сборка пакетов

Нужны зависимости сборки из README, Python 3.11+, curl, make, patch, tar/xz/bzip2, binutils и pkg-config. Для распространения используйте Ubuntu 24.04 либо эквивалентную базу glibc 2.39.

```sh
VK_FAST_OUTPUT="$PWD/dist" ./scripts/build-linux.sh
```

Упаковщик удаляет ненужные символы из копий исполняемых файлов и создаёт AppImage/tar.gz из явного небольшого набора файлов. Tauri CLI, linuxdeploy, копии GTK/WebKit, GObject-типов и модулей PipeWire для упаковки не нужны.

Уведомления о распространяемых компонентах сжаты в `usr/share/doc/vk-fast/LICENSES.txt.gz`; соответствующие рецепты и ссылки находятся в [licensing.md](licensing.md).
