//! A release's metadata footer. Missing VK fields are omitted; durations come
//! from the complete release track list. Dates use the desktop's local timezone.
use chrono::{DateTime, Datelike, Local, TimeZone, Timelike};
use eframe::egui::{self, RichText};

use crate::{i18n::Language, model::Track, theme, web_audio::AlbumStats};

#[derive(Clone, PartialEq)]
struct Key {
    language: Language,
    tracks: usize,
    seconds: u64,
    stats: AlbumStats,
}

#[derive(Clone)]
struct Info {
    key: Key,
    summary: Vec<String>,
    dates: Vec<String>,
}

pub fn show(ui: &mut egui::Ui, language: Language, tracks: &[Track], stats: &AlbumStats) {
    let key = Key {
        language,
        tracks: tracks.len(),
        seconds: tracks.iter().map(|track| u64::from(track.duration)).sum(),
        stats: stats.clone(),
    };
    let id = ui.make_persistent_id("release-information");
    let info = ui
        .ctx()
        .data(|data| data.get_temp::<Info>(id))
        .filter(|info| info.key == key)
        .unwrap_or_else(|| {
            // Cache presentation too: timezone conversion is not repeated each frame.
            let info = information(key);
            ui.ctx().data_mut(|data| data.insert_temp(id, info.clone()));
            info
        });
    ui.add_space(16.0);
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        for line in &info.summary {
            ui.add(
                egui::Label::new(RichText::new(line).color(theme::MUTED))
                    .wrap()
                    .selectable(false),
            );
        }
        if !info.dates.is_empty() {
            ui.add_space(12.0);
        }
        for line in &info.dates {
            ui.add(
                egui::Label::new(RichText::new(line).color(theme::MUTED))
                    .wrap()
                    .selectable(false),
            );
        }
    });
    ui.add_space(20.0);
}

pub fn counted(value: u64, language: Language, russian: [&str; 3], english: [&str; 2]) -> String {
    let noun = if language == Language::Russian {
        if (11..=14).contains(&(value % 100)) {
            russian[2]
        } else {
            match value % 10 {
                1 => russian[0],
                2..=4 => russian[1],
                _ => russian[2],
            }
        }
    } else if value == 1 {
        english[0]
    } else {
        english[1]
    };
    format!("{} {noun}", grouped(value, language))
}

fn grouped(value: u64, language: Language) -> String {
    let digits = value.to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index != 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(if language == Language::Russian {
                ' '
            } else {
                ','
            });
        }
        grouped.push(digit);
    }
    grouped
}

fn duration(total_seconds: u64, language: Language) -> String {
    let hours = total_seconds / 3600;
    let minutes = total_seconds / 60 % 60;
    let seconds = total_seconds % 60;
    let mut parts = Vec::new();
    if hours != 0 {
        parts.push(counted(
            hours,
            language,
            ["час", "часа", "часов"],
            ["hour", "hours"],
        ));
    }
    if minutes != 0 {
        parts.push(counted(
            minutes,
            language,
            ["минута", "минуты", "минут"],
            ["minute", "minutes"],
        ));
    }
    if seconds != 0 || parts.is_empty() {
        parts.push(counted(
            seconds,
            language,
            ["секунда", "секунды", "секунд"],
            ["second", "seconds"],
        ));
    }
    parts.join(" ")
}

fn date_label<T: TimeZone>(date: DateTime<T>, language: Language, current_year: i32) -> String {
    let months = if language == Language::Russian {
        [
            "января",
            "февраля",
            "марта",
            "апреля",
            "мая",
            "июня",
            "июля",
            "августа",
            "сентября",
            "октября",
            "ноября",
            "декабря",
        ]
    } else {
        [
            "January",
            "February",
            "March",
            "April",
            "May",
            "June",
            "July",
            "August",
            "September",
            "October",
            "November",
            "December",
        ]
    };
    let year = if date.year() == current_year {
        String::new()
    } else {
        format!(" {}", date.year())
    };
    format!(
        "{} {}{year} {} {:02}:{:02}",
        date.day(),
        months[date.month0() as usize],
        language.text("в", "at"),
        date.hour(),
        date.minute()
    )
}

fn information(key: Key) -> Info {
    let language = key.language;
    let mut summary = vec![
        counted(
            key.tracks as u64,
            language,
            ["трек", "трека", "треков"],
            ["track", "tracks"],
        ),
        duration(key.seconds, language),
    ];
    if let Some(plays) = key.stats.plays {
        summary.push(counted(
            plays,
            language,
            ["прослушивание", "прослушивания", "прослушиваний"],
            ["play", "plays"],
        ));
    }
    let current_year = Local::now().year();
    let dates = [
        (key.stats.created_at, language.text("создано", "created")),
        (key.stats.updated_at, language.text("изменено", "updated")),
    ]
    .into_iter()
    .filter_map(|(timestamp, label)| {
        let date = DateTime::from_timestamp(timestamp.filter(|timestamp| *timestamp > 0)?, 0)?
            .with_timezone(&Local);
        Some(format!(
            "{label} {}",
            date_label(date, language, current_year)
        ))
    })
    .collect();
    Info {
        key,
        summary,
        dates,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_summary_uses_real_total_duration_and_correct_plural_forms() {
        let key = Key {
            language: Language::Russian,
            tracks: 3,
            seconds: 377,
            stats: AlbumStats {
                plays: Some(861),
                ..Default::default()
            },
        };
        let info = information(key);
        assert_eq!(
            info.summary,
            ["3 трека", "6 минут 17 секунд", "861 прослушивание"]
        );
        assert!(info.dates.is_empty());
        for (count, expected) in [
            (1, "1 трек"),
            (2, "2 трека"),
            (5, "5 треков"),
            (11, "11 треков"),
            (21, "21 трек"),
            (24, "24 трека"),
        ] {
            assert_eq!(
                counted(
                    count,
                    Language::Russian,
                    ["трек", "трека", "треков"],
                    ["track", "tracks"]
                ),
                expected
            );
        }
        assert_eq!(duration(3600, Language::English), "1 hour");
        assert_eq!(
            duration(3661, Language::Russian),
            "1 час 1 минута 1 секунда"
        );
        assert_eq!(duration(0, Language::Russian), "0 секунд");
    }

    #[test]
    fn unknown_fields_are_omitted_and_zero_plays_are_not_fabricated_as_unknown() {
        let key = Key {
            language: Language::English,
            tracks: 1,
            seconds: 60,
            stats: AlbumStats::default(),
        };
        let unknown = information(key.clone());
        assert_eq!(unknown.summary, ["1 track", "1 minute"]);
        assert!(unknown.dates.is_empty());
        let mut known = key;
        known.stats.plays = Some(0);
        assert_eq!(
            information(known).summary,
            ["1 track", "1 minute", "0 plays"]
        );
        assert_eq!(grouped(1234567, Language::Russian), "1 234 567");
        assert_eq!(grouped(1234567, Language::English), "1,234,567");
    }

    #[test]
    fn dates_use_the_given_timezone_and_show_the_year_for_older_releases() {
        let zone = chrono::FixedOffset::east_opt(3 * 3600).unwrap();
        let date = zone.with_ymd_and_hms(2026, 7, 30, 20, 51, 0).unwrap();
        assert_eq!(date_label(date, Language::Russian, 2026), "30 июля в 20:51");
        assert_eq!(
            date_label(date, Language::English, 2026),
            "30 July at 20:51"
        );
        assert_eq!(
            date_label(date, Language::Russian, 2027),
            "30 июля 2026 в 20:51"
        );
    }
}
