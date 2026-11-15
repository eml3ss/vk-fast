//! The buttons beside a track follow the setting, and Download saves the
//! track into the chosen folder.
use super::*;
use crate::app::tests::{page, search_test_app};
use crate::track_buttons::{Button, Set};

fn row(
    ctx: &egui::Context,
    app: &mut MusicApp,
    track: &Track,
    tick: &mut u32,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Option<TrackAction>) {
    *tick += 1;
    let mut action = None;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 120.0),
            )),
            time: Some(f64::from(*tick) * 0.1),
            focused: true,
            events,
            ..Default::default()
        },
        |ui| action = app.track_row(ui, track, 0, false, track_sort::Extra::default()),
    );
    output.textures_delta.clear();
    (output, action)
}

/// Clicks the button just left of the duration: the rightmost one shown.
fn click_last_button(
    ctx: &egui::Context,
    app: &mut MusicApp,
    track: &Track,
    tick: &mut u32,
) -> Option<TrackAction> {
    let (output, _) = row(ctx, app, track, tick, Vec::new());
    let duration = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == track.duration_label() => {
                Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
            }
            _ => None,
        })
        .expect("the duration");
    let at = duration.left_center() - egui::vec2(6.0 + 11.0, 0.0);
    let mut action = None;
    for pressed in [true, false] {
        let events = vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        action = action.or(row(ctx, app, track, tick, events).1);
    }
    action
}

#[test]
fn a_rows_buttons_follow_the_setting_and_download_saves_into_the_folder() {
    let ctx = egui::Context::default();
    let mut app = search_test_app(&ctx);
    app.demo = false;
    let track = page(1, &[71]).items.remove(0);
    let mut tick = 0;
    // By default the heart sits next to the time (inactive without VK).
    assert!(!matches!(
        click_last_button(&ctx, &mut app, &track, &mut tick),
        Some(TrackAction::Menu(_, track_menu::Action::Download))
    ));
    // Only Download: it takes the heart's place, and the title gets the room.
    let only = Button::ALL.into_iter().fold(Set::default(), |set, button| {
        set.with(button, button == Button::Download)
    });
    app.track_buttons_lists = only;
    assert_eq!(
        download_app::controls_width(Set::default()) - download_app::controls_width(only),
        2.0 * 28.0
    );
    let action = click_last_button(&ctx, &mut app, &track, &mut tick);
    assert!(matches!(
        &action,
        Some(TrackAction::Menu(clicked, track_menu::Action::Download)) if clicked.key() == track.key()
    ));

    // The download goes to the chosen folder; without a VK client or a
    // link of its own there is nothing to save, and nothing is written.
    let folder = tempfile::tempdir().unwrap();
    app.download_dir = folder.path().to_string_lossy().into_owned();
    assert_eq!(app.download_dir(), folder.path());
    app.handle_track_action(&ctx, action.unwrap());
    assert!(app.downloading(track.key()));
    for worker in std::mem::take(&mut app.workers) {
        worker.join().unwrap();
    }
    app.poll_downloads();
    assert!(!app.downloading(track.key()));
    assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
    let (notice, _) = app.track_menu_notice.clone().unwrap();
    assert_eq!(notice, "Could not download the track");

    // The default folder is the system's Downloads.
    app.download_dir.clear();
    assert_eq!(app.download_dir(), crate::downloads::default_dir());
}

#[test]
fn the_player_and_lists_choose_their_buttons_separately_in_settings() {
    let ctx = egui::Context::default();
    let mut app = search_test_app(&ctx);
    let mut tick = 0;
    let mut frame = |app: &mut MusicApp, events: Vec<egui::Event>| {
        tick += 1;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640.0, 400.0),
                )),
                time: Some(f64::from(tick) * 0.1),
                events,
                ..Default::default()
            },
            |ui| app.track_buttons_settings(ui),
        );
        output.textures_delta.clear();
        output
    };
    let output = frame(&mut app, Vec::new());
    let text = |label: &str| {
        output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == label => {
                Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
            }
            _ => None,
        })
    };
    let download = text("Download").expect("the Download row");
    let lists = text("In lists").expect("the lists column");
    // The checkbox in the Download row under "In lists".
    let at = egui::pos2(lists.left() + 9.0, download.center().y);
    for pressed in [true, false] {
        frame(
            &mut app,
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    assert!(app.track_buttons_lists.has(Button::Download));
    assert!(
        !app.track_buttons_player.has(Button::Download),
        "only lists"
    );
}
