# Выпуск

Пакеты выпуска собирает GitHub Actions (`.github/workflows/release.yml`), а не компьютер разработчика: локально нужны только обычные `cargo test`/`clippy`.

## Шаги

1. Поднимите версию в `Cargo.toml` (и `Cargo.lock`: `cargo update -p vk-fast --offline`), версии в примерах `docs/linux.md` и `docs/windows.md`, добавьте раздел `## X.Y.Z` в начало `CHANGELOG.md`. Полный `cargo test --locked`, `cargo clippy --locked --all-targets -- -D warnings`, коммит в `main`.
2. Отправьте тег:

   ```sh
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```

3. Workflow собирает:
   - на Ubuntu 24.04 (база glibc 2.39) — `scripts/build-linux.sh`: AppImage и tar.gz, затем проверяет, что AppImage запускается (`--help`);
   - на Windows Server 2022 нативным MSVC — `scripts/build-windows.sh --installer`: portable ZIP (только `vk-fast.exe` в корне, это проверяется) и установщик NSIS, затем проверяет запуск `vk-fast.exe --print-data-paths`.
4. Когда обе сборки прошли, workflow сверяет тег с версией `Cargo.toml`, проверяет, что файлов ровно четыре с ожидаемыми именами, и создаёт **черновик** релиза `VK Fast X.Y.Z` с описанием из раздела `CHANGELOG.md`. Черновик не виден OTA и пользователям.
5. Проверьте обновление предыдущего опубликованного клиента новым пакетом: скачайте файлы черновика и предыдущего релиза во временную папку вне проекта (`gh release download vX.Y.Z --dir …`) и проверьте установку через Wine в отдельном префиксе, как требует `AGENTS.md`. Локальная сборка для этого не нужна.
6. Опубликуйте черновик:

   ```sh
   gh release edit vX.Y.Z --draft=false --latest
   ```

Ручной запуск workflow (**Actions → Release → Run workflow** или `gh workflow run release.yml`) собирает те же пакеты как артефакты запуска (хранятся 14 дней) без релиза — для проверки сборки перед тегом.

Локальная сборка пакетов (`scripts/build-linux.sh`, `scripts/build-windows.sh`) по-прежнему работает, см. [linux.md](linux.md) и [windows.md](windows.md), но для выпусков не нужна.
