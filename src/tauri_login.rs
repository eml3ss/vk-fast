//! VK sign-in in a private Tauri webview, hosted in a separate process so GTK's
//! main loop never competes with egui. Ordinary browser profiles are not read.
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{WebviewUrl, WebviewWindowBuilder};
use zeroize::Zeroizing;

use crate::{
    i18n::Language, model::Profile, web_audio::WebAudioClient, web_session::SessionCookie,
};

#[path = "tauri_login_flow.rs"]
mod flow;

const CHILD_MARKER: &str = "VK_FAST_WEB_LOGIN_CHILD";
const FRAME: &[u8] = b"VK_FAST_WEB_LOGIN:";
const MAX_REPLY: usize = 128 * 1024;
const LOGIN_URL: &str = "https://vk.ru/";

type WindowFlow = Arc<Mutex<flow::LoginWindowFlow>>;

enum Visibility {
    Show,
    Checking(u64),
    Recover(u64),
}

fn present_login(
    window: &tauri::WebviewWindow,
    flow: &mut flow::LoginWindowFlow,
    language: Language,
) {
    let covered = flow.cover_page();
    let _ = window.eval(format!("window.__vkFastLoginCover?.({covered});"));
    let _ = window.set_title(if covered {
        language.text("Проверяем вход VK…", "Checking VK sign-in…")
    } else {
        language.text("Вход в VK", "Sign in to VK")
    });
    if flow.show_if_allowed() {
        if window.show().is_err() {
            flow.show_failed();
        } else {
            let _ = window.set_focus();
        }
    }
}

fn login_cover_script(language: Language) -> String {
    include_str!("tauri_login_cover.js").replace(
        "__STATUS_TEXT__",
        &serde_json::to_string(language.text("Подключаем VK…", "Connecting to VK…")).unwrap(),
    )
}

/// Check navigation state on the UI thread as well as when work is queued.
/// A delayed recovery must not uncover the feed after a newer navigation.
fn update_visibility(
    window: &tauri::WebviewWindow,
    flow: &WindowFlow,
    done: &Arc<AtomicBool>,
    language: Language,
    action: Visibility,
) {
    let view = window.clone();
    let flow = flow.clone();
    let done = done.clone();
    let _ = window.run_on_main_thread(move || {
        if done.load(Ordering::Acquire) {
            return;
        }
        let Ok(mut flow) = flow.lock() else { return };
        let current = match action {
            Visibility::Checking(ticket) => flow.verification_started(ticket),
            Visibility::Show => true,
            Visibility::Recover(ticket) => flow.verification_failed(ticket),
        };
        if current {
            present_login(&view, &mut flow, language);
        }
    });
}

#[derive(Clone, Default)]
pub struct Control {
    canceled: Arc<AtomicBool>,
}
impl Control {
    pub fn cancel(&self) {
        self.canceled.store(true, Ordering::Release);
    }
    fn canceled(&self) -> bool {
        self.canceled.load(Ordering::Acquire)
    }
    pub fn is_canceled(&self) -> bool {
        self.canceled()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Canceled,
    Setup,
    Busy,
    Protocol,
    Interrupted,
}
impl Error {
    pub fn localized(&self, language: Language) -> String {
        match self {
            Self::Canceled => language.text("Вход отменён.", "Sign-in canceled."),
            Self::Setup => language.text(
                "Не удалось открыть окно входа VK.",
                "Could not open the VK sign-in window.",
            ),
            Self::Busy => language.text(
                "Окно входа VK уже открыто. Завершите вход в нём.",
                "A VK sign-in window is already open. Complete sign-in there.",
            ),
            Self::Protocol => language.text(
                "Не удалось получить сессию из окна входа VK. Повторите попытку.",
                "Could not receive the session from VK's sign-in window. Try again.",
            ),
            Self::Interrupted => language.text(
                "Окно входа VK неожиданно закрылось. Повторите попытку.",
                "The VK sign-in window closed unexpectedly. Try again.",
            ),
        }
        .into()
    }
}

pub struct SignedIn {
    pub profile: Profile,
    pub cookies: Vec<SessionCookie>,
}

// Never derive Debug: successful replies carry website credentials.
#[derive(Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Reply {
    Success {
        profile: Profile,
        cookies: Vec<SessionCookie>,
    },
    Canceled,
    Failed {
        kind: Fault,
    },
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Fault {
    Setup,
    Busy,
}

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Starts only this application's own webview helper. Credentials travel in a
/// private stdout pipe, never in arguments, logs, ordinary browser files or JS.
pub fn login(control: Control, language: Language) -> Result<SignedIn, Error> {
    if control.canceled() {
        return Err(Error::Canceled);
    }
    let executable = std::env::current_exe().map_err(|_| Error::Setup)?;
    let mut command = Command::new(executable);
    command
        .args([
            "--web-login",
            "--lang",
            if language == Language::Russian {
                "ru"
            } else {
                "en"
            },
        ])
        .env(CHILD_MARKER, "1")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    // A socket pair still backs the child's stdout, but unlike an ordinary pipe
    // supports a bounded read timeout. WebKit subprocesses cannot keep cancel
    // or successful completion blocked by inheriting an open stdout handle.
    #[cfg(unix)]
    let (mut stdout, output) = {
        use std::os::{fd::OwnedFd, unix::net::UnixStream};
        let (reader, writer) = UnixStream::pair().map_err(|_| Error::Setup)?;
        reader
            .set_read_timeout(Some(Duration::from_millis(100)))
            .map_err(|_| Error::Setup)?;
        (reader, Stdio::from(OwnedFd::from(writer)))
    };
    #[cfg(not(unix))]
    let output = Stdio::piped();
    command.stdout(output);
    let mut child = Process(command.spawn().map_err(|_| Error::Setup)?);
    #[cfg(not(unix))]
    let mut stdout = child.0.stdout.take().ok_or(Error::Setup)?;
    let overflow = Arc::new(AtomicBool::new(false));
    let reader_overflow = overflow.clone();
    let stop_reader = Arc::new(AtomicBool::new(false));
    let reader_stop = stop_reader.clone();
    let reader = thread::Builder::new()
        .name("vk-webview-response".into())
        .spawn(move || {
            let mut bytes = Zeroizing::new(Vec::new());
            let mut chunk = [0_u8; 8192];
            loop {
                match stdout.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(count) => {
                        bytes.extend_from_slice(&chunk[..count]);
                        if bytes.len() > MAX_REPLY {
                            reader_overflow.store(true, Ordering::Release);
                            break;
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                        ) =>
                    {
                        if reader_stop.load(Ordering::Acquire) {
                            break;
                        }
                    }
                    Err(error) => return Err(error),
                }
            }
            Ok::<_, io::Error>(bytes)
        })
        .map_err(|_| Error::Setup)?;
    let result = loop {
        if control.canceled() {
            break Err(Error::Canceled);
        }
        if overflow.load(Ordering::Acquire) {
            break Err(Error::Protocol);
        }
        match child.0.try_wait() {
            Ok(Some(_)) => break Ok(()),
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(_) => break Err(Error::Interrupted),
        }
    };
    // Reap before joining the pipe reader, including when network/WebKit stalls.
    if result.is_err() {
        let _ = child.0.kill();
    }
    let _ = child.0.wait();
    stop_reader.store(true, Ordering::Release);
    let bytes = reader
        .join()
        .map_err(|_| Error::Protocol)?
        .map_err(|_| Error::Protocol)?;
    result?;
    if control.canceled() {
        return Err(Error::Canceled);
    }
    decode_reply(&bytes)
}

fn decode_reply(bytes: &[u8]) -> Result<SignedIn, Error> {
    if bytes.len() > MAX_REPLY {
        return Err(Error::Protocol);
    }
    let starts: Vec<_> = bytes
        .windows(FRAME.len())
        .enumerate()
        .filter_map(|(index, part)| (part == FRAME).then_some(index))
        .collect();
    if starts.len() != 1 {
        return Err(Error::Protocol);
    }
    let body = &bytes[starts[0] + FRAME.len()..];
    let body = body
        .split(|byte| *byte == b'\n')
        .next()
        .ok_or(Error::Protocol)?;
    let reply: Reply = serde_json::from_slice(body).map_err(|_| Error::Protocol)?;
    match reply {
        Reply::Canceled => Err(Error::Canceled),
        Reply::Failed { kind: Fault::Busy } => Err(Error::Busy),
        Reply::Failed { kind: Fault::Setup } => Err(Error::Setup),
        Reply::Success { profile, cookies } => {
            if profile.id <= 0
                || profile.first_name.len() > 1024
                || profile.last_name.len() > 1024
                || cookies.is_empty()
                || cookies.len() > 256
                || cookies.iter().any(|cookie| !cookie.is_valid())
                || !has_session(&cookies)
            {
                return Err(Error::Protocol);
            }
            Ok(SignedIn { profile, cookies })
        }
    }
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}
fn has_session(cookies: &[SessionCookie]) -> bool {
    cookies.iter().any(|cookie| {
        matches!(cookie.name.as_str(), "remixsid" | "remixsid6")
            && !cookie.value.is_empty()
            && cookie.path == "/"
            && cookie.expires_unix.is_none_or(|expiry| expiry > now())
    })
}
fn profile_directory() -> io::Result<PathBuf> {
    #[cfg(windows)]
    {
        let application = crate::app_paths::state_dir()?;
        private_directory(&application)?;
        let webview = application.join("webview");
        private_directory(&webview)?;
        Ok(webview)
    }
    #[cfg(not(windows))]
    {
        let state = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|p| p.is_absolute())
                    .map(|p| p.join(".local/state"))
            })
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "User state directory unavailable")
            })?;
        fs::create_dir_all(&state)?;
        let application = state.join("vk-fast");
        private_directory(&application)?;
        let webview = application.join("webview");
        private_directory(&webview)?;
        Ok(webview)
    }
}
fn private_directory(path: &std::path::Path) -> io::Result<()> {
    match fs::create_dir(path) {
        Ok(()) => (),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(io::Error::other("Invalid webview directory"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        #[cfg(target_os = "linux")]
        if metadata.uid() != fs::metadata("/proc/self")?.uid() {
            return Err(io::Error::other("Invalid webview owner"));
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn profile_lock(directory: &std::path::Path) -> io::Result<File> {
    let path = directory.join(".login.lock");
    if let Ok(metadata) = fs::symlink_metadata(&path)
        && (!metadata.is_file() || metadata.file_type().is_symlink())
    {
        return Err(io::Error::other("Invalid login lock"));
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    file.try_lock().map_err(io::Error::from)?;
    Ok(file)
}

/// Explicit sign-out removes only the application's private webview data.
/// Keep the lock inode so another login cannot bypass cleanup by recreating it.
pub fn clear_profile() -> io::Result<()> {
    clear_directory(&profile_directory()?)
}

fn clear_directory(directory: &std::path::Path) -> io::Result<()> {
    let _lock = profile_lock(directory)?;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_name() == ".login.lock" {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            fs::remove_dir_all(entry.path())?;
        } else {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

fn navigation_allowed(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && url.host_str().is_some_and(|host| {
            host == "vk.ru"
                || host.ends_with(".vk.ru")
                || host == "vk.com"
                || host.ends_with(".vk.com")
        })
}
fn snapshot(window: &tauri::WebviewWindow) -> Vec<SessionCookie> {
    let mut output = Vec::new();
    let mut seen = HashSet::new();
    for host in ["vk.ru", "vk.com"] {
        let Ok(url) = reqwest::Url::parse(&format!("https://{host}/")) else {
            continue;
        };
        let Ok(cookies) = window.cookies_for_url(url) else {
            continue;
        };
        for cookie in cookies {
            let expires_unix = cookie
                .expires_datetime()
                .map(|expiry| expiry.unix_timestamp());
            let value = SessionCookie {
                domain: cookie.domain().unwrap_or(host).to_owned(),
                path: cookie.path().unwrap_or("/").to_owned(),
                name: cookie.name().to_owned(),
                value: cookie.value().to_owned(),
                secure: cookie.secure().unwrap_or(true),
                expires_unix,
            };
            if !value.is_valid() || expires_unix.is_some_and(|expiry| expiry <= now()) {
                continue;
            }
            if seen.insert((value.domain.clone(), value.path.clone(), value.name.clone())) {
                output.push(value);
                if output.len() > 256 {
                    return Vec::new();
                }
            }
        }
    }
    output
}
fn fingerprint(cookies: &[SessionCookie]) -> [u8; 32] {
    let mut hash = Sha256::new();
    let mut session: Vec<_> = cookies
        .iter()
        .filter(|cookie| matches!(cookie.name.as_str(), "remixsid" | "remixsid6"))
        .collect();
    session.sort_by_key(|cookie| (&cookie.domain, &cookie.path, &cookie.name));
    for cookie in session {
        hash.update(cookie.domain.as_bytes());
        hash.update([0]);
        hash.update(cookie.path.as_bytes());
        hash.update([0]);
        hash.update(cookie.name.as_bytes());
        hash.update([0]);
        hash.update(cookie.value.as_bytes());
        hash.update([0]);
    }
    hash.finalize().into()
}
fn verified_session_matches(cookies: &[SessionCookie], expected: [u8; 32]) -> bool {
    has_session(cookies) && fingerprint(cookies) == expected
}
fn finish_once(outcome: &Mutex<Option<Reply>>, done: &AtomicBool, reply: Reply) -> bool {
    let Ok(mut value) = outcome.lock() else {
        return false;
    };
    if done.load(Ordering::Acquire) {
        return false;
    }
    *value = Some(reply);
    done.store(true, Ordering::Release);
    true
}
fn write_reply(reply: &Reply) -> Result<(), String> {
    let bytes =
        Zeroizing::new(serde_json::to_vec(reply).map_err(|_| "Could not return sign-in result")?);
    if bytes.len() + FRAME.len() > MAX_REPLY {
        return Err("Sign-in result is too large".into());
    }
    let mut output = io::stdout().lock();
    output
        .write_all(FRAME)
        .and_then(|()| output.write_all(&bytes))
        .and_then(|()| output.write_all(b"\n"))
        .and_then(|()| output.flush())
        .map_err(|_| "Could not return sign-in result".into())
}

/// Only the parent process launches this mode. It never handles normal browser
/// profiles; its cookie jar lives in vk-fast/webview and has no remote commands.
pub fn run_helper(language: Language) -> Result<(), String> {
    if std::env::var(CHILD_MARKER).as_deref() != Ok("1") {
        return Err("Start VK sign-in from the application.".into());
    }
    let directory = match profile_directory() {
        Ok(directory) => directory,
        Err(_) => return write_reply(&Reply::Failed { kind: Fault::Setup }),
    };
    let _lock = match profile_lock(&directory) {
        Ok(lock) => lock,
        Err(_) => return write_reply(&Reply::Failed { kind: Fault::Busy }),
    };
    let outcome = Arc::new(Mutex::new(None));
    let done = Arc::new(AtomicBool::new(false));
    let visibility = Arc::new(Mutex::new(flow::LoginWindowFlow::default()));
    let finished_page = Arc::new(AtomicU64::new(u64::MAX));
    let (wake, wakes) = std::sync::mpsc::sync_channel(1);
    let setup_outcome = outcome.clone();
    let setup_done = done.clone();
    let setup_visibility = visibility.clone();
    let app = tauri::Builder::default()
        .setup(move |app| {
            let url = LOGIN_URL
                .parse()
                .map_err(|_| io::Error::other("Invalid VK login URL"))?;
            let page_visibility = setup_visibility.clone();
            let page_done = setup_done.clone();
            let page_finished = finished_page.clone();
            let mut window = WebviewWindowBuilder::new(app, "vk-login", WebviewUrl::External(url))
                .title(language.text("Вход в VK", "Sign in to VK"))
                .inner_size(500.0, 650.0)
                .min_inner_size(420.0, 540.0)
                .resizable(true)
                .maximizable(false)
                .center()
                .visible(false)
                .data_directory(directory.clone())
                .devtools(false)
                .general_autofill_enabled(false)
                .user_agent(crate::web_audio::USER_AGENT)
                .initialization_script(login_cover_script(language))
                .on_navigation(navigation_allowed)
                .on_page_load(move |window, page| {
                    if page_done.load(Ordering::Acquire) || !navigation_allowed(page.url()) {
                        return;
                    }
                    let started = page.event() == tauri::webview::PageLoadEvent::Started;
                    if let Ok(mut flow) = page_visibility.lock() {
                        // Redirects change the in-page cover, never native visibility.
                        // Cookies and HTTP stay on the worker thread.
                        flow.page_event(page.url(), started);
                        present_login(&window, &mut flow, language);
                        if !started && flow.finished_login_return(page.url()) {
                            page_finished.store(flow.ticket(), Ordering::Release);
                        }
                    }
                    let _ = wake.try_send(());
                });
            #[cfg(windows)]
            {
                window = window
                    .theme(Some(tauri::Theme::Dark))
                    .background_color(tauri::webview::Color(15, 17, 21, 255));
            }
            // Use the same app artwork for the auxiliary login window. On
            // Windows this comes from the executable's canonical ICO resource.
            if let Some(icon) = app.default_window_icon() {
                window = window.icon(icon.clone())?;
            }
            let window = window.build()?;
            let handle = app.handle().clone();
            let poll_done = setup_done.clone();
            let poll_outcome = setup_outcome.clone();
            thread::Builder::new()
                .name("vk-webview-session".into())
                .spawn(move || {
                    let mut previous = None;
                    let mut retry = Instant::now();
                    while !poll_done.load(Ordering::Acquire) {
                        // Tauri marshals this native operation to its runtime. Calling
                        // it from this worker avoids blocking the GTK/WebView loop.
                        let ticket = match setup_visibility.lock() {
                            Ok(flow) => flow.ticket(),
                            Err(_) => break,
                        };
                        let cookies = snapshot(&window);
                        if !setup_visibility
                            .lock()
                            .is_ok_and(|flow| flow.is_current(ticket))
                        {
                            previous = None;
                            continue;
                        }
                        let candidate = has_session(&cookies);
                        let current = (ticket, fingerprint(&cookies));
                        if candidate && (previous != Some(current) || Instant::now() >= retry) {
                            previous = Some(current);
                            retry = Instant::now() + Duration::from_secs(10);
                            update_visibility(
                                &window,
                                &setup_visibility,
                                &poll_done,
                                language,
                                Visibility::Checking(ticket),
                            );
                            let verified = WebAudioClient::new(cookies.clone())
                                .and_then(|client| client.session_profile());
                            let retry_in_place = matches!(
                                &verified,
                                Err(crate::web_audio::WebAudioError::Network
                                    | crate::web_audio::WebAudioError::Timeout
                                    | crate::web_audio::WebAudioError::Http(500..=599))
                            );
                            // A same-document sign-in can rotate the SID without
                            // any navigation event. Discard a stale verification
                            // before it can publish an account or reveal the UI.
                            let latest = snapshot(&window);
                            if !verified_session_matches(&latest, current.1)
                                || !setup_visibility
                                    .lock()
                                    .is_ok_and(|flow| flow.is_current(ticket))
                            {
                                previous = None;
                                if !has_session(&latest) {
                                    update_visibility(
                                        &window,
                                        &setup_visibility,
                                        &poll_done,
                                        language,
                                        Visibility::Recover(ticket),
                                    );
                                }
                                continue;
                            }
                            if let Ok(profile) = verified {
                                let cookies = latest;
                                let view = window.clone();
                                let flow = setup_visibility.clone();
                                let outcome = poll_outcome.clone();
                                let done = poll_done.clone();
                                let handle = handle.clone();
                                let _ = window.run_on_main_thread(move || {
                                    let Ok(mut flow) = flow.lock() else { return };
                                    if flow.is_current(ticket)
                                        && finish_once(
                                            &outcome,
                                            &done,
                                            Reply::Success { profile, cookies },
                                        )
                                    {
                                        flow.cancel();
                                        let _ = view.hide();
                                        handle.exit(0);
                                    }
                                });
                            } else if !retry_in_place {
                                update_visibility(
                                    &window,
                                    &setup_visibility,
                                    &poll_done,
                                    language,
                                    Visibility::Recover(ticket),
                                );
                            }
                        } else if !candidate && finished_page.load(Ordering::Acquire) == ticket {
                            update_visibility(
                                &window,
                                &setup_visibility,
                                &poll_done,
                                language,
                                Visibility::Recover(ticket),
                            );
                        }
                        update_visibility(
                            &window,
                            &setup_visibility,
                            &poll_done,
                            language,
                            Visibility::Show,
                        );
                        // Page events wake this immediately; the fallback also
                        // catches cookies set by same-document/SPA transitions.
                        let _ = wakes.recv_timeout(Duration::from_millis(100));
                    }
                })?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .map_err(|_| "Could not create the VK sign-in window")?;
    let exit_done = done.clone();
    let exit_outcome = outcome.clone();
    app.run_return(move |_, event| {
        if matches!(
            event,
            tauri::RunEvent::Exit | tauri::RunEvent::ExitRequested { .. }
        ) {
            if let Ok(mut flow) = visibility.lock() {
                flow.cancel();
            }
            let _ = finish_once(&exit_outcome, &exit_done, Reply::Canceled);
        }
    });
    let reply = outcome
        .lock()
        .map_err(|_| "Could not complete VK sign-in")?
        .take()
        .unwrap_or(Reply::Canceled);
    write_reply(&reply)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn cookies() -> Vec<SessionCookie> {
        vec![SessionCookie {
            domain: ".vk.ru".into(),
            path: "/".into(),
            name: "remixsid6".into(),
            value: "synthetic-session".into(),
            secure: true,
            expires_unix: None,
        }]
    }
    fn frame(reply: &Reply) -> Vec<u8> {
        let mut bytes = FRAME.to_vec();
        bytes.extend(serde_json::to_vec(reply).unwrap());
        bytes
    }
    #[test]
    fn helper_result_is_bounded_and_keeps_credentials_out_of_errors() {
        let reply = Reply::Success {
            profile: Profile {
                id: 42,
                first_name: "Test".into(),
                last_name: String::new(),
                avatar_url: None,
            },
            cookies: cookies(),
        };
        assert_eq!(decode_reply(&frame(&reply)).unwrap().profile.id, 42);
        assert!(matches!(
            decode_reply(&vec![b'x'; MAX_REPLY + 1]),
            Err(Error::Protocol)
        ));
        assert!(matches!(
            decode_reply(b"unframed synthetic-session"),
            Err(Error::Protocol)
        ));
        let mut duplicated = frame(&Reply::Canceled);
        duplicated.extend(frame(&Reply::Canceled));
        assert!(matches!(decode_reply(&duplicated), Err(Error::Protocol)));
        assert!(matches!(
            decode_reply(&frame(&Reply::Canceled)),
            Err(Error::Canceled)
        ));
        assert!(
            !Error::Protocol
                .localized(Language::English)
                .contains("synthetic-session")
        );
    }
    #[test]
    fn helper_requires_real_vk_session_not_foreign_or_expired_cookie() {
        let mut values = cookies();
        values[0].domain = "vk.ru.example.com".into();
        let reply = Reply::Success {
            profile: Profile {
                id: 42,
                first_name: String::new(),
                last_name: String::new(),
                avatar_url: None,
            },
            cookies: values,
        };
        assert!(matches!(decode_reply(&frame(&reply)), Err(Error::Protocol)));
        let mut values = cookies();
        values[0].expires_unix = Some(1);
        assert!(!has_session(&values));
        assert!(navigation_allowed(&LOGIN_URL.parse().unwrap()));
        for target in [
            "http://vk.ru/",
            "https://vk.ru.example.com/",
            "https://evil.example/",
            "file:///tmp/a",
        ] {
            assert!(!navigation_allowed(&target.parse().unwrap()));
        }
    }

    #[test]
    fn a_profile_check_cannot_accept_a_rotated_or_expired_session() {
        let original = cookies();
        let checked = fingerprint(&original);
        assert!(verified_session_matches(&original, checked));
        let mut latest = original.clone();
        latest[0].value = "synthetic-new-account-session".into();
        assert!(!verified_session_matches(&latest, checked));
        latest = original.clone();
        latest[0].expires_unix = Some(1);
        assert!(!verified_session_matches(&latest, checked));
        assert!(!verified_session_matches(&[], checked));
        latest = original;
        let mut language = latest[0].clone();
        language.name = "remixlang".into();
        language.value = "0".into();
        latest.push(language);
        latest.reverse();
        assert!(verified_session_matches(&latest, checked));
    }
}
