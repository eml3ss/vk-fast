//! One worker with a coalesced latest-track slot; rapid skips do not fan out HTTP.
use super::{Error, Lyrics, Mode, Provider, Query, Status, store};
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
    ApiKey(Zeroizing<String>),
    Generate,
}
pub enum Event {
    Font(Vec<u8>),
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
struct Fetch {
    id: u64,
    query: Query,
}
#[derive(Default)]
struct Jobs {
    config: Option<ConfigAction>,
    fetch: Option<Fetch>,
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
                let mut cache: VecDeque<(Query, Instant, Arc<Lyrics>)> = VecDeque::new();
                let mut font_attempted = false;
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
                    };
                    if stop() {
                        continue;
                    }
                    let cached = cache.iter().position(|(query, at, _)| {
                        query == &request.query && at.elapsed().as_secs() < 900
                    });
                    let result = if let Some(index) = cached {
                        let entry = cache.remove(index).unwrap();
                        let lyrics = entry.2.clone();
                        cache.push_back(entry);
                        Ok(lyrics)
                    } else {
                        match (&provider, &mut config) {
                            (Ok(provider), Ok(config)) => {
                                // A shared anonymous grant survives a rapid track skip.
                                let prepared = prepare(provider, config, &|| {
                                    worker_cancel.load(Ordering::Acquire)
                                });
                                let _ = reply.send(Event::Settings {
                                    result: Ok(config.status()),
                                    completed: false,
                                });
                                worker_repaint.request_repaint();
                                let result = prepared
                                    .and_then(|()| provider.fetch(config, &request.query, &stop));
                                if let Err(error) = result {
                                    match error {
                                        Error::Auth if config.mode == Mode::Anonymous => {
                                            config.token_rejected = true
                                        }
                                        Error::RateLimited(seconds) => match config.mode {
                                            Mode::Anonymous => {
                                                config.anonymous_retry_after =
                                                    store::now() + seconds
                                            }
                                            Mode::ApiKey => {
                                                config.api_retry_after = store::now() + seconds
                                            }
                                        },
                                        Error::Captcha => match config.mode {
                                            Mode::Anonymous => {
                                                config.anonymous_retry_after = store::now() + 300
                                            }
                                            Mode::ApiKey => {
                                                config.api_retry_after = store::now() + 300
                                            }
                                        },
                                        _ => {}
                                    }
                                    if matches!(
                                        error,
                                        Error::Auth | Error::RateLimited(_) | Error::Captcha
                                    ) {
                                        let _ = store::save(config);
                                        let _ = reply.send(Event::Settings {
                                            result: Ok(config.status()),
                                            completed: false,
                                        });
                                    }
                                }
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
                        if !font_attempted
                            && lyrics.lines.iter().any(|line| {
                                line.text
                                    .chars()
                                    .any(|ch| matches!(ch as u32,0x3000..=0x9fff|0xac00..=0xd7af))
                            })
                        {
                            font_attempted = true;
                            if let Some(font) = cjk_font() {
                                let _ = reply.send(Event::Font(font));
                            }
                        }
                        cache.retain(|(query, _, _)| query != &request.query);
                        cache.push_back((request.query.clone(), Instant::now(), lyrics.clone()));
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
            jobs.config = Some(action);
            jobs.fetch = None;
        }
        let _ = self.wake.try_send(());
    }
    pub fn fetch(&self, query: Query) -> u64 {
        let id = self.latest.fetch_add(1, Ordering::AcqRel) + 1;
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.fetch = Some(Fetch { id, query });
        }
        let _ = self.wake.try_send(());
        id
    }
    pub fn cancel_fetch(&self) {
        self.latest.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.fetch = None;
        }
    }
    pub fn poll(&self) -> Option<Event> {
        self.replies.try_recv().ok()
    }
}

fn cjk_font() -> Option<Vec<u8>> {
    if cfg!(test) {
        return None;
    }
    #[cfg(windows)]
    let paths = {
        let root = std::env::var_os("SystemRoot")
            .map(std::path::PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| "C:\\Windows".into());
        ["meiryo.ttc", "msgothic.ttc", "msyh.ttc"]
            .into_iter()
            .map(|name| root.join("Fonts").join(name))
            .collect::<Vec<_>>()
    };
    #[cfg(not(windows))]
    let paths = [
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    ]
    .into_iter()
    .map(std::path::PathBuf::from)
    .collect::<Vec<_>>();
    for path in paths {
        if std::fs::metadata(&path)
            .ok()
            .is_some_and(|meta| meta.len() <= 32 * 1024 * 1024)
            && let Ok(bytes) = std::fs::read(path)
            && !bytes.is_empty()
        {
            return Some(bytes);
        }
    }
    None
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
    let status = config.status();
    if status.request_retry_after > now {
        return Err(Error::RateLimited(status.request_retry_after - now));
    }
    match config.mode {
        Mode::ApiKey if config.api_key.is_empty() => Err(Error::MissingKey),
        Mode::Anonymous if config.token_rejected => Err(Error::Auth),
        Mode::Anonymous if config.user_token.is_empty() => generate(provider, config, canceled),
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
        let first = service.fetch(query.clone());
        query.key.1 = 2;
        let last = service.fetch(query.clone());
        assert!(last > first);
        let pending = jobs.lock().unwrap().fetch.take().unwrap();
        assert_eq!(pending.id, last);
        assert_eq!(pending.query, query);
        service.fetch(query);
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
        config.token_retry_after = store::now() + 300;
        assert!(matches!(
            generate(&provider, &mut config, &|| false),
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
