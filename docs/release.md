# Выпуск

Пакеты выпуска собирает GitHub Actions (`.github/workflows/release.yml`), а не компьютер разработчика: локально нужны только обычные `cargo test`/`clippy`.

## Перед выпуском: TLS-стек

Соединения с VK идут через BoringSSL (крейт `btls-sys`, собирается в программу) и `wreq` с профилем Chrome из `wreq-util`. BoringSSL не выпускает версий и бюллетеней безопасности, `cargo audit` его уязвимостей почти не показывает, а новый снимок BoringSSL приходит только с новым выпуском `btls-sys`. Поэтому перед каждым выпуском:

1. Проверьте, вышли ли новые версии:

   ```sh
   cargo update --dry-run -p btls-sys -p btls -p wreq -p wreq-util
   ```

2. Если вышли, обновите только их (`cargo update -p <крейт>`) и посмотрите `git diff Cargo.lock`. Cargo при этом может перевыбрать версии у посторонних крейтов (например, `windows-sys`); тогда верните `Cargo.lock` и поменяйте вручную только `version` и `checksum` обновляемого крейта — `cargo build --locked` проверит, что файл согласован. Версию поднимите и в `Cargo.toml`.
3. Проверьте, что соединение работает и VK по-прежнему видит Chrome:

   ```sh
   cargo build --locked && ./target/debug/vk-fast --check-tls
   cargo test --locked fingerprint_is_chromes -- --ignored
   ```

   Второй тест спрашивает у tls.peet.ws отпечаток рукопожатия и сверяет его с записанным (JA4 и HTTP/2 Chrome). Если отпечаток поменялся из-за нового профиля Chrome в `wreq-util`, это нормально: поднимите вместе `wreq_util::Profile`, `CHROME_VERSION`/`SEC_CH_UA` в `src/chrome_http.rs`, `client_platform::USER_AGENT`, а новые значения запишите в тест и в `AGENTS.md`. Если поменялся без смены профиля — разберитесь до выпуска.

## Шаги

1. Пройдите проверку TLS-стека выше. Поднимите версию в `Cargo.toml` (и `Cargo.lock`: `cargo update -p vk-fast --offline`), версии в примерах `docs/linux.md` и `docs/windows.md`, запись `<release version="X.Y.Z" date="ГГГГ-ММ-ДД"/>` первой в `<releases>` файла `flatpak/io.github.eml3ss.VkFast.metainfo.xml` (без неё сборка Flatpak остановится), добавьте раздел `## X.Y.Z` в начало `CHANGELOG.md`. Полный `cargo test --locked`, `cargo clippy --locked --all-targets -- -D warnings`, коммит в `main`.
2. Отправьте тег:

   ```sh
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```

3. Workflow собирает:
   - на Ubuntu 24.04 (база glibc 2.39) — `scripts/build-linux.sh`: AppImage и tar.gz, затем проверяет, что AppImage запускается (`--help`);
   - на Ubuntu 24.04 — Flatpak-пакет `VK-Fast-X.Y.Z-linux-x86_64.flatpak` из `flatpak/` ([flatpak.md](flatpak.md)), затем ставит его и проверяет запуск (`--help`, `--check-tls`) в песочнице;
   - на Windows Server 2022 нативным MSVC — `scripts/build-windows.sh --installer`: portable ZIP (только `vk-fast.exe` в корне, это проверяется) и установщик NSIS, затем проверяет запуск `vk-fast.exe --print-data-paths`.
4. Когда все сборки прошли, workflow сверяет тег с версией `Cargo.toml`, проверяет, что файлов ровно пять с ожидаемыми именами, и создаёт **черновик** релиза `VK Fast X.Y.Z` с описанием из раздела `CHANGELOG.md`. Черновик не виден OTA и пользователям.
5. Опубликуйте черновик:

   ```sh
   gh release edit vX.Y.Z --draft=false --latest
   ```

Ручной запуск workflow (**Actions → Release → Run workflow** или `gh workflow run release.yml`) собирает те же пакеты как артефакты запуска (хранятся 14 дней) без релиза — для проверки сборки перед тегом.

Локальная сборка пакетов (`scripts/build-linux.sh`, `scripts/build-windows.sh`) по-прежнему работает, см. [linux.md](linux.md) и [windows.md](windows.md), но для выпусков не нужна.
