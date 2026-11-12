# Файлы тем

При запуске VK Fast читает JSON-файлы из папки `themes` рядом с `preferences.json`. Девять штатных тем создаются автоматически, если их ещё нет: «Тёмная», «Чёрная (OLED)», «Графит», «Полночь», «Светлая», Kanagawa, Catppuccin, Tokyo Night и Rosé Pine. Приложение сохраняет существующие файлы, в том числе отредактированные.

| Установка | Папка тем |
| --- | --- |
| Windows, настройки рядом с EXE | `<папка программы>\config\themes` |
| Windows, настройки в AppData | `%LOCALAPPDATA%\VK Fast\config\themes` |
| Linux | `$XDG_CONFIG_HOME/vk-fast/themes` или `~/.config/vk-fast/themes` |

На Windows папка определяется выбором **настроек** в `vk-fast-storage.ini`; расположение кэша на неё не влияет. Portable-архиву по-прежнему достаточно одного EXE.

Чтобы сделать свою тему, скопируйте один из файлов в этой папке и откройте копию в редакторе. Подойдёт любое имя файла с расширением `.json`. Внутри измените:

- `id` — уникальный идентификатор, до 64 символов: строчные латинские буквы, цифры, дефис и подчёркивание. Идентификатор сохраняется в `preferences.json`, поэтому переименование файла не сбрасывает выбор. Для правки штатной темы оставьте её `id` прежним.
- `name.ru` и `name.en` — названия для русского и английского интерфейса, до 80 символов каждое.
- `palette` — базовые цвета в формате `#rrggbb`. `panel_line` может быть `null`, если не нужна линия между панелями и страницей. `light` включает поведение светлой темы. `chroma` (от 0 до 4) задаёт долю оттенка исходных серых, `tint` — добавку к красному, зелёному и синему каналам тёмных оттенков (от −255 до 255 для каждого канала). Необязательные поля: `active` — заливка выбранного пункта в списках навигации (боковая панель, разделы настроек, открытый плейлист; без него — `selected`), `on_active` — текст и значки на ней (без него — обычный текст и значок цвета акцента), `on_primary` — текст и значки на залитых кнопках и кнопке воспроизведения. Без `on_primary` приложение само меняет текст на заливке на контрастный там, где обычный читается хуже 2,5:1.

В [шаблоне тёмной темы](../assets/themes/dark.json) есть все поля. Перезапустите VK Fast после добавления или правки файла. Затем выберите тему в **Настройки → Оформление**.

Неверный JSON, неподходящие значения и файлы больше 32 КиБ пропускаются. При одинаковом `id` используется первый корректный файл в порядке имён. Если файл штатной темы не читается, действует палитра из поставки. Если пропала пользовательская тема, приложение показывает тёмную и сохраняет её прежний `id` до выбора другой темы.

Выбор цвета акцента по-прежнему хранится отдельно в `preferences.json` и применяется к базовым цветам выбранной темы. Пока акцент не выбран, у темы остаётся свой: если он не синий, в разделе «Цвет акцента» он показан как выбранный «Свой цвет», а оттенки акцента в интерфейсе подстраиваются под него. Готовый цвет, в том числе «Синий», перекрашивает тему в выбранный. Выбранный цвет акцента относится только к текущей теме и сохраняется между перезапусками, пока её не сменят: при смене темы акцент возвращается к собственному цвету новой темы (у штатных — синий).

Kanagawa (вариант Wave), Catppuccin (Mocha), Tokyo Night (Night) и Rosé Pine повторяют цвета одноимённых тем их авторов — rebelot/kanagawa.nvim, Catppuccin, folke/tokyonight.nvim и Rosé Pine (все под лицензией MIT); оттенки, которых в исходных палитрах нет, подобраны под них. У Kanagawa, Catppuccin, Tokyo Night и Rosé Pine свой акцент: пока цвет акцента не выбран, он показан в разделе «Цвет акцента» как «Свой цвет».

## Тема из Noctalia

Тему можно генерировать из палитры [Noctalia](https://noctalia.dev) её пользовательским шаблоном, без пересборки Noctalia. Сохраните шаблон, например, как `~/.config/noctalia/templates/vk-fast-theme.json.in`:

```json
{
  "id": "noctalia",
  "name": { "ru": "Noctalia", "en": "Noctalia" },
  "palette": {
    "accent": "{{ colors.primary.dark.hex }}",
    "muted": "{{ colors.on_surface_variant.dark.hex }}",
    "text": "{{ colors.on_surface.dark.hex }}",
    "strong": "{{ colors.on_surface.dark.hex | lighten 10 }}",
    "error": "{{ colors.error.dark.hex }}",
    "warning": "{{ colors.error.dark.hex | set_hue 30 }}",
    "light": false,
    "background": "{{ colors.surface.dark.hex }}",
    "panel": "{{ colors.surface_container_low.dark.hex }}",
    "panel_line": null,
    "surface": "{{ colors.surface_container_low.dark.hex }}",
    "primary": "{{ colors.primary.dark.hex }}",
    "selected": "{{ colors.primary_container.dark.hex | darken 8 }}",
    "active": "{{ colors.primary.dark.hex }}",
    "on_active": "{{ colors.on_primary.dark.hex }}",
    "on_primary": "{{ colors.on_primary.dark.hex }}",
    "scroll_handle": "{{ colors.surface_container_highest.dark.hex }}",
    "scroll_hovered": "{{ colors.surface_container_highest.dark.hex | lighten 8 }}",
    "scroll_active": "{{ colors.outline.dark.hex }}",
    "field": "{{ colors.surface_container_lowest.dark.hex }}",
    "link": "{{ colors.primary.dark.hex | lighten 10 }}",
    "control": "{{ colors.surface_container.dark.hex }}",
    "control_stroke": "{{ colors.surface_container_highest.dark.hex }}",
    "control_hovered": "{{ colors.surface_container_high.dark.hex }}",
    "control_hovered_stroke": "{{ colors.outline.dark.hex }}",
    "chroma": 0.0,
    "tint": [0.0, 0.0, 0.0]
  }
}
```

и подключите его в `~/.config/noctalia/config.toml`:

```toml
[theme.templates.user.vk_fast]
input_path = "/home/USER/.config/noctalia/templates/vk-fast-theme.json.in"
output_path = "$XDG_CONFIG_HOME/vk-fast/themes/noctalia.json"
```

Noctalia пересоздаёт тему при каждой смене палитры (вручную — `noctalia msg templates-apply`); VK Fast подхватывает её при следующем запуске. Шаблон берёт тёмный вариант палитры: тема VK Fast тёмная.
