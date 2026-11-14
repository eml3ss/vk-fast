//! The context menu of a picture opened in the viewer (an album cover, a
//! user's or community's avatar): Copy image and Save as. The file comes
//! from VK on a worker thread (or, without the network, the largest copy on
//! disk); the save dialog is the system's and also runs on the worker.
use std::{
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use eframe::egui::{self, ColorImage};

use crate::{cover_cache::DiskCache, i18n::Language, icons::Icon, theme, track_menu};

/// How long the line under the picture says what happened.
const NOTICE: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Copied,
    Saved,
    LoadFailed,
    SaveFailed,
}

enum Reply {
    /// Put on the clipboard by the UI thread, which owns the clipboard.
    Image(ColorImage),
    Done(Outcome),
    /// The save dialog was closed without a file.
    Canceled,
}

/// Shared with the worker: whether one is running, and its answer.
#[derive(Default)]
struct Task {
    busy: bool,
    reply: Option<Reply>,
    shown: Option<(Outcome, Instant)>,
}

type Shared = Arc<Mutex<Task>>;

/// A file name from the page's title: no path separators or characters
/// systems refuse, never empty.
fn file_name(title: &str, extension: &str) -> String {
    let name: String = title
        .chars()
        .map(|c| {
            if c.is_control() || r#"/\:*?"<>|"#.contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(100)
        .collect::<String>()
        .trim_matches(['.', ' '])
        .to_owned();
    let name = if name.is_empty() {
        "cover".into()
    } else {
        name
    };
    format!("{name}.{extension}")
}

fn spawn(
    shared: &Shared,
    ctx: &egui::Context,
    disk: Arc<DiskCache>,
    url: &str,
    save: Option<(String, Language)>,
) {
    let Ok(mut task) = shared.lock() else { return };
    if task.busy {
        return;
    }
    task.busy = true;
    task.shown = None;
    drop(task);
    let worker = shared.clone();
    let ctx = ctx.clone();
    let url = url.to_owned();
    let started = thread::Builder::new()
        .name("vk-image-menu".into())
        .spawn(move || {
            let reply = match crate::cover_art::original(&disk, &url) {
                Err(()) => Reply::Done(Outcome::LoadFailed),
                Ok(bytes) => match save {
                    None => crate::cover_art::decode_full(&bytes)
                        .map_or(Reply::Done(Outcome::LoadFailed), Reply::Image),
                    Some((title, language)) => save_as(&bytes, &title, language),
                },
            };
            if let Ok(mut task) = worker.lock() {
                task.busy = false;
                task.reply = Some(reply);
            }
            ctx.request_repaint();
        });
    if started.is_err()
        && let Ok(mut task) = shared.lock()
    {
        task.busy = false;
        task.shown = Some((Outcome::LoadFailed, Instant::now()));
    }
}

fn save_as(bytes: &[u8], title: &str, language: Language) -> Reply {
    let extension = crate::cover_art::image_extension(bytes).unwrap_or("jpg");
    let Some(path) = rfd::FileDialog::new()
        .set_title(language.text("Сохранить изображение", "Save image"))
        .set_file_name(file_name(title, extension))
        .add_filter(language.text("Изображение", "Image"), &[extension])
        .save_file()
    else {
        return Reply::Canceled;
    };
    match std::fs::write(&path, bytes) {
        Ok(()) => Reply::Done(Outcome::Saved),
        Err(_) => Reply::Done(Outcome::SaveFailed),
    }
}

/// The menu on `image`, the picture of `url` in the viewer, and the line
/// under it saying how the last copy or save went. `title` names the file.
pub fn show(
    ui: &mut egui::Ui,
    image: &egui::Response,
    disk: Arc<DiskCache>,
    url: &str,
    title: &str,
    language: Language,
) {
    let id = egui::Id::new(("image-menu", url));
    let shared: Shared = ui.data_mut(|data| data.get_temp_mut_or_default::<Shared>(id).clone());
    let busy = shared.lock().is_ok_and(|mut task| {
        match task.reply.take() {
            Some(Reply::Image(picture)) => {
                ui.ctx().copy_image(picture);
                task.shown = Some((Outcome::Copied, Instant::now()));
            }
            Some(Reply::Done(outcome)) => task.shown = Some((outcome, Instant::now())),
            Some(Reply::Canceled) | None => {}
        }
        task.busy
    });
    if let Some(menu) = egui::Popup::context_menu(image)
        .id(id.with("menu"))
        .show(|ui| {
            ui.set_width(240.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            let copy = language.text("Копировать изображение", "Copy image");
            let save = language.text("Сохранить как…", "Save as…");
            let chosen = if track_menu::row(ui, Icon::Copy, copy, !busy, false, false).clicked() {
                Some(false)
            } else if track_menu::row(ui, Icon::Save, save, !busy, false, false).clicked() {
                Some(true)
            } else {
                None
            };
            if chosen.is_some() {
                ui.close();
            }
            chosen
        })
        && let Some(save) = menu.inner
    {
        let save = save.then(|| (title.to_owned(), language));
        spawn(&shared, ui.ctx(), disk, url, save);
    }
    let (busy, shown) = shared
        .lock()
        .map_or((false, None), |task| (task.busy, task.shown));
    let text = if busy {
        Some((
            language.text("Загрузка изображения…", "Loading the image…"),
            false,
        ))
    } else {
        shown
            .filter(|(_, at)| at.elapsed() < NOTICE)
            .map(|(outcome, at)| {
                ui.ctx()
                    .request_repaint_after(NOTICE.saturating_sub(at.elapsed()));
                match outcome {
                    Outcome::Copied => (
                        language.text("Изображение скопировано", "Image copied"),
                        false,
                    ),
                    Outcome::Saved => {
                        (language.text("Изображение сохранено", "Image saved"), false)
                    }
                    Outcome::LoadFailed => (
                        language.text(
                            "Не удалось загрузить изображение",
                            "Could not load the image",
                        ),
                        true,
                    ),
                    Outcome::SaveFailed => (
                        language.text("Не удалось сохранить файл", "Could not save the file"),
                        true,
                    ),
                }
            })
    };
    if let Some((text, error)) = text {
        // Under the picture, on the dimmed page: always light text.
        let color = if error {
            theme::error()
        } else {
            egui::Color32::from_gray(225)
        };
        ui.painter().with_clip_rect(ui.ctx().content_rect()).text(
            image.rect.center_bottom() + egui::vec2(0.0, 22.0),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional(14.0),
            color,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_picture_is_named_after_the_page_without_unsafe_characters() {
        assert_eq!(
            file_name("Album: Live / 2024", "jpg"),
            "Album Live 2024.jpg"
        );
        assert_eq!(file_name("  \u{7}  ", "png"), "cover.png");
        assert_eq!(file_name("...", "webp"), "cover.webp");
        assert_eq!(file_name(&"a".repeat(300), "jpg").len(), 104);
    }
}
