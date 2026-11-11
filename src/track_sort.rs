//! Sorting a track list from its column header: title, date added (only
//! where VK reports it) and duration. A third click returns VK's order.
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Column {
    #[default]
    Original,
    Title,
    Album,
    Added,
    Duration,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Sort {
    pub column: Column,
    pub descending: bool,
}

/// A sorted row order and what it was built from: list origin, sort,
/// view revision, list length and filter.
pub(super) type Cached = (String, Sort, u64, usize, String, Arc<[usize]>);

/// Width of the date column in a track row: wide enough for its Russian
/// header and sort arrow to stay clear of the row buttons.
pub(super) const DATE_WIDTH: f32 = 138.0;
/// The middle of a row's duration, from the row's inner right edge.
const DURATION_CENTER: f32 = 36.0;
/// Width the row keeps on the right for mix, dislike, heart and duration.
pub(super) const CONTROLS_WIDTH: f32 = 142.0;
/// Narrowest title before the list drops a column.
const TITLE_MIN: f32 = 200.0;
const ALBUM_MIN: f32 = 140.0;
const GAP: f32 = 6.0;

/// The optional columns a list offers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Extra {
    pub album: bool,
    pub date: bool,
}

/// Column widths of one row; header and rows share them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Columns {
    pub title: f32,
    pub album: Option<f32>,
    pub date: bool,
}

impl Extra {
    /// Fits the columns into `rest`, the width for title, album and date.
    /// A narrow list drops the date first, then the album.
    pub(super) fn fit(self, rest: f32) -> Columns {
        let need = |album: bool, date: bool| {
            TITLE_MIN
                + if album { GAP + ALBUM_MIN } else { 0.0 }
                + if date { GAP + DATE_WIDTH } else { 0.0 }
        };
        let mut date = self.date;
        let mut album = self.album;
        if date && rest < need(album, true) {
            date = false;
        }
        if album && rest < need(true, date) {
            album = false;
        }
        let flexible = (rest - if date { GAP + DATE_WIDTH } else { 0.0 }).max(0.0);
        if album {
            // Like other players: the album takes a share of the title.
            let album = ((flexible - GAP) * 0.4).max(ALBUM_MIN);
            Columns {
                title: (flexible - GAP - album).max(0.0),
                album: Some(album),
                date,
            }
        } else {
            Columns {
                title: flexible,
                album: None,
                date,
            }
        }
    }
}

impl Sort {
    pub fn active(self) -> bool {
        self.column != Column::Original || self.descending
    }

    /// "#" clicked: the list from its end.
    pub fn reversed(self) -> bool {
        self.column == Column::Original && self.descending
    }

    /// Ascending, then descending, then VK's own order. Dates start with the
    /// newest, like other players.
    pub fn clicked(self, column: Column) -> Self {
        if column == Column::Original {
            // VK's order is already ascending: "#" toggles the reverse.
            return Self {
                column,
                descending: !self.reversed(),
            };
        }
        if self.column != column {
            return Self {
                column,
                descending: column == Column::Added,
            };
        }
        if self.descending == (column == Column::Added) {
            Self {
                column,
                descending: !self.descending,
            }
        } else {
            Self::default()
        }
    }
}

/// Source indices in display order. Ties keep VK's order.
pub(super) fn order(tracks: &[Track], rows: impl Iterator<Item = usize>, sort: Sort) -> Vec<usize> {
    let mut rows: Vec<usize> = rows.filter(|index| *index < tracks.len()).collect();
    if !sort.active() {
        return rows;
    }
    if sort.reversed() {
        rows.reverse();
        return rows;
    }
    let titles: HashMap<usize, String> = match sort.column {
        Column::Title => rows
            .iter()
            .map(|index| (*index, tracks[*index].display_title().to_lowercase()))
            .collect(),
        Column::Album => rows
            .iter()
            .map(|index| {
                (
                    *index,
                    tracks[*index]
                        .display_album_title()
                        .map(str::to_lowercase)
                        .unwrap_or_default(),
                )
            })
            .collect(),
        _ => HashMap::new(),
    };
    rows.sort_by(|a, b| {
        let ordering = match sort.column {
            Column::Original => std::cmp::Ordering::Equal,
            Column::Title => titles[a].cmp(&titles[b]),
            // Tracks without an album stay last in both directions.
            Column::Album => match (titles[a].is_empty(), titles[b].is_empty()) {
                (false, true) | (true, false) if sort.descending => {
                    titles[b].is_empty().cmp(&titles[a].is_empty())
                }
                (false, true) | (true, false) => titles[a].is_empty().cmp(&titles[b].is_empty()),
                _ => titles[a].cmp(&titles[b]),
            },
            Column::Added => tracks[*a].added.cmp(&tracks[*b].added),
            Column::Duration => tracks[*a].duration.cmp(&tracks[*b].duration),
        };
        if sort.descending {
            ordering.reverse()
        } else {
            ordering
        }
    });
    rows
}

/// "8 авг. 2022" / "Aug 8, 2022", in local time.
pub(super) fn date_label(timestamp: i64, language: Language) -> String {
    use chrono::{Datelike, TimeZone};
    let Some(date) = chrono::Local.timestamp_opt(timestamp, 0).single() else {
        return String::new();
    };
    const RU: [&str; 12] = [
        "янв.",
        "февр.",
        "мар.",
        "апр.",
        "мая",
        "июн.",
        "июл.",
        "авг.",
        "сент.",
        "окт.",
        "нояб.",
        "дек.",
    ];
    const EN: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month = date.month0() as usize;
    match language {
        Language::Russian => format!("{} {} {}", date.day(), RU[month], date.year()),
        Language::English => format!("{} {}, {}", EN[month], date.day(), date.year()),
    }
}

/// The column header above a track list; returns the column clicked.
pub(super) fn header(
    ui: &mut egui::Ui,
    language: Language,
    sort: Sort,
    extra: Extra,
    index_width: f32,
    scrollbar: bool,
) -> Option<Column> {
    let (full, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), Sense::hover());
    // Rows inside their own scroll area leave room for its bar on the right.
    let mut rect = full;
    if scrollbar {
        rect.max.x -= ui.spacing().scroll.allocated_width();
    }
    // The same insets as a track row: 4 px, then index, cover and title.
    let inner = rect.shrink2(egui::vec2(4.0, 0.0));
    let title_left = inner.left() + index_width + 6.0;
    let columns = extra.fit(inner.width() - index_width - 6.0 - 40.0 - 6.0 - CONTROLS_WIDTH);
    let title_width = columns.title;
    let album_left = title_left + 40.0 + 6.0 + title_width + GAP;
    let date_left = album_left + columns.album.map_or(0.0, |width| width + GAP);
    let font = egui::FontId::proportional(11.5);
    let mut clicked = None;
    let mut cell = |ui: &mut egui::Ui,
                    column: Column,
                    area: egui::Rect,
                    label: Option<&str>,
                    align: egui::Align2,
                    tooltip: &str| {
        let response = ui
            .interact(
                area,
                ui.id().with(("track-sort", column as u8)),
                Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(tooltip);
        let selected = sort.column == column && sort.active();
        let color = if selected {
            theme::ACCENT
        } else if response.hovered() {
            ui.visuals().text_color()
        } else {
            MUTED
        };
        let anchor = match align {
            egui::Align2::RIGHT_CENTER => area.right_center(),
            _ => area.left_center(),
        };
        let end = if let Some(label) = label {
            let text = ui.painter().text(anchor, align, label, font.clone(), color);
            if align == egui::Align2::RIGHT_CENTER {
                text.left() - 4.0
            } else {
                text.right() + 4.0
            }
        } else {
            // A row's duration follows its buttons and ends about 21 px
            // before the row's edge; the clock sits over its middle.
            let icon = egui::Rect::from_center_size(
                area.right_center() - egui::vec2(DURATION_CENTER, 0.0),
                egui::vec2(14.0, 14.0),
            );
            icons::paint(ui.painter(), icon, Icon::Clock, color);
            icon.left() - 3.0
        };
        if selected {
            // A small triangle beside the label shows the direction.
            let center = egui::pos2(
                if align == egui::Align2::RIGHT_CENTER {
                    end - 3.0
                } else {
                    end + 3.0
                },
                area.center().y,
            );
            let points = if sort.descending {
                vec![
                    center + egui::vec2(-3.5, -2.0),
                    center + egui::vec2(3.5, -2.0),
                    center + egui::vec2(0.0, 2.5),
                ]
            } else {
                vec![
                    center + egui::vec2(-3.5, 2.0),
                    center + egui::vec2(3.5, 2.0),
                    center + egui::vec2(0.0, -2.5),
                ]
            };
            ui.painter().add(egui::Shape::convex_polygon(
                points,
                color,
                egui::Stroke::NONE,
            ));
        }
        if response.clicked() {
            clicked = Some(column);
        }
    };
    let t = |ru, en| language.text(ru, en);
    let mid = |left: f32, width: f32| {
        egui::Rect::from_min_size(
            egui::pos2(left, rect.top()),
            egui::vec2(width.max(1.0), 30.0),
        )
    };
    cell(
        ui,
        Column::Original,
        mid(inner.left(), index_width),
        Some("#"),
        egui::Align2::RIGHT_CENTER,
        if sort.reversed() {
            t("С начала списка", "From the start")
        } else {
            t("С конца списка", "From the end")
        },
    );
    cell(
        ui,
        Column::Title,
        mid(title_left, 40.0 + 6.0 + title_width),
        Some(t("НАЗВАНИЕ", "TITLE")),
        egui::Align2::LEFT_CENTER,
        t("Сортировать по названию", "Sort by title"),
    );
    if let Some(width) = columns.album {
        cell(
            ui,
            Column::Album,
            mid(album_left, width),
            Some(t("АЛЬБОМ", "ALBUM")),
            egui::Align2::LEFT_CENTER,
            t("Сортировать по альбому", "Sort by album"),
        );
    }
    if columns.date {
        cell(
            ui,
            Column::Added,
            mid(date_left, DATE_WIDTH),
            Some(t("ДАТА ДОБАВЛЕНИЯ", "DATE ADDED")),
            egui::Align2::LEFT_CENTER,
            t("Сортировать по дате добавления", "Sort by date added"),
        );
    }
    cell(
        ui,
        Column::Duration,
        mid(inner.right() - 80.0, 80.0),
        None,
        egui::Align2::RIGHT_CENTER,
        t("Сортировать по длительности", "Sort by duration"),
    );
    ui.painter().hline(
        full.x_range(),
        rect.bottom() - 0.5,
        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
    );
    ui.add_space(4.0);
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: i64, title: &str, duration: u32, added: Option<i64>) -> Track {
        let mut track = crate::app::tests::page(1, &[id]).items.remove(0);
        track.title = title.into();
        track.duration = duration;
        track.added = added;
        track
    }

    #[test]
    fn columns_sort_both_ways_then_return_to_vk_order() {
        let tracks = [
            track(1, "beta", 200, Some(30)),
            track(2, "Alpha", 100, Some(10)),
            track(3, "gamma", 300, Some(20)),
        ];
        let by = |sort| order(&tracks, 0..tracks.len(), sort);
        let mut sort = Sort::default();
        assert_eq!(by(sort), [0, 1, 2]);
        sort = sort.clicked(Column::Title);
        assert_eq!(by(sort), [1, 0, 2]);
        sort = sort.clicked(Column::Title);
        assert_eq!(by(sort), [2, 0, 1]);
        sort = sort.clicked(Column::Title);
        assert!(!sort.active());
        sort = sort.clicked(Column::Added);
        assert_eq!(by(sort), [0, 2, 1], "dates start with the newest");
        sort = sort.clicked(Column::Added);
        assert_eq!(by(sort), [1, 2, 0]);
        sort = sort.clicked(Column::Duration);
        assert_eq!(by(sort), [1, 0, 2]);
        // A filtered list keeps only its rows.
        assert_eq!(order(&tracks, [2, 0].into_iter(), sort), [0, 2]);
        sort = sort.clicked(Column::Original);
        assert!(sort.reversed());
        assert_eq!(by(sort), [2, 1, 0], "# lists from the end");
        sort = sort.clicked(Column::Original);
        assert!(!sort.active());
    }

    #[test]
    fn the_date_header_and_its_sort_arrow_fit_before_the_row_buttons() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
        output.textures_delta.clear();
        for label in ["ДАТА ДОБАВЛЕНИЯ", "DATE ADDED"] {
            let width = ctx.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(label.into(), egui::FontId::proportional(11.5), MUTED)
                    .size()
                    .x
            });
            // The label, the sort arrow (4 + 7 px) and a clear gap.
            assert!(width + 11.0 + 10.0 <= DATE_WIDTH, "{label}: {width}");
        }
    }

    #[test]
    fn narrow_lists_drop_the_date_first_then_the_album() {
        let both = Extra {
            album: true,
            date: true,
        };
        let wide = both.fit(800.0);
        assert!(wide.date && wide.album.is_some());
        assert!((wide.title + GAP + wide.album.unwrap() + GAP + DATE_WIDTH - 800.0).abs() < 0.01);
        let middle = both.fit(TITLE_MIN + GAP + ALBUM_MIN + 20.0);
        assert!(!middle.date && middle.album.is_some_and(|album| album >= ALBUM_MIN));
        assert!(middle.title >= TITLE_MIN);
        let narrow = both.fit(TITLE_MIN + 20.0);
        assert!(!narrow.date && narrow.album.is_none());
        assert_eq!(narrow.title, TITLE_MIN + 20.0);
        // A list without albums keeps its date longer.
        let dated = Extra {
            album: false,
            date: true,
        }
        .fit(TITLE_MIN + GAP + DATE_WIDTH);
        assert!(dated.date);
    }

    #[test]
    fn albums_sort_by_title_with_albumless_tracks_last() {
        let mut tracks = vec![
            track(1, "a", 1, None),
            track(2, "b", 1, None),
            track(3, "c", 1, None),
        ];
        for (track, album) in tracks.iter_mut().zip([Some("Beta"), None, Some("alpha")]) {
            track.album = album.map(|title| crate::model::AlbumRef {
                owner_id: -1,
                id: 1,
                title: title.into(),
                access_key: None,
                cover_url: None,
            });
        }
        let sort = Sort::default().clicked(Column::Album);
        assert_eq!(order(&tracks, 0..3, sort), [2, 0, 1]);
        assert_eq!(order(&tracks, 0..3, sort.clicked(Column::Album)), [0, 2, 1]);
    }

    #[test]
    fn dates_read_naturally_in_both_languages() {
        use chrono::TimeZone;
        let noon = chrono::Local
            .with_ymd_and_hms(2022, 8, 8, 12, 0, 0)
            .single()
            .unwrap()
            .timestamp();
        assert_eq!(date_label(noon, Language::Russian), "8 авг. 2022");
        assert_eq!(date_label(noon, Language::English), "Aug 8, 2022");
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    use crate::app::tests::{account_ui_frame, account_ui_text_rect, page, search_test_app};

    #[test]
    fn album_column_lines_up_with_rows_and_narrow_windows_drop_date_then_album() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        // Enough rows for the scrollbar the header leaves room for.
        let ids: Vec<i64> = (1..=40).collect();
        let mut tracks = page(ids.len(), &ids).items;
        for (index, track) in tracks.iter_mut().enumerate() {
            track.owner_id = 42;
            track.added = Some(1_660_000_000 + index as i64);
            track.album = Some(crate::model::AlbumRef {
                owner_id: -9,
                id: 5 + index as i64,
                title: format!("Record {index}"),
                access_key: None,
                cover_url: None,
            });
        }
        app.library.tracks = tracks;
        app.library.exhausted = true;
        app.page = MusicPage::Library;
        let mut frame = 0;
        let mut shown = Vec::new();
        for width in [1400.0, 940.0, 700.0] {
            let size = egui::vec2(width, 760.0);
            let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            for _ in 0..3 {
                output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
            }
            let album = account_ui_text_rect(&output, "ALBUM");
            let date = account_ui_text_rect(&output, "DATE ADDED");
            if let Some(header) = album {
                let row = account_ui_text_rect(&output, "Record 0").unwrap();
                assert!(
                    (row.left() - header.left()).abs() < 1.5,
                    "{row:?} {header:?}"
                );
                if let Some(date) = date {
                    assert!(date.left() > row.right());
                }
            } else {
                assert!(account_ui_text_rect(&output, "Record 0").is_none());
            }
            shown.push((album.is_some(), date.is_some()));
        }
        assert_eq!(shown, [(true, true), (true, false), (false, false)]);
    }
}
