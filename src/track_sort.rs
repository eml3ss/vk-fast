//! Sorting a track list from its column header: title, date added (only
//! where VK reports it) and duration. A third click returns VK's order.
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Column {
    #[default]
    Original,
    Title,
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

/// Width of the date column in a track row.
pub(super) const DATE_WIDTH: f32 = 112.0;
/// The middle of a row's duration, from the row's inner right edge.
const DURATION_CENTER: f32 = 36.0;
/// Width the row keeps on the right for mix, dislike, heart and duration.
const CONTROLS_WIDTH: f32 = 142.0;

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
    let titles: HashMap<usize, String> = if sort.column == Column::Title {
        rows.iter()
            .map(|index| (*index, tracks[*index].display_title().to_lowercase()))
            .collect()
    } else {
        HashMap::new()
    };
    rows.sort_by(|a, b| {
        let ordering = match sort.column {
            Column::Original => std::cmp::Ordering::Equal,
            Column::Title => titles[a].cmp(&titles[b]),
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
    show_date: bool,
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
    let title_width = (inner.width()
        - index_width
        - 6.0
        - 40.0
        - 6.0
        - CONTROLS_WIDTH
        - if show_date { DATE_WIDTH + 6.0 } else { 0.0 })
    .max(0.0);
    let date_left = title_left + 40.0 + 6.0 + title_width + 6.0;
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
    if show_date {
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
