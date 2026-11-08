//! Local settings search. It never shares queries with the VK music search.
use eframe::egui::{self, Response, Sense};

use crate::{
    i18n::Language,
    icons::{self, Icon},
    theme,
};

#[derive(Clone, Copy)]
pub enum Setting {
    Language,
    SystemTitlebar,
    CompactSeek,
    DoubleClickPlay,
    NormalizeVolume,
    Lyrics,
    MinimizeToTray,
    Logging,
    DiscordRpc,
    CacheLimit,
    CacheClear,
    CacheStatus,
    About,
    #[cfg(any(windows, test))]
    Updates,
}

pub struct Query {
    words: Vec<String>,
}

impl Query {
    pub fn new(text: &str) -> Self {
        Self {
            words: text
                .to_lowercase()
                .replace('ё', "е")
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    pub fn matches(&self, setting: Setting) -> bool {
        let keywords = match setting {
            Setting::Language => {
                "общие язык языки интерфейса русский английский перевод локализация general interface language languages russian english translation localization"
            }
            Setting::SystemTitlebar => {
                "интерфейс системный заголовок окна панель оформление рамка кнопки interface system title bar titlebar custom window decoration frame caption controls"
            }
            Setting::CompactSeek => {
                "интерфейс короткая полоска по центру плеер вид прогресс перемотка interface centered compact seek bar player progress timeline layout"
            }
            Setting::DoubleClickPlay => {
                "интерфейс воспроизведение запускать трек треки щелчок щелчком клик кликом двойной двойным одинарный одинарным нажатие два раза мышь interface playback play track tracks single double click clicks mouse"
            }
            Setting::NormalizeVolume => {
                "воспроизведение нормализация нормализовать выравнивание громкость громкости звука звук треки playback normalize normalization volume loudness sound audio level tracks"
            }
            Setting::Lyrics => {
                "воспроизведение текст тексты песни слова синхронизация мусиксматч музиксматч анонимный доступ токен ключ playback lyrics musixmatch vk genius source fallback anonymous token api key synced источник резервный"
            }
            Setting::MinimizeToTray => {
                "общие системный трей сворачивать свернуть при закрытии закрытие фон значок general minimize minimise to system tray when close closing background icon"
            }
            Setting::Logging => {
                "общие логи логирование журнал диагностика отладка запись файла ошибки general logs logging log diagnostic diagnostics debug file errors"
            }
            Setting::DiscordRpc => {
                "общие дискорд активность статус музыка general discord rpc rich presence status activity music"
            }
            Setting::CacheLimit => {
                "кеш кэш аудиокэш аудиокеш лимит размер гигабайты хранение диск cache audio limit size budget storage disk gib gb"
            }
            Setting::CacheClear => {
                "кеш кэш очистить очистка удалить обложки аудио история cache clear delete remove artwork audio history"
            }
            Setting::CacheStatus => {
                "кеш кэш хранилище занято свободно данные загруженное аудио обложки история прослушиваний cache storage used free status space loaded audio data artwork play history"
            }
            Setting::About => {
                "о приложении приложение описание версия лицензия about application version license gpl vk fast"
            }
            #[cfg(any(windows, test))]
            Setting::Updates => {
                "о приложении обновление обновления автоматически проверить новая версия windows github ota update updates automatically automatic check download version release"
            }
        };
        self.words.iter().all(|word| keywords.contains(word))
    }
}

pub fn input(ui: &mut egui::Ui, query: &mut String, language: Language) -> Response {
    egui::Frame::new()
        .fill(theme::SURFACE)
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(12, 7))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.spacing_mut().interact_size.y = 24.0;
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.horizontal(|ui| {
                icons::show(ui, Icon::Search, theme::MUTED, 18.0);
                let show_clear = !query.is_empty();
                let width = (ui.available_width() - if show_clear { 32.0 } else { 0.0 }).max(0.0);
                let mut edit = ui.add(
                    egui::TextEdit::singleline(query)
                        .id_salt("settings-search-input")
                        .char_limit(256)
                        .frame(egui::Frame::NONE)
                        .desired_width(width)
                        .hint_text(language.text("Поиск по настройкам", "Search settings")),
                );
                if show_clear {
                    let (rect, clear) =
                        ui.allocate_exact_size(egui::vec2(24.0, 24.0), Sense::click());
                    icons::paint(
                        ui.painter(),
                        rect.shrink(2.0),
                        Icon::Close,
                        if clear.hovered() {
                            ui.visuals().text_color()
                        } else {
                            theme::MUTED
                        },
                    );
                    let label = language.text("Очистить поиск настроек", "Clear settings search");
                    clear.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
                    });
                    if clear
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text(label)
                        .clicked()
                    {
                        query.clear();
                        edit.mark_changed();
                        edit.request_focus();
                    }
                }
                edit
            })
            .inner
        })
        .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discord_rpc_matches_general_and_activity_terms_in_both_languages() {
        for query in [
            "Discord Rich Presence",
            "GENERAL DISCORD",
            "rpc status",
            "activity music",
            "ДИСКОРД",
            "общие активность",
            "статус музыка",
        ] {
            assert!(
                Query::new(query).matches(Setting::DiscordRpc),
                "Missing {query}"
            );
        }
        assert!(!Query::new("discord").matches(Setting::Logging));
        assert!(!Query::new("cache limit").matches(Setting::DiscordRpc));
        assert!(!Query::new("активность").matches(Setting::NormalizeVolume));
    }

    #[test]
    fn settings_queries_match_both_languages_case_and_multiple_words() {
        assert!(Query::new("  ГРОМКОСТЬ  ").matches(Setting::NormalizeVolume));
        assert!(Query::new("SoUnD volume").matches(Setting::NormalizeVolume));
        assert!(Query::new("normalize volume").matches(Setting::NormalizeVolume));
        assert!(Query::new("выравнивание громкости").matches(Setting::NormalizeVolume));
        assert!(Query::new("minimize tray closing").matches(Setting::MinimizeToTray));
        assert!(Query::new("системный трей").matches(Setting::MinimizeToTray));
        assert!(!Query::new("громкость").matches(Setting::CacheLimit));
        assert!(Query::new("cache limit").matches(Setting::CacheLimit));
        assert!(!Query::new("cache limit").matches(Setting::CacheClear));
        assert!(Query::new("кэш очистить").matches(Setting::CacheClear));
        assert!(Query::new("title bar").matches(Setting::SystemTitlebar));
        assert!(Query::new("двойным щелчком").matches(Setting::DoubleClickPlay));
        assert!(Query::new("double click").matches(Setting::DoubleClickPlay));
        assert!(Query::new("одинарный клик").matches(Setting::DoubleClickPlay));
        assert!(!Query::new("double click").matches(Setting::NormalizeVolume));
        assert!(Query::new("gpl").matches(Setting::About));
        assert!(!Query::new("nonexistent").matches(Setting::Language));
        assert!(Query::new(" \n ").is_empty());
        assert!(Query::new("").matches(Setting::NormalizeVolume));
    }
}
