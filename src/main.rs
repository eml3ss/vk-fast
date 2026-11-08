#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod app;
mod app_icon;
mod app_paths;
mod artist_catalog;
mod artist_releases;
mod audio_cache;
mod audio_output;
mod audio_upload;
mod audio_upload_ui;
mod autoscroll;
mod avatar_cache;
mod carousel;
mod client_platform;
mod cover_art;
mod cover_cache;
mod cover_play;
mod detail_ui;
mod diagnostics;
mod discord_rpc;
mod home_ui;
mod i18n;
mod icons;
mod keybinds;
mod library_cache;
mod link_router;
mod lyrics;
mod lyrics_ui;
mod media_decoder;
mod mix_settings_ui;
mod model;
mod music_session_store;
mod owner_ui;
mod play_order;
mod playback;
mod playback_modes;
mod playback_resume;
mod playlist_cover;
mod playlist_create_ui;
mod playlist_edit_ui;
mod playlist_pins;
mod playlist_ui;
mod preferences;
mod qr_auth;
mod qr_login_ui;
mod recent_cache;
mod release_info;
mod repaint_target;
mod screen_cache;
mod scrolling;
mod search_swipe;
mod search_ui;
mod seek_bar;
mod settings_search;
mod single_instance;
mod system_media;
mod system_tray;
mod theme;
mod titlebar;
mod track_menu;
mod ui_fonts;
#[cfg(any(windows, test))]
mod updater;
mod web_audio;
mod web_session;
mod window_focus;
mod window_shell;
mod window_state;

use eframe::egui;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(windows)]
    if args.first().is_some_and(|arg| arg == "--apply-update") {
        if args.len() != 2 {
            std::process::exit(2);
        }
        if let Err(error) = updater::install::run_helper(std::path::Path::new(&args[1])) {
            updater::install::report_error(std::path::Path::new(&args[1]), error);
            std::process::exit(1);
        }
        return Ok(());
    }
    #[cfg(windows)]
    let update_stage = if args.iter().any(|arg| arg == "--update-complete") {
        if args.len() != 2 || args[0] != "--update-complete" {
            std::process::exit(2);
        }
        Some(std::path::PathBuf::from(&args[1]))
    } else {
        None
    };
    #[cfg(windows)]
    if args.iter().any(|arg| arg == "--print-data-paths") {
        let config =
            app_paths::config_dir().map_err(|error| eframe::Error::AppCreation(Box::new(error)))?;
        let cache =
            app_paths::cache_dir().map_err(|error| eframe::Error::AppCreation(Box::new(error)))?;
        println!("{}", serde_json::json!({"config": config, "cache": cache}));
        return Ok(());
    }
    if args
        .iter()
        .any(|arg| arg == "--import-music" || arg == "--list-browser-profiles")
    {
        eprintln!("Импорт данных браузера удалён из VK Fast.");
        std::process::exit(2);
    }
    if args
        .iter()
        .any(|arg| arg == "--check-music" || arg == "--check-covers")
    {
        if let Err(message) = check_music(args.iter().any(|arg| arg == "--check-covers")) {
            eprintln!("{message}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--check-playback") {
        if let Err(message) = check_playback() {
            eprintln!("{message}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--check-search") {
        if let Err(message) = check_search() {
            eprintln!("{message}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--check-playlists") {
        if let Err(message) = check_playlists() {
            eprintln!("{message}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--check-details") {
        if let Err(message) = check_details() {
            eprintln!("{message}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--check-track-versions") {
        if let Err(message) = check_track_versions() {
            eprintln!("{message}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--check-mix") {
        if let Err(message) = check_mix() {
            eprintln!("{message}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--check-home") {
        if let Err(message) = check_home() {
            eprintln!("{message}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--check-audio-cache") {
        if let Err(message) = check_audio_cache() {
            eprintln!("{message}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--check-qr-start") {
        if let Err(message) = check_qr_start() {
            eprintln!("{message}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "VK Fast / Rust + egui\n\nUsage: vk-fast [--home] [--search [query]] [--demo] [--login] [--lang ru|en]\n\n  --home  Open Home\n  --search [query]  Open Search, optionally run a query\n  --demo  Show sample tracks without connecting to VK\n  --login  Show the QR sign-in code immediately\n  --lang  Interface language: ru or en (overrides the saved setting)\n\nSign in by scanning the QR code with the VK app on your phone. Startup only restores an existing session."
        );
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--licenses") {
        println!("VK Fast — GPL-3.0-only\n\n{}", include_str!("../LICENSE"));
        println!("\nInter — SIL OFL 1.1\n");
        println!("{}", include_str!("../assets/fonts/OFL.txt"));
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--check-fonts") {
        ui_fonts::diagnostic();
        return Ok(());
    }
    // The system link handler: VK Music links open in VK Fast, every other
    // link goes straight on to the user's browser without starting the player.
    let open_link = match args.iter().position(|arg| arg == "--open-url") {
        Some(position) => {
            let Some(link) = args.get(position + 1) else {
                eprintln!("--open-url requires a link.");
                std::process::exit(2);
            };
            if link_router::parse(link).is_none() {
                return forward_link(link);
            }
            Some(link.clone())
        }
        None => None,
    };
    #[cfg(feature = "screenshot")]
    let screenshot_path = args
        .windows(2)
        .find(|pair| pair[0] == "--screenshot")
        .map(|pair| std::path::PathBuf::from(&pair[1]));
    if args.last().is_some_and(|arg| arg == "--lang") {
        eprintln!("--lang requires a value: ru or en.");
        std::process::exit(2);
    }
    let language_override = if let Some(option) = args.windows(2).find(|pair| pair[0] == "--lang") {
        let Some(language) = i18n::Language::from_code(&option[1]) else {
            eprintln!("Unsupported language. Use --lang ru or --lang en.");
            std::process::exit(2);
        };
        Some(language)
    } else {
        None
    };
    // The launcher's Wayland activation token lets a running instance come
    // forward on desktops that ignore a window's own focus request.
    #[cfg(target_os = "linux")]
    let activation_token = std::env::var("XDG_ACTIVATION_TOKEN")
        .ok()
        .filter(|token| window_focus::valid_token(token));
    #[cfg(not(target_os = "linux"))]
    let activation_token: Option<String> = None;
    // Helpers and CLI checks above remain independent. Claim the GUI instance
    // before touching preferences, sessions, audio or diagnostic files.
    let instance = match single_instance::acquire(
        |process_id| {
            #[cfg(windows)]
            let _ = eframe::allow_foreground_activation(process_id);
            #[cfg(not(windows))]
            let _ = process_id;
        },
        open_link.as_deref(),
        activation_token.as_deref(),
    )
    .map_err(|error| eframe::Error::AppCreation(Box::new(error)))?
    {
        single_instance::Acquisition::Primary(primary) => primary,
        single_instance::Acquisition::AlreadyRunning => return Ok(()),
    };
    let activation = instance.activations();
    if let Some(link) = &open_link {
        activation.open_link(link);
    }
    // Keep an enabled link handler pointing at this copy of the application.
    let _ = std::thread::Builder::new()
        .name("vk-link-handler".into())
        .spawn(link_router::refresh_registration);
    #[cfg(windows)]
    {
        app_paths::config_dir().map_err(|error| eframe::Error::AppCreation(Box::new(error)))?;
        app_paths::cache_dir().map_err(|error| eframe::Error::AppCreation(Box::new(error)))?;
    }
    if preferences::migrate_legacy().is_err() {
        eprintln!("Не удалось перенести настройки VK Fast в каталог конфигурации.");
    }
    if diagnostics::init(preferences::load_logging_enabled()).is_err() {
        eprintln!("Could not enable VK Fast diagnostic logging.");
    }
    let language = language_override.unwrap_or_else(preferences::load_language);
    let demo = args.iter().any(|arg| arg == "--demo");
    let qr_sign_in = args
        .iter()
        .any(|arg| arg == "--login" || arg == "--qr-login");
    let system_titlebar = preferences::load_system_titlebar();
    ui_fonts::preload();
    #[cfg(windows)]
    let _ = app_icon::tray_icon();
    let options = move || eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("VK Fast")
            .with_icon(app_icon::native_icon())
            .with_app_id(app_icon::APP_ID)
            .with_decorations(system_titlebar)
            .with_inner_size([1040.0, 720.0])
            .with_min_inner_size([800.0, 540.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    let result = window_shell::run_native(
        "vk-fast",
        options,
        Box::new(move |cc: &eframe::CreationContext<'_>| {
            let mut app = app::MusicApp::new(&cc.egui_ctx, demo, language);
            app.attach_instance(activation);
            if qr_sign_in {
                app.connect_qr(&cc.egui_ctx);
            } else if !demo {
                app.restore_session(&cc.egui_ctx);
            }
            if args.iter().any(|arg| arg == "--home") {
                app.open_home(&cc.egui_ctx);
            }
            if args.iter().any(|arg| arg == "--search") {
                let query = args
                    .windows(2)
                    .find(|pair| pair[0] == "--search" && !pair[1].starts_with("--"))
                    .map(|pair| pair[1].clone());
                app.open_search(&cc.egui_ctx, query);
            }
            if args.iter().any(|arg| arg == "--settings") {
                app.open_settings(&cc.egui_ctx);
            }
            #[cfg(feature = "screenshot")]
            let app = app.with_screenshot(screenshot_path.clone());
            #[cfg(windows)]
            if let Some(stage) = &update_stage {
                updater::install::complete_restart(stage)
                    .map_err(|error| std::io::Error::other(error.message(language)))?;
            }
            Ok(app)
        }),
    );
    diagnostics::shutdown();
    drop(instance);
    result
}

/// Hand a non-music link to the browser chosen before VK Fast took over web
/// links. Never reopens it through the system: that would come back here.
fn forward_link(link: &str) -> eframe::Result {
    let web = link.starts_with("https://") || link.starts_with("http://");
    if web && link.len() <= link_router::MAX_URL && link_router::forward(link).is_ok() {
        return Ok(());
    }
    eprintln!("VK Fast could not pass the link to a browser.");
    #[cfg(target_os = "linux")]
    if web {
        let _ = std::process::Command::new("notify-send")
            .args([
                "VK Fast",
                "Не удалось открыть ссылку в браузере. Отключите «Открывать ссылки VK Музыки в VK Fast» или выберите браузер в настройках системы.",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    std::process::exit(1);
}

fn check_playlists() -> Result<(), String> {
    let language = i18n::Language::Russian;
    let (profile, cookies) = music_session_store::restore_profile()
        .map_err(|_| "Не удалось прочитать сохранённый вход.")?
        .ok_or("Сохранённого входа нет.")?;
    let client =
        web_audio::WebAudioClient::new(cookies).map_err(|error| error.localized(language))?;
    client
        .verify_owner(profile.id)
        .map_err(|error| error.localized(language))?;
    let canceled = std::sync::atomic::AtomicBool::new(false);
    let playlists = client
        .playlists(profile.id, &canceled)
        .map_err(|error| error.localized(language))?;
    println!("Плейлистов в библиотеке: {}.", playlists.len());
    let Some(playlist) = playlists
        .iter()
        .find(|playlist| playlist.track_count != Some(0))
    else {
        return Ok(());
    };
    let page = client
        .album_page(profile.id, &playlist.album, &canceled)
        .map_err(|error| error.localized(language))?;
    println!("Треков в проверенном плейлисте: {}.", page.tracks.len());
    if let Some(track) = page.tracks.first() {
        client
            .resolve(track.key())
            .map_err(|error| error.localized(language))?;
        println!("Аудиоссылка трека из плейлиста доступна.");
    }
    Ok(())
}

fn check_search() -> Result<(), String> {
    let language = i18n::Language::Russian;
    let (profile, cookies) = music_session_store::restore_profile()
        .map_err(|_| "Не удалось прочитать сохранённый вход.")?
        .ok_or("Сохранённого входа нет.")?;
    let client =
        web_audio::WebAudioClient::new(cookies).map_err(|error| error.localized(language))?;
    client
        .verify_owner(profile.id)
        .map_err(|error| error.localized(language))?;
    let canceled = std::sync::atomic::AtomicBool::new(false);
    let popular = client
        .search_popular(profile.id, &canceled)
        .map_err(|error| error.localized(language))?;
    let hints = client
        .search_hints(profile.id, "nort", &canceled)
        .map_err(|error| error.localized(language))?;
    let first = client
        .search(profile.id, "nortmirage", 0, &canceled)
        .map_err(|error| error.localized(language))?;
    println!(
        "Популярных запросов: {}; подсказок: {}; первая порция: {} треков, {} исполнителей, {} альбомов, {} плейлистов.",
        popular.len(),
        hints.len(),
        first.tracks.len(),
        first.artists.len(),
        first.albums.len(),
        first.playlists.len()
    );
    let track = first
        .tracks
        .first()
        .ok_or("VK не вернул треки для проверки поиска.")?;
    client
        .resolve(track.key())
        .map_err(|error| error.localized(language))?;
    if first.has_more {
        let next = client
            .search(profile.id, "nortmirage", first.next_offset, &canceled)
            .map_err(|error| error.localized(language))?;
        let unique = next
            .tracks
            .iter()
            .filter(|track| !first.tracks.iter().any(|first| first.key() == track.key()))
            .count();
        println!(
            "Следующая порция: {} треков; новых: {unique}.",
            next.tracks.len()
        );
    }
    println!("Аудиоссылка результата поиска доступна.");
    Ok(())
}

fn check_track_versions() -> Result<(), String> {
    let language = i18n::Language::Russian;
    let (profile, cookies) = music_session_store::restore_profile()
        .map_err(|_| "Не удалось прочитать сохранённый вход.")?
        .ok_or("Сохранённого входа нет.")?;
    let client =
        web_audio::WebAudioClient::new(cookies).map_err(|error| error.localized(language))?;
    client
        .verify_owner(profile.id)
        .map_err(|error| error.localized(language))?;
    let canceled = std::sync::atomic::AtomicBool::new(false);
    let page = client
        .page_batch(profile.id, 0, 100)
        .map_err(|error| error.localized(language))?;
    let keys: Vec<_> = page.items.iter().map(model::Track::key).collect();
    if keys.is_empty() {
        return Err("В библиотеке нет треков для проверки.".into());
    }
    let native = client
        .tracks_metadata(profile.id, &keys, &canceled)
        .map_err(|error| error.localized(language))?;
    let legacy_versions = page
        .items
        .iter()
        .filter(|track| track.subtitle.is_some())
        .count();
    let native_versions = native
        .iter()
        .filter(|track| track.subtitle.is_some())
        .count();
    let matching = page
        .items
        .iter()
        .filter(|track| {
            track.subtitle.is_some()
                && native
                    .iter()
                    .any(|other| other.key() == track.key() && other.subtitle == track.subtitle)
        })
        .count();
    println!(
        "Проверено треков: {}. Версий в веб-ответе: {legacy_versions}; в API: {native_versions}; совпадающих: {matching}.",
        page.items.len()
    );
    // Check only the reported release if it occurs in this one library batch.
    // No broad catalog search, playback or credential output is involved.
    if let Some(album) = native
        .iter()
        .find(|track| track.title.eq_ignore_ascii_case("CICADATEKK"))
        .and_then(|track| track.album.as_ref())
    {
        let album = client
            .album_page(profile.id, album, &canceled)
            .map_err(|error| error.localized(language))?;
        let versions = album
            .tracks
            .iter()
            .filter(|track| track.subtitle.is_some())
            .count();
        println!(
            "Проверенный релиз: {} треков, пометок версии: {versions}.",
            album.tracks.len()
        );
    }
    Ok(())
}

fn check_details() -> Result<(), String> {
    let language = i18n::Language::Russian;
    let (profile, cookies) = music_session_store::restore_profile()
        .map_err(|_| "Не удалось прочитать сохранённый вход.")?
        .ok_or("Сохранённого входа нет.")?;
    let client =
        web_audio::WebAudioClient::new(cookies).map_err(|error| error.localized(language))?;
    client
        .verify_owner(profile.id)
        .map_err(|error| error.localized(language))?;
    let canceled = std::sync::atomic::AtomicBool::new(false);
    let page = client
        .page_batch(profile.id, 0, 100)
        .map_err(|error| error.localized(language))?;
    let legacy_canonical = page
        .items
        .iter()
        .filter(|track| !track.artists.is_empty())
        .count();
    let legacy_multiple = page
        .items
        .iter()
        .filter(|track| track.artists.len() > 1)
        .count();
    println!(
        "Треков с исполнителями в библиотеке: {legacy_canonical}; с несколькими: {legacy_multiple}."
    );
    let keys: Vec<_> = page.items.iter().map(model::Track::key).collect();
    let metadata = client
        .tracks_metadata(profile.id, &keys, &canceled)
        .map_err(|error| error.localized(language))?;
    println!(
        "Дополнение старого кэша: {} треков, с несколькими исполнителями: {}.",
        metadata.len(),
        metadata
            .iter()
            .filter(|track| track.artists.len() > 1)
            .count()
    );
    let key = page.items.first().ok_or("В библиотеке нет треков.")?.key();
    let track = client
        .track_metadata(profile.id, key, &canceled)
        .map_err(|error| error.localized(language))?;
    let artist = track.artists.first().ok_or("VK не прислал исполнителя.")?;
    let artist = client
        .artist_page(profile.id, artist, &canceled)
        .map_err(|error| error.localized(language))?;
    println!(
        "Страница исполнителя: {} треков, {} релизов, изображение: {}.",
        artist.tracks.len(),
        artist.discography.len(),
        artist.image_url.is_some()
    );
    let album = artist
        .discography
        .first()
        .map(|release| &release.album)
        .or(track.album.as_ref())
        .ok_or("У исполнителя нет релизов.")?;
    let album = client
        .album_page(profile.id, album, &canceled)
        .map_err(|error| error.localized(language))?;
    println!(
        "Страница альбома: {} треков, исполнителей: {}, обложка: {}.",
        album.tracks.len(),
        album.artists.len(),
        album.album.cover_url.is_some()
    );
    println!(
        "Статистика релиза: прослушивания: {}; дата создания: {}; дата изменения: {}.",
        album.stats.plays.is_some(),
        album.stats.created_at.is_some(),
        album.stats.updated_at.is_some()
    );
    if let Some(track) = album.tracks.first() {
        let _stream = client
            .resolve_cached(track.key(), &canceled)
            .map_err(|error| error.localized(language))?;
        println!("Аудиопоток трека из альбома доступен.");
    }
    Ok(())
}

fn check_mix() -> Result<(), String> {
    let language = i18n::Language::Russian;
    let (profile, cookies) = music_session_store::restore_profile()
        .map_err(|_| "Не удалось прочитать сохранённый вход.")?
        .ok_or("Сохранённого входа нет.")?;
    let client =
        web_audio::WebAudioClient::new(cookies).map_err(|error| error.localized(language))?;
    client
        .verify_owner(profile.id)
        .map_err(|error| error.localized(language))?;
    let canceled = std::sync::atomic::AtomicBool::new(false);
    let first = client
        .vk_mix(profile.id, &canceled)
        .map_err(|error| error.localized(language))?;
    println!("Треков первой порции VK Mix: {}", first.tracks.len());
    let second = client
        .vk_mix(profile.id, &canceled)
        .map_err(|error| error.localized(language))?;
    println!("Треков следующей порции VK Mix: {}", second.tracks.len());
    let first_keys: std::collections::HashSet<_> =
        first.tracks.iter().map(model::Track::key).collect();
    let new_count = second
        .tracks
        .iter()
        .filter(|track| !first_keys.contains(&track.key()))
        .count();
    println!("Новых треков во второй порции: {new_count}");
    if let Some(track) = first.tracks.first() {
        let _url = client
            .resolve_cached(track.key(), &canceled)
            .map_err(|error| error.localized(language))?;
        println!("Аудиопоток первого трека VK Mix доступен.");
    }

    Ok(())
}

fn check_home() -> Result<(), String> {
    let language = i18n::Language::Russian;
    let (profile, cookies) = music_session_store::restore_profile()
        .map_err(|_| "Не удалось прочитать сохранённый вход.")?
        .ok_or("Сохранённого входа нет.")?;
    let client =
        web_audio::WebAudioClient::new(cookies).map_err(|error| error.localized(language))?;
    client
        .verify_owner(profile.id)
        .map_err(|error| error.localized(language))?;
    let canceled = std::sync::atomic::AtomicBool::new(false);
    let collections = client
        .home(profile.id, &canceled)
        .map_err(|error| error.localized(language))?;
    println!("Подборок VK получено: {}", collections.len());
    if let Some(collection) = collections.first() {
        let tracks = client
            .playlist_tracks(profile.id, collection, &canceled)
            .map_err(|error| error.localized(language))?;
        println!("Треков первой подборки получено: {}", tracks.len());
    }
    Ok(())
}

fn check_qr_start() -> Result<(), String> {
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };
    let control = qr_auth::Control::default();
    let worker_control = control.clone();
    let (sender, events) = mpsc::sync_channel(8);
    let worker = std::thread::spawn(move || qr_auth::start(worker_control, sender));
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut ready = false;
    while Instant::now() < deadline {
        match events.recv_timeout(Duration::from_millis(100)) {
            Ok(qr_auth::Event::Ready { url, expires_in }) => {
                ready = expires_in > 0 && qrcode::QrCode::new(url.as_bytes()).is_ok();
                break;
            }
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => (),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    control.cancel();
    drop(events);
    let result = worker.join().map_err(|_| "Проверка QR-входа прервана.")?;
    if !ready {
        return Err(match result {
            Err(error) => error.localized(i18n::Language::Russian),
            Ok(_) => "VK не выдал QR-код.".into(),
        });
    }
    println!(
        "VK выдал действующий QR-код; создание и отмена новой сессии работают. Вход с телефона не выполнялся."
    );
    Ok(())
}

fn saved_profile() -> Result<model::Profile, String> {
    music_session_store::restore_profile()
        .map_err(|_| "Не удалось прочитать сохранённый профиль.")?
        .map(|(profile, _)| profile)
        .ok_or_else(|| "Сначала войдите в VK Fast.".into())
}

fn check_playback() -> Result<(), String> {
    use playback::{PlaybackState, Player};
    use std::{
        thread,
        time::{Duration, Instant},
    };
    let language = i18n::Language::Russian;
    let saved = saved_profile()?;
    let cookies = music_session_store::load(saved.id)
        .map_err(|_| "Не удалось прочитать подключение музыки.")?
        .ok_or("Сохранённого подключения музыки нет.")?;
    let client =
        web_audio::WebAudioClient::new(cookies).map_err(|error| error.localized(language))?;
    client
        .verify_owner(saved.id)
        .map_err(|error| error.localized(language))?;
    let page = client
        .page(saved.id, 0)
        .map_err(|error| error.localized(language))?;
    let track = page
        .items
        .iter()
        .find(|track| track.duration > 45)
        .ok_or("В библиотеке нет подходящего трека для проверки.")?;
    let url = client
        .resolve(track.key())
        .map_err(|error| error.localized(language))?;
    println!("Аудиоссылка получена. Проверяем декодер и устройство вывода с нулевой громкостью.");
    let mut player = Player::new();
    player.set_volume(0.0);
    player.play(url, track.duration, 0.0);
    let wait_for =
        |player: &Player, predicate: &dyn Fn(&playback::Snapshot) -> bool| -> Result<(), String> {
            let deadline = Instant::now() + Duration::from_secs(25);
            loop {
                let snapshot = player.snapshot();
                if let PlaybackState::Failed(error) = &snapshot.state {
                    return Err(error.localized(language));
                }
                if predicate(&snapshot) {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err("Воспроизведение не перешло в ожидаемое состояние вовремя.".into());
                }
                thread::sleep(Duration::from_millis(50));
            }
        };
    wait_for(&player, &|snapshot| {
        matches!(snapshot.state, PlaybackState::Playing) && snapshot.position >= 0.4
    })?;
    wait_for(&player, &|snapshot| {
        snapshot.buffered_until > snapshot.position + 2.0
    })?;
    let steady_until = player.snapshot().position + 15.0;
    wait_for(&player, &|snapshot| {
        matches!(snapshot.state, PlaybackState::Playing) && snapshot.position >= steady_until
    })?;
    if player.underruns() != 0 {
        return Err(format!(
            "При непрерывном воспроизведении обнаружены опустошения PCM-буфера: {}.",
            player.underruns()
        ));
    }
    player.pause();
    wait_for(&player, &|snapshot| {
        matches!(snapshot.state, PlaybackState::Paused)
    })?;
    thread::sleep(Duration::from_millis(150));
    let paused = player.snapshot().position;
    thread::sleep(Duration::from_millis(200));
    if (player.snapshot().position - paused).abs() > 0.02 {
        return Err("Позиция продолжает меняться на паузе.".into());
    }
    let buffered = player.snapshot().buffered_until;
    let decoder_starts = player.decoder_starts();
    let ahead = paused + 1.0;
    player.seek(ahead);
    wait_for(&player, &|snapshot| {
        matches!(snapshot.state, PlaybackState::Paused)
            && (snapshot.position - ahead).abs() < 0.005
            && snapshot.buffered_until >= buffered - 0.005
    })?;
    if player.decoder_starts() != decoder_starts {
        return Err("Перемотка в готовый буфер перезапустила декодер.".into());
    }
    player.seek(paused);
    wait_for(&player, &|snapshot| {
        matches!(snapshot.state, PlaybackState::Paused)
            && (snapshot.position - paused).abs() < 0.005
            && snapshot.buffered_until >= buffered - 0.005
    })?;
    if player.decoder_starts() != decoder_starts {
        return Err("Возврат в готовое аудио перезапустил декодер.".into());
    }
    player.resume();
    wait_for(&player, &|snapshot| {
        matches!(snapshot.state, PlaybackState::Playing) && snapshot.position > paused + 0.2
    })?;
    let playing_seek = player.snapshot().position + 0.5;
    player.seek(playing_seek);
    if matches!(player.snapshot().state, PlaybackState::Loading) {
        return Err("Готовое аудио показало повторную загрузку при перемотке.".into());
    }
    wait_for(&player, &|snapshot| {
        matches!(snapshot.state, PlaybackState::Playing) && snapshot.position > playing_seek + 0.1
    })?;
    if player.decoder_starts() != decoder_starts {
        return Err("Перемотка во время воспроизведения перезапустила декодер.".into());
    }
    let outside = (player.snapshot().buffered_until + 10.0).min(f64::from(track.duration) - 1.0);
    if outside > player.snapshot().buffered_until + 0.5 {
        player.seek(outside);
        wait_for(&player, &|snapshot| {
            matches!(snapshot.state, PlaybackState::Playing) && snapshot.position > outside + 0.2
        })?;
        if player.decoder_starts() <= decoder_starts {
            return Err("Перемотка за пределы буфера не запустила декодирование.".into());
        }
    }
    player.stop();
    wait_for(&player, &|snapshot| {
        matches!(snapshot.state, PlaybackState::Stopped)
    })?;
    println!(
        "Нативный аудиовывод работает; перемотка вперёд и назад в буфере сохраняет декодер. Пауза, перемотка за буфер и остановка работают."
    );
    println!("Опустошений PCM-буфера: {}.", player.underruns());
    Ok(())
}

fn wait_audio(
    player: &playback::Player,
    ready: impl Fn(&playback::Snapshot) -> bool,
) -> Result<(), String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
    loop {
        let snapshot = player.snapshot();
        if let playback::PlaybackState::Failed(error) = &snapshot.state {
            return Err(error.localized(i18n::Language::Russian));
        }
        if ready(&snapshot) {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err("Проверка аудиокэша превысила время ожидания.".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn check_audio_cache() -> Result<(), String> {
    use audio_cache::{AudioCache, CacheKey, DEFAULT_LIMIT_BYTES};
    use playback::{PlaybackState, Player};
    let language = i18n::Language::Russian;
    let saved = saved_profile()?;
    let cookies = music_session_store::load(saved.id)
        .map_err(|_| "Не удалось прочитать подключение музыки.")?
        .ok_or("Нет сохранённого подключения музыки.")?;
    let client =
        web_audio::WebAudioClient::new(cookies).map_err(|error| error.localized(language))?;
    client
        .verify_owner(saved.id)
        .map_err(|error| error.localized(language))?;
    let page = client
        .page(saved.id, 0)
        .map_err(|error| error.localized(language))?;
    let track = page
        .items
        .iter()
        .find(|track| track.duration > 70)
        .ok_or("Нет подходящего трека для проверки кэша.")?;
    let url = client
        .resolve(track.key())
        .map_err(|error| error.localized(language))?;
    let key = CacheKey {
        account_id: saved.id,
        owner_id: track.owner_id,
        track_id: track.id,
        duration: track.duration,
    };
    let cache = AudioCache::new(DEFAULT_LIMIT_BYTES).map_err(|_| "Не удалось открыть аудиокэш.")?;
    let mut player = Player::with_cache(cache.clone());
    player.set_volume(0.0);
    player.play_cached(key, Some(url.clone()), 0.0);
    wait_audio(&player, |snapshot| {
        matches!(snapshot.state, PlaybackState::Playing) && snapshot.position > 0.3
    })?;
    let starts = player.decoder_starts();
    for target in [10.0, 20.0, 30.0, 40.0] {
        wait_audio(&player, |snapshot| snapshot.buffered_until > target + 1.0)?;
        player.seek(target);
        wait_audio(&player, |snapshot| {
            matches!(snapshot.state, PlaybackState::Playing) && snapshot.position > target + 0.15
        })?;
    }
    if player.decoder_starts() != starts {
        return Err("Перемотка по готовому аудио перезапустила декодер.".into());
    }
    player.pause();
    wait_audio(&player, |snapshot| {
        matches!(snapshot.state, PlaybackState::Paused)
    })?;
    player.seek(1.0);
    wait_audio(&player, |snapshot| {
        matches!(snapshot.state, PlaybackState::Paused) && (snapshot.position - 1.0).abs() < 0.005
    })?;
    if player.decoder_starts() != starts {
        return Err("Возврат на старый участок дискового кэша перезапустил декодер.".into());
    }
    player.stop();
    drop(player);
    if let Some(end) = cache
        .complete_frames(&key)
        .map_err(|_| "Не удалось проверить окончание кэша.")?
        && !cache
            .contains_range(&key, 0, end)
            .map_err(|_| "Не удалось проверить диапазоны кэша.")?
    {
        return Err("В полностью загруженном треке обнаружен пропуск кэша.".into());
    }
    drop(cache);
    let cache =
        AudioCache::new(DEFAULT_LIMIT_BYTES).map_err(|_| "Не удалось заново открыть аудиокэш.")?;
    let mut replay = Player::with_cache(cache);
    replay.set_volume(0.0);
    replay.play_cached(key, Some(url), 1.0);
    wait_audio(&replay, |snapshot| {
        matches!(snapshot.state, PlaybackState::Playing) && snapshot.position > 1.3
    })?;
    if replay.decoder_starts() != 0 {
        return Err("Повторное воспроизведение старого участка не использовало диск.".into());
    }
    replay.stop();
    println!(
        "Дисковый кэш работает: возврат с 40-й на 1-ю секунду и повторное открытие кэша не запускают декодер. Проверка выполнена без звука."
    );
    Ok(())
}

fn check_music(check_covers: bool) -> Result<(), String> {
    let language = i18n::Language::Russian;
    let saved = saved_profile()?;
    let cookies = music_session_store::load(saved.id)
        .map_err(|_| "Не удалось прочитать сохранённое подключение музыки.")?
        .ok_or("Сохранённого подключения музыки нет.")?;
    let client =
        web_audio::WebAudioClient::new(cookies).map_err(|error| error.localized(language))?;
    client
        .verify_owner(saved.id)
        .map_err(|error| error.localized(language))?;
    let page = client
        .page(saved.id, 0)
        .map_err(|error| error.localized(language))?;
    println!(
        "Сохранённая сессия работает. Первая страница: {} треков.",
        page.items.len()
    );
    if check_covers {
        let with_covers = page
            .items
            .iter()
            .filter(|track| track.cover_url.is_some())
            .count();
        println!(
            "Обложки в метаданных: {with_covers} из {}.",
            page.items.len()
        );
        let url = page
            .items
            .iter()
            .find_map(|track| track.cover_url.as_deref())
            .ok_or("VK не вернул обложки на этой странице.")?;
        if !cover_art::check(url) {
            return Err("Не удалось загрузить и декодировать обложку.".into());
        }
        println!("Обложка загружается и декодируется.");
    }
    if page.exhausted == Some(false) {
        let next = client
            .page(saved.id, page.items.len())
            .map_err(|error| error.localized(language))?;
        println!("Следующая страница: {} треков.", next.items.len());
    }
    Ok(())
}
