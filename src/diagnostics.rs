//! Explicit, bounded local diagnostics. Authentication material is never a log
//! format: callers record stage/category messages, and a second filter strips
//! URLs, credentials, paths and identifier-like values before queuing them.
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Mutex, Once, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    thread::{self, JoinHandle},
};

const LIMIT: u64 = 2 * 1024 * 1024;
const QUEUE: usize = 256;
const MAX_MESSAGE_CHARS: usize = 2048;

#[derive(Default)]
struct Logger {
    enabled: AtomicBool,
    control: Mutex<()>,
    sender: Mutex<Option<SyncSender<String>>>,
    worker: Mutex<Option<JoinHandle<io::Result<()>>>>,
}

fn logger() -> &'static Logger {
    static LOGGER: OnceLock<Logger> = OnceLock::new();
    LOGGER.get_or_init(Logger::default)
}

pub fn file_path() -> io::Result<PathBuf> {
    #[cfg(windows)]
    let base = crate::app_paths::config_dir()?;
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|path| path.join(".config"))
        })
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "Configuration directory unavailable",
            )
        })?
        .join("vk-fast");
    Ok(base.join("logs/vk-fast.log"))
}

/// Install a payload-free panic diagnostic. Disabled logging creates no files.
pub fn init(active: bool) -> io::Result<()> {
    static PANIC_HOOK: Once = Once::new();
    PANIC_HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if let Some(location) = info.location() {
                let file = Path::new(location.file())
                    .file_name()
                    .and_then(|file| file.to_str())
                    .unwrap_or("source");
                event(
                    "panic",
                    &format!(
                        "panic at {file}:{} thread={} (payload omitted)",
                        location.line(),
                        thread::current().name().unwrap_or("unnamed")
                    ),
                );
            } else {
                event("panic", "panic (payload omitted)");
            }
            previous(info);
        }));
    });
    set_enabled(active)
}

pub fn enabled() -> bool {
    #[cfg(test)]
    if CAPTURE.with(|capture| capture.borrow().is_some()) {
        return true;
    }
    logger().enabled.load(Ordering::Acquire)
}

#[cfg(test)]
thread_local! {
    /// A test's own log, kept apart from the shared one and other tests.
    static CAPTURE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// The interface's own words (labels, buttons, notices), learned as the app
/// shows them while logging is on. A widget label among them is logged as
/// is; any other text (a track, playlist or artist name) is not.
fn ui_texts() -> &'static Mutex<std::collections::HashSet<&'static str>> {
    static TEXTS: OnceLock<Mutex<std::collections::HashSet<&'static str>>> = OnceLock::new();
    TEXTS.get_or_init(Default::default)
}

/// Called by `Language::text` for both of its translations.
pub fn note_ui_text(russian: &'static str, english: &'static str) {
    if !enabled() {
        return;
    }
    if let Ok(mut texts) = ui_texts().lock() {
        texts.insert(russian);
        texts.insert(english);
    }
}

/// A widget's label for the log: the interface's own words as they are,
/// anything else, such as a track's title, as `[content]`.
pub fn ui_label(label: &str) -> String {
    let label = label.trim();
    if label.is_empty() {
        return "[no label]".into();
    }
    let known = ui_texts().lock().is_ok_and(|texts| texts.contains(label));
    if known {
        format!("«{label}»")
    } else {
        "[content]".into()
    }
}

pub fn set_enabled(active: bool) -> io::Result<()> {
    let state = logger();
    let _control = state
        .control
        .lock()
        .map_err(|_| io::Error::other("Diagnostic controller unavailable"))?;
    if active && state.enabled.load(Ordering::Acquire) {
        return Ok(());
    }
    stop(state)?;
    if !active {
        return Ok(());
    }
    start(state, &file_path()?)?;
    event(
        "app",
        &format!(
            "diagnostics enabled version={} os={} arch={}{}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            desktop()
        ),
    );
    Ok(())
}

/// The desktop session on Linux: Wayland or X11, and which desktop. These
/// are names of software, not of the person.
fn desktop() -> String {
    if !cfg!(target_os = "linux") {
        return String::new();
    }
    let value = |name: &str| {
        std::env::var(name)
            .ok()
            .map(|value| {
                value
                    .chars()
                    .filter(|character| {
                        character.is_ascii_alphanumeric() || ":._-".contains(*character)
                    })
                    .take(40)
                    .collect::<String>()
            })
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "-".into())
    };
    format!(
        " session={} desktop={} wayland_display={}",
        value("XDG_SESSION_TYPE"),
        value("XDG_CURRENT_DESKTOP"),
        std::env::var_os("WAYLAND_DISPLAY").is_some()
    )
}

fn start(state: &Logger, path: &Path) -> io::Result<()> {
    let writer = LogWriter::open(path, LIMIT)?;
    let (sender, receiver) = mpsc::sync_channel::<String>(QUEUE);
    let worker = thread::Builder::new()
        .name("vk-diagnostics".into())
        .spawn(move || {
            let mut writer = writer;
            while let Ok(line) = receiver.recv() {
                writer.line(&line)?;
            }
            writer.flush()
        })?;
    *state
        .worker
        .lock()
        .map_err(|_| io::Error::other("Diagnostic worker unavailable"))? = Some(worker);
    *state
        .sender
        .lock()
        .map_err(|_| io::Error::other("Diagnostic queue unavailable"))? = Some(sender);
    state.enabled.store(true, Ordering::Release);
    Ok(())
}

/// Sends an HTTP request and, when logging is on, writes what kind of
/// request it was (`what`, a fixed name), how it ended and how long it
/// took. Never its address, parameters or answer.
pub trait SendLogged {
    fn send_logged(
        self,
        component: &str,
        what: &str,
    ) -> reqwest::Result<reqwest::blocking::Response>;
}

impl SendLogged for reqwest::blocking::RequestBuilder {
    fn send_logged(
        self,
        component: &str,
        what: &str,
    ) -> reqwest::Result<reqwest::blocking::Response> {
        let started = std::time::Instant::now();
        let result = self.send();
        if enabled() {
            let outcome = match &result {
                Ok(response) => format!("status={}", response.status().as_u16()),
                Err(error) if error.is_timeout() => "timed out".into(),
                Err(error) if error.is_connect() => "no connection".into(),
                Err(_) => "failed".into(),
            };
            event(
                component,
                &format!("{what} {outcome} ms={}", started.elapsed().as_millis()),
            );
        }
        result
    }
}

/// Not for audio callbacks. This never waits for disk or for a full queue.
pub fn event(component: &str, message: &str) {
    #[cfg(test)]
    {
        let captured = CAPTURE.with(|capture| {
            capture.borrow_mut().as_mut().map(|log| {
                log.push_str(&record(component, message));
            })
        });
        if captured.is_some() {
            return;
        }
    }
    queue_event(logger(), component, message);
}

/// One line of the log.
fn record(component: &str, message: &str) -> String {
    let component: String = component
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || "._-".contains(*character))
        .take(64)
        .collect();
    let message = sanitize(message);
    let timestamp = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%:z");
    format!("{timestamp} [{component}] {message}\n")
}

fn queue_event(state: &Logger, component: &str, message: &str) {
    if !state.enabled.load(Ordering::Acquire) {
        return;
    }
    let line = record(component, message);
    let sender = state.sender.lock().ok().and_then(|sender| sender.clone());
    if let Some(sender) = sender
        && matches!(
            sender.try_send(line),
            Err(mpsc::TrySendError::Disconnected(_))
        )
    {
        state.enabled.store(false, Ordering::Release);
    }
}

/// Drain accepted records on normal shutdown; never create a log implicitly.
pub fn shutdown() {
    let state = logger();
    if let Ok(_control) = state.control.lock() {
        let _ = stop(state);
    }
}

fn stop(state: &Logger) -> io::Result<()> {
    state.enabled.store(false, Ordering::Release);
    state
        .sender
        .lock()
        .map_err(|_| io::Error::other("Diagnostic queue unavailable"))?
        .take();
    if let Some(worker) = state
        .worker
        .lock()
        .map_err(|_| io::Error::other("Diagnostic worker unavailable"))?
        .take()
    {
        worker
            .join()
            .map_err(|_| io::Error::other("Diagnostic worker failed"))??;
    }
    Ok(())
}

fn sanitize(message: &str) -> String {
    let bounded: String = message.chars().take(MAX_MESSAGE_CHARS).collect();
    let bounded = bounded
        .split_whitespace()
        .map(|word| {
            if word.contains("://") || word.to_ascii_lowercase().starts_with("www.") {
                "[url]"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let lower = bounded.to_ascii_lowercase();
    if [
        "cookie",
        "authorization",
        "access_token",
        "refresh_token",
        "password",
        "remixsid",
        "client_secret",
        "code_verifier",
        "code_challenge",
        "auth_code",
        "auth code",
        "device_code",
        "owner_id",
        "user_id",
        "account_id",
        "audio_id",
        "track_id",
        "profile_id",
        "owner=",
        "account=",
        "token=",
        "token:",
        "\"token\"",
        "secret=",
        "bearer ",
    ]
    .iter()
    .any(|key| lower.contains(key))
    {
        return "[sensitive fields omitted]".into();
    }
    bounded
        .split_whitespace()
        .map(|word| {
            let trimmed = word.trim_matches(['\"', '\'', '(', ')', '[', ']', '{', '}', ',']);
            let lower = trimmed.to_ascii_lowercase();
            if lower.contains("://") || lower.starts_with("www.") {
                return "[url]".into();
            }
            if trimmed.starts_with('/')
                || trimmed.contains('\\')
                || [
                    "/home/",
                    "/Users/",
                    "/run/user/",
                    "/tmp/",
                    "/mnt/",
                    "/private/",
                ]
                .iter()
                .any(|prefix| trimmed.contains(prefix))
                || trimmed.as_bytes().windows(3).any(|piece| {
                    piece[0].is_ascii_alphabetic()
                        && piece[1] == b':'
                        && matches!(piece[2], b'/' | b'\\')
                })
                || (trimmed.len() > 2
                    && trimmed.as_bytes()[1] == b':'
                    && matches!(trimmed.as_bytes()[2], b'/' | b'\\'))
            {
                return "[path]".into();
            }
            if (trimmed.starts_with("eyJ") && trimmed.matches('.').count() >= 2)
                || (trimmed.len() >= 32
                    && trimmed
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_-=./".contains(&byte)))
            {
                return "[identifier]".into();
            }
            let mut output = String::new();
            let mut digits = String::new();
            let flush = |output: &mut String, digits: &mut String| {
                if digits.len() >= 5 {
                    output.push_str("[number]");
                } else {
                    output.push_str(digits);
                }
                digits.clear();
            };
            for character in word.chars() {
                if character.is_ascii_digit() {
                    digits.push(character);
                } else {
                    flush(&mut output, &mut digits);
                    if !character.is_control() {
                        output.push(character);
                    }
                }
            }
            flush(&mut output, &mut digits);
            output
        })
        .collect::<Vec<String>>()
        .join(" ")
}

struct LogWriter {
    path: PathBuf,
    file: Option<File>,
    size: u64,
    limit: u64,
}

fn check_file(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => Err(
            io::Error::new(io::ErrorKind::PermissionDenied, "Unsafe diagnostic file"),
        ),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

impl LogWriter {
    fn open(path: &Path, limit: u64) -> io::Result<Self> {
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Diagnostic parent unavailable")
        })?;
        if let Some(config) = parent.parent() {
            fs::create_dir_all(config)?;
            if fs::symlink_metadata(config)?.file_type().is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Unsafe diagnostic config directory",
                ));
            }
        }
        fs::create_dir_all(parent)?;
        if fs::symlink_metadata(parent)?.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Unsafe diagnostic directory",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
        check_file(path)?;
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        let size = file.metadata()?.len();
        Ok(Self {
            path: path.into(),
            file: Some(file),
            size,
            limit,
        })
    }

    fn backup(&self, number: u8) -> PathBuf {
        self.path.with_extension(format!("log.{number}"))
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.flush()?;
        self.file.take();
        for path in [&self.path, &self.backup(1), &self.backup(2)] {
            check_file(path)?;
        }
        match fs::remove_file(self.backup(2)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        if self.backup(1).exists() {
            fs::rename(self.backup(1), self.backup(2))?;
        }
        if self.path.exists() {
            fs::rename(&self.path, self.backup(1))?;
        }
        let replacement = Self::open(&self.path, self.limit)?;
        *self = replacement;
        Ok(())
    }

    fn line(&mut self, line: &str) -> io::Result<()> {
        if self.size > 0 && self.size.saturating_add(line.len() as u64) > self.limit {
            self.rotate()?;
        }
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::other("Diagnostic file closed"))?
            .write_all(line.as_bytes())?;
        self.size += line.len() as u64;
        Ok(())
    }
    fn flush(&mut self) -> io::Result<()> {
        if let Some(file) = &mut self.file {
            file.flush()?;
        }
        Ok(())
    }
}

/// Runs `run` with logging on for this thread only, and returns what it
/// wrote. The shared log and other tests are not touched.
#[cfg(test)]
pub fn capture_for_test<R>(run: impl FnOnce() -> R) -> (R, String) {
    CAPTURE.with(|capture| *capture.borrow_mut() = Some(String::new()));
    let result = run();
    let log = CAPTURE.with(|capture| capture.borrow_mut().take().unwrap_or_default());
    (result, log)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let mut bytes = [0u8; 12];
            getrandom::fill(&mut bytes).unwrap();
            let suffix: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
            let path = std::env::temp_dir().join(format!("vk-fast-diagnostics-{suffix}"));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> PathBuf {
            self.0.join("logs/vk-fast.log")
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn credentials_urls_paths_and_identifiers_do_not_reach_the_log_record() {
        for message in [
            "Cookie: remixsid=secret",
            "access_token=secret",
            "Authorization: Bearer secret",
            "owner_id=42 raw",
            "code_verifier=secret",
        ] {
            assert_eq!(sanitize(message), "[sensitive fields omitted]");
        }
        let message = sanitize(
            "resolve error=NoStream url=https://vk.ru/audio?token=secret /home/friend/private.json C:\\Users\\Friend\\secret.txt code=618422 id=123456789",
        );
        assert!(
            !message.contains("secret")
                && !message.contains("friend")
                && !message.contains("Friend")
                && !message.contains("618422")
                && !message.contains("123456789")
        );
        assert!(message.contains("error=NoStream"));
        assert_eq!(
            sanitize("error=Network http_status=403 attempt=2"),
            "error=Network http_status=403 attempt=2"
        );
        assert_eq!(
            sanitize("exit_failed code=0xc000_0135; spawn_failed os_error=0x0000_36b1"),
            "exit_failed code=0xc000_0135; spawn_failed os_error=0x0000_36b1"
        );
        assert!(
            !sanitize("eyJsecret12345678.eyJpayload12345678.signature12345678").contains("secret")
        );
    }

    #[test]
    fn rotating_writer_keeps_only_three_bounded_files_and_preserves_recent_records() {
        let directory = Directory::new();
        let path = directory.path();
        let mut writer = LogWriter::open(&path, 64).unwrap();
        for index in 0..20 {
            writer
                .line(&format!("event number {index:02} safe diagnostic\n"))
                .unwrap();
        }
        writer.flush().unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("19"));
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 3);
        for path in [&path, &writer.backup(1), &writer.backup(2)] {
            assert!(fs::metadata(path).unwrap().len() <= 64);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn bounded_worker_drains_on_disable_and_does_not_write_after_acceptance_is_stopped() {
        let directory = Directory::new();
        let path = directory.path();
        let state = Logger::default();
        start(&state, &path).unwrap();
        queue_event(&state, "playback", "accepted first");
        stop(&state).unwrap();
        assert!(!state.enabled.load(Ordering::Acquire));
        assert!(state.sender.lock().unwrap().is_none());
        let first = fs::read_to_string(&path).unwrap();
        assert!(first.contains("accepted first"));
        queue_event(&state, "playback", "should not appear");
        assert_eq!(fs::read_to_string(&path).unwrap(), first);
        start(&state, &path).unwrap();
        queue_event(&state, "tray.windows", "accepted second");
        stop(&state).unwrap();
        let final_text = fs::read_to_string(path).unwrap();
        assert!(final_text.contains("accepted first") && final_text.contains("accepted second"));
        assert!(!final_text.contains("should not appear"));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_cannot_redirect_diagnostics_into_another_file() {
        use std::os::unix::fs::symlink;
        let directory = Directory::new();
        fs::create_dir(directory.path().parent().unwrap()).unwrap();
        let unrelated = directory.0.join("private");
        fs::write(&unrelated, "unchanged").unwrap();
        symlink(&unrelated, directory.path()).unwrap();
        assert!(LogWriter::open(&directory.path(), LIMIT).is_err());
        assert_eq!(fs::read_to_string(unrelated).unwrap(), "unchanged");
    }
}
