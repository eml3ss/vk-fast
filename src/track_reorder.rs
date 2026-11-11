//! Dragging tracks into a new order in My music and the account's own
//! playlists. The row moves at once; VK's audio.reorder confirms it in the
//! background and a refusal puts the row back.
use super::*;
use row_drag::Neighbour;

pub(super) struct Request {
    owner: i64,
    playlist: Option<AlbumRef>,
    key: (i64, i64),
    /// Where the row was, to put it back if VK refuses.
    undo: Neighbour<(i64, i64)>,
    canceled: Arc<AtomicBool>,
    receiver: Receiver<Result<(), WebAudioError>>,
}

impl Drop for Request {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
    }
}

impl MusicApp {
    fn reorder_ready(&self) -> bool {
        !self.demo
            && self.reorder_request.is_none()
            && self.filter.trim().is_empty()
            && !self.current_sort().active()
            && self
                .session
                .as_ref()
                .is_some_and(|session| session.music.is_some())
    }

    /// My music in VK's own order, with nothing filtered or sorted.
    pub(super) fn library_reorderable(&self) -> bool {
        self.page == MusicPage::Library && !self.library_tail_mode() && self.reorder_ready()
    }

    /// The account's own open playlist, in VK's order.
    /// A track listed twice would share its row's motion, so such
    /// playlists keep their order.
    pub(super) fn playlist_reorderable(&self) -> bool {
        if self.editable_own_playlist().is_none() || !self.reorder_ready() {
            return false;
        }
        let mut seen = HashSet::new();
        self.view_tracks()
            .iter()
            .all(|track| seen.insert(track.key()))
    }

    /// A row dropped at a new place in the open list.
    pub(super) fn reorder_view(&mut self, ctx: &egui::Context, moved: row_drag::Move) {
        let playlist = if self.library_reorderable() {
            None
        } else if self.playlist_reorderable() {
            self.editable_own_playlist()
        } else {
            return;
        };
        let visible = self
            .track_view_cache
            .visible(self.view_tracks(), &self.filter);
        let tracks = self.view_tracks();
        let key_at = |row: usize| {
            tracks
                .get(visible.source_index(row))
                .map_or((0, 0), Track::key)
        };
        let Some(neighbour) = moved.neighbour(key_at) else {
            return;
        };
        let key = key_at(moved.from);
        let Some(owner) = self.session.as_ref().map(|session| session.profile.id) else {
            return;
        };
        let Some(undo) = self.move_row(playlist.as_ref(), key, neighbour) else {
            return;
        };
        let Some(client) = self
            .session
            .as_ref()
            .and_then(|session| session.music.clone())
        else {
            self.move_row(playlist.as_ref(), key, undo);
            return;
        };
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_cancel = canceled.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let repaint = self.repaint_target.with_context(ctx);
        let worker_playlist = playlist.clone();
        let vk_neighbour = match neighbour {
            Neighbour::After(other) => crate::web_audio::Neighbour::After(other),
            Neighbour::Before(other) => crate::web_audio::Neighbour::Before(other),
        };
        self.workers.retain(|worker| !worker.is_finished());
        match thread::Builder::new()
            .name("vk-reorder".into())
            .spawn(move || {
                let result = client.reorder_audio(
                    owner,
                    key,
                    worker_playlist.as_ref(),
                    vk_neighbour,
                    &worker_cancel,
                );
                let _ = sender.send(result);
                repaint.request_repaint();
            }) {
            Ok(worker) => {
                self.workers.push(worker);
                if playlist.is_none() {
                    self.save_library(ctx);
                }
                self.reorder_request = Some(Request {
                    owner,
                    playlist,
                    key,
                    undo,
                    canceled,
                    receiver,
                });
            }
            Err(_) => {
                self.move_row(playlist.as_ref(), key, undo);
                self.track_menu_error = Some(WebAudioError::Network);
            }
        }
    }

    pub(super) fn poll_reorder(&mut self, ctx: &egui::Context) {
        let Some(request) = &self.reorder_request else {
            return;
        };
        let result = match request.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(WebAudioError::Network),
        };
        let request = self.reorder_request.take().unwrap();
        if self.session.as_ref().map(|session| session.profile.id) != Some(request.owner) {
            return;
        }
        match result {
            Ok(()) => {}
            Err(WebAudioError::Canceled) => {}
            Err(error) => {
                self.move_row(request.playlist.as_ref(), request.key, request.undo);
                if request.playlist.is_none() {
                    self.save_library(ctx);
                }
                self.track_menu_error = Some(error);
                ctx.request_repaint();
            }
        }
    }

    /// Moves a row in My music or a loaded playlist; returns where it was.
    fn move_row(
        &mut self,
        playlist: Option<&AlbumRef>,
        key: (i64, i64),
        neighbour: Neighbour<(i64, i64)>,
    ) -> Option<Neighbour<(i64, i64)>> {
        let tracks = match playlist {
            None => &mut self.library.tracks,
            Some(playlist) => {
                let detail = MusicPage::Playlist(playlist.clone()).detail_key()?;
                match Arc::make_mut(self.details.get_mut(&detail)?) {
                    DetailPage::Playlist(data) => &mut data.tracks,
                    _ => return None,
                }
            }
        };
        let index = tracks.iter().position(|track| track.key() == key)?;
        let undo = row_drag::current_neighbour(tracks, Track::key, index)?;
        if !row_drag::apply(tracks, Track::key, key, neighbour) {
            return None;
        }
        self.track_view_cache.invalidate();
        self.sort_cache = None;
        self.synchronize_order();
        Some(undo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{page, search_test_app};

    fn reply(
        app: &mut MusicApp,
        key: (i64, i64),
        undo: Neighbour<(i64, i64)>,
        result: Result<(), WebAudioError>,
    ) {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(result).ok().unwrap();
        app.reorder_request = Some(Request {
            owner: 42,
            playlist: None,
            key,
            undo,
            canceled: Arc::new(AtomicBool::new(false)),
            receiver,
        });
    }

    fn library_ids(app: &MusicApp) -> Vec<i64> {
        app.library.tracks.iter().map(|track| track.id).collect()
    }

    #[test]
    fn my_music_rows_move_at_once_and_a_refusal_puts_them_back() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        let mut tracks = page(4, &[1, 2, 3, 4]).items;
        for track in &mut tracks {
            track.owner_id = 42;
        }
        app.library.tracks = tracks;
        app.page = MusicPage::Library;
        let undo = app
            .move_row(None, (42, 1), Neighbour::After((42, 3)))
            .unwrap();
        assert_eq!(library_ids(&app), [2, 3, 1, 4]);
        assert_eq!(undo, Neighbour::Before((42, 2)));
        reply(&mut app, (42, 1), undo, Err(WebAudioError::Rejected));
        app.poll_reorder(&ctx);
        assert_eq!(library_ids(&app), [1, 2, 3, 4]);
        assert_eq!(app.track_menu_error, Some(WebAudioError::Rejected));
        app.move_row(None, (42, 4), Neighbour::Before((42, 1)));
        reply(&mut app, (42, 4), Neighbour::After((42, 3)), Ok(()));
        app.poll_reorder(&ctx);
        assert_eq!(library_ids(&app), [4, 1, 2, 3]);
        assert!(app.reorder_request.is_none());
    }

    #[test]
    fn only_my_music_and_own_playlists_in_vk_order_can_be_reordered() {
        let ctx = egui::Context::default();
        let mut app = search_test_app(&ctx);
        app.demo = false;
        app.page = MusicPage::Library;
        // The test session has no VK connection.
        assert!(!app.library_reorderable());
        let mut foreign = AlbumRef {
            owner_id: -7,
            id: 3,
            title: "Theirs".into(),
            access_key: None,
            cover_url: None,
        };
        app.page = MusicPage::Playlist(foreign.clone());
        assert!(app.own_playlist().is_none());
        foreign.owner_id = 42;
        app.page = MusicPage::Playlist(foreign);
        assert!(app.own_playlist().is_some());
        app.page = MusicPage::Search("x".into());
        assert!(!app.library_reorderable() && !app.playlist_reorderable());
    }
}

#[cfg(test)]
mod drag_tests {
    use super::*;
    use crate::app::tests::{account_ui_app, account_ui_frame, account_ui_text_rect, page};

    #[test]
    fn a_row_carried_out_of_my_music_joins_the_queue_or_an_own_playlist() {
        let ctx = egui::Context::default();
        let mut app = account_ui_app(&ctx, Language::English);
        let client = WebAudioClient::new(vec![crate::web_session::SessionCookie {
            domain: ".vk.ru".into(),
            path: "/".into(),
            name: "remixsid".into(),
            value: "synthetic-carry-test".into(),
            secure: true,
            expires_unix: None,
        }])
        .unwrap();
        app.session = Some(Session {
            profile: Profile {
                id: 42,
                first_name: "Test".into(),
                last_name: "Account".into(),
                avatar_url: None,
            },
            cache_warning: false,
            music: Some(client),
        });
        let mut tracks = page(4, &[1, 2, 3, 4]).items;
        for (index, track) in tracks.iter_mut().enumerate() {
            track.owner_id = 42;
            track.title = format!("Carried {index}");
            track.subtitle = Some(String::new());
            track.library_liked = Some(true);
            if track.artists.is_empty() {
                track.artists = vec![crate::model::ArtistRef {
                    id: "synthetic".into(),
                    name: "Artist".into(),
                }];
            }
        }
        app.library.tracks = tracks;
        app.library.exhausted = true;
        app.page = MusicPage::Library;
        app.queue_ui.open = true;
        // An own playlist, deliberately not in the editable list: the drop
        // reaches "add to playlist", which then refuses without asking VK.
        app.playlist_items = vec![crate::playlist_ui::PlaylistItem {
            reference: AlbumRef {
                id: 9,
                owner_id: 42,
                title: "Drop list".into(),
                access_key: None,
                cover_url: None,
            },
            kind: crate::web_audio::CollectionKind::Playlist,
            subtitle: String::new(),
            can_edit: true,
            pinned: false,
        }];
        assert!(app.library_reorderable());
        let size = egui::vec2(1300.0, 760.0);
        let mut frame = 0;
        let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        for _ in 0..3 {
            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        }
        let press = |pos, pressed| {
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        let page = app.page_rect;
        // The title in the list: the queue panel lists the same tracks.
        let in_list = |output: &egui::FullOutput, title: &str| {
            crate::app::tests::account_ui_shapes(output)
                .into_iter()
                .find_map(|shape| match shape {
                    egui::Shape::Text(text)
                        if text.galley.text() == title
                            && page.contains(text.visual_bounding_rect().center()) =>
                    {
                        Some(text.visual_bounding_rect())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing {title} in the list"))
        };
        // Carries the row from its title to `to` in small steps.
        let carry = |app: &mut MusicApp, frame: &mut u32, from: egui::Rect, to: egui::Pos2| {
            let start = egui::pos2(from.right() + 40.0, from.center().y);
            account_ui_frame(&ctx, app, size, frame, press(start, true));
            let mut last = egui::FullOutput::default();
            for step in 1..=12 {
                let at = start + (to - start) * (step as f32 / 12.0);
                last =
                    account_ui_frame(&ctx, app, size, frame, vec![egui::Event::PointerMoved(at)]);
            }
            for _ in 0..3 {
                last =
                    account_ui_frame(&ctx, app, size, frame, vec![egui::Event::PointerMoved(to)]);
            }
            account_ui_frame(&ctx, app, size, frame, press(to, false));
            let mut settled = egui::FullOutput::default();
            for _ in 0..2 {
                settled = account_ui_frame(&ctx, app, size, frame, Vec::new());
            }
            (last, settled)
        };

        let queue = egui::pos2(size.x - 120.0, size.y * 0.4);
        let from = in_list(&output, "Carried 0");
        let (carrying, settled) = carry(&mut app, &mut frame, from, queue);
        output = settled;
        // Away from the list: a small card with the title at the pointer,
        // while the row waits in its place.
        let titles: Vec<_> = crate::app::tests::account_ui_shapes(&carrying)
            .into_iter()
            .filter_map(|shape| match shape {
                egui::Shape::Text(text) if text.galley.text() == "Carried 0" => {
                    Some(text.visual_bounding_rect())
                }
                _ => None,
            })
            .collect();
        assert!(
            titles
                .iter()
                .any(|rect| rect.left() > page.right() && (rect.center().y - queue.y).abs() < 24.0),
            "the card follows the pointer: {titles:?}"
        );
        assert!(
            titles.iter().any(|rect| page.contains(rect.center())),
            "the row stays in the list: {titles:?}"
        );
        let first = app.library.tracks[0].key();
        assert!(
            app.play_order
                .queued()
                .iter()
                .any(|entry| entry.key == first)
        );
        assert!(crate::track_drag::carried(&ctx).is_none());

        let list = account_ui_text_rect(&output, "Drop list").unwrap();
        let from = in_list(&output, "Carried 1");
        let (_, settled) = carry(&mut app, &mut frame, from, list.center());
        output = settled;
        assert_eq!(app.track_menu_error, Some(WebAudioError::Rejected));

        // A filtered list cannot be reordered, but its rows still carry.
        app.filter = "Carried".into();
        assert!(!app.library_reorderable());
        for _ in 0..2 {
            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        }
        let third = app.library.tracks[2].key();
        let from = in_list(&output, "Carried 2");
        carry(&mut app, &mut frame, from, queue);
        assert!(
            app.play_order
                .queued()
                .iter()
                .any(|entry| entry.key == third)
        );

        // Neither drop moved a row, and nothing went to VK.
        let ids: Vec<_> = app.library.tracks.iter().map(|track| track.id).collect();
        assert_eq!(ids, [1, 2, 3, 4]);
        assert!(app.reorder_request.is_none() && app.workers.is_empty());
    }

    #[test]
    fn my_music_rows_lift_and_slide_and_escape_cancels_without_vk() {
        let ctx = egui::Context::default();
        let mut app = account_ui_app(&ctx, Language::English);
        let client = WebAudioClient::new(vec![crate::web_session::SessionCookie {
            domain: ".vk.ru".into(),
            path: "/".into(),
            name: "remixsid".into(),
            value: "synthetic-reorder-test".into(),
            secure: true,
            expires_unix: None,
        }])
        .unwrap();
        app.session = Some(Session {
            profile: Profile {
                id: 42,
                first_name: "Test".into(),
                last_name: "Account".into(),
                avatar_url: None,
            },
            cache_warning: false,
            music: Some(client),
        });
        let mut tracks = page(4, &[1, 2, 3, 4]).items;
        for (index, track) in tracks.iter_mut().enumerate() {
            track.owner_id = 42;
            track.title = format!("Mine {index}");
            // Complete rows: nothing to fetch from VK.
            track.subtitle = Some(String::new());
            track.library_liked = Some(true);
            if track.artists.is_empty() {
                track.artists = vec![crate::model::ArtistRef {
                    id: "synthetic".into(),
                    name: "Artist".into(),
                }];
            }
        }
        app.library.tracks = tracks;
        app.library.exhausted = true;
        app.page = MusicPage::Library;
        assert!(app.library_reorderable());
        let size = egui::vec2(1100.0, 760.0);
        let mut frame = 0;
        let mut output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        for _ in 0..3 {
            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        }
        let first = account_ui_text_rect(&output, "Mine 0").unwrap();
        let second = account_ui_text_rect(&output, "Mine 1").unwrap();
        // Press beside the title, away from links and buttons.
        let start = egui::pos2(first.right() + 40.0, first.center().y);
        let press = |pos, pressed| {
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        account_ui_frame(&ctx, &mut app, size, &mut frame, press(start, true));
        let mut at = start;
        for _ in 0..6 {
            at.y += 12.0;
            output = account_ui_frame(
                &ctx,
                &mut app,
                size,
                &mut frame,
                vec![egui::Event::PointerMoved(at)],
            );
        }
        let lifted = account_ui_text_rect(&output, "Mine 0").unwrap();
        assert!(
            (lifted.center().y - (first.center().y + 72.0)).abs() < 2.0,
            "the row follows the pointer: {lifted:?}"
        );
        for _ in 0..4 {
            output = account_ui_frame(&ctx, &mut app, size, &mut frame, Vec::new());
        }
        let slid = account_ui_text_rect(&output, "Mine 1").unwrap();
        assert!(
            slid.top() < second.top() - 40.0,
            "the neighbour slides up into the gap"
        );
        account_ui_frame(
            &ctx,
            &mut app,
            size,
            &mut frame,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        account_ui_frame(&ctx, &mut app, size, &mut frame, press(at, false));
        let ids: Vec<_> = app.library.tracks.iter().map(|track| track.id).collect();
        assert_eq!(ids, [1, 2, 3, 4]);
        assert!(app.reorder_request.is_none() && app.workers.is_empty());
    }
}
