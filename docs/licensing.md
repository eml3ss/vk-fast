# Лицензии VK Fast

Собственный код VK Fast распространяется по **GNU GPL версии 3** (`GPL-3.0-only`). Полный неизменённый текст находится в [LICENSE](../LICENSE); источник — [официальный сайт GNU](https://www.gnu.org/licenses/gpl-3.0.txt). Обозначение `only` фиксирует версию 3 и не добавляет разрешение автоматически выбирать будущие версии GPL.

| Компонент | Условия |
| --- | --- |
| Собственный код VK Fast | GPL-3.0-only, [LICENSE](../LICENSE). |
| Встроенный шрифт Inter | SIL Open Font License 1.1; исходное уведомление и текст сохранены в [assets/fonts/OFL.txt](../assets/fonts/OFL.txt). |
| Rust-зависимости | Каждая сохраняет собственную лицензию и уведомления. Поле `license` VK Fast не меняет лицензии библиотек. Версии фиксируются в [Cargo.lock](../Cargo.lock). |
| FFmpeg | При запуске из Cargo используется системная программа. Готовые Linux- и Windows-сборки включают минимальный декодер; рецепты и ссылки на закреплённые исходники приведены ниже. |
| NSIS | Установщик создаётся NSIS 3.13; исходный [COPYING](third-party/windows/NSIS.txt) сохранён отдельно от рабочего Windows-архива. |

## FFmpeg

Для воспроизведения VK Fast запускает FFmpeg отдельным процессом. Обычный `cargo build` его не скачивает. Готовые пакеты содержат минимальный декодер; библиотеки `libavcodec`/`libavformat` не связываются с Rust-программой. [Состав Windows-декодера](windows-ffmpeg.md), [Linux-сборка](linux.md).

Проверенная в текущей среде сборка `ffmpeg n9.0.2` содержит параметры `--enable-gpl` и `--enable-version3`. Эти параметры относятся к сборке FFmpeg. На другом компьютере её состав и условия могут отличаться; посмотреть свою сборку можно командами `ffmpeg -version` и `ffmpeg -L`.

По [официальному описанию лицензии FFmpeg](https://ffmpeg.org/doxygen/trunk/md_LICENSE.html), включение GPL-компонентов через `--enable-gpl` меняет применяемые условия FFmpeg на GPL; `--enable-version3` выбирает версию 3. Его внешние библиотеки также могут влиять на условия конкретной сборки. См. также [страницу лицензирования FFmpeg](https://ffmpeg.org/legal.html).

## Распространение

GitHub Release 0.1.0 содержит Linux AppImage/tar.gz и Windows portable.zip/NSIS; выпуск 0.1.1 обновляет Windows portable.zip и NSIS, добавляя OTA. Дополнительные архивы исходников к релизам не прикладываются. Исходники VK Fast, локальные патчи библиотек, Cargo.lock и рецепты сборки находятся в соответствующем теге этого репозитория (`v0.1.0`, `v0.1.1`).

- Rust-компоненты: [Windows-уведомления](third-party/windows/Rust-notices.txt), [Linux-уведомления](third-party/linux/Rust-notices.txt). Версии закреплены в Cargo.lock; исходники этих версий доступны на crates.io по ссылкам из уведомлений.
- FFmpeg 9.0.2: [исходный код](https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz), [Windows-рецепт](../scripts/build-ffmpeg-windows.sh), [Linux-рецепт](../scripts/build-ffmpeg-linux.sh), [исходные уведомления](third-party/windows/decoder/ffmpeg). SHA-256 исходного архива закреплён в рецептах.
- Mbed TLS 3.6.7: [выпуск с исходниками](https://github.com/Mbed-TLS/mbedtls/releases/tag/mbedtls-3.6.7), [лицензия](third-party/windows/decoder/mbedtls/LICENSE). Корни Mozilla и патч FFmpeg находятся в [scripts/ffmpeg-tls](../scripts/ffmpeg-tls).
- NSIS 3.13: [исходный код](https://downloads.sourceforge.net/project/nsis/NSIS%203/3.13/nsis-3.13-src.tar.bz2); закреплённый SHA-256 и рецепт упаковки — в [windows_installer.py](../scripts/windows_installer.py).
- GTK, WebKit и остальные Linux-библиотеки: [точные версии и ссылки на соответствующие исходники Ubuntu](third-party/linux/Ubuntu-sources.tsv). Уведомления о них также находятся внутри Linux-пакета в `usr/share/doc/vk-fast`.

При распространении сохраняйте эти уведомления и доступ к соответствующему исходному коду. Рабочий Windows-архив содержит только исполняемые файлы и пустые папки данных; локальный упаковщик может дополнительно создавать отдельный сопровождающий архив, который не загружается в GitHub Release.

Изучение протокола VK по открытым примерам не меняет выбранную лицензию собственного кода. Исходники сторонних проектов при переносе сохраняют свои лицензии и уведомления; GPL VK Fast не заменяет их автоматически.
