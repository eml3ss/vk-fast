//! What the interface did, for the diagnostic log when it is on: each click,
//! toggle and slider change with the widget's own label (a track's or
//! playlist's name shows as `[content]`), shortcut keys, and every change of
//! the visible state (page, panels, dialogs, playback, errors, notices,
//! settings, window), plus frames slow enough to be felt. Typed text, search
//! queries, names and identifiers are never written.
use super::*;
use egui::output::OutputEvent;

/// A frame that takes longer than this is felt as a stall.
const SLOW_FRAME: Duration = Duration::from_millis(80);

#[derive(Default)]
pub(super) struct FrameLog {
    state: Vec<(&'static str, String)>,
    /// The window size last written, in steps of 50 points.
    size: Option<(i32, i32)>,
}

fn widget_kind(kind: egui::WidgetType) -> String {
    format!("{kind:?}")
}

impl MusicApp {
    /// Call last in the frame, with the time it started.
    pub(super) fn log_frame(&mut self, ctx: &egui::Context, started: Instant) {
        if !crate::diagnostics::enabled() {
            return;
        }
        self.log_ui_events(ctx);
        self.log_state(ctx);
        let spent = started.elapsed();
        if spent >= SLOW_FRAME {
            crate::diagnostics::event(
                "ui",
                &format!(
                    "slow frame ms={} page={}",
                    spent.as_millis(),
                    page_kind(&self.page)
                ),
            );
        }
    }

    fn log_ui_events(&self, ctx: &egui::Context) {
        let page = page_kind(&self.page);
        let events = ctx.output(|output| output.events.clone());
        for event in events {
            let (what, info) = match &event {
                OutputEvent::Clicked(info) => ("click", info),
                OutputEvent::DoubleClicked(info) => ("double click", info),
                OutputEvent::TripleClicked(info) => ("triple click", info),
                OutputEvent::ValueChanged(info) => ("change", info),
                // Buttons take focus on every click; only text fields tell
                // something by it.
                OutputEvent::FocusGained(info) if info.typ == egui::WidgetType::TextEdit => {
                    ("focus", info)
                }
                _ => continue,
            };
            let mut line = format!(
                "{what} {} {}",
                widget_kind(info.typ),
                info.label
                    .as_deref()
                    .map_or_else(|| "[no label]".into(), crate::diagnostics::ui_label)
            );
            if let Some(selected) = info.selected {
                line.push_str(if selected { " on" } else { " off" });
            }
            // Numbers only: a text field's value is what was typed.
            if let Some(value) = info
                .value
                .filter(|_| info.typ != egui::WidgetType::TextEdit)
            {
                line.push_str(&format!(" value={value:.2}"));
            }
            if !info.enabled {
                line.push_str(" disabled");
            }
            line.push_str(&format!(" page={page}"));
            crate::diagnostics::event("ui", &line);
        }
        ctx.input(|input| {
            for event in &input.events {
                match event {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        repeat: false,
                        modifiers,
                        ..
                    } if shortcut_key(*key, *modifiers) => {
                        crate::diagnostics::event(
                            "ui",
                            &format!("key {}{key:?} page={page}", modifier_prefix(*modifiers)),
                        );
                    }
                    egui::Event::PointerButton {
                        button:
                            button @ (egui::PointerButton::Secondary | egui::PointerButton::Middle),
                        pressed: true,
                        pos,
                        ..
                    } => {
                        crate::diagnostics::event(
                            "ui",
                            &format!(
                                "{button:?} press x={} y={} page={page}",
                                pos.x.round(),
                                pos.y.round()
                            ),
                        );
                    }
                    _ => {}
                }
            }
        });
    }

    fn log_state(&mut self, ctx: &egui::Context) {
        let state = self.diagnostic_state(ctx);
        let mut log = std::mem::take(&mut self.frame_log);
        for (name, value) in &state {
            let before = log
                .state
                .iter()
                .find(|(previous, _)| previous == name)
                .map(|(_, value)| value);
            if before != Some(value) {
                let line = match before {
                    Some(before) => format!("{name}: {before} -> {value}"),
                    None => format!("{name}: {value}"),
                };
                crate::diagnostics::event("state", &line);
            }
        }
        log.state = state;
        // The window size, in coarse steps so that a drag writes a few lines.
        let size = ctx.content_rect().size();
        let stepped = (
            (size.x / 50.0).round() as i32,
            (size.y / 50.0).round() as i32,
        );
        if log.size != Some(stepped) {
            log.size = Some(stepped);
            crate::diagnostics::event(
                "state",
                &format!(
                    "window: {}x{} points, scale {:.2}",
                    size.x.round(),
                    size.y.round(),
                    ctx.pixels_per_point()
                ),
            );
        }
        self.frame_log = log;
    }

    /// The visible state, as names of kinds and counts: no titles, names or
    /// identifiers.
    fn diagnostic_state(&self, ctx: &egui::Context) -> Vec<(&'static str, String)> {
        let error = |error: Option<&WebAudioError>| {
            error.map_or_else(|| "none".into(), |error| format!("{error:?}"))
        };
        let ui_error = |error: Option<&UiError>| {
            error.map_or_else(|| "none".into(), |error| format!("{error:?}"))
        };
        let snapshot = self.player.snapshot();
        let viewport = ctx.input(|input| {
            (
                input.viewport().maximized.unwrap_or(false),
                input.viewport().fullscreen.unwrap_or(false),
                input.viewport().focused.unwrap_or(true),
            )
        });
        vec![
            ("page", page_kind(&self.page).into()),
            (
                "settings",
                if self.settings_open {
                    format!("open {}", self.settings_section.title(Language::English))
                } else {
                    "closed".into()
                },
            ),
            ("queue panel", self.queue_ui.open.to_string()),
            (
                "lyrics",
                if self.lyrics.fullscreen {
                    "fullscreen".into()
                } else {
                    self.lyrics.open.to_string()
                },
            ),
            ("lyrics state", self.lyrics.diagnostic()),
            (
                "dialogs",
                [
                    (self.qr_open, "qr sign-in"),
                    (self.mix_settings.open, "mix settings"),
                    (self.playlist_create_form.is_some(), "new playlist"),
                    (self.playlist_controls.has_modal(), "playlist edit"),
                    (self.audio_upload.has_modal(), "upload"),
                ]
                .into_iter()
                .filter_map(|(open, name)| open.then_some(name))
                .collect::<Vec<_>>()
                .join(", ")
                .or_none(),
            ),
            (
                "account",
                match &self.session {
                    _ if self.demo => "demo".into(),
                    None => "signed out".into(),
                    Some(session) if session.music.is_none() => "signed in, no music".into(),
                    Some(_) => "signed in".into(),
                },
            ),
            (
                "library",
                format!(
                    "{} tracks loaded{}, {} playlists",
                    self.library.tracks.len(),
                    if self.library.exhausted { " (all)" } else { "" },
                    self.playlist_items.len()
                ),
            ),
            (
                "playback",
                format!(
                    "{:?}{}{}",
                    snapshot.state,
                    if self.playback_pending.is_some() {
                        ", resolving"
                    } else {
                        ""
                    },
                    if self.playing.is_some() {
                        ", track set"
                    } else {
                        ""
                    }
                ),
            ),
            (
                "modes",
                format!(
                    "shuffle={} repeat={:?} volume={:.2}",
                    self.play_order.shuffle(),
                    self.play_order.repeat(),
                    snapshot.volume
                ),
            ),
            (
                "queue",
                format!(
                    "source={} added={}{}",
                    self.queue_origin.as_deref().map_or("none", origin_kind),
                    self.play_order.queued().len(),
                    if self.prepared_mix.is_some() {
                        ", mix ahead"
                    } else {
                        ""
                    }
                ),
            ),
            (
                "mix",
                if self.mix_active {
                    format!(
                        "{} more={}{}",
                        match self.mix_seed {
                            None => "vk mix",
                            Some(MixSeed::Track(_)) => "track mix",
                            Some(MixSeed::Playlist(_)) => "playlist mix",
                            Some(MixSeed::Catalog(_)) => "catalog mix",
                        },
                        self.mix_has_more,
                        if self.mix_request.is_some() {
                            ", loading"
                        } else {
                            ""
                        }
                    )
                } else if self.mix_request.is_some() {
                    "starting".into()
                } else {
                    "off".into()
                },
            ),
            ("error auth", ui_error(self.auth_error.as_ref())),
            ("error music", ui_error(self.audio_error.as_ref())),
            ("error playback", ui_error(self.playback_error.as_ref())),
            ("error qr", ui_error(self.qr_error.as_ref())),
            ("error home", error(self.home_error.as_ref())),
            ("error page", error(self.detail_error.as_ref())),
            ("error playlists", error(self.playlist_error.as_ref())),
            ("error track menu", error(self.track_menu_error.as_ref())),
            ("error like", error(self.like_error.as_ref())),
            ("error mix", error(self.mix_error.as_ref())),
            ("error search", error(self.search.results_error.as_ref())),
            (
                "notice",
                self.track_menu_notice.as_ref().map_or_else(
                    || "none".into(),
                    |(text, _)| crate::diagnostics::ui_label(text),
                ),
            ),
            (
                "window",
                format!(
                    "maximized={} fullscreen={} focused={}",
                    viewport.0, viewport.1, viewport.2
                ),
            ),
            (
                "settings values",
                format!(
                    "language={:?} theme={} accent={} tray={} discord={} broadcast={} \
                     titlebar={} compact_seek={} double_click={} search_on_enter={} \
                     autoscroll={} explicit={} backdrop={} volume_keys={} mix_after_queue={} \
                     equalizer={}:{} crossfade={}",
                    self.language,
                    self.theme.diagnostic_code(),
                    // A picked colour is logged only as custom.
                    crate::palette::Accent::of(self.accent)
                        .map_or("custom", crate::palette::Accent::code),
                    self.minimize_to_tray,
                    self.discord_rpc_enabled,
                    self.broadcast,
                    self.system_titlebar,
                    self.compact_seek_bar,
                    self.double_click_to_play,
                    self.search_on_enter,
                    self.middle_autoscroll,
                    self.explicit_badge,
                    self.animated_backdrop,
                    self.volume_keys_enabled,
                    self.mix_after_queue,
                    if self.equalizer.enabled { "on" } else { "off" },
                    // A preset's kind only: names are the user's text, and
                    // gains change every frame of a drag.
                    self.equalizer.builtin().map_or(
                        if self.equalizer.custom().is_some() {
                            "custom"
                        } else {
                            "manual"
                        },
                        crate::equalizer::Builtin::id
                    ),
                    if self.crossfade {
                        format!("{}s", self.crossfade_seconds)
                    } else {
                        "off".to_owned()
                    }
                ),
            ),
        ]
    }
}

trait OrNone {
    /// "none" for an empty list.
    fn or_none(self) -> String;
}

impl OrNone for String {
    fn or_none(self) -> String {
        if self.is_empty() { "none".into() } else { self }
    }
}

/// The kind of page, without what it shows.
fn page_kind(page: &MusicPage) -> &'static str {
    match page {
        MusicPage::Home => "Home",
        MusicPage::Search(query) if query.is_empty() => "Search",
        MusicPage::Search(_) => "Search results",
        MusicPage::Library => "My music",
        MusicPage::Recent => "Recent",
        MusicPage::Collection(_) => "Collection",
        MusicPage::Artist(_) => "Artist",
        MusicPage::Album(_) => "Album",
        MusicPage::Playlist(_) => "Playlist",
        MusicPage::Owner(_) => "Owner",
        MusicPage::TrackAlbum(..) => "Release",
        MusicPage::ArtistLibrary(_) => "Artist in My music",
    }
}

/// The kind of list a queue came from, without its identity.
fn origin_kind(origin: &str) -> &'static str {
    let kind = origin.split(':').next().unwrap_or_default();
    match kind {
        "library" => "my music",
        "recent" => "recent",
        "search" => "search",
        "collection" => "collection",
        "artist" | "artist-name" => "artist",
        "artist-library" => "artist in my music",
        "album" => "album",
        "playlist" => "playlist",
        "owner" => "owner",
        "mix" => "vk mix",
        "track-mix" => "track mix",
        "playlist-mix" => "playlist mix",
        "catalog-mix" => "catalog mix",
        _ => "other",
    }
}

/// Keys that act, never ones that type: special keys, and any key with Ctrl,
/// Alt or Command.
fn shortcut_key(key: egui::Key, modifiers: egui::Modifiers) -> bool {
    use egui::Key;
    modifiers.ctrl
        || modifiers.alt
        || modifiers.mac_cmd
        || matches!(
            key,
            Key::Escape
                | Key::Enter
                | Key::Space
                | Key::Tab
                | Key::ArrowUp
                | Key::ArrowDown
                | Key::ArrowLeft
                | Key::ArrowRight
                | Key::PageUp
                | Key::PageDown
                | Key::Home
                | Key::End
                | Key::Delete
                | Key::F1
                | Key::F2
                | Key::F3
                | Key::F4
                | Key::F5
                | Key::F6
                | Key::F7
                | Key::F8
                | Key::F9
                | Key::F10
                | Key::F11
                | Key::F12
        )
}

fn modifier_prefix(modifiers: egui::Modifiers) -> String {
    let mut prefix = String::new();
    for (held, name) in [
        (modifiers.ctrl, "Ctrl+"),
        (modifiers.alt, "Alt+"),
        (modifiers.shift, "Shift+"),
        (modifiers.mac_cmd, "Cmd+"),
    ] {
        if held {
            prefix.push_str(name);
        }
    }
    prefix
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicks_are_logged_with_interface_labels_but_never_track_names() {
        let ((), log) = crate::diagnostics::capture_for_test(|| {
            let ctx = egui::Context::default();
            let mut app = crate::app::tests::account_ui_app(&ctx, Language::English);
            app.demo = true;
            app.library.tracks = crate::model::demo_tracks();
            app.library.tracks[0].title = "Secret Song Title".into();
            let mut output =
                ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1040.0, 720.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        // The interface draws once, so its words are known.
                        app.render(ui);
                        ui.ctx().output_mut(|output| {
                            output.events.clear();
                            for label in ["Settings", "Secret Song Title"] {
                                output.events.push(OutputEvent::Clicked(
                                    egui::WidgetInfo::labeled(
                                        egui::WidgetType::Button,
                                        true,
                                        label,
                                    ),
                                ));
                            }
                            output.events.push(OutputEvent::ValueChanged(
                                egui::WidgetInfo::text_edit(true, "", "typed search words", ""),
                            ));
                        });
                        app.log_frame(ui.ctx(), Instant::now());
                    },
                );
            output.textures_delta.clear();
        });
        assert!(
            log.contains("click Button «Settings» page=My music"),
            "{log}"
        );
        assert!(
            log.contains("click Button [content] page=My music"),
            "{log}"
        );
        assert!(log.contains("change TextEdit"), "{log}");
        assert!(log.contains("[state] page: My music"), "{log}");
        assert!(log.contains("[state] account: demo"), "{log}");
        for private in ["Secret Song", "typed search words"] {
            assert!(!log.contains(private), "{private} leaked: {log}");
        }
    }

    #[test]
    fn queue_sources_and_pages_are_logged_by_kind_only() {
        assert_eq!(origin_kind("playlist:42_9"), "playlist");
        assert_eq!(origin_kind("track-mix:-8_91"), "track mix");
        assert_eq!(origin_kind("search:secret song"), "search");
        assert_eq!(
            page_kind(&MusicPage::Search("secret song".into())),
            "Search results"
        );
        // Typing is not a shortcut; acting keys are.
        let none = egui::Modifiers::NONE;
        assert!(!shortcut_key(egui::Key::A, none));
        assert!(!shortcut_key(egui::Key::Num1, none));
        assert!(shortcut_key(egui::Key::A, egui::Modifiers::CTRL));
        assert!(shortcut_key(egui::Key::Space, none));
        assert!(shortcut_key(egui::Key::Escape, none));
    }
}
