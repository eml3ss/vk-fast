//! Optional local Discord IPC presence. No HTTP, OAuth or voice commands.
use std::{
    collections::{HashMap, VecDeque},
    io::{self, Read, Write},
    path::PathBuf,
    sync::{Arc, Condvar, Mutex, OnceLock, TryLockError, Weak},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;
use serde_json::{Value, json};

const GITHUB_URL: &str = "https://github.com/eml3ss/vk-fast";
const APP_ICON_URL: &str =
    "https://raw.githubusercontent.com/eml3ss/vk-fast/main/assets/vk-fast-icon-512.png";
const MAX_FRAME: usize = 64 * 1024;
const UPDATE_INTERVAL: Duration = Duration::from_secs(5);
const RECONNECT_INTERVAL: Duration = Duration::from_secs(3);
const IO_TIMEOUT: Duration = Duration::from_millis(300);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(1500);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Playing,
    /// Pausing clears the activity rather than publishing a paused card.
    Paused,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub cover_url: Option<String>,
    pub track_url: Option<String>,
    pub album_url: Option<String>,
    pub duration: f64,
    pub position: f64,
    pub state: State,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Prepared {
    title: String,
    artist: String,
    album: Option<String>,
    cover_url: Option<String>,
    track_url: Option<String>,
    album_url: Option<String>,
    duration_ms: u64,
    start_ms: Option<u64>,
}

fn text(value: &str) -> String {
    let mut output = String::new();
    for character in value
        .chars()
        .take(256)
        .skip_while(|c| c.is_whitespace())
        .filter(|c| !c.is_control())
    {
        if output.len() + character.len_utf8() > 128 {
            break;
        }
        output.push(character);
    }
    output.truncate(output.trim_end().len());
    if output.chars().count() == 1 {
        output.push('\u{200b}');
    }
    output
}

pub(crate) fn public_cover(value: &str) -> Option<String> {
    if value.len() > 2048 {
        return None;
    }
    // Parse only: this module never requests the image itself.
    let url = reqwest::Url::parse(value).ok()?;
    let host = url.host_str()?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.port().is_some_and(|port| port != 443)
        || !(host == "userapi.com"
            || host.ends_with(".userapi.com")
            || host == "vkuserphoto.ru"
            || host.ends_with(".vkuserphoto.ru"))
    {
        return None;
    }
    let path = url.path().to_ascii_lowercase();
    if ![".jpg", ".jpeg", ".png", ".webp", ".avif"]
        .iter()
        .any(|suffix| path.ends_with(suffix))
        || url.query_pairs().any(|(key, _)| credential_query_key(&key))
    {
        return None;
    }
    Some(url.into())
}

fn credential_query_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    let sensitive = |word: &str| {
        matches!(
            word,
            "token"
                | "secret"
                | "auth"
                | "authorization"
                | "password"
                | "passwd"
                | "cookie"
                | "cookies"
                | "session"
                | "sessionid"
                | "sessionkey"
                | "remixsid"
                | "remixsid6"
                | "code"
                | "verifier"
                | "credential"
                | "credentials"
                | "accesstoken"
                | "refreshtoken"
                | "idtoken"
                | "authtoken"
                | "oauthtoken"
                | "clientsecret"
                | "codeverifier"
                | "pkceverifier"
        )
    };
    sensitive(&key)
        || key
            .split(|character: char| !character.is_ascii_alphanumeric())
            .any(sensitive)
}

fn public_page(value: &str) -> Option<String> {
    if value.len() > 2048
        || value
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return None;
    }
    let url = reqwest::Url::parse(value).ok()?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("vk.ru" | "vk.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.port_or_known_default() != Some(443)
    {
        return None;
    }
    let path = url.path();
    let valid_pair = |value: &str| {
        let Some((owner, id)) = value.split_once('_') else {
            return false;
        };
        let digits = |value: &str| {
            !value.is_empty()
                && value.bytes().all(|byte| byte.is_ascii_digit())
                && value.parse::<u64>().is_ok_and(|number| number > 0)
        };
        digits(owner.strip_prefix('-').unwrap_or(owner)) && digits(id)
    };
    let valid = url.query().is_none()
        && ["/audio", "/music/album/"]
            .iter()
            .any(|prefix| path.strip_prefix(prefix).is_some_and(valid_pair));
    valid.then(|| url.into())
}

impl Prepared {
    fn new(snapshot: Snapshot, now: SystemTime) -> Option<Self> {
        if snapshot.state == State::Paused {
            return None;
        }
        let title = text(&snapshot.title);
        if title.is_empty() {
            return None;
        }
        let artist = text(&snapshot.artist);
        let artist = if artist.is_empty() {
            "VK Fast".into()
        } else {
            artist
        };
        let duration = if snapshot.duration.is_finite() {
            snapshot.duration.clamp(0.0, 7.0 * 24.0 * 3600.0)
        } else {
            0.0
        };
        let position = if snapshot.position.is_finite() {
            snapshot
                .position
                .clamp(0.0, if duration > 0.0 { duration } else { 604_800.0 })
        } else {
            0.0
        };
        let now_ms = now
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let now_ms = u64::try_from(now_ms).unwrap_or(u64::MAX);
        Some(Self {
            title,
            artist,
            album: snapshot
                .album
                .as_deref()
                .map(text)
                .filter(|s| !s.is_empty()),
            cover_url: snapshot.cover_url.as_deref().and_then(public_cover),
            track_url: snapshot.track_url.as_deref().and_then(public_page),
            album_url: snapshot.album_url.as_deref().and_then(public_page),
            duration_ms: (duration * 1000.0).round() as u64,
            start_ms: (snapshot.state == State::Playing)
                .then(|| now_ms.saturating_sub((position * 1000.0).round() as u64)),
        })
    }

    fn same_activity(&self, other: &Self) -> bool {
        self.title == other.title
            && self.artist == other.artist
            && self.album == other.album
            && self.cover_url == other.cover_url
            && self.track_url == other.track_url
            && self.album_url == other.album_url
            && self.duration_ms == other.duration_ms
            && match (self.start_ms, other.start_ms) {
                (Some(a), Some(b)) => a.abs_diff(b) <= 1000,
                (None, None) => true,
                _ => false,
            }
    }

    fn activity(&self) -> Value {
        let mut activity = json!({
            "type": 2,
            "details": self.title,
            "state": self.artist,
            "buttons": [{"label":"GitHub", "url":GITHUB_URL}],
        });
        if let Some(start_ms) = self.start_ms {
            let start = (start_ms / 1000).max(1);
            activity["timestamps"] = if self.duration_ms > 0 {
                json!({"start":start,"end":start.saturating_add(self.duration_ms.div_ceil(1000))})
            } else {
                json!({"start":start})
            };
        }
        activity["assets"] = json!({
            "large_image":self.cover_url.as_deref().unwrap_or(APP_ICON_URL),
            "large_text":self.album.as_deref().unwrap_or(&self.title),
        });
        if let Some(track) = &self.track_url {
            activity["details_url"] = json!(track);
        }
        if let Some(album) = &self.album_url {
            activity["assets"]["large_url"] = json!(album);
        }
        activity
    }
}

#[derive(Default)]
struct Desired {
    latest: Option<Prepared>,
    stopping: bool,
}

#[derive(Default)]
struct Shared {
    desired: Mutex<Desired>,
    changed: Condvar,
}

impl Shared {
    fn snapshot(&self) -> (Option<Prepared>, bool) {
        let state = self
            .desired
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        (state.latest.clone(), state.stopping)
    }

    fn wait(&self, delay: Duration) {
        let state = self
            .desired
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let _ = self.changed.wait_timeout(state, delay);
    }
}

pub struct DiscordRpc {
    shared: Arc<Shared>,
}

impl DiscordRpc {
    pub fn new(application_id: String) -> Self {
        Self::start(
            application_id,
            endpoints(),
            UPDATE_INTERVAL,
            RECONNECT_INTERVAL,
        )
    }

    fn start(
        application_id: String,
        endpoints: Vec<PathBuf>,
        update: Duration,
        retry: Duration,
    ) -> Self {
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        if application_id.len() < 8
            || application_id.len() > 20
            || !application_id.bytes().all(|byte| byte.is_ascii_digit())
            || application_id.parse::<u64>().ok().is_none_or(|id| id == 0)
        {
            shared
                .desired
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .stopping = true;
            crate::diagnostics::event("discord", "invalid_application_id");
        } else if thread::Builder::new()
            .name("vk-discord-rpc".into())
            .spawn(move || worker_loop(worker, application_id, endpoints, update, retry))
            .is_err()
        {
            shared
                .desired
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .stopping = true;
            crate::diagnostics::event("discord", "worker_start_failed");
        }
        Self { shared }
    }

    pub fn publish(&self, snapshot: Snapshot) {
        let prepared = Prepared::new(snapshot, SystemTime::now());
        let mut desired = self
            .shared
            .desired
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !desired.stopping {
            desired.latest = prepared;
            self.shared.changed.notify_one();
        }
    }

    pub fn clear(&self) {
        self.shared
            .desired
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .latest = None;
        self.shared.changed.notify_one();
    }
}

impl Drop for DiscordRpc {
    fn drop(&mut self) {
        let mut desired = self
            .shared
            .desired
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        desired.latest = None;
        desired.stopping = true;
        self.shared.changed.notify_one();
    }
}

fn endpoints() -> Vec<PathBuf> {
    #[cfg(windows)]
    return (0..10)
        .map(|index| PathBuf::from(format!(r"\\?\pipe\discord-ipc-{index}")))
        .collect();
    #[cfg(unix)]
    {
        let mut bases = Vec::new();
        for variable in ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"] {
            if let Some(path) = std::env::var_os(variable).map(PathBuf::from)
                && path.is_absolute()
                && !bases.contains(&path)
            {
                bases.push(path);
            }
        }
        if !bases.contains(&PathBuf::from("/tmp")) {
            bases.push("/tmp".into());
        }
        let mut addresses = Vec::new();
        for base in bases {
            for suffix in [
                "",
                "app/com.discordapp.Discord",
                "app/com.discordapp.DiscordCanary",
                "snap.discord",
            ] {
                for index in 0..10 {
                    let path = base.join(suffix).join(format!("discord-ipc-{index}"));
                    if !addresses.contains(&path) {
                        addresses.push(path);
                    }
                }
            }
        }
        addresses
    }
}

#[derive(Debug)]
enum Fault {
    Io,
    Protocol,
    Server,
    Timeout,
    Canceled,
}

impl From<io::Error> for Fault {
    fn from(_: io::Error) -> Self {
        Self::Io
    }
}

#[derive(Default)]
struct Frames(Vec<u8>);

impl Frames {
    fn push(&mut self, bytes: &[u8]) -> Result<(), Fault> {
        if self.0.len() + bytes.len() > (MAX_FRAME + 8) * 2 {
            return Err(Fault::Protocol);
        }
        self.0.extend_from_slice(bytes);
        Ok(())
    }

    fn next(&mut self) -> Result<Option<(u32, Vec<u8>)>, Fault> {
        if self.0.len() < 8 {
            return Ok(None);
        }
        let opcode = u32::from_le_bytes(self.0[..4].try_into().map_err(|_| Fault::Protocol)?);
        let length =
            u32::from_le_bytes(self.0[4..8].try_into().map_err(|_| Fault::Protocol)?) as usize;
        if length > MAX_FRAME {
            return Err(Fault::Protocol);
        }
        if self.0.len() < length + 8 {
            return Ok(None);
        }
        let payload = self.0[8..8 + length].to_vec();
        self.0.drain(..8 + length);
        Ok(Some((opcode, payload)))
    }
}

fn frame(opcode: u32, payload: &[u8]) -> Result<Vec<u8>, Fault> {
    if payload.len() > MAX_FRAME {
        return Err(Fault::Protocol);
    }
    let mut bytes = Vec::with_capacity(8 + payload.len());
    bytes.extend_from_slice(&opcode.to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(payload);
    Ok(bytes)
}

#[derive(Deserialize)]
struct Envelope {
    evt: Option<String>,
    nonce: Option<String>,
    data: Option<ErrorData>,
}

#[derive(Deserialize)]
struct ErrorData {
    code: Option<i32>,
}

struct Connection {
    stream: platform::Stream,
    frames: Frames,
    serial: u64,
    last_ping: Instant,
    awaiting_pong: Option<Instant>,
}

impl Connection {
    fn send(
        &mut self,
        opcode: u32,
        payload: &[u8],
        shared: &Shared,
        closing: bool,
    ) -> Result<(), Fault> {
        let bytes = frame(opcode, payload)?;
        let deadline = Instant::now() + IO_TIMEOUT;
        let mut sent = 0;
        while sent < bytes.len() {
            if Instant::now() >= deadline {
                return Err(Fault::Timeout);
            }
            if !closing && sent == 0 && shared.snapshot().1 {
                return Err(Fault::Canceled);
            }
            match self.stream.write(&bytes[sent..]) {
                Ok(0) => {}
                Ok(count) => {
                    sent += count;
                    continue;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(_) => return Err(Fault::Io),
            }
            shared.wait(Duration::from_millis(5));
        }
        Ok(())
    }

    fn activity(
        &mut self,
        desired: Option<&Prepared>,
        shared: &Shared,
        closing: bool,
    ) -> Result<String, Fault> {
        self.serial = self.serial.wrapping_add(1);
        let nonce = format!("vk-fast-{}", self.serial);
        let command = json!({"cmd":"SET_ACTIVITY","nonce":nonce,"args":{
            "pid":std::process::id(),"activity":desired.map(Prepared::activity)
        }});
        self.send(
            1,
            &serde_json::to_vec(&command).map_err(|_| Fault::Protocol)?,
            shared,
            closing,
        )?;
        Ok(nonce)
    }

    fn receive(&mut self, shared: &Shared, closing: bool) -> Result<Vec<Envelope>, Fault> {
        let mut messages = Vec::new();
        let mut processed = 0;
        let mut chunk = [0_u8; 4096];
        for _ in 0..16 {
            while let Some((opcode, payload)) = self.frames.next()? {
                processed += 1;
                match opcode {
                    1 => {
                        let message: Envelope =
                            serde_json::from_slice(&payload).map_err(|_| Fault::Protocol)?;
                        if message.evt.as_deref() == Some("ERROR") {
                            if let Some(code) = message.data.and_then(|data| data.code) {
                                crate::diagnostics::event(
                                    "discord",
                                    &format!("rpc_error code={code}"),
                                );
                            }
                            return Err(Fault::Server);
                        }
                        messages.push(message);
                    }
                    2 => return Err(Fault::Io),
                    3 if !closing => self.send(4, &payload, shared, false)?,
                    3 => {}
                    4 => self.awaiting_pong = None,
                    _ => return Err(Fault::Protocol),
                }
                if processed >= 16 {
                    return Ok(messages);
                }
            }
            match self.stream.read(&mut chunk) {
                Ok(0) => {
                    // PIPE_NOWAIT maps both idle reads and pipe EOF to zero.
                    // A heartbeat bounds reconnection when the peer disappears.
                    #[cfg(windows)]
                    break;
                    #[cfg(unix)]
                    return Err(Fault::Io);
                }
                Ok(count) => self.frames.push(&chunk[..count])?,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => return Err(Fault::Io),
            }
        }
        Ok(messages)
    }

    fn handshake(&mut self, id: &str, shared: &Shared) -> Result<(), Fault> {
        self.send(
            0,
            &serde_json::to_vec(&json!({"v":1,"client_id":id})).map_err(|_| Fault::Protocol)?,
            shared,
            false,
        )?;
        let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
        while Instant::now() < deadline {
            let (desired, stopping) = shared.snapshot();
            if stopping || desired.is_none() {
                return Err(Fault::Canceled);
            }
            if self
                .receive(shared, false)?
                .iter()
                .any(|message| message.evt.as_deref() == Some("READY"))
            {
                return Ok(());
            }
            shared.wait(Duration::from_millis(10));
        }
        Err(Fault::Timeout)
    }

    fn close(&mut self, shared: &Shared) {
        if let Ok(nonce) = self.activity(None, shared, true) {
            let deadline = Instant::now() + Duration::from_millis(150);
            while Instant::now() < deadline {
                match self.receive(shared, true) {
                    Ok(messages)
                        if messages
                            .iter()
                            .any(|message| message.nonce.as_deref() == Some(&nonce)) =>
                    {
                        break;
                    }
                    Err(_) => break,
                    _ => shared.wait(Duration::from_millis(5)),
                }
            }
        }
    }
}

fn connect(addresses: &[PathBuf], cursor: &mut usize, shared: &Shared) -> Option<Connection> {
    let deadline = Instant::now() + Duration::from_millis(500);
    let start = *cursor;
    for offset in 0..addresses.len() {
        if shared.snapshot().1 || Instant::now() >= deadline {
            break;
        }
        let index = (start + offset) % addresses.len();
        *cursor = (index + 1) % addresses.len();
        if let Ok(stream) = platform::connect(&addresses[index]) {
            return Some(Connection {
                stream,
                frames: Frames::default(),
                serial: 0,
                last_ping: Instant::now(),
                awaiting_pong: None,
            });
        }
    }
    None
}

fn worker_loop(
    shared: Arc<Shared>,
    id: String,
    addresses: Vec<PathBuf>,
    update: Duration,
    retry: Duration,
) {
    // A rapid disable/enable cannot let an old activity:null erase its
    // successor's presence (Discord identifies both by the same app ID/PID).
    // The handoff waits only on this worker, never on the caller/UI thread.
    static OWNERS: OnceLock<Mutex<HashMap<String, Weak<Mutex<()>>>>> = OnceLock::new();
    let owner = {
        let mut owners = OWNERS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        owners.retain(|_, owner| owner.strong_count() > 0);
        let owner = owners
            .get(&id)
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| Arc::new(Mutex::new(())));
        owners.insert(id.clone(), Arc::downgrade(&owner));
        owner
    };
    let _ownership = loop {
        if shared.snapshot().1 {
            return;
        }
        match owner.try_lock() {
            Ok(guard) => break guard,
            Err(TryLockError::Poisoned(error)) => break error.into_inner(),
            Err(TryLockError::WouldBlock) => shared.wait(Duration::from_millis(10)),
        }
    };
    let mut connection: Option<Connection> = None;
    let mut last_sent: Option<Prepared> = None;
    let mut next_update = Instant::now();
    let mut reconnect = Instant::now();
    let mut cursor = 0;
    let mut activity_times = VecDeque::new();
    loop {
        let (desired, stopping) = shared.snapshot();
        if stopping {
            if let Some(mut active) = connection {
                active.close(&shared);
            }
            return;
        }
        let now = Instant::now();
        if connection.is_none() && desired.is_some() && now >= reconnect {
            if let Some(mut candidate) = connect(&addresses, &mut cursor, &shared) {
                match candidate.handshake(&id, &shared) {
                    Ok(()) => {
                        connection = Some(candidate);
                        last_sent = None;
                        crate::diagnostics::event("discord", "connected");
                    }
                    Err(Fault::Canceled) => {}
                    Err(_) => crate::diagnostics::event("discord", "handshake_failed"),
                }
            }
            reconnect = Instant::now() + retry;
        }
        if let Some(active) = connection.as_mut() {
            let result = (|| -> Result<(), Fault> {
                active.receive(&shared, false)?;
                while activity_times
                    .front()
                    .is_some_and(|sent: &Instant| sent.elapsed() >= Duration::from_secs(20))
                {
                    activity_times.pop_front();
                }
                // Re-read after connection/READY: only the newest queued track wins.
                let (desired, stopping) = shared.snapshot();
                if stopping {
                    return Ok(());
                }
                if desired.is_none() && last_sent.is_some() {
                    active.activity(None, &shared, false)?;
                    activity_times.push_back(Instant::now());
                    last_sent = None;
                } else if let Some(desired) = desired
                    && last_sent.as_ref().is_none_or(|last| !last.same_activity(&desired))
                    && Instant::now() >= next_update
                    // Keep one of five activity slots for an immediate clear.
                    && activity_times.len() < 4
                {
                    active.activity(Some(&desired), &shared, false)?;
                    activity_times.push_back(Instant::now());
                    last_sent = Some(desired);
                    next_update = Instant::now() + update;
                }
                if active
                    .awaiting_pong
                    .is_some_and(|sent| sent.elapsed() >= HEARTBEAT_TIMEOUT)
                {
                    return Err(Fault::Timeout);
                }
                if active.last_ping.elapsed() >= HEARTBEAT_INTERVAL
                    && active.awaiting_pong.is_none()
                {
                    active.last_ping = Instant::now();
                    active.awaiting_pong = Some(Instant::now());
                    active.send(3, b"{}", &shared, false)?;
                }
                Ok(())
            })();
            if let Err(error) = result {
                // Preserve the live connection for the shutdown clear rather
                // than discarding it when cancellation interrupts a write.
                if shared.snapshot().1 && matches!(&error, Fault::Canceled) {
                    continue;
                }
                crate::diagnostics::event(
                    "discord",
                    match error {
                        Fault::Server => "server_error",
                        Fault::Protocol => "protocol_error",
                        _ => "disconnected",
                    },
                );
                connection = None;
                last_sent = None;
                reconnect = Instant::now() + retry;
            }
        }
        shared.wait(if connection.is_some() {
            Duration::from_millis(40)
        } else {
            Duration::from_millis(250)
        });
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::os::unix::fs::FileTypeExt;
    pub type Stream = std::os::unix::net::UnixStream;
    pub fn connect(path: &std::path::Path) -> io::Result<Stream> {
        let metadata = std::fs::metadata(path)?;
        if !metadata.file_type().is_socket() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::MetadataExt;
            static UID: OnceLock<Option<u32>> = OnceLock::new();
            let uid = UID.get_or_init(|| {
                std::fs::metadata("/proc/self")
                    .ok()
                    .map(|metadata| metadata.uid())
            });
            if *uid != Some(metadata.uid()) {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
        }
        let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
        socket.connect_timeout(&socket2::SockAddr::unix(path)?, Duration::from_millis(25))?;
        socket.set_nonblocking(true)?;
        Ok(socket.into())
    }
}

#[cfg(test)]
pub(crate) fn test_activity(snapshot: Snapshot) -> Option<Value> {
    Prepared::new(snapshot, SystemTime::now()).map(|prepared| prepared.activity())
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP_ID: &str = "1565752977157922916";

    fn snapshot(title: &str, position: f64, state: State) -> Snapshot {
        Snapshot {
            title: title.into(),
            artist: "Synthetic Artist".into(),
            album: Some("Test Album".into()),
            cover_url: Some(
                "https://sun9-85.vkuserphoto.ru/impg/synthetic/cover.jpg?quality=95&sign=public-image-sign&size=96&type=album&from=bu&blur=1"
                    .into(),
            ),
            track_url: Some("https://vk.ru/audio-7_42".into()),
            album_url: Some("https://vk.ru/music/album/-7_9".into()),
            duration: 180.0,
            position,
            state,
        }
    }

    #[test]
    fn payload_is_listening_with_seconds_timing_exact_repository_and_public_image_only() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let prepared = Prepared::new(snapshot("Track", 30.0, State::Playing), now).unwrap();
        let value = prepared.activity();
        assert_eq!(value["type"], 2);
        assert_eq!(value["details"], "Track");
        assert_eq!(value["state"], "Synthetic Artist");
        assert_eq!(value["details_url"], "https://vk.ru/audio-7_42");
        assert!(value.get("state_url").is_none());
        assert_eq!(
            value["assets"]["large_url"],
            "https://vk.ru/music/album/-7_9"
        );
        assert_eq!(
            value["timestamps"],
            json!({"start":1_699_999_970_u64,"end":1_700_000_150_u64})
        );
        assert_eq!(
            value["buttons"],
            json!([{"label":"GitHub","url":"https://github.com/eml3ss/vk-fast"}])
        );
        assert!(
            value["assets"]["large_image"]
                .as_str()
                .unwrap()
                .contains("sign=public-image-sign")
        );
        assert!(
            Prepared::new(
                snapshot("Track", 31.0, State::Playing),
                now + Duration::from_secs(1)
            )
            .unwrap()
            .same_activity(&prepared)
        );
        assert!(
            !Prepared::new(
                snapshot("Track", 90.0, State::Playing),
                now + Duration::from_secs(1)
            )
            .unwrap()
            .same_activity(&prepared)
        );
        assert!(Prepared::new(snapshot("Track", 90.0, State::Paused), now).is_none());
        for url in [
            "https://vk.com/audio1_2",
            "https://audio.vkuseraudio.net/track.mp3",
            "http://sun1.userapi.com/a.jpg",
            "https://sun1.userapi.com/a.jpg?access_token=private",
            "https://sun1.userapi.com/a.jpg?cookie=private",
            "https://user:password@sun1.userapi.com/a.jpg",
            "https://sun1.userapi.com/a.jpg#private",
            "https://127.0.0.1/a.jpg",
            "https://localhost/a.jpg",
            "https://sun1.userapi.com.evil.test/a.jpg",
            "https://sun9-85.vkuserphoto.ru.evil.test/a.jpg",
            "https://evilvkuserphoto.ru/a.jpg",
            "https://sun9-85.vkuserphoto.ru/audio.mp3?sign=public",
        ] {
            assert!(public_cover(url).is_none(), "rejected cover fixture");
        }
        let mut invalid = snapshot("   ", f64::NAN, State::Playing);
        assert!(Prepared::new(invalid.clone(), now).is_none());
        invalid.title = "я".repeat(1000);
        invalid.duration = f64::INFINITY;
        let normalized = Prepared::new(invalid, now).unwrap();
        assert!(normalized.title.len() <= 128);
        assert_eq!(normalized.duration_ms, 0);
        let mut single = snapshot("X", 0.0, State::Playing);
        single.artist.clear();
        single.cover_url = None;
        let single = Prepared::new(single, now).unwrap().activity();
        assert!(single["details"].as_str().unwrap().chars().count() >= 2);
        assert_eq!(single["state"], "VK Fast");
        assert_eq!(single["assets"]["large_image"], APP_ICON_URL);
    }

    #[test]
    fn track_and_album_links_allow_only_public_vk_resources_and_affect_dedup() {
        for url in [
            "https://vk.ru/audio-7_42",
            "https://vk.com/audio7_42",
            "https://vk.ru/music/album/-7_9",
        ] {
            assert_eq!(public_page(url).as_deref(), Some(url));
        }
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let original = snapshot("Track", 0.0, State::Playing);
        let prepared = Prepared::new(original.clone(), now).unwrap();
        for field in 0..2 {
            let mut changed = original.clone();
            if field == 0 {
                changed.track_url = Some("https://vk.ru/audio-7_43".into());
            } else {
                changed.album_url = None;
            }
            assert!(!prepared.same_activity(&Prepared::new(changed, now).unwrap()));
        }
        for url in [
            "http://vk.ru/audio-7_42",
            "file:///private/track",
            "https://evil.test/audio-7_42",
            "https://vk.ru.evil.test/audio-7_42",
            "https://m.vk.ru/audio-7_42",
            "https://user:secret@vk.ru/audio-7_42",
            "https://vk.ru:8443/audio-7_42",
            "https://vk.ru/audio-7_42#private",
            "https://vk.ru/audio-7_42?access_key=private",
            "https://vk.ru/music/album/-7_9_private-key",
            "https://vk.ru/music/playlist/-7_9_private-key",
            "https://vk.ru/music/album/0_9",
            "https://vk.ru/audio-7_0",
            "https://vk.ru/artist/synthetic-artist",
            "https://vk.ru/music/playlist/-7_9",
            "https://vk.ru/audio?performer=1&q=Artist",
            "https://vk.ru/artist/0",
            "https://vk.ru/artist/..",
            "https://vk.ru/login?code=private",
            "https://vk.ru/away.php?to=https://evil.test",
            "https://vk.ru/method/audio.get?access_token=private",
            "https://audio.vkuseraudio.net/track.mp3",
            "https://vk.ru/mp3/audio_api_unavailable.mp3",
            "https://vk.ru/search?c[q]=Artist&c[section]=audio",
            "https://vk.ru/audio?q=Artist",
            "https://vk.ru/audio?performer=0&q=Artist",
            "https://vk.ru/audio?performer=1&q=Artist&token=private",
            "https://vk.ru/audio?performer=1&q=A&q=B",
            "https://vk.ru/audio?performer=1&q=%0AA",
        ] {
            assert!(
                public_page(url).is_none(),
                "non-public or unsupported link must not be published"
            );
        }
        let mut invalid = original;
        invalid.track_url = Some("https://vk.ru/mp3/private.mp3".into());
        invalid.album_url = Some("https://vk.ru/music/album/-7_9?access_key=private".into());
        let value = test_activity(invalid).unwrap();
        assert!(
            value.get("details_url").is_none()
                && value.get("state_url").is_none()
                && value["assets"].get("large_url").is_none()
        );
        assert_eq!(value["buttons"][0]["url"], GITHUB_URL);
    }

    #[test]
    fn vk_photo_signed_covers_keep_image_parameters_and_reject_credential_names() {
        let cover = "https://sun9-85.vkuserphoto.ru/impg/synthetic/cover.jpg?quality=95&sign=public%2Fimage&size=240x240&type=album&c_uniq_tag=public&signature=synthetic&from=bu&blur=2";
        assert_eq!(public_cover(cover).as_deref(), Some(cover));
        let mut source = snapshot("Track", 0.0, State::Playing);
        source.cover_url = Some(cover.into());
        let prepared =
            Prepared::new(source, UNIX_EPOCH + Duration::from_secs(1_700_000_000)).unwrap();
        assert_eq!(prepared.activity()["assets"]["large_image"], cover);
        assert_ne!(prepared.activity()["assets"]["large_image"], APP_ICON_URL);
        assert!(
            public_cover(
                "https://sun1.userapi.com/image.jpg?signature=synthetic&unknown_image_option=1"
            )
            .is_some()
        );
        for key in [
            "access_token",
            "AccessToken",
            "refresh-token",
            "client.secret",
            "Authorization",
            "password",
            "COOKIE",
            "session_id",
            "remixsid6",
            "code",
            "code_verifier",
        ] {
            assert!(
                public_cover(&format!(
                    "https://sun9-85.vkuserphoto.ru/impg/synthetic/cover.jpg?{key}=private"
                ))
                .is_none(),
                "credential key must be rejected"
            );
        }
    }

    #[test]
    fn fragmented_and_coalesced_frames_are_preserved_and_oversize_headers_rejected() {
        let one = frame(3, b"\0binary\xff").unwrap();
        let two = frame(1, br#"{"evt":"READY"}"#).unwrap();
        let mut input = Frames::default();
        for byte in &one[..one.len() - 1] {
            input.push(&[*byte]).unwrap();
            assert!(input.next().unwrap().is_none());
        }
        input.push(&[one[one.len() - 1]]).unwrap();
        input.push(&two).unwrap();
        assert_eq!(input.next().unwrap(), Some((3, b"\0binary\xff".to_vec())));
        assert_eq!(
            input.next().unwrap(),
            Some((1, br#"{"evt":"READY"}"#.to_vec()))
        );
        assert!(input.next().unwrap().is_none());
        let mut oversized = Frames::default();
        oversized.push(&[1, 0, 0, 0]).unwrap();
        oversized
            .push(&((MAX_FRAME + 1) as u32).to_le_bytes())
            .unwrap();
        assert!(matches!(oversized.next(), Err(Fault::Protocol)));
        assert!(frame(1, &vec![0; MAX_FRAME + 1]).is_err());
    }

    #[cfg(unix)]
    mod ipc {
        use super::*;
        use std::{
            fs,
            os::unix::net::{UnixListener, UnixStream},
            sync::{
                atomic::{AtomicU64, Ordering},
                mpsc,
            },
        };

        struct Directory(PathBuf);
        impl Directory {
            fn new() -> Self {
                static NEXT: AtomicU64 = AtomicU64::new(0);
                let path = std::env::temp_dir().join(format!(
                    "vk-fast-discord-test-{}-{}-{}",
                    std::process::id(),
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_nanos(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                use std::os::unix::fs::PermissionsExt;
                fs::create_dir(&path).unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
                Self(path)
            }
        }
        impl Drop for Directory {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        fn listener(directory: &Directory) -> (UnixListener, PathBuf) {
            let path = directory.0.join("fake-discord-ipc");
            let listener = UnixListener::bind(&path).unwrap();
            listener.set_nonblocking(true).unwrap();
            (listener, path)
        }
        fn accept(listener: &UnixListener) -> UnixStream {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        stream
                            .set_write_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        return stream;
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "test connect deadline");
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("synthetic accept failure: {error}"),
                }
            }
        }
        fn packet(stream: &mut UnixStream) -> (u32, Vec<u8>) {
            let mut header = [0; 8];
            stream.read_exact(&mut header).unwrap();
            let opcode = u32::from_le_bytes(header[..4].try_into().unwrap());
            let length = u32::from_le_bytes(header[4..].try_into().unwrap()) as usize;
            assert!(length <= MAX_FRAME);
            let mut data = vec![0; length];
            stream.read_exact(&mut data).unwrap();
            (opcode, data)
        }
        fn send(stream: &mut UnixStream, opcode: u32, value: &Value) {
            stream
                .write_all(&frame(opcode, &serde_json::to_vec(value).unwrap()).unwrap())
                .unwrap();
        }
        fn handshake(stream: &mut UnixStream) {
            let (opcode, data) = packet(stream);
            assert_eq!(opcode, 0);
            assert_eq!(
                serde_json::from_slice::<Value>(&data).unwrap(),
                json!({"v":1,"client_id":APP_ID})
            );
        }
        fn activity(stream: &mut UnixStream) -> Value {
            let (opcode, data) = packet(stream);
            assert_eq!(opcode, 1);
            let value: Value = serde_json::from_slice(&data).unwrap();
            assert_eq!(value["cmd"], "SET_ACTIVITY");
            send(stream, 1, &json!({"nonce":value["nonce"],"data":{}}));
            value["args"]["activity"].clone()
        }
        fn client(path: PathBuf) -> DiscordRpc {
            DiscordRpc::start(
                APP_ID.into(),
                vec![path],
                Duration::from_millis(60),
                Duration::from_millis(30),
            )
        }
        fn receive<T>(rx: &mpsc::Receiver<T>) -> T {
            rx.recv_timeout(Duration::from_secs(3))
                .expect("fake IPC event deadline")
        }

        #[test]
        fn newest_state_waits_for_ready_ping_is_echoed_and_clear_drop_close_without_ui_wait() {
            let directory = Directory::new();
            let (listener, path) = listener(&directory);
            let (event_tx, event_rx) = mpsc::channel();
            let (ready_tx, ready_rx) = mpsc::channel();
            let server = thread::spawn(move || {
                let mut stream = accept(&listener);
                handshake(&mut stream);
                event_tx.send("handshake").unwrap();
                receive(&ready_rx);
                let mut combined = frame(3, b"\0opaque\xff").unwrap();
                combined.extend(frame(1, br#"{"evt":"READY"}"#).unwrap());
                for chunk in combined.chunks(3) {
                    stream.write_all(chunk).unwrap();
                }
                assert_eq!(packet(&mut stream), (4, b"\0opaque\xff".to_vec()));
                let newest = activity(&mut stream);
                assert_eq!(newest["details"], "Newest");
                assert_eq!(newest["details_url"], "https://vk.ru/audio-7_42");
                assert!(newest.get("state_url").is_none());
                assert_eq!(newest["state"], "Synthetic Artist");
                assert_eq!(
                    newest["assets"]["large_url"],
                    "https://vk.ru/music/album/-7_9"
                );
                assert_eq!(newest["buttons"][0]["url"], GITHUB_URL);
                assert_eq!(
                    newest["assets"]["large_image"],
                    snapshot("Newest", 10.0, State::Playing).cover_url.unwrap()
                );
                event_tx.send("activity").unwrap();
                let changed = activity(&mut stream);
                assert_eq!(changed["details"], "Newest");
                assert_eq!(
                    changed["assets"]["large_url"],
                    "https://vk.ru/music/album/-7_10"
                );
                assert_eq!(changed["buttons"][0]["url"], GITHUB_URL);
                event_tx.send("link_updated").unwrap();
                assert!(activity(&mut stream).is_null());
                event_tx.send("clear").unwrap();
                assert!(activity(&mut stream).is_null());
                let mut byte = [0];
                assert_eq!(stream.read(&mut byte).unwrap(), 0);
                event_tx.send("closed").unwrap();
            });
            let rpc = client(path);
            rpc.publish(snapshot("Old", 10.0, State::Playing));
            assert_eq!(receive(&event_rx), "handshake");
            rpc.publish(snapshot("Intermediate", 10.0, State::Playing));
            rpc.publish(snapshot("Newest", 10.0, State::Playing));
            ready_tx.send(()).unwrap();
            assert_eq!(receive(&event_rx), "activity");
            let mut links = snapshot("Newest", 10.0, State::Playing);
            links.album_url = Some("https://vk.ru/music/album/-7_10".into());
            rpc.publish(links);
            assert_eq!(receive(&event_rx), "link_updated");
            rpc.clear();
            assert_eq!(receive(&event_rx), "clear");
            let start = Instant::now();
            drop(rpc);
            assert!(start.elapsed() < Duration::from_millis(100));
            assert_eq!(receive(&event_rx), "closed");
            server.join().unwrap();
        }

        #[test]
        fn normal_elapsed_progress_deduplicates_seek_pause_resume_update_and_remain_rate_limited() {
            let directory = Directory::new();
            let (listener, path) = listener(&directory);
            let (tx, rx) = mpsc::channel();
            let server = thread::spawn(move || {
                let mut stream = accept(&listener);
                handshake(&mut stream);
                send(&mut stream, 1, &json!({"evt":"READY"}));
                let first = activity(&mut stream);
                tx.send(first.clone()).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_millis(150)))
                    .unwrap();
                let mut byte = [0];
                assert!(
                    matches!(stream.read(&mut byte),Err(error) if matches!(error.kind(),io::ErrorKind::WouldBlock|io::ErrorKind::TimedOut))
                );
                tx.send(Value::String("quiet".into())).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                tx.send(activity(&mut stream)).unwrap();
                let last_playing = Instant::now();
                let paused = activity(&mut stream);
                assert!(paused.is_null());
                tx.send(paused).unwrap();
                let resumed = activity(&mut stream);
                assert!(last_playing.elapsed() >= Duration::from_millis(45));
                tx.send(resumed).unwrap();
                assert!(activity(&mut stream).is_null());
            });
            let rpc = client(path);
            rpc.publish(snapshot("Track", 10.0, State::Playing));
            let first = receive(&rx);
            for _ in 0..50 {
                rpc.publish(snapshot("Track", 10.3, State::Playing));
            }
            assert_eq!(receive(&rx), "quiet");
            rpc.publish(snapshot("Track", 90.0, State::Playing));
            let seek = receive(&rx);
            assert!(
                first["timestamps"]["start"]
                    .as_u64()
                    .unwrap()
                    .abs_diff(seek["timestamps"]["start"].as_u64().unwrap())
                    > 2
            );
            rpc.publish(snapshot("Track", 90.0, State::Paused));
            assert!(receive(&rx).is_null());
            rpc.publish(snapshot("Track", 90.0, State::Playing));
            assert!(receive(&rx).get("timestamps").is_some());
            drop(rpc);
            server.join().unwrap();
        }

        #[test]
        fn pause_clears_immediately_and_resume_keeps_an_already_expired_send_deadline() {
            let directory = Directory::new();
            let (listener, path) = listener(&directory);
            let (tx, rx) = mpsc::channel();
            let server = thread::spawn(move || {
                let mut stream = accept(&listener);
                handshake(&mut stream);
                send(&mut stream, 1, &json!({"evt":"READY"}));
                assert_eq!(activity(&mut stream)["details"], "Track");
                tx.send(()).unwrap();
                assert!(activity(&mut stream).is_null());
                tx.send(()).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_millis(250)))
                    .unwrap();
                assert!(activity(&mut stream).get("timestamps").is_some());
                tx.send(()).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                assert!(activity(&mut stream).is_null());
            });
            let rpc = DiscordRpc::start(
                APP_ID.into(),
                vec![path],
                Duration::from_millis(500),
                Duration::from_millis(30),
            );
            rpc.publish(snapshot("Track", 10.0, State::Playing));
            receive(&rx);
            thread::sleep(Duration::from_millis(600));
            rpc.publish(snapshot("Track", 10.0, State::Paused));
            receive(&rx);
            rpc.publish(snapshot("Track", 10.0, State::Playing));
            receive(&rx);
            drop(rpc);
            server.join().unwrap();
        }

        #[test]
        fn server_error_and_invalid_frames_reconnect_and_clear_during_handshake_suppresses_activity()
         {
            let directory = Directory::new();
            let (listener, path) = listener(&directory);
            let (tx, rx) = mpsc::channel();
            let (go_tx, go_rx) = mpsc::channel();
            let server = thread::spawn(move || {
                let mut stream = accept(&listener);
                handshake(&mut stream);
                send(
                    &mut stream,
                    1,
                    &json!({"evt":"ERROR","data":{"code":4000,"message":"synthetic unlogged error"}}),
                );
                drop(stream);
                let mut stream = accept(&listener);
                handshake(&mut stream);
                stream.write_all(&[1, 0, 0, 0, 255, 255, 255, 127]).unwrap();
                drop(stream);
                let mut stream = accept(&listener);
                handshake(&mut stream);
                tx.send(()).unwrap();
                receive(&go_rx);
                let _ = stream.write_all(&frame(1, br#"{"evt":"READY"}"#).unwrap());
                let mut byte = [0];
                match stream.read(&mut byte) {
                    Ok(0) => {}
                    Err(error) if error.kind() == io::ErrorKind::ConnectionReset => {}
                    _ => panic!("cleared handshake emitted an activity"),
                }
            });
            let rpc = client(path);
            rpc.publish(snapshot("Track", 10.0, State::Playing));
            receive(&rx);
            rpc.publish(snapshot("Track", 10.0, State::Paused));
            go_tx.send(()).unwrap();
            server.join().unwrap();
            drop(rpc);
        }

        #[test]
        fn rapid_reenable_waits_for_old_clear_and_disconnect_before_new_presence() {
            let directory = Directory::new();
            let (listener, path) = listener(&directory);
            let (tx, rx) = mpsc::channel();
            let server = thread::spawn(move || {
                let mut old = accept(&listener);
                handshake(&mut old);
                send(&mut old, 1, &json!({"evt":"READY"}));
                assert_eq!(activity(&mut old)["details"], "Old");
                tx.send("old").unwrap();
                let (opcode, bytes) = packet(&mut old);
                assert_eq!(opcode, 1);
                let clear: Value = serde_json::from_slice(&bytes).unwrap();
                assert!(clear["args"]["activity"].is_null());
                assert_eq!(clear["args"]["pid"], std::process::id());
                let deadline = Instant::now() + Duration::from_millis(80);
                while Instant::now() < deadline {
                    assert!(
                        matches!(listener.accept(),Err(error) if error.kind()==io::ErrorKind::WouldBlock)
                    );
                    thread::sleep(Duration::from_millis(5));
                }
                send(&mut old, 1, &json!({"nonce":clear["nonce"],"data":{}}));
                let mut byte = [0];
                assert_eq!(old.read(&mut byte).unwrap(), 0);
                drop(old);
                let mut new = accept(&listener);
                handshake(&mut new);
                send(&mut new, 1, &json!({"evt":"READY"}));
                assert_eq!(activity(&mut new)["details"], "New");
                tx.send("new").unwrap();
                assert!(activity(&mut new).is_null());
            });
            let old = client(path.clone());
            old.publish(snapshot("Old", 10.0, State::Playing));
            assert_eq!(receive(&rx), "old");
            let start = Instant::now();
            drop(old);
            assert!(start.elapsed() < Duration::from_millis(100));
            let new = client(path);
            new.publish(snapshot("New", 10.0, State::Playing));
            assert_eq!(receive(&rx), "new");
            drop(new);
            server.join().unwrap();
        }

        #[test]
        fn shutdown_after_partial_write_finishes_that_frame_before_clear() {
            let (sender, mut receiver) = UnixStream::pair().unwrap();
            receiver
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let socket = socket2::Socket::from(sender);
            socket.set_send_buffer_size(1024).unwrap();
            socket.set_nonblocking(true).unwrap();
            let shared = Arc::new(Shared::default());
            let stopping = shared.clone();
            let worker = thread::spawn(move || {
                let mut connection = Connection {
                    stream: socket.into(),
                    frames: Frames::default(),
                    serial: 0,
                    last_ping: Instant::now(),
                    awaiting_pong: None,
                };
                connection
                    .send(3, &vec![42; 16 * 1024], &shared, false)
                    .unwrap();
                connection.activity(None, &shared, true).unwrap();
            });
            let mut header = [0; 8];
            receiver.read_exact(&mut header).unwrap();
            assert_eq!(u32::from_le_bytes(header[..4].try_into().unwrap()), 3);
            stopping.desired.lock().unwrap().stopping = true;
            stopping.changed.notify_one();
            let mut payload = vec![0; 16 * 1024];
            receiver.read_exact(&mut payload).unwrap();
            assert!(payload.iter().all(|byte| *byte == 42));
            let (opcode, bytes) = packet(&mut receiver);
            assert_eq!(opcode, 1);
            assert!(serde_json::from_slice::<Value>(&bytes).unwrap()["args"]["activity"].is_null());
            worker.join().unwrap();
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use interprocess::{
        ConnectWaitMode,
        os::windows::named_pipe::{DuplexPipeStream, pipe_mode},
    };
    pub struct Stream(DuplexPipeStream<pipe_mode::Bytes>);
    pub fn connect(path: &std::path::Path) -> io::Result<Stream> {
        let stream = DuplexPipeStream::connect_by_path_with_wait_mode(
            path.as_os_str(),
            ConnectWaitMode::Timeout(Duration::ZERO),
        )?;
        stream.set_nonblocking(true)?;
        Ok(Stream(stream))
    }
    impl Read for Stream {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            self.0.read(bytes)
        }
    }
    impl Write for Stream {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.write(bytes)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl Drop for Stream {
        fn drop(&mut self) {
            self.0.assume_flushed();
        }
    }
}
