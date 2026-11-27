# Flatpak

Flatpak-версия VK Fast имеет ID `io.github.eml3ss.VkFast` и собирается на рантайме freedesktop 26.08 (Rust из `org.freedesktop.Sdk.Extension.rust-stable`, libclang для bindgen из `llvm23`). Файлы лежат в `flatpak/`:

- `io.github.eml3ss.VkFast.yml` — манифест;
- `io.github.eml3ss.VkFast.desktop` — ярлык;
- `io.github.eml3ss.VkFast.metainfo.xml` — описание для центров приложений, с записью `<release>` на каждый выпуск.

`flatpak/cargo-sources.json` в Git не хранится: его делает из `Cargo.lock` [flatpak-cargo-generator](https://github.com/flatpak/flatpak-builder-tools/tree/master/cargo) (CI берёт закреплённый коммит и сверяет SHA-256). Поэтому сама сборка идёт без сети, а все крейты проверяются по контрольным суммам из `Cargo.lock`.

## Выпуск

Workflow выпуска (`.github/workflows/release.yml`, задание `flatpak`) на Ubuntu 24.04:

1. проверяет `.desktop` (`desktop-file-validate`) и metainfo (`appstreamcli validate`), а также то, что в metainfo есть `<release>` текущей версии;
2. генерирует `cargo-sources.json`;
3. собирает `flatpak-builder` и упаковывает `VK-Fast-X.Y.Z-linux-x86_64.flatpak` со ссылкой на Flathub для рантайма;
4. ставит пакет и запускает `--help` и `--check-tls` внутри песочницы.

Пользователь ставит файл так (рантайм скачается с Flathub):

```sh
flatpak install --user VK-Fast-X.Y.Z-linux-x86_64.flatpak
```

Файл не обновляется сам: новую версию ставят тем же способом поверх старой.

## Локальная сборка

Нужны `flatpak`, `flatpak-builder` и около 3 ГБ под рантайм и SDK. Из корня репозитория:

```sh
python3 -I flatpak-cargo-generator.py Cargo.lock -o flatpak/cargo-sources.json
flatpak-builder --user --install-deps-from=flathub --install --force-clean \
  flatpak/build flatpak/io.github.eml3ss.VkFast.yml
flatpak run io.github.eml3ss.VkFast
```

## Песочница

Разрешения манифеста и зачем они:

| Разрешение | Зачем |
| --- | --- |
| `--share=network` | VK, тексты песен, обложки |
| `--socket=wayland`, `--socket=fallback-x11`, `--device=dri`, `--share=ipc` | окно и OpenGL |
| `--filesystem=xdg-run/pipewire-0` | звук напрямую в PipeWire, как в обычной сборке |
| `--filesystem=xdg-download` | скачанные треки в «Загрузки»; другую папку выбирают через портал |
| `--own-name=org.mpris.MediaPlayer2.vk-fast` | медиаклавиши и управление плеером из системы (MPRIS) |
| `--talk-name=org.kde.StatusNotifierWatcher` | значок в трее |
| `--filesystem=xdg-run/discord-ipc-0`, `xdg-run/app/com.discordapp.Discord:create` | статус в Discord (обычный и Flatpak Discord) |

Внутри песочницы (`flatpak::active`, по файлу `/.flatpak-info`) приложение:

- даёт окну ID `io.github.eml3ss.VkFast`, чтобы рабочий стол нашёл его ярлык и значок;
- не предлагает «Открывать ссылки VK Музыки в VK Fast»: песочница не может менять обработчики ссылок системы;
- не вызывает `niri msg`, `hyprctl` и `swaymsg` (их нет в песочнице, а PID там свои): второй запуск выводит окно через токен активации;
- регистрирует значок в трее без имени `StatusNotifierItem-PID-N` на шине, которое песочнице занять нельзя;
- «Копировать изображение» пробует буфер egui, потому что `wl-copy` в песочнице нет; в сеансе Wayland egui картинки не копирует, поэтому там это пока не работает (нужен буфер обмена Wayland без внешней программы).

Настройки, сессия и кэши Flatpak-версии хранятся в `~/.var/app/io.github.eml3ss.VkFast/` отдельно от обычной сборки, поэтому войти в неё нужно отдельно.

## Flathub

Манифест годится и для Flathub. В репозитории Flathub источник `type: dir` заменяется на этот репозиторий с тегом и коммитом выпуска, а `cargo-sources.json` кладётся рядом с манифестом:

```yaml
      - type: git
        url: https://github.com/eml3ss/vk-fast.git
        tag: vX.Y.Z
        commit: <коммит тега>
      - cargo-sources.json
```

Перед подачей заявки в metainfo нужны скриншоты (`<screenshots>`): Flathub без них приложение не принимает.
