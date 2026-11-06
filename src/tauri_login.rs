//! VK sign-in in a private Tauri webview, hosted in a separate process so GTK's
//! main loop never competes with egui. Ordinary browser profiles are not read.
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
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
use zeroize::{Zeroize, Zeroizing};

use crate::{
    i18n::Language, model::Profile, web_audio::WebAudioClient, web_session::SessionCookie,
};

#[path = "tauri_login_flow.rs"]
mod flow;
pub use crate::web_login_state::Control;
use crate::web_login_state::{Action, Command as InputCommand, Snapshot as LoginSnapshot, Step};

const CHILD_MARKER: &str = "VK_FAST_WEB_LOGIN_CHILD";
const FRAME: &[u8] = b"VK_FAST_WEB_LOGIN:";
const MAX_REPLY: usize = 128 * 1024;
const LOGIN_URL: &str = "https://vk.ru/";

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
    State {
        view: LoginSnapshot,
    },
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
    if control.is_canceled() {
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
        .stdin(Stdio::piped())
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
    let mut input = child.0.stdin.take().ok_or(Error::Setup)?;
    let overflow = Arc::new(AtomicBool::new(false));
    let reader_overflow = overflow.clone();
    let stop_reader = Arc::new(AtomicBool::new(false));
    let reader_stop = stop_reader.clone();
    let reader_control = control.clone();
    let reader = thread::Builder::new()
        .name("vk-webview-response".into())
        .spawn(move || {
            let mut decoder = ResponseStream::default();
            let mut chunk = [0_u8; 8192];
            loop {
                match stdout.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(count) => {
                        if decoder.push(&chunk[..count], &reader_control).is_err() {
                            chunk.zeroize();
                            reader_overflow.store(true, Ordering::Release);
                            return Err(io::Error::other("Invalid sign-in response"));
                        }
                        chunk.zeroize();
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
            decoder
                .finish()
                .map_err(|_| io::Error::other("Incomplete sign-in response"))
        })
        .map_err(|_| Error::Setup)?;
    let result = loop {
        if control.is_canceled() {
            break Err(Error::Canceled);
        }
        if overflow.load(Ordering::Acquire) {
            break Err(Error::Protocol);
        }
        if let Some(command) = control.take_command() {
            let mut bytes =
                Zeroizing::new(serde_json::to_vec(&command).map_err(|_| Error::Protocol)?);
            bytes.push(b'\n');
            if bytes.len() > MAX_COMMAND || input.write_all(&bytes).is_err() {
                break Err(Error::Interrupted);
            }
        }
        match child.0.try_wait() {
            Ok(Some(_)) => break Ok(()),
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(_) => break Err(Error::Interrupted),
        }
    };
    drop(input);
    // Reap before joining the pipe reader, including when network/WebKit stalls.
    if result.is_err() {
        let _ = child.0.kill();
    }
    let _ = child.0.wait();
    stop_reader.store(true, Ordering::Release);
    let read_result = reader.join();
    result?;
    let bytes = read_result
        .map_err(|_| Error::Protocol)?
        .map_err(|_| Error::Protocol)?;
    if control.is_canceled() {
        return Err(Error::Canceled);
    }
    let signed_in = decode_reply(&bytes)?;
    control.publish(LoginSnapshot {
        step: Step::Finishing,
        ..Default::default()
    });
    Ok(signed_in)
}

const MAX_COMMAND: usize = 4096;

#[derive(Default)]
struct ResponseStream {
    line: Zeroizing<Vec<u8>>,
    final_reply: Option<Zeroizing<Vec<u8>>>,
}
impl ResponseStream {
    fn push(&mut self, bytes: &[u8], control: &Control) -> Result<(), Error> {
        for byte in bytes {
            if self.line.len() >= MAX_REPLY {
                return Err(Error::Protocol);
            }
            self.line.push(*byte);
            if *byte != b'\n' {
                continue;
            }
            if self.line.starts_with(FRAME) {
                let reply: Reply = serde_json::from_slice(&self.line[FRAME.len()..])
                    .map_err(|_| Error::Protocol)?;
                match reply {
                    Reply::State { view } => {
                        if !view.valid() {
                            return Err(Error::Protocol);
                        }
                        if self.final_reply.is_none() {
                            control.publish(view);
                        }
                    }
                    _ => {
                        if self.final_reply.is_some() {
                            return Err(Error::Protocol);
                        }
                        self.final_reply = Some(Zeroizing::new(self.line.to_vec()));
                    }
                }
            }
            self.line.zeroize();
            self.line.clear();
        }
        Ok(())
    }
    fn finish(self) -> Result<Zeroizing<Vec<u8>>, Error> {
        self.final_reply.ok_or(Error::Protocol)
    }
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
        Reply::State { .. } => Err(Error::Protocol),
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
        && matches!(
            url.host_str(),
            Some("vk.ru" | "vk.com" | "id.vk.ru" | "id.vk.com" | "login.vk.ru" | "login.vk.com")
        )
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
    if bytes.len() + FRAME.len() + 1 > MAX_REPLY {
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

fn bridge_script() -> String {
    let mut script = include_str!("tauri_login_bridge.mjs").replace(
        "export function installAuthBridge",
        "function installAuthBridge",
    );
    script.push_str("\ninstallAuthBridge({window, document, location, getComputedStyle, HTMLInputElement, Event, setTimeout, globalThis, URL});");
    script
}

fn evaluate(window: &tauri::WebviewWindow, script: String) -> Option<serde_json::Value> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    window
        .eval_with_callback(script, move |value| {
            let parsed = (value.len() <= 8192)
                .then(|| serde_json::from_str(&value).ok())
                .flatten();
            let _ = sender.try_send(parsed);
        })
        .ok()?;
    receiver
        .recv_timeout(Duration::from_millis(700))
        .ok()
        .flatten()
}

fn inspect(window: &tauri::WebviewWindow) -> Option<LoginSnapshot> {
    let url = window.url().ok()?;
    if !navigation_allowed(&url) {
        return None;
    }
    let value = evaluate(window, "window.__vkFastAuth?.snapshot() || null".into())?;
    let state: LoginSnapshot = serde_json::from_value(value).ok()?;
    state.valid().then_some(state)
}

fn perform(window: &tauri::WebviewWindow, command: &InputCommand) -> bool {
    if !command.valid() || !window.url().is_ok_and(|url| navigation_allowed(&url)) {
        return false;
    }
    let Ok(bytes) = serde_json::to_string(command) else {
        return false;
    };
    let script = Zeroizing::new(format!("window.__vkFastAuth?.perform({bytes}) || false"));
    let _bytes = Zeroizing::new(bytes);
    evaluate(window, script.to_string())
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

fn challenge_window(window: &tauri::WebviewWindow, change: flow::Change, covered: bool) {
    let view = window.clone();
    let _ = window.run_on_main_thread(move || {
        let _ = view.eval(format!("window.__vkFastLoginCover?.({covered});"));
        match change {
            flow::Change::Show(w, h) | flow::Change::Resize(w, h) => {
                let _ = view.set_min_size(None::<tauri::Size>);
                let _ =
                    view.set_max_size(Some(tauri::LogicalSize::new(f64::from(w), f64::from(h))));
                let _ =
                    view.set_min_size(Some(tauri::LogicalSize::new(f64::from(w), f64::from(h))));
                let _ = view.set_size(tauri::LogicalSize::new(f64::from(w), f64::from(h)));
                if matches!(change, flow::Change::Show(..)) {
                    let _ = view.center();
                    let _ = view.show();
                    #[cfg(target_os = "windows")]
                    if let Ok(handle) = view.hwnd() {
                        eframe::focus_windows_window(handle.0 as isize);
                    }
                    #[cfg(not(target_os = "windows"))]
                    let _ = view.set_focus();
                }
            }
            flow::Change::Hide => {
                let _ = view.hide();
            }
            flow::Change::None => {}
        }
    });
}

/// The hidden helper owns VK's real website. Only a human challenge or an
/// explicit fallback opens its frameless window; the website has no native IPC.
pub fn run_helper(language: Language) -> Result<(), String> {
    if std::env::var(CHILD_MARKER).as_deref() != Ok("1") {
        return Err("Start VK sign-in from the application.".into());
    }
    let directory = match profile_directory() {
        Ok(path) => path,
        Err(_) => return write_reply(&Reply::Failed { kind: Fault::Setup }),
    };
    let _lock = match profile_lock(&directory) {
        Ok(lock) => lock,
        Err(_) => return write_reply(&Reply::Failed { kind: Fault::Busy }),
    };
    let outcome = Arc::new(Mutex::new(None));
    let done = Arc::new(AtomicBool::new(false));
    let revision = Arc::new(AtomicU64::new(0));
    let dismissed = Arc::new(AtomicBool::new(false));
    let (commands, receiver) = std::sync::mpsc::sync_channel::<InputCommand>(1);
    thread::Builder::new()
        .name("vk-login-commands".into())
        .spawn(move || {
            let mut reader = BufReader::new(io::stdin());
            loop {
                let mut bytes = Zeroizing::new(Vec::new());
                let read = (&mut reader)
                    .take((MAX_COMMAND + 1) as u64)
                    .read_until(b'\n', &mut bytes);
                if !matches!(read, Ok(1..))
                    || bytes.len() > MAX_COMMAND
                    || bytes.last() != Some(&b'\n')
                {
                    break;
                }
                let Ok(command) = serde_json::from_slice::<InputCommand>(&bytes) else {
                    break;
                };
                if !command.valid() || commands.send(command).is_err() {
                    break;
                }
            }
        })
        .map_err(|_| "Could not create sign-in channel")?;
    let setup_done = done.clone();
    let setup_outcome = outcome.clone();
    let setup_revision = revision.clone();
    let app=tauri::Builder::default().setup(move |app| {
        let page_revision=setup_revision.clone();
        let cover=include_str!("tauri_login_cover.js").replace("__STATUS_TEXT__",&serde_json::to_string(language.text("Подключаем VK…","Connecting to VK…")).unwrap());
        let mut builder=WebviewWindowBuilder::new(app,"vk-login",WebviewUrl::External(LOGIN_URL.parse()?))
            .title(language.text("Проверка VK", "VK verification"))
            .inner_size(340.0,400.0).min_inner_size(340.0,400.0).max_inner_size(340.0,400.0)
            .resizable(false).maximizable(false).decorations(false).skip_taskbar(true)
            .center().visible(false).focused(false).data_directory(directory)
            .devtools(false).general_autofill_enabled(false).user_agent(crate::web_audio::USER_AGENT)
            .initialization_script(cover).initialization_script(bridge_script())
            .on_navigation(navigation_allowed)
            .on_page_load(move |_, page| {
                if page.event()==tauri::webview::PageLoadEvent::Started && navigation_allowed(page.url()) {page_revision.fetch_add(1,Ordering::AcqRel);}
            });
        #[cfg(windows)] { builder=builder.theme(Some(tauri::Theme::Dark)).background_color(tauri::webview::Color(15,17,21,255)); }
        if let Some(icon)=app.default_window_icon() {builder=builder.icon(icon.clone())?;}
        let window=builder.build()?;
        let close_window=window.clone();let close_done=setup_done.clone();let close_dismissed=dismissed.clone();
        window.on_window_event(move |event| {
            if let tauri::WindowEvent::CloseRequested{api,..}=event
                && !close_done.load(Ordering::Acquire) {
                api.prevent_close();close_dismissed.store(true,Ordering::Release);let _=close_window.hide();
            }
        });
        let handle=app.handle().clone();
        thread::Builder::new().name("vk-native-sign-in".into()).spawn(move || {
            let mut popup=flow::Popup::default();let mut previous_view=None;let mut previous_session=None;
            let mut retry=Instant::now();let mut loading_since=Instant::now();
            let mut entry_attempt: Option<(String,u64,u8,Instant)>=None;
            let mut last_good: Option<(u64,LoginSnapshot)>=None;
            while !setup_done.load(Ordering::Acquire) {
                let epoch=setup_revision.load(Ordering::Acquire);
                let inspected=inspect(&window).filter(|view|last_good.as_ref().is_none_or(|(at,old)|*at==epoch || old.document!=view.document));
                let ready=inspected.is_some();
                let mut view=if let Some(view)=inspected {
                    last_good=Some((epoch,view.clone()));view
                } else if let Some((_,previous))=last_good.as_ref().filter(|(at,_)|*at==epoch) {
                    let mut view=previous.clone();view.busy=true;
                    view
                } else {LoginSnapshot::default()};
                let raw_step=view.step;
                if raw_step!=Step::Loading {loading_since=Instant::now();}
                // A slow navigation is not an unknown form. Offer the original
                // page without discarding the password for this explicit attempt.
                if raw_step==Step::Loading && loading_since.elapsed()>Duration::from_secs(10) {view.open_page_available=true;}
                let mut ack=false;let mut open=false;
                match receiver.try_recv() {
                    Ok(command)=>{
                        if command.action==Action::OpenPage {open=true;} else {
                            let accepted=ready && setup_revision.load(Ordering::Acquire)==epoch && perform(&window,&command);
                            view=inspect(&window).unwrap_or_default();
                            if !accepted {view.error=true;} else {popup.resume();}
                        }
                        ack=true;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected)=>{
                        if finish_once(&setup_outcome,&setup_done,Reply::Canceled) {handle.exit(0);}break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty)=>{}
                }
                if view.step==Step::Entry && !view.busy {
                    let fresh=entry_attempt.as_ref().is_none_or(|(doc,rev,_,_)|doc!=&view.document || *rev!=view.revision);
                    if fresh {entry_attempt=Some((view.document.clone(),view.revision,0,Instant::now()-Duration::from_secs(2)));}
                    if let Some((_,_,tries,last))=&mut entry_attempt {
                        if *tries<3 && last.elapsed()>=Duration::from_secs(1) {
                            let command=serde_json::json!({"document":view.document,"revision":view.revision,"action":"entry"});
                            let _=evaluate(&window,format!("window.__vkFastAuth?.perform({command}) || false"));
                            *tries+=1;*last=Instant::now();
                        } else if *tries>=3 && last.elapsed()>=Duration::from_secs(1) {view.step=Step::Manual;}
                    }
                }
                let change=popup.update(&view,open,dismissed.swap(false,Ordering::AcqRel));
                challenge_window(&window,change,view.step!=Step::Captcha && !popup.manual());
                if ack || previous_view.as_ref()!=Some(&view) {
                    if write_reply(&Reply::State{view:view.clone()}).is_err() {handle.exit(0);break;}
                    previous_view=Some(view.clone());
                }
                // A recognized challenge or credential form must be completed in
                // VK before a saved cookie can be accepted as a finished login.
                if ready && raw_step==Step::Loading && !view.document.is_empty() {
                    let ticket=setup_revision.load(Ordering::Acquire);
                    let cookies=snapshot(&window);let current=(ticket,fingerprint(&cookies));
                    if has_session(&cookies) && (previous_session!=Some(current) || Instant::now()>=retry) {
                        previous_session=Some(current);retry=Instant::now()+Duration::from_secs(10);
                        let verified=WebAudioClient::new(cookies.clone()).and_then(|client|client.session_profile());
                        let latest=snapshot(&window);
                        let latest_view=inspect(&window);
                        if setup_revision.load(Ordering::Acquire)!=ticket || !verified_session_matches(&latest,current.1)
                            || latest_view.as_ref().is_none_or(|v|v.step!=Step::Loading) {previous_session=None;continue;}
                        if let Ok(profile)=verified {
                            let outcome=setup_outcome.clone();let done=setup_done.clone();let revision=setup_revision.clone();let handle=handle.clone();let view=window.clone();
                            let _=window.run_on_main_thread(move || {
                                if revision.load(Ordering::Acquire)==ticket && finish_once(&outcome,&done,Reply::Success{profile,cookies:latest}) {
                                    let _=view.hide();handle.exit(0);
                                }
                            });
                        }
                    }
                }
                thread::sleep(Duration::from_millis(150));
            }
        })?;
        Ok(())
    }).build(tauri::generate_context!()).map_err(|_|"Could not create VK sign-in")?;
    let exit_done = done.clone();
    let exit_outcome = outcome.clone();
    app.run_return(move |_, event| {
        if matches!(
            event,
            tauri::RunEvent::Exit | tauri::RunEvent::ExitRequested { .. }
        ) {
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
            "https://unrelated.vk.ru/",
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

    #[test]
    fn progress_stream_is_bounded_per_frame_and_preserves_one_terminal_reply() {
        let control = Control::default();
        let mut decoder = ResponseStream::default();
        for revision in 0..700 {
            let mut bytes = frame(&Reply::State {
                view: LoginSnapshot {
                    document: "synthetic-document".into(),
                    revision,
                    step: Step::Identifier,
                    ..Default::default()
                },
            });
            bytes.push(b'\n');
            for chunk in bytes.chunks(7) {
                decoder.push(chunk, &control).unwrap();
            }
            assert!(decoder.line.is_empty());
        }
        assert_eq!(control.snapshot().revision, 699);
        let mut success = frame(&Reply::Success {
            profile: Profile {
                id: 42,
                first_name: "Synthetic".into(),
                last_name: String::new(),
                avatar_url: None,
            },
            cookies: cookies(),
        });
        success.push(b'\n');
        decoder.push(&success, &control).unwrap();
        let mut late = frame(&Reply::State {
            view: LoginSnapshot::default(),
        });
        late.push(b'\n');
        decoder.push(&late, &control).unwrap();
        assert_eq!(control.snapshot().revision, 699);
        assert_eq!(
            decode_reply(&decoder.finish().unwrap()).unwrap().profile.id,
            42
        );
        let mut duplicate = ResponseStream::default();
        duplicate.push(&success, &control).unwrap();
        assert!(duplicate.push(&success, &control).is_err());
        assert!(
            ResponseStream::default()
                .push(&vec![b'x'; MAX_REPLY + 1], &control)
                .is_err()
        );
    }
    #[test]
    #[ignore]
    fn local_hidden_webview_and_frameless_challenge_probe() {
        // Local blank app page + synthetic fields only. Never navigates to VK.
        let directory =
            std::env::temp_dir().join(format!("vk-fast-native-login-probe-{}", std::process::id()));
        private_directory(&directory).unwrap();
        let report = Arc::new(Mutex::new(None));
        let output = report.clone();
        let profile = directory.clone();
        let app=tauri::Builder::default().any_thread().setup(move |app| {
        let window=WebviewWindowBuilder::new(app,"vk-login",WebviewUrl::App("index.html".into()))
            .title("VK Fast — local UI test").inner_size(340.0,400.0).min_inner_size(340.0,400.0).max_inner_size(340.0,400.0)
            .decorations(false).resizable(false).skip_taskbar(true).visible(false).focused(false)
            .data_directory(profile).build()?;
        let handle=app.handle().clone();
        thread::spawn(move || {
            let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                for _ in 0..30 {
                    if evaluate(&window,"document.readyState".into()).is_some_and(|v|v=="complete"){break;}
                    thread::sleep(Duration::from_millis(100));
                }
                assert!(!window.is_visible().unwrap());assert!(!window.is_decorated().unwrap());
                let html="<form name='EnterPhoneOrMailForm'><input name='login' type='tel'><button type='submit' data-test-id='submit_btn'>Continue</button></form>";
                let mut setup=format!("document.body.innerHTML={}; document.querySelector('form').onsubmit=e=>{{e.preventDefault();window.fixtureValue=document.querySelector('input').value;}};",serde_json::to_string(html).unwrap());
                setup.push_str(&include_str!(concat!(env!("CARGO_MANIFEST_DIR"),"/src/tauri_login_bridge.mjs")).replace("export function installAuthBridge","function installAuthBridge"));
                setup.push_str(";installAuthBridge({window,document,location:{protocol:'https:',hostname:'vk.ru',pathname:'/',href:'https://vk.ru/'},getComputedStyle,HTMLInputElement,Event,setTimeout,globalThis,URL});true;");
                assert_eq!(evaluate(&window,setup),Some(serde_json::Value::Bool(true)));
                let view:LoginSnapshot=serde_json::from_value(evaluate(&window,"window.__vkFastAuth.snapshot()".into()).unwrap()).unwrap();
                assert_eq!(view.step,Step::Identifier);
                let control=Control::default();control.publish(view);
                assert!(control.send_credentials("+1234567890".into(),"synthetic-password".into()));
                let command=control.take_command().unwrap();
                assert!(!perform(&window,&command), "Production credentials cannot be dispatched to a local test origin");
                let call=format!("window.__vkFastAuth.perform({})",serde_json::to_string(&command).unwrap());
                assert_eq!(evaluate(&window,call),Some(serde_json::Value::Bool(true)));
                let mut submitted=false;
                for _ in 0..30 {
                    submitted=evaluate(&window,"window.fixtureValue || ''".into()).is_some_and(|v|v=="+1234567890");
                    if submitted {break;}
                    thread::sleep(Duration::from_millis(100));
                }
                assert!(submitted,"Hidden webview did not process the native form command");
                assert!(!window.is_visible().unwrap());
                let html="<form><div hidden style='display:block'><input name='username' autocomplete='username' value='synthetic-login'></div><input name='password' type='password' autocomplete='current-password'><button type='submit' disabled>Continue</button></form>";
                let setup=format!("document.body.innerHTML={}; window.fixturePasswordSubmits=0; document.querySelector('[name=password]').oninput=()=>setTimeout(()=>document.querySelector('button').disabled=false,150); document.querySelector('form').onsubmit=e=>{{e.preventDefault();window.fixturePassword=document.querySelector('[name=password]').value;window.fixturePasswordSubmits++;}};true;",serde_json::to_string(html).unwrap());
                assert_eq!(evaluate(&window,setup),Some(serde_json::Value::Bool(true)));
                let mut dispatched=false;
                for _ in 0..30 {
                    let view:LoginSnapshot=serde_json::from_value(evaluate(&window,"window.__vkFastAuth.snapshot()".into()).unwrap()).unwrap();
                    assert_eq!(view.step,Step::Password);control.publish(view);
                    if let Some(command)=control.take_command() {
                        assert_eq!(command.value,"synthetic-password");
                        let call=format!("window.__vkFastAuth.perform({})",serde_json::to_string(&command).unwrap());
                        assert_eq!(evaluate(&window,call),Some(serde_json::Value::Bool(true)));
                        dispatched=true;break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                assert!(dispatched,"Disabled Continue blocked the saved password");
                let mut password_submitted=false;
                for _ in 0..30 {
                    password_submitted=evaluate(&window,"window.fixturePassword === 'synthetic-password' && window.fixturePasswordSubmits === 1".into()).is_some_and(|v|v==true);
                    if password_submitted {break;}
                    thread::sleep(Duration::from_millis(100));
                }
                assert!(password_submitted,"Delayed validation did not submit the password exactly once");
                assert_eq!(evaluate(&window,"document.querySelector('[name=username]').value === 'synthetic-login'".into()),Some(serde_json::Value::Bool(true)));
                assert!(!window.is_visible().unwrap(),"Password entry must remain hidden");
                evaluate(&window,"document.body.innerHTML='<div data-test-id=\"captcha-widget\" style=\"background:white;border-radius:16px;padding:24px;font:18px sans-serif\">Local verification UI test — no CAPTCHA service</div>'; true".into());
                let view:LoginSnapshot=serde_json::from_value(evaluate(&window,"window.__vkFastAuth.snapshot()".into()).unwrap()).unwrap();
                let diagnostic=if view.step!=Step::Captcha {evaluate(&window,"(() => { const e=document.querySelector('[data-test-id=captcha-widget]'); const nodes=[]; for(let n=e;n;n=n.parentElement){const s=getComputedStyle(n);nodes.push({tag:n.tagName,hidden:n.hidden,inert:n.inert,aria:n.getAttribute('aria-hidden'),display:s.display,visibility:s.visibility,opacity:s.opacity});} return {present:!!e,rect:e?.getBoundingClientRect().toJSON(),viewport:[innerWidth,innerHeight],nodes}; })()".into())}else{None};
                assert_eq!(view.step,Step::Captcha,"Local fixture visibility: {diagnostic:?}");
                let mut popup=flow::Popup::default();challenge_window(&window,popup.update(&view,false,false),false);
                thread::sleep(Duration::from_millis(600));
                assert!(window.is_visible().unwrap());assert!(!window.is_decorated().unwrap());
                let size=window.inner_size().unwrap();let scale=window.scale_factor().unwrap();
                let width=size.width as f64/scale;let height=size.height as f64/scale;
                assert!(width<=640.0 && height<=800.0,"Compositor expanded the challenge to {width}×{height}");
                eprintln!("Local popup: {width}×{height}, decorations=false; hidden identifier and delayed password submission passed");
                challenge_window(&window,flow::Change::Hide,true);
                thread::sleep(Duration::from_millis(200));assert!(!window.is_visible().unwrap());
            }));
            *output.lock().unwrap()=Some(result.is_ok());handle.exit(0);
        });
        Ok(())
    }).build(tauri::generate_context!()).unwrap();
        app.run_return(|_, _| {});
        let passed = report.lock().unwrap().unwrap_or(false);
        let _ = std::fs::remove_dir_all(directory);
        assert!(passed, "Local native webview probe failed");
    }
}
