//! One worker with a coalesced latest-track slot; rapid skips do not fan out HTTP.
use super::{Error, Lyrics, Mode, Provider, Query, Source, Status, store};
use crate::repaint_target::RepaintTarget;
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::Instant,
};
use zeroize::{Zeroize, Zeroizing};

pub enum ConfigAction {
    Reload,
    Mode(Mode),
    Source(Source),
    Fallback(bool),
    ApiKey(Zeroizing<String>),
    Generate,
}
pub enum Event {
    Settings {
        result: Result<Status, Error>,
        completed: bool,
    },
    Lyrics {
        id: u64,
        query: Query,
        result: Result<Arc<Lyrics>, Error>,
    },
}
pub struct VkSession {
    pub owner: i64,
    pub client: crate::web_audio::WebAudioClient,
}

struct Fetch {
    id: u64,
    query: Query,
    vk: Option<VkSession>,
    cancel: Arc<AtomicBool>,
}
#[derive(Default)]
struct Jobs {
    config: Option<ConfigAction>,
    fetch: Option<Fetch>,
    cancel: Option<Arc<AtomicBool>>,
}

pub struct Service {
    jobs: Arc<Mutex<Jobs>>,
    wake: SyncSender<()>,
    replies: Receiver<Event>,
    latest: Arc<AtomicU64>,
    canceled: Arc<AtomicBool>,
    repaint: RepaintTarget,
}
impl Drop for Service {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
        self.cancel_fetch();
        let _ = self.wake.try_send(());
    }
}
impl Service {
    pub fn new(ctx: &eframe::egui::Context) -> Result<Self, Error> {
        let jobs = Arc::new(Mutex::new(Jobs::default()));
        let (wake, wakes) = mpsc::sync_channel(1);
        let (reply, replies) = mpsc::channel();
        let latest = Arc::new(AtomicU64::new(0));
        let canceled = Arc::new(AtomicBool::new(false));
        let repaint = RepaintTarget::default();
        repaint.set_context(ctx);
        let worker_jobs = jobs.clone();
        let worker_latest = latest.clone();
        let worker_cancel = canceled.clone();
        let worker_repaint = repaint.clone();
        thread::Builder::new()
            .name("vk-lyrics".into())
            .spawn(move || {
                let provider = Provider::new();
                let mut config = store::load();
                let _ = reply.send(Event::Settings {
                    result: config
                        .as_ref()
                        .map(|config| config.status())
                        .map_err(|error| *error),
                    completed: false,
                });
                worker_repaint.request_repaint();
                let mut cache: VecDeque<(Query, Instant, Arc<Lyrics>, Option<i64>)> =
                    VecDeque::new();
                while wakes.recv().is_ok() && !worker_cancel.load(Ordering::Acquire) {
                    let action = worker_jobs
                        .lock()
                        .ok()
                        .and_then(|mut jobs| jobs.config.take());
                    if let Some(action) = action {
                        let result = match action {
                            ConfigAction::Reload => match store::load() {
                                Ok(loaded) => {
                                    let status = loaded.status();
                                    config = Ok(loaded);
                                    Ok(status)
                                }
                                Err(error) => Err(error),
                            },
                            action => match (&provider, &mut config) {
                                (Ok(provider), Ok(config)) => {
                                    configure(provider, config, action, &|| {
                                        worker_cancel.load(Ordering::Acquire)
                                    })
                                }
                                (Err(error), _) => Err(*error),
                                (_, Err(error)) => Err(*error),
                            },
                        };
                        if result.is_ok() {
                            cache.clear();
                        }
                        let _ = reply.send(Event::Settings {
                            result,
                            completed: true,
                        });
                        if result.is_err()
                            && let Ok(config) = &config
                        {
                            let _ = reply.send(Event::Settings {
                                result: Ok(config.status()),
                                completed: false,
                            });
                        }
                        worker_repaint.request_repaint();
                    }
                    let request = worker_jobs
                        .lock()
                        .ok()
                        .and_then(|mut jobs| jobs.fetch.take());
                    let Some(request) = request else {
                        continue;
                    };
                    let stop = || {
                        worker_cancel.load(Ordering::Acquire)
                            || worker_latest.load(Ordering::Acquire) != request.id
                            || request.cancel.load(Ordering::Acquire)
                    };
                    if stop() {
                        continue;
                    }
                    let owner = request.vk.as_ref().map(|vk| vk.owner);
                    let cached = cache.iter().position(|(query, at, _, account)| {
                        query == &request.query && *account == owner && at.elapsed().as_secs() < 900
                    });
                    let result = if let Some(index) = cached {
                        let entry = cache.remove(index).unwrap();
                        let lyrics = entry.2.clone();
                        cache.push_back(entry);
                        Ok(lyrics)
                    } else {
                        match (&provider, &mut config) {
                            (Ok(provider), Ok(config)) => {
                                let result = resolve_sources(
                                    config.source,
                                    config.fallback_to_genius,
                                    &stop,
                                    |source| {
                                        fetch_source(
                                            provider,
                                            config,
                                            source,
                                            &request,
                                            &stop,
                                            &|| worker_cancel.load(Ordering::Acquire),
                                        )
                                    },
                                );
                                let _ = reply.send(Event::Settings {
                                    result: Ok(config.status()),
                                    completed: false,
                                });
                                worker_repaint.request_repaint();
                                result
                            }
                            (Err(error), _) => Err(*error),
                            (_, Err(error)) => Err(*error),
                        }
                    };
                    if stop() {
                        continue;
                    }
                    if let Ok(lyrics) = &result {
                        cache.retain(|(query, _, _, account)| {
                            query != &request.query || *account != owner
                        });
                        cache.push_back((
                            request.query.clone(),
                            Instant::now(),
                            lyrics.clone(),
                            owner,
                        ));
                        while cache.len() > 32 {
                            cache.pop_front();
                        }
                    }
                    let displayed = result.as_ref().ok().cloned();
                    let _ = reply.send(Event::Lyrics {
                        id: request.id,
                        query: request.query,
                        result,
                    });
                    worker_repaint.request_repaint();
                    if let (Ok(provider), Some(lyrics)) = (&provider, displayed) {
                        provider.record_display(&lyrics, &stop);
                    }
                }
            })
            .map_err(|_| Error::Network)?;
        Ok(Self {
            jobs,
            wake,
            replies,
            latest,
            canceled,
            repaint,
        })
    }

    pub fn rebind(&self, ctx: &eframe::egui::Context) {
        self.repaint.set_context(ctx);
    }
    pub fn configure(&self, action: ConfigAction) {
        self.latest.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut jobs) = self.jobs.lock() {
            if let Some(cancel) = jobs.cancel.take() {
                cancel.store(true, Ordering::Release);
            }
            jobs.config = Some(action);
            jobs.fetch = None;
        }
        let _ = self.wake.try_send(());
    }
    pub fn fetch(&self, query: Query, vk: Option<VkSession>) -> u64 {
        let id = self.latest.fetch_add(1, Ordering::AcqRel) + 1;
        if let Ok(mut jobs) = self.jobs.lock() {
            if let Some(cancel) = jobs.cancel.take() {
                cancel.store(true, Ordering::Release);
            }
            let cancel = Arc::new(AtomicBool::new(false));
            jobs.cancel = Some(cancel.clone());
            jobs.fetch = Some(Fetch {
                id,
                query,
                vk,
                cancel,
            });
        }
        let _ = self.wake.try_send(());
        id
    }
    pub fn cancel_fetch(&self) {
        self.latest.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut jobs) = self.jobs.lock() {
            if let Some(cancel) = jobs.cancel.take() {
                cancel.store(true, Ordering::Release);
            }
            jobs.fetch = None;
        }
    }
    pub fn poll(&self) -> Option<Event> {
        self.replies.try_recv().ok()
    }
}

fn resolve_sources(
    source: Source,
    fallback: bool,
    canceled: &impl Fn() -> bool,
    mut fetch: impl FnMut(Source) -> Result<Arc<Lyrics>, Error>,
) -> Result<Arc<Lyrics>, Error> {
    if canceled() {
        return Err(Error::Canceled);
    }
    match fetch(source) {
        Err(Error::Canceled) => Err(Error::Canceled),
        Err(error) if fallback && source != Source::Genius => {
            if canceled() {
                return Err(Error::Canceled);
            }
            match fetch(Source::Genius) {
                Ok(lyrics) => Ok(lyrics),
                Err(Error::Canceled) => Err(Error::Canceled),
                Err(_) => Err(error),
            }
        }
        result => result,
    }
}

fn fetch_source(
    provider: &Provider,
    config: &mut store::Config,
    source: Source,
    request: &Fetch,
    canceled: &impl Fn() -> bool,
    shutdown: &impl Fn() -> bool,
) -> Result<Arc<Lyrics>, Error> {
    if canceled() {
        return Err(Error::Canceled);
    }
    let now = store::now();
    let retry = match source {
        Source::Genius => config.genius_retry_after,
        Source::Vk => config.vk_retry_after,
        Source::Musixmatch => match config.mode {
            Mode::Anonymous => config.anonymous_retry_after,
            Mode::ApiKey => config.api_retry_after,
        },
    };
    if retry > now {
        return Err(Error::RateLimited(retry - now));
    }
    let result = match source {
        Source::Musixmatch => prepare(provider, config, shutdown)
            .and_then(|()| provider.fetch(config, &request.query, canceled)),
        Source::Genius => provider.genius(&request.query, canceled),
        Source::Vk => request.vk.as_ref().ok_or(Error::NoSession).and_then(|vk| {
            if cfg!(test) {
                return Err(Error::NotFound);
            }
            vk.client
                .track_lyrics(vk.owner, request.query.key, &request.cancel)
                .map_err(super::vk::error)
                .and_then(|value| super::vk::parse(&value))
                .map(Arc::new)
        }),
    };
    if let Err(error) = result {
        let wait = match error {
            Error::RateLimited(seconds) => seconds.clamp(30, 86400),
            Error::Captcha => 300,
            _ => 0,
        };
        if source == Source::Musixmatch && config.mode == Mode::Anonymous && error == Error::Auth {
            config.token_rejected = true;
        }
        if wait > 0 {
            let slot = match source {
                Source::Genius => &mut config.genius_retry_after,
                Source::Vk => &mut config.vk_retry_after,
                Source::Musixmatch => match config.mode {
                    Mode::Anonymous => &mut config.anonymous_retry_after,
                    Mode::ApiKey => &mut config.api_retry_after,
                },
            };
            *slot = now + wait;
        }
        if wait > 0 || error == Error::Auth {
            let _ = store::save(config);
        }
    }
    result
}

fn generate(
    provider: &Provider,
    config: &mut store::Config,
    canceled: &impl Fn() -> bool,
) -> Result<(), Error> {
    let now = store::now();
    let next = config.token_retry_after.max(config.anonymous_retry_after);
    if next > now {
        return Err(Error::RateLimited(next - now));
    }
    config.token_retry_after = now + 300;
    store::save(config)?;
    let token = provider.token(canceled)?;
    if canceled() {
        return Err(Error::Canceled);
    }
    let mut candidate = config.clone();
    candidate.user_token.zeroize();
    candidate.user_token = token.to_string();
    candidate.token_app_id = super::APP_ID.into();
    candidate.token_created = now;
    candidate.token_rejected = false;
    store::save(&candidate)?;
    *config = candidate;
    Ok(())
}

fn prepare(
    provider: &Provider,
    config: &mut store::Config,
    canceled: &impl Fn() -> bool,
) -> Result<(), Error> {
    let now = store::now();
    let retry = match config.mode {
        Mode::Anonymous => config.anonymous_retry_after,
        Mode::ApiKey => config.api_retry_after,
    };
    if retry > now {
        return Err(Error::RateLimited(retry - now));
    }
    match config.mode {
        Mode::ApiKey if config.api_key.is_empty() => Err(Error::MissingKey),
        Mode::Anonymous if config.token_rejected => Err(Error::Auth),
        Mode::Anonymous if !config.has_current_token() => generate(provider, config, canceled),
        _ => Ok(()),
    }
}

fn configure(
    provider: &Provider,
    config: &mut store::Config,
    action: ConfigAction,
    canceled: &impl Fn() -> bool,
) -> Result<Status, Error> {
    match action {
        ConfigAction::Reload => {}
        ConfigAction::Source(source) => {
            let mut candidate = config.clone();
            candidate.source = source;
            store::save(&candidate)?;
            *config = candidate;
        }
        ConfigAction::Fallback(enabled) => {
            let mut candidate = config.clone();
            candidate.fallback_to_genius = enabled;
            store::save(&candidate)?;
            *config = candidate;
        }
        ConfigAction::Mode(mode) => {
            let mut candidate = config.clone();
            candidate.mode = mode;
            store::save(&candidate)?;
            *config = candidate;
        }
        ConfigAction::ApiKey(key) => {
            let key = key.trim();
            if !store::valid_secret(key) {
                return Err(Error::MissingKey);
            }
            let mut candidate = config.clone();
            if candidate.api_key != key {
                candidate.api_retry_after = 0;
            }
            candidate.api_key.zeroize();
            candidate.api_key = key.to_owned();
            candidate.mode = Mode::ApiKey;
            store::save(&candidate)?;
            *config = candidate;
        }
        ConfigAction::Generate => {
            generate(provider, config, canceled)?;
        }
    }
    Ok(config.status())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn genius_fallback_is_opt_in_runs_once_and_respects_cancellation() {
        let mut calls = Vec::new();
        let result = resolve_sources(Source::Vk, true, &|| false, |source| {
            calls.push(source);
            match source {
                Source::Vk => Err(Error::NotFound),
                _ => Ok(Arc::new(super::super::test_data(2, false))),
            }
        })
        .unwrap();
        assert!(!result.synced);
        assert_eq!(calls, [Source::Vk, Source::Genius]);
        for (source, fallback, error, expected) in [
            (Source::Vk, false, Error::NotFound, vec![Source::Vk]),
            (Source::Genius, true, Error::NotFound, vec![Source::Genius]),
            (Source::Vk, true, Error::Canceled, vec![Source::Vk]),
        ] {
            calls.clear();
            assert_eq!(
                resolve_sources(source, fallback, &|| false, |source| {
                    calls.push(source);
                    Err(error)
                })
                .unwrap_err(),
                error
            );
            assert_eq!(calls, expected);
        }
        assert_eq!(
            resolve_sources(Source::Vk, true, &|| true, |_| panic!(
                "canceled work must not fetch"
            ))
            .unwrap_err(),
            Error::Canceled
        );
    }

    #[test]
    fn selecting_sources_preserves_credentials_and_independent_backoff() {
        let provider = Provider::new().unwrap();
        let mut config = store::Config::default();
        config.user_token = "synthetic-existing-token".into();
        config.anonymous_retry_after = store::now() + 300;
        configure(
            &provider,
            &mut config,
            ConfigAction::Source(Source::Vk),
            &|| false,
        )
        .unwrap();
        configure(
            &provider,
            &mut config,
            ConfigAction::Fallback(true),
            &|| false,
        )
        .unwrap();
        assert_eq!(config.source, Source::Vk);
        assert!(config.fallback_to_genius);
        assert_eq!(config.status().request_retry_after, 0);
        assert_eq!(config.user_token, "synthetic-existing-token");
        assert!(config.anonymous_retry_after > store::now());
    }
    #[test]
    fn rapid_track_changes_keep_only_the_latest_fetch_and_configuration_cancels_old_work() {
        let jobs = Arc::new(Mutex::new(Jobs::default()));
        let (wake, _receive) = mpsc::sync_channel(1);
        let (_reply, replies) = mpsc::channel();
        let service = Service {
            jobs: jobs.clone(),
            wake,
            replies,
            latest: Arc::new(AtomicU64::new(0)),
            canceled: Arc::new(AtomicBool::new(false)),
            repaint: RepaintTarget::default(),
        };
        let mut query = Query {
            key: (42, 1),
            title: "Synthetic".into(),
            artist: "Test".into(),
            album: String::new(),
            duration: 180,
            speed_variant: false,
        };
        let first = service.fetch(query.clone(), None);
        query.key.1 = 2;
        let last = service.fetch(query.clone(), None);
        assert!(last > first);
        let pending = jobs.lock().unwrap().fetch.take().unwrap();
        assert_eq!(pending.id, last);
        assert_eq!(pending.query, query);
        service.fetch(query, None);
        assert!(pending.cancel.load(Ordering::Acquire));
        service.configure(ConfigAction::Mode(Mode::ApiKey));
        assert!(jobs.lock().unwrap().fetch.is_none());
        assert!(matches!(
            jobs.lock().unwrap().config,
            Some(ConfigAction::Mode(Mode::ApiKey))
        ));
    }
    #[test]
    fn generation_and_credentials_respect_backoff_without_rotating_tokens_or_exposing_secrets() {
        let provider = Provider::new().unwrap();
        let mut config = store::Config::default();
        config.user_token = "synthetic-current-token".into();
        config.token_app_id = super::super::APP_ID.into();
        config.token_retry_after = store::now() + 300;
        assert!(matches!(
            generate(&provider, &mut config, &|| false),
            Err(Error::RateLimited(_))
        ));
        assert_eq!(config.user_token, "synthetic-current-token");
        // Moving away from the retired API must not reset an existing quota.
        config.token_app_id.clear();
        assert!(matches!(
            prepare(&provider, &mut config, &|| false),
            Err(Error::RateLimited(_))
        ));
        assert_eq!(config.user_token, "synthetic-current-token");
        config.token_rejected = true;
        assert_eq!(prepare(&provider, &mut config, &|| false), Err(Error::Auth));
        configure(
            &provider,
            &mut config,
            ConfigAction::ApiKey(Zeroizing::new("synthetic-private-key".into())),
            &|| false,
        )
        .unwrap();
        config.api_retry_after = store::now() + 600;
        configure(
            &provider,
            &mut config,
            ConfigAction::ApiKey(Zeroizing::new("synthetic-private-key".into())),
            &|| false,
        )
        .unwrap();
        assert!(matches!(
            prepare(&provider, &mut config, &|| false),
            Err(Error::RateLimited(_))
        ));
        let public = format!("{:?}", config.status());
        assert!(!public.contains("synthetic"));
        configure(
            &provider,
            &mut config,
            ConfigAction::Mode(Mode::Anonymous),
            &|| false,
        )
        .unwrap();
        assert_eq!(config.api_key, "synthetic-private-key");
        assert_eq!(config.user_token, "synthetic-current-token");
    }
}
